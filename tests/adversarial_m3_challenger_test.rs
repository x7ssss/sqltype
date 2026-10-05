//! Challenger 1 Empirical Adversarial Stress-Test Suite for Milestone 3 (R3: Join Tree Nullability Inference Engine)
//!
//! Mandatory Challenge Dimensions:
//! 1. Deeply nested outer joins: `(a RIGHT JOIN (b FULL JOIN c)) LEFT JOIN (d JOIN e)`, and variations.
//! 2. Self-joins with asymmetric outer joins: `users u1 LEFT JOIN users u2`, verifying non-null vs nullable and zero alias leakage.
//! 3. Composite types with inner nullable fields under outer joins: verifying outer `| null` added without breaking inner field types.
//! 4. Ambiguous column references across joined tables in an aliased join.

use sqltype::analyzer::analyze_query;
use sqltype::catalog::{Catalog, CompositeTypeAttribute, CompositeTypeMetadata};
use sqltype::codegen::generate_file_ts;
use sqltype::nullability::{
    JoinKind, JoinTreeNode, format_nullable, has_root_null, propagate_join_nullability,
    strip_root_null,
};

#[test]
fn test_deeply_nested_outer_joins_mandated_scenario() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE a (id INT PRIMARY KEY, val_a TEXT NOT NULL);
        CREATE TABLE b (id INT PRIMARY KEY, val_b TEXT NOT NULL);
        CREATE TABLE c (id INT PRIMARY KEY, val_c TEXT NOT NULL);
        CREATE TABLE d (id INT PRIMARY KEY, val_d TEXT NOT NULL);
        CREATE TABLE e (id INT PRIMARY KEY, val_e TEXT NOT NULL);
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // Mandated test scenario: (a RIGHT JOIN (b FULL JOIN c)) LEFT JOIN (d JOIN e)
    let sql = r#"
        SELECT
            a.val_a,
            b.val_b,
            c.val_c,
            d.val_d,
            e.val_e
        FROM (a RIGHT JOIN (b FULL JOIN c ON b.id = c.id) ON a.id = b.id)
        LEFT JOIN (d JOIN e ON d.id = e.id)
        ON b.id = d.id;
    "#;

    let analyzed = analyze_query(sql, &catalog, None)
        .expect("Nested outer join query must analyze successfully");
    assert_eq!(analyzed.fields.len(), 5);

    // Analysis:
    // Outermost join: LEFT JOIN
    //   Left subtree: (a RIGHT JOIN (b FULL JOIN c))
    //     - a is on the LEFT of RIGHT JOIN -> null-producing (true)
    //     - (b FULL JOIN c) is on the RIGHT of RIGHT JOIN (preserved by RIGHT JOIN)
    //       - b is in FULL JOIN -> null-producing (true)
    //       - c is in FULL JOIN -> null-producing (true)
    //   Right subtree: (d JOIN e) (INNER JOIN)
    //     - On right side of outermost LEFT JOIN -> null-producing (true)
    //     - Both d and e inherit null-producing (true)
    // Consequently: ALL 5 tables are null-producing in this tree!
    assert_eq!(analyzed.fields[0].name, "val_a");
    assert_eq!(
        analyzed.fields[0].ts_type, "string | null",
        "val_a must be nullable (left of RIGHT JOIN)"
    );

    assert_eq!(analyzed.fields[1].name, "val_b");
    assert_eq!(
        analyzed.fields[1].ts_type, "string | null",
        "val_b must be nullable (FULL JOIN)"
    );

    assert_eq!(analyzed.fields[2].name, "val_c");
    assert_eq!(
        analyzed.fields[2].ts_type, "string | null",
        "val_c must be nullable (FULL JOIN)"
    );

    assert_eq!(analyzed.fields[3].name, "val_d");
    assert_eq!(
        analyzed.fields[3].ts_type, "string | null",
        "val_d must be nullable (right of outer LEFT JOIN)"
    );

    assert_eq!(analyzed.fields[4].name, "val_e");
    assert_eq!(
        analyzed.fields[4].ts_type, "string | null",
        "val_e must be nullable (right of outer LEFT JOIN)"
    );

    // Codegen verification
    let ts = generate_file_ts(&[analyzed]);
    assert!(ts.contains("val_a: string | null;"));
    assert!(ts.contains("val_b: string | null;"));
    assert!(ts.contains("val_c: string | null;"));
    assert!(ts.contains("val_d: string | null;"));
    assert!(ts.contains("val_e: string | null;"));

    // Verify propagation directly via nullability engine API
    let parsed = pg_query::parse(sql).unwrap();
    let stmt = parsed.protobuf.stmts.first().unwrap();
    if let Some(node) = &stmt.stmt
        && let Some(pg_query::NodeEnum::SelectStmt(select)) = &node.node
    {
        let scope = propagate_join_nullability(&select.from_clause, &catalog);
        assert!(scope.bindings["a"].is_null_producing());
        assert!(scope.bindings["b"].is_null_producing());
        assert!(scope.bindings["c"].is_null_producing());
        assert!(scope.bindings["d"].is_null_producing());
        assert!(scope.bindings["e"].is_null_producing());
    }

    // Verify propagation directly on JoinTreeNode AST
    let a_node = JoinTreeNode::Table {
        rel_name: "a".to_string(),
        schema: None,
        alias: None,
        colnames: Vec::new(),
        is_null_producing: false,
    };
    let b_node = JoinTreeNode::Table {
        rel_name: "b".to_string(),
        schema: None,
        alias: None,
        colnames: Vec::new(),
        is_null_producing: false,
    };
    let c_node = JoinTreeNode::Table {
        rel_name: "c".to_string(),
        schema: None,
        alias: None,
        colnames: Vec::new(),
        is_null_producing: false,
    };
    let b_full_c = JoinTreeNode::Join {
        kind: JoinKind::Full,
        left: Box::new(b_node),
        right: Box::new(c_node),
        alias: None,
        is_null_producing: false,
    };
    let left_subtree = JoinTreeNode::Join {
        kind: JoinKind::Right,
        left: Box::new(a_node),
        right: Box::new(b_full_c),
        alias: None,
        is_null_producing: false,
    };
    let d_node = JoinTreeNode::Table {
        rel_name: "d".to_string(),
        schema: None,
        alias: None,
        colnames: Vec::new(),
        is_null_producing: false,
    };
    let e_node = JoinTreeNode::Table {
        rel_name: "e".to_string(),
        schema: None,
        alias: None,
        colnames: Vec::new(),
        is_null_producing: false,
    };
    let right_subtree = JoinTreeNode::Join {
        kind: JoinKind::Inner,
        left: Box::new(d_node),
        right: Box::new(e_node),
        alias: None,
        is_null_producing: false,
    };
    let mut tree_root = JoinTreeNode::Join {
        kind: JoinKind::Left,
        left: Box::new(left_subtree),
        right: Box::new(right_subtree),
        alias: None,
        is_null_producing: false,
    };
    tree_root.propagate_nullability(false);
    let nulls = tree_root.collect_table_nullabilities();
    for (tbl, _, is_null) in nulls {
        assert!(is_null, "Table {} must be null-producing", tbl);
    }
}

#[test]
fn test_deeply_nested_outer_joins_contrast_preserved_branch() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE a (id INT PRIMARY KEY, val_a TEXT NOT NULL);
        CREATE TABLE b (id INT PRIMARY KEY, val_b TEXT NOT NULL);
        CREATE TABLE c (id INT PRIMARY KEY, val_c TEXT NOT NULL);
        CREATE TABLE d (id INT PRIMARY KEY, val_d TEXT NOT NULL);
        CREATE TABLE e (id INT PRIMARY KEY, val_e TEXT NOT NULL);
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // Contrast tree: (a LEFT JOIN (b FULL JOIN c)) LEFT JOIN (d JOIN e)
    // Here, `a` is on left of LEFT JOIN, so `a` MUST NOT be null-producing!
    let sql = r#"
        SELECT
            a.val_a,
            b.val_b,
            c.val_c,
            d.val_d,
            e.val_e
        FROM (a LEFT JOIN (b FULL JOIN c ON b.id = c.id) ON a.id = b.id)
        LEFT JOIN (d JOIN e ON d.id = e.id)
        ON a.id = d.id;
    "#;

    let analyzed =
        analyze_query(sql, &catalog, None).expect("Contrast query must analyze successfully");
    assert_eq!(analyzed.fields[0].name, "val_a");
    assert_eq!(
        analyzed.fields[0].ts_type, "string",
        "val_a must remain non-null (preserved branch)"
    );

    assert_eq!(analyzed.fields[1].name, "val_b");
    assert_eq!(analyzed.fields[1].ts_type, "string | null");

    assert_eq!(analyzed.fields[2].name, "val_c");
    assert_eq!(analyzed.fields[2].ts_type, "string | null");

    assert_eq!(analyzed.fields[3].name, "val_d");
    assert_eq!(analyzed.fields[3].ts_type, "string | null");

    assert_eq!(analyzed.fields[4].name, "val_e");
    assert_eq!(analyzed.fields[4].ts_type, "string | null");
}

#[test]
fn test_deeply_nested_outer_joins_mixed_right_heavy_propagation() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE a (id INT PRIMARY KEY, val_a TEXT NOT NULL);
        CREATE TABLE b (id INT PRIMARY KEY, val_b TEXT NOT NULL);
        CREATE TABLE c (id INT PRIMARY KEY, val_c TEXT NOT NULL);
        CREATE TABLE d (id INT PRIMARY KEY, val_d TEXT NOT NULL);
        CREATE TABLE e (id INT PRIMARY KEY, val_e TEXT NOT NULL);
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // Tree: ((a LEFT JOIN b) INNER JOIN c) RIGHT JOIN (d LEFT JOIN e)
    // Outermost join is RIGHT JOIN:
    //   Left subtree: ((a LEFT JOIN b) INNER JOIN c) is nullified by RIGHT JOIN!
    //     - val_a, val_b, val_c ALL become nullable!
    //   Right subtree: (d LEFT JOIN e)
    //     - d is preserved by RIGHT JOIN and LEFT JOIN -> val_d remains NOT NULL!
    //     - e is on right of LEFT JOIN -> val_e becomes nullable!
    let sql = r#"
        SELECT
            a.val_a,
            b.val_b,
            c.val_c,
            d.val_d,
            e.val_e
        FROM ((a LEFT JOIN b ON a.id = b.id) INNER JOIN c ON a.id = c.id)
        RIGHT JOIN (d LEFT JOIN e ON d.id = e.id)
        ON c.id = d.id;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("Query must analyze successfully");
    assert_eq!(analyzed.fields[0].name, "val_a");
    assert_eq!(analyzed.fields[0].ts_type, "string | null");

    assert_eq!(analyzed.fields[1].name, "val_b");
    assert_eq!(analyzed.fields[1].ts_type, "string | null");

    assert_eq!(analyzed.fields[2].name, "val_c");
    assert_eq!(analyzed.fields[2].ts_type, "string | null");

    assert_eq!(analyzed.fields[3].name, "val_d");
    assert_eq!(
        analyzed.fields[3].ts_type, "string",
        "val_d must be preserved NOT NULL"
    );

    assert_eq!(analyzed.fields[4].name, "val_e");
    assert_eq!(analyzed.fields[4].ts_type, "string | null");
}

#[test]
fn test_self_join_asymmetric_nullability_and_catalog_invariance() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE users (
            id INT PRIMARY KEY,
            col TEXT NOT NULL,
            email TEXT NOT NULL
        );
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // Mandated test scenario: users u1 LEFT JOIN users u2
    // Verifying u1.col remains non-null while u2.col becomes nullable without cross-alias leakage
    let sql = r#"
        SELECT
            u1.id AS u1_id,
            u1.col AS u1_col,
            u1.email AS u1_email,
            u2.id AS u2_id,
            u2.col AS u2_col,
            u2.email AS u2_email
        FROM users u1
        LEFT JOIN users u2 ON u1.id = u2.id;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("Self-join query must succeed");
    assert_eq!(analyzed.fields.len(), 6);

    // u1 columns MUST remain non-nullable
    assert_eq!(analyzed.fields[0].name, "u1_id");
    assert_eq!(analyzed.fields[0].ts_type, "number");
    assert_eq!(analyzed.fields[1].name, "u1_col");
    assert_eq!(analyzed.fields[1].ts_type, "string");
    assert_eq!(analyzed.fields[2].name, "u1_email");
    assert_eq!(analyzed.fields[2].ts_type, "string");

    // u2 columns MUST be nullable
    assert_eq!(analyzed.fields[3].name, "u2_id");
    assert_eq!(analyzed.fields[3].ts_type, "number | null");
    assert_eq!(analyzed.fields[4].name, "u2_col");
    assert_eq!(analyzed.fields[4].ts_type, "string | null");
    assert_eq!(analyzed.fields[5].name, "u2_email");
    assert_eq!(analyzed.fields[5].ts_type, "string | null");

    // Transposed test: users u2 LEFT JOIN users u1
    let sql_transposed = r#"
        SELECT
            u1.col AS u1_col,
            u2.col AS u2_col
        FROM users u2
        LEFT JOIN users u1 ON u2.id = u1.id;
    "#;
    let analyzed_trans =
        analyze_query(sql_transposed, &catalog, None).expect("Transposed self-join must succeed");
    assert_eq!(analyzed_trans.fields[0].name, "u1_col");
    assert_eq!(
        analyzed_trans.fields[0].ts_type, "string | null",
        "u1 is now right side of LEFT JOIN, must be nullable"
    );
    assert_eq!(analyzed_trans.fields[1].name, "u2_col");
    assert_eq!(
        analyzed_trans.fields[1].ts_type, "string",
        "u2 is now left side of LEFT JOIN, must be non-null"
    );

    // 4-Way Self-Join: (u1 LEFT JOIN u2) LEFT JOIN (u3 LEFT JOIN u4)
    let sql_4way = r#"
        SELECT
            u1.col AS col1,
            u2.col AS col2,
            u3.col AS col3,
            u4.col AS col4
        FROM (users u1 LEFT JOIN users u2 ON u1.id = u2.id)
        LEFT JOIN (users u3 LEFT JOIN users u4 ON u3.id = u4.id)
        ON u1.id = u3.id;
    "#;
    let analyzed_4way =
        analyze_query(sql_4way, &catalog, None).expect("4-way self-join must succeed");
    assert_eq!(
        analyzed_4way.fields[0].ts_type, "string",
        "u1 must remain non-null"
    );
    assert_eq!(
        analyzed_4way.fields[1].ts_type, "string | null",
        "u2 must be nullable"
    );
    assert_eq!(
        analyzed_4way.fields[2].ts_type, "string | null",
        "u3 must be nullable"
    );
    assert_eq!(
        analyzed_4way.fields[3].ts_type, "string | null",
        "u4 must be nullable"
    );

    // Catalog Invariance Check: underlying 'users' table MUST NOT be mutated!
    let users_tbl = catalog.get_table("users").expect("users table must exist");
    assert!(
        !users_tbl.get_column("col").unwrap().is_nullable,
        "users.col DDL nullability must remain false"
    );
    assert!(
        !users_tbl.get_column("email").unwrap().is_nullable,
        "users.email DDL nullability must remain false"
    );
    assert!(
        !users_tbl.get_column("id").unwrap().is_nullable,
        "users.id DDL nullability must remain false"
    );
    assert!(
        catalog.get_table("u1").is_none(),
        "Alias u1 must not leak into catalog"
    );
    assert!(
        catalog.get_table("u2").is_none(),
        "Alias u2 must not leak into catalog"
    );
    assert!(
        catalog.get_table("u3").is_none(),
        "Alias u3 must not leak into catalog"
    );
    assert!(
        catalog.get_table("u4").is_none(),
        "Alias u4 must not leak into catalog"
    );
}

#[test]
fn test_composite_types_inner_nullable_under_outer_joins() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TYPE address_t AS (
            street TEXT,
            zip TEXT
        );
        CREATE TYPE user_profile_t AS (
            bio TEXT,
            addr address_t
        );
        CREATE TABLE users (
            id INT PRIMARY KEY,
            name TEXT NOT NULL
        );
        CREATE TABLE profiles (
            user_id INT PRIMARY KEY,
            contact user_profile_t NOT NULL
        );
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // Standalone unit invariants for depth-0 union formatting
    let inner_nullable = "{ bio: string | null; addr: { street: string; zip: string | null } }";
    assert!(
        !has_root_null(inner_nullable),
        "Depth-1 null must not be classified as root null"
    );
    let wrapped = format_nullable(inner_nullable);
    assert_eq!(
        wrapped,
        "{ bio: string | null; addr: { street: string; zip: string | null } } | null"
    );
    assert!(
        has_root_null(&wrapped),
        "Depth-0 null must be classified as root null"
    );
    assert_eq!(
        format_nullable(&wrapped),
        wrapped,
        "format_nullable must be idempotent"
    );
    assert_eq!(
        strip_root_null(&wrapped),
        inner_nullable,
        "strip_root_null must preserve inner nulls"
    );

    // Inject an inner nullable field into `user_profile_t`
    // (bio is `string | null` and address_t zip is `string | null`)
    catalog.composite_types.insert(
        "address_t".to_string(),
        CompositeTypeMetadata {
            name: "address_t".to_string(),
            schema: Some("public".to_string()),
            attributes: vec![
                CompositeTypeAttribute {
                    name: "street".to_string(),
                    pg_type: "text".to_string(),
                    ts_type: "string".to_string(),
                },
                CompositeTypeAttribute {
                    name: "zip".to_string(),
                    pg_type: "text".to_string(),
                    ts_type: "string | null".to_string(), // Inner nullable field!
                },
            ],
        },
    );

    catalog.composite_types.insert(
        "user_profile_t".to_string(),
        CompositeTypeMetadata {
            name: "user_profile_t".to_string(),
            schema: Some("public".to_string()),
            attributes: vec![
                CompositeTypeAttribute {
                    name: "bio".to_string(),
                    pg_type: "text".to_string(),
                    ts_type: "string | null".to_string(), // Inner nullable field!
                },
                CompositeTypeAttribute {
                    name: "addr".to_string(),
                    pg_type: "address_t".to_string(),
                    ts_type: "{ street: string; zip: string | null }".to_string(), // Nested inner null!
                },
            ],
        },
    );

    // 1. Preserved branch vs null-producing branch: users u LEFT JOIN profiles p
    // u.name is non-null. p.contact is nullable from outer join!
    let sql_outer = r#"
        SELECT
            u.id,
            u.name,
            p.contact
        FROM users u
        LEFT JOIN profiles p ON u.id = p.user_id;
    "#;

    let analyzed_outer = analyze_query(sql_outer, &catalog, None).expect("Query must succeed");
    assert_eq!(analyzed_outer.fields[0].ts_type, "number");
    assert_eq!(analyzed_outer.fields[1].ts_type, "string");

    // Expected: outer object wrapped with `| null`, inner `{ bio: string | null; addr: { street: string; zip: string | null } }` PRESERVED intact!
    let contact_field = &analyzed_outer.fields[2];
    assert_eq!(contact_field.name, "contact");
    assert_eq!(
        contact_field.ts_type,
        "{ bio: string | null; addr: { street: string; zip: string | null } } | null",
        "Outer | null must be appended without corrupting inner nullable fields"
    );

    // 2. INNER JOIN: p.contact is NOT null-producing
    // Inner fields maintain their nullabilities, but outer object has NO `| null`!
    let sql_inner = r#"
        SELECT
            u.name,
            p.contact
        FROM users u
        INNER JOIN profiles p ON u.id = p.user_id;
    "#;

    let analyzed_inner = analyze_query(sql_inner, &catalog, None).expect("Inner join must succeed");
    assert_eq!(
        analyzed_inner.fields[1].ts_type,
        "{ bio: string | null; addr: { street: string; zip: string | null } }",
        "Inner join must NOT have outer | null"
    );

    // Codegen verification
    let ts_code = generate_file_ts(&[analyzed_outer]);
    assert!(
        ts_code.contains(
            "contact: { bio: string | null; addr: { street: string; zip: string | null } } | null;"
        ),
        "Generated TypeScript must have exact composite type with inner nulls and outer null:\n{}",
        ts_code
    );
}

#[test]
fn test_ambiguous_column_references_in_aliased_join() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE t1 (
            id INT PRIMARY KEY,
            status TEXT NOT NULL,
            col1 TEXT NOT NULL
        );
        CREATE TABLE t2 (
            id INT PRIMARY KEY,
            status TEXT NOT NULL,
            col2 TEXT NOT NULL
        );
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // Aliased join: (t1 JOIN t2 ON t1.id = t2.id) AS j
    // Duplicate columns in j: 'id', 'status'.
    // Distinct columns in j: 'col1' (from t1), 'col2' (from t2).

    // 1. Explicit reference to ambiguous column j.status -> MUST FAIL
    let sql_status = "SELECT j.status FROM (t1 JOIN t2 ON t1.id = t2.id) AS j;";
    let err_status = analyze_query(sql_status, &catalog, None).unwrap_err();
    assert!(
        err_status.contains("ambiguous"),
        "j.status must be flagged ambiguous, got: {}",
        err_status
    );

    // 2. Explicit reference to ambiguous column j.id -> MUST FAIL
    let sql_id = "SELECT j.id FROM (t1 JOIN t2 ON t1.id = t2.id) AS j;";
    let err_id = analyze_query(sql_id, &catalog, None).unwrap_err();
    assert!(
        err_id.contains("ambiguous"),
        "j.id must be flagged ambiguous, got: {}",
        err_id
    );

    // 3. Unqualified reference to ambiguous column `status` -> MUST FAIL
    let sql_unqual_status = "SELECT status FROM (t1 JOIN t2 ON t1.id = t2.id) AS j;";
    let err_unqual_status = analyze_query(sql_unqual_status, &catalog, None).unwrap_err();
    assert!(
        err_unqual_status.contains("ambiguous"),
        "Unqualified 'status' must be flagged ambiguous, got: {}",
        err_unqual_status
    );

    // 4. Unqualified reference to ambiguous column `id` -> MUST FAIL
    let sql_unqual_id = "SELECT id FROM (t1 JOIN t2 ON t1.id = t2.id) AS j;";
    let err_unqual_id = analyze_query(sql_unqual_id, &catalog, None).unwrap_err();
    assert!(
        err_unqual_id.contains("ambiguous"),
        "Unqualified 'id' must be flagged ambiguous, got: {}",
        err_unqual_id
    );

    // 5. Distinct columns j.col1 and j.col2 -> MUST SUCCEED
    let sql_distinct = "SELECT j.col1, j.col2 FROM (t1 JOIN t2 ON t1.id = t2.id) AS j;";
    let res_distinct =
        analyze_query(sql_distinct, &catalog, None).expect("Distinct columns must succeed");
    assert_eq!(res_distinct.fields.len(), 2);
    assert_eq!(res_distinct.fields[0].name, "col1");
    assert_eq!(res_distinct.fields[0].ts_type, "string");
    assert_eq!(res_distinct.fields[1].name, "col2");
    assert_eq!(res_distinct.fields[1].ts_type, "string");

    // 6. Unqualified reference to distinct column `col1` -> MUST SUCCEED
    let sql_unqual_valid = "SELECT col1 FROM (t1 JOIN t2 ON t1.id = t2.id) AS j;";
    let res_unqual = analyze_query(sql_unqual_valid, &catalog, None)
        .expect("Unqualified distinct col1 must succeed");
    assert_eq!(res_unqual.fields[0].name, "col1");
    assert_eq!(res_unqual.fields[0].ts_type, "string");

    // 7. Aliased join inside outer join:
    // `other o LEFT JOIN (t1 JOIN t2 ON t1.id = t2.id) AS j ON o.id = 1`
    catalog
        .apply_sql("CREATE TABLE other (id INT PRIMARY KEY, name TEXT NOT NULL);")
        .unwrap();
    let sql_outer_aliased = r#"
        SELECT
            o.name,
            j.col1,
            j.col2
        FROM other o
        LEFT JOIN (t1 JOIN t2 ON t1.id = t2.id) AS j
        ON o.id = 1;
    "#;
    let res_outer_aliased =
        analyze_query(sql_outer_aliased, &catalog, None).expect("Outer aliased join must succeed");
    assert_eq!(res_outer_aliased.fields[0].ts_type, "string");
    assert_eq!(res_outer_aliased.fields[1].ts_type, "string | null");
    assert_eq!(res_outer_aliased.fields[2].ts_type, "string | null");

    // Ambiguous column in outer aliased join must still fail
    let sql_outer_ambig = r#"
        SELECT j.status
        FROM other o
        LEFT JOIN (t1 JOIN t2 ON t1.id = t2.id) AS j
        ON o.id = 1;
    "#;
    let err_outer_ambig = analyze_query(sql_outer_ambig, &catalog, None).unwrap_err();
    assert!(err_outer_ambig.contains("ambiguous"));

    // 8. Scope Isolation: alias 'j' MUST NOT leak into catalog
    assert!(!catalog.tables.contains_key("j"));
    let sql_leaked = "SELECT * FROM j;";
    let err_leaked = analyze_query(sql_leaked, &catalog, None).unwrap_err();
    assert!(
        err_leaked.contains("does not exist"),
        "Subsequent query referencing leaked alias must fail, got: {}",
        err_leaked
    );
}
