//! Comprehensive Test Suite for Milestone 3 (R3: Join Tree Nullability Inference Engine)
//!
//! Tests:
//! 1. Nested outer joins: `(A LEFT JOIN B) FULL JOIN (C RIGHT JOIN D)`
//! 2. Self-joins: `employees e LEFT JOIN employees m ON e.manager_id = m.id`
//! 3. Aliased joins: `(a JOIN b) AS j` with scope isolation and ambiguity detection
//! 4. Composite type column nullability projection under outer joins
//! 5. Preservation of DDL nullability vs join-induced nullability
//! 6. Exact TypeScript interface code generation

use sqltype::analyzer::analyze_query;
use sqltype::catalog::Catalog;
use sqltype::codegen::generate_file_ts;
use sqltype::nullability::{
    ColumnBinding, JoinKind, JoinTreeNode, format_nullable, project_ts_type,
    propagate_join_nullability, strip_root_null,
};

#[test]
fn test_nested_outer_joins_full_propagation() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE a (id INT PRIMARY KEY, a_val TEXT NOT NULL);
        CREATE TABLE b (id INT PRIMARY KEY, b_val TEXT NOT NULL);
        CREATE TABLE c (id INT PRIMARY KEY, c_val TEXT NOT NULL);
        CREATE TABLE d (id INT PRIMARY KEY, d_val TEXT NOT NULL);
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // (A LEFT JOIN B) FULL JOIN (C RIGHT JOIN D)
    let sql = r#"
        SELECT
            a.a_val,
            b.b_val,
            c.c_val,
            d.d_val
        FROM (a LEFT JOIN b ON a.id = b.id)
        FULL JOIN (c RIGHT JOIN d ON c.id = d.id)
        ON a.id = d.id;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("Query analysis should succeed");
    assert_eq!(analyzed.fields.len(), 4);

    // Because the outer join is FULL JOIN, BOTH subtrees are null-producing.
    // In (A LEFT JOIN B), A inherits null-producing, and B is null-producing from LEFT JOIN.
    // In (C RIGHT JOIN D), C is null-producing from RIGHT JOIN, and D inherits null-producing.
    // Therefore, all 4 columns must project as string | null.
    assert_eq!(analyzed.fields[0].name, "a_val");
    assert_eq!(analyzed.fields[0].ts_type, "string | null");

    assert_eq!(analyzed.fields[1].name, "b_val");
    assert_eq!(analyzed.fields[1].ts_type, "string | null");

    assert_eq!(analyzed.fields[2].name, "c_val");
    assert_eq!(analyzed.fields[2].ts_type, "string | null");

    assert_eq!(analyzed.fields[3].name, "d_val");
    assert_eq!(analyzed.fields[3].ts_type, "string | null");

    // Verify TypeScript codegen contains exact nullable fields
    let ts = generate_file_ts(&[analyzed]);
    assert!(ts.contains("a_val: string | null;"));
    assert!(ts.contains("b_val: string | null;"));
    assert!(ts.contains("c_val: string | null;"));
    assert!(ts.contains("d_val: string | null;"));
}

#[test]
fn test_nested_outer_join_mixed_nullability() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE orders (id INT PRIMARY KEY, order_num TEXT NOT NULL);
        CREATE TABLE items (id INT PRIMARY KEY, order_id INT NOT NULL, name TEXT NOT NULL);
        CREATE TABLE discounts (id INT PRIMARY KEY, item_id INT NOT NULL, code TEXT NOT NULL);
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // orders LEFT JOIN (items JOIN discounts)
    // Left side: orders (NOT null-producing)
    // Right side: (items JOIN discounts) is on the right of LEFT JOIN -> BOTH items and discounts are null-producing!
    let sql = r#"
        SELECT
            o.order_num,
            i.name AS item_name,
            d.code AS discount_code
        FROM orders o
        LEFT JOIN (items i JOIN discounts d ON i.id = d.item_id)
        ON o.id = i.order_id;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("Query analysis should succeed");
    assert_eq!(analyzed.fields[0].name, "order_num");
    assert_eq!(
        analyzed.fields[0].ts_type, "string",
        "orders is left side of LEFT JOIN, must be NOT NULL"
    );

    assert_eq!(analyzed.fields[1].name, "item_name");
    assert_eq!(
        analyzed.fields[1].ts_type, "string | null",
        "items is inside right side of LEFT JOIN, must be nullable"
    );

    assert_eq!(analyzed.fields[2].name, "discount_code");
    assert_eq!(
        analyzed.fields[2].ts_type, "string | null",
        "discounts is inside right side of LEFT JOIN, must be nullable"
    );
}

#[test]
fn test_self_join_nullability_and_ddl_preservation() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE employees (
            id INT PRIMARY KEY,
            name TEXT NOT NULL,
            manager_id INT
        );
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // Self-join: employees e LEFT JOIN employees m
    let sql = r#"
        SELECT
            e.id AS emp_id,
            e.name AS emp_name,
            m.id AS mgr_id,
            m.name AS mgr_name
        FROM employees e
        LEFT JOIN employees m ON e.manager_id = m.id;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("Query analysis should succeed");
    assert_eq!(analyzed.fields[0].name, "emp_id");
    assert_eq!(analyzed.fields[0].ts_type, "number");

    assert_eq!(analyzed.fields[1].name, "emp_name");
    assert_eq!(analyzed.fields[1].ts_type, "string");

    assert_eq!(analyzed.fields[2].name, "mgr_id");
    assert_eq!(analyzed.fields[2].ts_type, "number | null");

    assert_eq!(analyzed.fields[3].name, "mgr_name");
    assert_eq!(analyzed.fields[3].ts_type, "string | null");

    // Verify DDL preservation: Underlying employees table columns must NOT be mutated destructively!
    let emp_table = catalog
        .get_table("employees")
        .expect("Table must exist in catalog");
    let name_col = emp_table
        .get_column("name")
        .expect("name column must exist");
    assert!(
        !name_col.is_nullable,
        "DDL nullability of employees.name must remain false in catalog"
    );
    let id_col = emp_table.get_column("id").expect("id column must exist");
    assert!(
        !id_col.is_nullable,
        "DDL nullability of employees.id must remain false in catalog"
    );
}

#[test]
fn test_aliased_join_scope_isolation_and_no_catalog_pollution() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE customers (
            cust_id INT PRIMARY KEY,
            email TEXT NOT NULL
        );
        CREATE TABLE orders (
            order_id INT PRIMARY KEY,
            customer_id INT NOT NULL,
            total NUMERIC NOT NULL
        );
        CREATE TABLE shipments (
            ship_id INT PRIMARY KEY,
            order_id INT NOT NULL
        );
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // 1. Query with an aliased join `(customers c JOIN orders o) AS j`
    let query1 = r#"
        SELECT
            j.cust_id,
            j.email,
            j.order_id,
            j.total
        FROM (customers c JOIN orders o ON c.cust_id = o.customer_id) AS j;
    "#;

    let analyzed1 = analyze_query(query1, &catalog, None).expect("Aliased join query must succeed");
    assert_eq!(analyzed1.fields.len(), 4);
    assert_eq!(analyzed1.fields[0].name, "cust_id");
    assert_eq!(analyzed1.fields[0].ts_type, "number");
    assert_eq!(analyzed1.fields[1].name, "email");
    assert_eq!(analyzed1.fields[1].ts_type, "string");

    // 2. CRITICAL SCOPE ISOLATION TEST:
    // Join alias `j` must NOT have been inserted into `catalog.tables`!
    assert!(
        catalog.get_table("j").is_none(),
        "Scope isolation failure: Join alias 'j' leaked into persistent catalog.tables"
    );
    assert!(
        !catalog.tables.contains_key("j"),
        "Scope isolation failure: catalog.tables contains key 'j'"
    );

    // 3. Second query: Left join onto aliased join
    let query2 = r#"
        SELECT
            s.ship_id,
            j.email,
            j.total
        FROM shipments s
        LEFT JOIN (customers c JOIN orders o ON c.cust_id = o.customer_id) AS j
        ON s.order_id = j.order_id;
    "#;

    let analyzed2 =
        analyze_query(query2, &catalog, None).expect("Left join with aliased join must succeed");
    assert_eq!(analyzed2.fields[0].name, "ship_id");
    assert_eq!(analyzed2.fields[0].ts_type, "number");
    assert_eq!(analyzed2.fields[1].name, "email");
    assert_eq!(analyzed2.fields[1].ts_type, "string | null");
    assert_eq!(analyzed2.fields[2].name, "total");
    assert_eq!(analyzed2.fields[2].ts_type, "number | null");

    // Again verify catalog isolation
    assert!(catalog.get_table("j").is_none());
}

#[test]
fn test_aliased_join_ambiguous_column_detection() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE t1 (id INT PRIMARY KEY, status TEXT NOT NULL);
        CREATE TABLE t2 (id INT PRIMARY KEY, status TEXT NOT NULL);
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // Both t1 and t2 have 'status'. Referencing j.status must be flagged as ambiguous.
    let sql = r#"
        SELECT j.status
        FROM (t1 JOIN t2 ON t1.id = t2.id) AS j;
    "#;

    let result = analyze_query(sql, &catalog, None);
    assert!(
        result.is_err(),
        "Referencing duplicate column on aliased join must return error"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("ambiguous"),
        "Error message should mention ambiguous column, got: {}",
        err
    );
}

#[test]
fn test_composite_type_nullability_projection_under_outer_joins() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TYPE address_t AS (
            street TEXT,
            zip TEXT
        );
        CREATE TYPE company_t AS (
            corp_name TEXT,
            hq address_t
        );
        CREATE TABLE users (
            id INT PRIMARY KEY,
            username TEXT NOT NULL
        );
        CREATE TABLE profiles (
            user_id INT PRIMARY KEY,
            home address_t NOT NULL,
            work address_t,
            employer company_t NOT NULL
        );
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // 1. LEFT JOIN: profiles is null-producing
    let sql_outer = r#"
        SELECT
            u.id,
            u.username,
            p.home,
            p.work,
            p.employer
        FROM users u
        LEFT JOIN profiles p ON u.id = p.user_id;
    "#;

    let analyzed_outer =
        analyze_query(sql_outer, &catalog, None).expect("Outer join query must succeed");
    assert_eq!(analyzed_outer.fields[0].name, "id");
    assert_eq!(analyzed_outer.fields[0].ts_type, "number");

    assert_eq!(analyzed_outer.fields[1].name, "username");
    assert_eq!(analyzed_outer.fields[1].ts_type, "string");

    // Composite type columns under outer join must wrap the outer object with `| null`!
    assert_eq!(analyzed_outer.fields[2].name, "home");
    assert_eq!(
        analyzed_outer.fields[2].ts_type, "{ street: string; zip: string } | null",
        "Outer join must wrap composite type with | null without substring false-positives"
    );

    assert_eq!(analyzed_outer.fields[3].name, "work");
    assert_eq!(
        analyzed_outer.fields[3].ts_type,
        "{ street: string; zip: string } | null"
    );

    // Nested composite type: company_t contains address_t.
    assert_eq!(analyzed_outer.fields[4].name, "employer");
    assert_eq!(
        analyzed_outer.fields[4].ts_type,
        "{ corp_name: string; hq: { street: string; zip: string } } | null",
        "Nested composite type under outer join must wrap outer object with | null"
    );

    // 2. INNER JOIN on same tables: profiles is NOT null-producing
    let sql_inner = r#"
        SELECT
            u.id,
            p.home,
            p.employer
        FROM users u
        INNER JOIN profiles p ON u.id = p.user_id;
    "#;

    let analyzed_inner =
        analyze_query(sql_inner, &catalog, None).expect("Inner join query must succeed");
    // Under inner join, home is NOT null-producing and home was defined NOT NULL in DDL!
    // So the outer object must NOT have `| null` appended!
    assert_eq!(analyzed_inner.fields[1].name, "home");
    assert_eq!(
        analyzed_inner.fields[1].ts_type, "{ street: string; zip: string }",
        "Inner join on NOT NULL composite column must NOT have outer | null"
    );

    assert_eq!(analyzed_inner.fields[2].name, "employer");
    assert_eq!(
        analyzed_inner.fields[2].ts_type,
        "{ corp_name: string; hq: { street: string; zip: string } }",
        "Inner join on NOT NULL nested composite column must NOT have outer | null"
    );

    // Verify codegen for the outer join
    let ts_code = generate_file_ts(&[analyzed_outer]);
    assert!(
        ts_code.contains("home: { street: string; zip: string } | null;"),
        "TypeScript codegen must contain exact outer nullable composite type:\n{}",
        ts_code
    );
    assert!(
        ts_code.contains(
            "employer: { corp_name: string; hq: { street: string; zip: string } } | null;"
        ),
        "TypeScript codegen must contain exact nested composite outer nullable type:\n{}",
        ts_code
    );
}

#[test]
fn test_standalone_nullability_helpers() {
    // 1. Primitive formatting
    assert_eq!(format_nullable("string"), "string | null");
    assert_eq!(format_nullable("number | null"), "number | null");
    assert_eq!(format_nullable("unknown"), "unknown");

    // 2. Object formatting with internal nulls (e.g. composite types with inner nullable properties)
    let inner_null = "{ lat: number; lng: number | null }";
    assert_eq!(
        format_nullable(inner_null),
        "{ lat: number; lng: number | null } | null",
        "Object with inner null must still be wrapped with | null when outer is nullable"
    );

    // 3. Object already containing root null
    let already_null = "{ lat: number; lng: number | null } | null";
    assert_eq!(format_nullable(already_null), already_null);

    // 4. Strip root null without damaging inner nullable properties
    assert_eq!(strip_root_null("string | null"), "string");
    assert_eq!(strip_root_null("string"), "string");
    assert_eq!(
        strip_root_null("{ a: string | null } | null"),
        "{ a: string | null }"
    );
    assert_eq!(
        strip_root_null("{ a: string | null }"),
        "{ a: string | null }"
    );

    // 5. Project TS type helper
    assert_eq!(project_ts_type("string", true), "string | null");
    assert_eq!(project_ts_type("string", false), "string");
    assert_eq!(project_ts_type("string | null", false), "string");
    assert_eq!(
        project_ts_type("{ x: number | null }", true),
        "{ x: number | null } | null"
    );
    assert_eq!(
        project_ts_type("{ x: number | null }", false),
        "{ x: number | null }"
    );
}

#[test]
fn test_column_binding_and_join_tree_node() {
    let col = ColumnBinding::new(
        "score".to_string(),
        "int4".to_string(),
        "number".to_string(),
        false, // ddl_nullable = false
        false,
        false,
    );

    // In a non-null-producing table binding
    assert!(!col.effective_nullable(false));
    // In a null-producing table binding
    assert!(col.effective_nullable(true));
    // DDL nullability remains false
    assert!(!col.ddl_nullable);

    // Test JoinTreeNode top-down propagation across multiple join levels
    // Tree: A LEFT JOIN (B FULL JOIN C)
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
    let right_tree = JoinTreeNode::Join {
        kind: JoinKind::Full,
        left: Box::new(b_node),
        right: Box::new(c_node),
        alias: None,
        is_null_producing: false,
    };
    let a_node = JoinTreeNode::Table {
        rel_name: "a".to_string(),
        schema: None,
        alias: None,
        colnames: Vec::new(),
        is_null_producing: false,
    };
    let mut root = JoinTreeNode::Join {
        kind: JoinKind::Left,
        left: Box::new(a_node),
        right: Box::new(right_tree),
        alias: None,
        is_null_producing: false,
    };

    root.propagate_nullability(false);
    let nulls = root.collect_table_nullabilities();
    let a_null = nulls.iter().find(|t| t.0 == "a").unwrap().2;
    let b_null = nulls.iter().find(|t| t.0 == "b").unwrap().2;
    let c_null = nulls.iter().find(|t| t.0 == "c").unwrap().2;

    assert!(!a_null, "Table A must NOT be null-producing");
    assert!(b_null, "Table B must be null-producing");
    assert!(c_null, "Table C must be null-producing");
}

#[test]
fn test_propagate_join_nullability_api() {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql("CREATE TABLE users (id INT PRIMARY KEY, name TEXT NOT NULL);")
        .unwrap();

    let parse_result = pg_query::parse("SELECT * FROM users u").unwrap();
    let stmt = parse_result.protobuf.stmts.first().unwrap();
    if let Some(node) = &stmt.stmt
        && let Some(pg_query::NodeEnum::SelectStmt(select)) = &node.node
    {
        let scope = propagate_join_nullability(&select.from_clause, &catalog);
        assert!(scope.bindings.contains_key("u"));
        assert!(!scope.bindings["u"].is_null_producing());
        assert!(!scope.bindings["u"].columns["name"].ddl_nullable);
    }
}
