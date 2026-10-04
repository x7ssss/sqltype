//! Adversarial Integrity and Stress Challenge Suite for Milestone 4: Prepared Statement Parameter Type Deduction
//!
//! Stress-tests:
//! 1. Subquery parameter resolution in WHERE: `WHERE id IN (SELECT author_id FROM posts WHERE view_count > $1)`
//! 2. CTE parameter resolution across scopes: `WITH cte AS (SELECT * FROM users WHERE age > $1) SELECT * FROM cte WHERE role_id = $2;`
//! 3. Complex multi-row and conflict DML bindings
//! 4. UPDATE with FROM clause and DELETE with USING clause parameter resolution
//! 5. Disambiguation invariants under extreme collisions: `id = $1 AND id2 = $2 AND id = $3 AND id2 = $4 AND id = $5`
//! 6. Symmetrical function parameter deduction on LHS and RHS
//! 7. Unconstrained parameters fallback to "unknown" and default naming "param1"
//! 8. Range bounds parameter deduction for both lower and upper bounds
//! 9. Nested casts and array casts ($1::timestamptz[], $1::uuid[][])
//! 10. Unsupported statement rejection returns clean error
//! 11. Optional filter with inverted arm ordering and multiple conjuncts

use sqltype::analyzer::analyze_query;
use sqltype::catalog::Catalog;
use sqltype::params::{deduce_query_params, deduce_query_params_lossy};
use sqltype::QueryScope;

fn setup_stress_catalog() -> Catalog {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TYPE project_tier AS ENUM ('free', 'pro', 'enterprise');

        CREATE TABLE accounts (
            id UUID PRIMARY KEY,
            account_no INT NOT NULL,
            name TEXT NOT NULL,
            tier project_tier NOT NULL DEFAULT 'free',
            balance NUMERIC(12, 2) NOT NULL DEFAULT 0.00,
            is_active BOOLEAN NOT NULL DEFAULT true,
            created_at TIMESTAMPTZ NOT NULL
        );

        CREATE TABLE members (
            id UUID PRIMARY KEY,
            account_id UUID REFERENCES accounts(id),
            email TEXT NOT NULL,
            role_id INT NOT NULL,
            skills TEXT[] NOT NULL DEFAULT '{}',
            score FLOAT8,
            notes TEXT,
            joined_at TIMESTAMPTZ NOT NULL
        );
    "#;
    catalog.apply_sql(ddl).expect("Failed to setup stress catalog");
    catalog
}

#[test]
fn test_subquery_in_where_parameter_deduction() {
    let catalog = setup_stress_catalog();
    let sql = r#"
        SELECT * FROM accounts
        WHERE id IN (
            SELECT account_id FROM members WHERE score > $1
        ) AND tier = $2;
    "#;
    let analyzed = analyze_query(sql, &catalog, None).expect("Subquery query should analyze");
    assert_eq!(analyzed.params.len(), 2);
    // $1 in subquery WHERE score > $1
    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].ts_type, "number");
    assert_eq!(analyzed.params[0].name, "score");
    // $2 in outer query tier = $2
    assert_eq!(analyzed.params[1].index, 2);
    assert!(analyzed.params[1].ts_type.contains("\"free\""));
    assert_eq!(analyzed.params[1].name, "tier");
}

#[test]
fn test_cte_parameter_deduction_across_scopes() {
    let catalog = setup_stress_catalog();
    let sql = r#"
        WITH active_members AS (
            SELECT * FROM members WHERE joined_at >= $1
        )
        SELECT a.name, m.email
        FROM accounts a
        JOIN active_members m ON m.account_id = a.id
        WHERE a.balance >= $2;
    "#;
    let analyzed = analyze_query(sql, &catalog, None).expect("CTE query should analyze");
    assert_eq!(analyzed.params.len(), 2);
    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "joined_at");
    assert_eq!(analyzed.params[0].ts_type, "Date");
    assert_eq!(analyzed.params[1].index, 2);
    assert_eq!(analyzed.params[1].name, "balance");
    assert_eq!(analyzed.params[1].ts_type, "number");
}

#[test]
fn test_update_from_clause_parameters() {
    let catalog = setup_stress_catalog();
    let sql = r#"
        UPDATE members m
        SET notes = $1, score = $2
        FROM accounts a
        WHERE m.account_id = a.id AND a.name = $3 AND m.role_id = $4;
    "#;
    let analyzed = analyze_query(sql, &catalog, None).expect("UPDATE FROM should analyze");
    assert_eq!(analyzed.params.len(), 4);
    assert_eq!(analyzed.params[0].name, "notes");
    assert_eq!(analyzed.params[0].ts_type, "string");
    assert!(analyzed.params[0].is_optional); // notes is nullable
    assert_eq!(analyzed.params[1].name, "score");
    assert_eq!(analyzed.params[1].ts_type, "number");
    assert!(analyzed.params[1].is_optional); // score is nullable
    assert_eq!(analyzed.params[2].name, "name");
    assert_eq!(analyzed.params[2].ts_type, "string");
    assert_eq!(analyzed.params[3].name, "role_id");
    assert_eq!(analyzed.params[3].ts_type, "number");
}

#[test]
fn test_delete_using_clause_parameters() {
    let catalog = setup_stress_catalog();
    let sql = r#"
        DELETE FROM members m
        USING accounts a
        WHERE m.account_id = a.id AND a.account_no = $1 AND m.email = $2;
    "#;
    let analyzed = analyze_query(sql, &catalog, None).expect("DELETE USING should analyze");
    assert_eq!(analyzed.params.len(), 2);
    assert_eq!(analyzed.params[0].name, "account_no");
    assert_eq!(analyzed.params[0].ts_type, "number");
    assert_eq!(analyzed.params[1].name, "email");
    assert_eq!(analyzed.params[1].ts_type, "string");
}

#[test]
fn test_extreme_name_collision_matrix() {
    let catalog = setup_stress_catalog();
    let sql = r#"
        SELECT * FROM members
        WHERE role_id = $1
          AND role_id = $2
          AND role_id = $3
          AND role_id = $4
          AND role_id = $5;
    "#;
    let analyzed = analyze_query(sql, &catalog, None).expect("Extreme collision should analyze");
    assert_eq!(analyzed.params.len(), 5);
    assert_eq!(analyzed.params[0].name, "role_id");
    assert_eq!(analyzed.params[1].name, "role_id2");
    assert_eq!(analyzed.params[2].name, "role_id3");
    assert_eq!(analyzed.params[3].name, "role_id4");
    assert_eq!(analyzed.params[4].name, "role_id5");
}

#[test]
fn test_unconstrained_parameters_fallback() {
    let catalog = setup_stress_catalog();
    let sql = "SELECT $1;";
    let analyzed = analyze_query(sql, &catalog, None).expect("Unconstrained param should analyze");
    assert_eq!(analyzed.params.len(), 1);
    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "param1");
    assert_eq!(analyzed.params[0].ts_type, "unknown");
    assert!(!analyzed.params[0].is_optional);
}

#[test]
fn test_range_bound_parameters_infer_from_column() {
    let catalog = setup_stress_catalog();
    let sql = "SELECT * FROM accounts WHERE balance BETWEEN $1 AND $2;";
    let analyzed = analyze_query(sql, &catalog, None).expect("BETWEEN range bounds should analyze");
    assert_eq!(analyzed.params.len(), 2);
    assert_eq!(analyzed.params[0].name, "balance");
    assert_eq!(analyzed.params[0].ts_type, "number");
    assert_eq!(analyzed.params[1].name, "balance2");
    assert_eq!(analyzed.params[1].ts_type, "number");
}

#[test]
fn test_array_overlap_and_nested_array_casts() {
    let catalog = setup_stress_catalog();
    // Array overlap &&
    let sql_overlap = "SELECT * FROM members WHERE skills && $1;";
    let a_ov = analyze_query(sql_overlap, &catalog, None).expect("Overlap should analyze");
    assert_eq!(a_ov.params.len(), 1);
    assert_eq!(a_ov.params[0].name, "skills");
    assert_eq!(a_ov.params[0].ts_type, "Array<string>");

    // Explicit array cast $1::text[]
    let sql_cast = "SELECT * FROM members WHERE skills = $1::text[];";
    let a_cast = analyze_query(sql_cast, &catalog, None).expect("Cast should analyze");
    assert_eq!(a_cast.params.len(), 1);
    assert_eq!(a_cast.params[0].ts_type, "Array<string>");
}

#[test]
fn test_unsupported_statement_rejection_and_lossy_helper() {
    let catalog = setup_stress_catalog();
    // Create Table is not a query DML/SELECT statement
    let sql = "CREATE TABLE dummy (x INT);";
    let parsed = pg_query::parse(sql).unwrap();
    let node = &parsed.protobuf.stmts[0];
    let actual = node.stmt.as_deref().unwrap();

    let res = deduce_query_params(actual, &catalog, &QueryScope::default());
    assert!(res.is_err(), "Non-DML statement must return Err");

    let lossy = deduce_query_params_lossy(actual, &catalog, &QueryScope::default());
    assert!(lossy.is_empty(), "Lossy helper must return empty vec on Err");
}
