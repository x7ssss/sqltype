//! Independent Forensic Auditor Stress-Test Suite for Milestone 3 (R3: Join Tree Nullability Inference Engine)
//!
//! Validates:
//! 1. Complex nested join tree nullability propagation (LEFT inside RIGHT inside INNER)
//! 2. 3-way self-joins with mixed outer joins and catalog DDL immutability
//! 3. Cross-query alias scope isolation and cache leakage prevention
//! 4. Robust TypeScript type projection on complex types (literals, tuples, deep objects)
//! 5. Cross-schema join nullability propagation directly via nullability engine
//! 6. Join ambiguity detection and aliased column mapping

use sqltype::analyzer::analyze_query;
use sqltype::catalog::Catalog;
use sqltype::nullability::{
    JoinKind, JoinTreeNode, format_nullable, propagate_join_nullability, strip_root_null,
};

#[test]
fn test_adversarial_deep_nested_mixed_joins() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE t_a (id INT PRIMARY KEY, val_a TEXT NOT NULL);
        CREATE TABLE t_b (id INT PRIMARY KEY, val_b TEXT NOT NULL);
        CREATE TABLE t_c (id INT PRIMARY KEY, val_c TEXT NOT NULL);
        CREATE TABLE t_d (id INT PRIMARY KEY, val_d TEXT NOT NULL);
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // ((t_a LEFT JOIN t_b) RIGHT JOIN t_c) INNER JOIN t_d
    let sql = r#"
        SELECT
            t_a.val_a,
            t_b.val_b,
            t_c.val_c,
            t_d.val_d
        FROM ((t_a LEFT JOIN t_b ON t_a.id = t_b.id)
              RIGHT JOIN t_c ON t_a.id = t_c.id)
        INNER JOIN t_d ON t_c.id = t_d.id;
    "#;

    let analyzed =
        analyze_query(sql, &catalog, None).expect("Query must parse and analyze cleanly");
    assert_eq!(analyzed.fields.len(), 4);

    assert_eq!(analyzed.fields[0].name, "val_a");
    assert_eq!(
        analyzed.fields[0].ts_type, "string | null",
        "val_a must be nullable because left subtree is nullified by RIGHT JOIN"
    );

    assert_eq!(analyzed.fields[1].name, "val_b");
    assert_eq!(
        analyzed.fields[1].ts_type, "string | null",
        "val_b must be nullable from both LEFT JOIN and parent RIGHT JOIN"
    );

    assert_eq!(analyzed.fields[2].name, "val_c");
    assert_eq!(
        analyzed.fields[2].ts_type, "string",
        "val_c must remain NOT NULL as preserved side of RIGHT JOIN and INNER JOIN"
    );

    assert_eq!(analyzed.fields[3].name, "val_d");
    assert_eq!(
        analyzed.fields[3].ts_type, "string",
        "val_d must remain NOT NULL as side of INNER JOIN"
    );
}

#[test]
fn test_adversarial_three_way_self_join_and_catalog_invariance() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE nodes (
            id INT PRIMARY KEY,
            parent_id INT,
            label TEXT NOT NULL
        );
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // 3-way self-join: (n1 LEFT JOIN n2) RIGHT JOIN n3
    let sql = r#"
        SELECT
            n1.label AS root_label,
            n2.label AS mid_label,
            n3.label AS leaf_label
        FROM (nodes n1 LEFT JOIN nodes n2 ON n1.id = n2.parent_id)
        RIGHT JOIN nodes n3 ON n2.id = n3.parent_id;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("3-way self join must succeed");
    assert_eq!(analyzed.fields[0].name, "root_label");
    assert_eq!(analyzed.fields[0].ts_type, "string | null");

    assert_eq!(analyzed.fields[1].name, "mid_label");
    assert_eq!(analyzed.fields[1].ts_type, "string | null");

    assert_eq!(analyzed.fields[2].name, "leaf_label");
    assert_eq!(analyzed.fields[2].ts_type, "string");

    // Catalog invariance: verify underlying nodes table was not mutated!
    let nodes_tbl = catalog.get_table("nodes").expect("nodes table must exist");
    assert!(
        !nodes_tbl.get_column("label").unwrap().is_nullable,
        "nodes.label DDL nullability must remain false"
    );
    assert!(
        !nodes_tbl.get_column("id").unwrap().is_nullable,
        "nodes.id DDL nullability must remain false"
    );
    assert!(
        catalog.get_table("n1").is_none(),
        "Alias n1 must not exist in catalog"
    );
    assert!(
        catalog.get_table("n2").is_none(),
        "Alias n2 must not exist in catalog"
    );
    assert!(
        catalog.get_table("n3").is_none(),
        "Alias n3 must not exist in catalog"
    );
}

#[test]
fn test_adversarial_scope_isolation_and_cross_query_leakage() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE products (id INT PRIMARY KEY, sku TEXT NOT NULL);
        CREATE TABLE categories (id INT PRIMARY KEY, name TEXT NOT NULL);
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // Query 1: Join with alias `cp`
    let q1 = r#"
        SELECT cp.sku, cp.name
        FROM (products p JOIN categories c ON p.id = c.id) AS cp;
    "#;
    let res1 = analyze_query(q1, &catalog, None).expect("Query 1 must succeed");
    assert_eq!(res1.fields[0].name, "sku");
    assert_eq!(res1.fields[1].name, "name");

    // Query 2: Attempting to reference `cp` in a query without defining it must FAIL!
    let q2 = "SELECT sku FROM cp;";
    let res2 = analyze_query(q2, &catalog, None);
    assert!(
        res2.is_err(),
        "Referencing leaked alias cp across queries must return error"
    );
    let err2 = res2.unwrap_err();
    assert!(
        err2.contains("does not exist"),
        "Expected table does not exist error, got: {}",
        err2
    );

    // Query 3: Using `cp` as an alias for a completely different table must succeed without collision
    let q3 = "SELECT cp.sku FROM products cp;";
    let res3 = analyze_query(q3, &catalog, None).expect("Query 3 reusing alias cp must succeed");
    assert_eq!(res3.fields[0].name, "sku");
    assert_eq!(res3.fields[0].ts_type, "string");
}

#[test]
fn test_adversarial_format_nullable_complex_type_stress() {
    // 1. Quoted pipes inside string literal union types
    let literal_union = "'pending' | 'completed' | 'failed'";
    assert_eq!(
        format_nullable(literal_union),
        "'pending' | 'completed' | 'failed' | null"
    );

    // 2. String literal containing pipe characters inside quotes
    let quoted_pipe = "'value|with|pipe'";
    assert_eq!(format_nullable(quoted_pipe), "'value|with|pipe' | null");

    // 3. Deeply nested composite object
    let deep_obj = "{ meta: { details: { sub: string | null } } }";
    assert_eq!(
        format_nullable(deep_obj),
        "{ meta: { details: { sub: string | null } } } | null"
    );
    assert_eq!(
        strip_root_null("{ meta: { details: { sub: string | null } } } | null"),
        "{ meta: { details: { sub: string | null } } }"
    );

    // 4. Tuples with inner nulls
    let tuple_type = "[string, number | null]";
    assert_eq!(
        format_nullable(tuple_type),
        "[string, number | null] | null"
    );
    assert_eq!(
        strip_root_null("[string, number | null] | null"),
        "[string, number | null]"
    );

    // 5. Generics with nested objects
    let generic_type = "Record<string, Array<{ id: number | null }>>";
    assert_eq!(
        format_nullable(generic_type),
        "Record<string, Array<{ id: number | null }>> | null"
    );

    // 6. Already nullable at root level must not double wrap
    assert_eq!(format_nullable("string | null"), "string | null");
    assert_eq!(
        format_nullable("{ x: number } | null"),
        "{ x: number } | null"
    );
}

#[test]
fn test_adversarial_cross_schema_join_nullability_via_engine() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE public.tenants (
            id INT PRIMARY KEY,
            domain TEXT NOT NULL
        );
        CREATE TABLE custom_audit.logs (
            id INT PRIMARY KEY,
            tenant_id INT NOT NULL,
            payload JSONB NOT NULL
        );
    "#;
    catalog.apply_sql(ddl).expect("Failed to apply DDL");

    // Use nullability engine directly: SELECT * FROM public.tenants t LEFT JOIN custom_audit.logs l ON t.id = l.tenant_id
    let parse_result = pg_query::parse(
        "SELECT * FROM public.tenants t LEFT JOIN custom_audit.logs l ON t.id = l.tenant_id;",
    )
    .unwrap();
    let stmt = parse_result.protobuf.stmts.first().unwrap();
    if let Some(node) = &stmt.stmt
        && let Some(pg_query::NodeEnum::SelectStmt(select)) = &node.node
    {
        let scope = propagate_join_nullability(&select.from_clause, &catalog);
        let t_binding = scope.bindings.get("t").expect("Binding 't' must exist");
        let l_binding = scope.bindings.get("l").expect("Binding 'l' must exist");

        assert!(
            !t_binding.is_null_producing(),
            "public.tenants must NOT be null-producing"
        );
        assert!(
            l_binding.is_null_producing(),
            "custom_audit.logs must BE null-producing"
        );

        let domain_col = t_binding.get_column("domain").unwrap();
        let payload_col = l_binding.get_column("payload").unwrap();

        assert!(
            !domain_col.effective_nullable(t_binding.is_null_producing()),
            "t.domain must not be nullable"
        );
        assert!(
            payload_col.effective_nullable(l_binding.is_null_producing()),
            "l.payload must be nullable"
        );
    }
}

#[test]
fn test_adversarial_join_tree_node_api_direct() {
    // Construct tree: ((T1 FULL JOIN T2) INNER JOIN T3)
    let t1 = JoinTreeNode::Table {
        rel_name: "t1".to_string(),
        schema: None,
        alias: None,
        colnames: Vec::new(),
        is_null_producing: false,
    };
    let t2 = JoinTreeNode::Table {
        rel_name: "t2".to_string(),
        schema: None,
        alias: None,
        colnames: Vec::new(),
        is_null_producing: false,
    };
    let full = JoinTreeNode::Join {
        kind: JoinKind::Full,
        left: Box::new(t1),
        right: Box::new(t2),
        alias: None,
        is_null_producing: false,
    };
    let t3 = JoinTreeNode::Table {
        rel_name: "t3".to_string(),
        schema: None,
        alias: None,
        colnames: Vec::new(),
        is_null_producing: false,
    };
    let mut root = JoinTreeNode::Join {
        kind: JoinKind::Inner,
        left: Box::new(full),
        right: Box::new(t3),
        alias: None,
        is_null_producing: false,
    };

    root.propagate_nullability(false);
    let nulls = root.collect_table_nullabilities();

    let t1_null = nulls.iter().find(|t| t.0 == "t1").unwrap().2;
    let t2_null = nulls.iter().find(|t| t.0 == "t2").unwrap().2;
    let t3_null = nulls.iter().find(|t| t.0 == "t3").unwrap().2;

    assert!(t1_null, "t1 in FULL JOIN must be null-producing");
    assert!(t2_null, "t2 in FULL JOIN must be null-producing");
    assert!(!t3_null, "t3 in INNER JOIN must NOT be null-producing");
}
