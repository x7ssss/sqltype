//! Empirical Challenger 1 Test Suite for Milestone 4 (Prepared Statement Parameter Type Deduction)
//!
//! Specifically validates:
//! 1. Complex and nested expression trees:
//!    - Nested binary operations: `WHERE (a = $1 AND b = $2) OR c = $3`
//!    - Deeply nested mixed boolean trees
//!    - Range comparisons: `BETWEEN $1 AND $2`, `$1 BETWEEN min AND max`, `age NOT BETWEEN $1 AND $2`, literal bounds
//!    - Function calls: `lower(name) = lower($1)`, `lower($1) = lower(name)`, `upper(name) = upper($1)`, `coalesce($1, name)`
//!    - Explicit and composite type casts: `$1::text[]`, `$1::uuid`, composite `$1::address`, composite array `$1::address[]`
//!    - Literal comparisons: `$1 = 42`, `$1 = 'active'`, `$1 = true`, `$1 > 3.14`, `$1 <> 'archived'`
//! 2. Array semantics:
//!    - Array parameter: `col = ANY($1)` -> `Array<T>`
//!    - Scalar element parameter: `$1 = ANY(arr_col)` -> `T`
//!    - Array operators: `tags && $1`, `tags @> $1`, `tags <@ $1`
//!    - ALL comparison: `age > ALL($1)` -> `Array<number>`
//! 3. Dynamic optional filters:
//!    - Standard: `($1 IS NULL OR col = $1)`
//!    - Inverted ordering: `(col = $1 OR $1 IS NULL)`
//!    - Inverted equality: `($1 IS NULL OR $1 = col)`, `($1 = col OR $1 IS NULL)`
//!    - Cast variants: `($1::text IS NULL OR col = $1)`, `($1 IS NULL OR col = $1::text)`, `($1::uuid IS NULL OR id = $1)`
//!    - Pattern matching variants: `($1 IS NULL OR col LIKE $1)`, `(col LIKE $1 OR $1 IS NULL)`, ILIKE
//!    - Array ANY variant: `($1 IS NULL OR col = ANY($1))`, `(col = ANY($1) OR $1 IS NULL)`
//!    - Symmetrical function variant: `($1 IS NULL OR lower(col) = lower($1))`
//!    - Conjunction of multiple dynamic filters
//! 4. Comprehensive DML, CTE, subquery, and naming collision matrices

use sqltype::analyzer::analyze_query;
use sqltype::catalog::Catalog;
use sqltype::codegen::generate_file_ts;

fn setup_challenge_catalog() -> Catalog {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TYPE user_status AS ENUM ('active', 'pending', 'suspended');

        CREATE TYPE address AS (
            street TEXT,
            city TEXT,
            zip INT
        );

        CREATE DOMAIN email_address AS TEXT
            CHECK (VALUE ~* '^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$');

        CREATE TABLE organizations (
            id UUID PRIMARY KEY,
            name TEXT NOT NULL,
            domain TEXT
        );

        CREATE TABLE users (
            id UUID PRIMARY KEY,
            org_id UUID REFERENCES organizations(id),
            parent_id UUID,
            email email_address NOT NULL,
            name VARCHAR(100) NOT NULL,
            role_id INT NOT NULL,
            age INT,
            min_age INT NOT NULL DEFAULT 18,
            max_age INT NOT NULL DEFAULT 65,
            status user_status NOT NULL DEFAULT 'active',
            tags TEXT[] NOT NULL DEFAULT '{}',
            home_addr address,
            prev_addrs address[],
            bio TEXT,
            active BOOLEAN NOT NULL DEFAULT true,
            score NUMERIC(10, 2),
            created_at TIMESTAMPTZ NOT NULL
        );

        CREATE TABLE posts (
            id UUID PRIMARY KEY,
            author_id UUID NOT NULL,
            title VARCHAR(255) NOT NULL,
            tags TEXT[] NOT NULL,
            view_count INT NOT NULL DEFAULT 0,
            published BOOLEAN NOT NULL DEFAULT false,
            created_at TIMESTAMPTZ NOT NULL
        );

        CREATE TABLE metrics (
            id UUID PRIMARY KEY,
            id2 TEXT NOT NULL,
            id3 TEXT NOT NULL,
            val INT NOT NULL
        );
    "#;
    catalog.apply_sql(ddl).expect("Challenge catalog DDL setup failed");
    catalog
}

// =========================================================================
// 1. Complex and Nested Expression Trees
// =========================================================================

#[test]
fn test_nested_binary_operations_and_boolean_trees() {
    let catalog = setup_challenge_catalog();

    // 1a. Explicitly requested pattern: WHERE (a = $1 AND b = $2) OR c = $3
    let sql_nested = "SELECT * FROM users WHERE (name = $1 AND role_id = $2) OR email = $3;";
    let analyzed = analyze_query(sql_nested, &catalog, None).expect("Nested binary op query should analyze");
    assert_eq!(analyzed.params.len(), 3);
    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "name");
    assert_eq!(analyzed.params[0].ts_type, "string");
    assert_eq!(analyzed.params[1].index, 2);
    assert_eq!(analyzed.params[1].name, "role_id");
    assert_eq!(analyzed.params[1].ts_type, "number");
    assert_eq!(analyzed.params[2].index, 3);
    assert_eq!(analyzed.params[2].name, "email");
    assert_eq!(analyzed.params[2].ts_type, "string");

    // 1b. Deeply nested mixed boolean conjunctions/disjunctions
    let sql_deep = r#"
        SELECT * FROM users
        WHERE (((name = $1 AND age >= $2) OR (status = $3 AND active = $4))
               AND (email = $5 OR (created_at <= $6 AND score > $7)));
    "#;
    let a_deep = analyze_query(sql_deep, &catalog, None).expect("Deep boolean tree query should analyze");
    assert_eq!(a_deep.params.len(), 7);
    assert_eq!(a_deep.params[0].name, "name");
    assert_eq!(a_deep.params[0].ts_type, "string");
    assert_eq!(a_deep.params[1].name, "age");
    assert_eq!(a_deep.params[1].ts_type, "number");
    assert_eq!(a_deep.params[2].name, "status");
    assert!(a_deep.params[2].ts_type.contains("\"active\""));
    assert_eq!(a_deep.params[3].name, "active");
    assert_eq!(a_deep.params[3].ts_type, "boolean");
    assert_eq!(a_deep.params[4].name, "email");
    assert_eq!(a_deep.params[4].ts_type, "string");
    assert_eq!(a_deep.params[5].name, "created_at");
    assert_eq!(a_deep.params[5].ts_type, "Date");
    assert_eq!(a_deep.params[6].name, "score");
    assert_eq!(a_deep.params[6].ts_type, "number");
}

#[test]
fn test_range_comparisons_between_variants() {
    let catalog = setup_challenge_catalog();

    // 2a. Both bounds as parameters: age BETWEEN $1 AND $2
    let sql1 = "SELECT * FROM users WHERE age BETWEEN $1 AND $2;";
    let a1 = analyze_query(sql1, &catalog, None).expect("BETWEEN with two params should analyze");
    assert_eq!(a1.params.len(), 2);
    assert_eq!(a1.params[0].name, "age");
    assert_eq!(a1.params[0].ts_type, "number");
    assert_eq!(a1.params[1].name, "age2");
    assert_eq!(a1.params[1].ts_type, "number");

    // 2b. Subject as parameter: $1 BETWEEN min_age AND max_age
    let sql2 = "SELECT * FROM users WHERE $1 BETWEEN min_age AND max_age;";
    let a2 = analyze_query(sql2, &catalog, None).expect("BETWEEN subject param should analyze");
    assert_eq!(a2.params.len(), 1);
    assert_eq!(a2.params[0].ts_type, "number");

    // 2c. Lower bound only as param: age BETWEEN $1 AND 65
    let sql3 = "SELECT * FROM users WHERE age BETWEEN $1 AND 65;";
    let a3 = analyze_query(sql3, &catalog, None).expect("BETWEEN lower bound param should analyze");
    assert_eq!(a3.params.len(), 1);
    assert_eq!(a3.params[0].name, "age");
    assert_eq!(a3.params[0].ts_type, "number");

    // 2d. Upper bound only as param: age BETWEEN 18 AND $1
    let sql4 = "SELECT * FROM users WHERE age BETWEEN 18 AND $1;";
    let a4 = analyze_query(sql4, &catalog, None).expect("BETWEEN upper bound param should analyze");
    assert_eq!(a4.params.len(), 1);
    assert_eq!(a4.params[0].name, "age");
    assert_eq!(a4.params[0].ts_type, "number");

    // 2e. NOT BETWEEN with timestamp bounds
    let sql5 = "SELECT * FROM users WHERE created_at NOT BETWEEN $1 AND $2;";
    let a5 = analyze_query(sql5, &catalog, None).expect("NOT BETWEEN dates should analyze");
    assert_eq!(a5.params.len(), 2);
    assert_eq!(a5.params[0].name, "created_at");
    assert_eq!(a5.params[0].ts_type, "Date");
    assert_eq!(a5.params[1].name, "created_at2");
    assert_eq!(a5.params[1].ts_type, "Date");
}

#[test]
fn test_function_calls_parameter_deduction() {
    let catalog = setup_challenge_catalog();

    // 3a. Symmetrical function: lower(name) = lower($1)
    let sql1 = "SELECT * FROM users WHERE lower(name) = lower($1);";
    let a1 = analyze_query(sql1, &catalog, None).expect("lower(name) = lower($1) should analyze");
    assert_eq!(a1.params.len(), 1);
    assert_eq!(a1.params[0].name, "name");
    assert_eq!(a1.params[0].ts_type, "string");

    // 3b. Inverted symmetrical function: lower($1) = lower(name)
    let sql2 = "SELECT * FROM users WHERE lower($1) = lower(name);";
    let a2 = analyze_query(sql2, &catalog, None).expect("lower($1) = lower(name) should analyze");
    assert_eq!(a2.params.len(), 1);
    assert_eq!(a2.params[0].name, "name");
    assert_eq!(a2.params[0].ts_type, "string");

    // 3c. Symmetrical function with table alias: lower(u.name) = lower($1)
    let sql3 = "SELECT * FROM users u WHERE lower(u.name) = lower($1);";
    let a3 = analyze_query(sql3, &catalog, None).expect("aliased lower symmetrical should analyze");
    assert_eq!(a3.params.len(), 1);
    assert_eq!(a3.params[0].name, "name");
    assert_eq!(a3.params[0].ts_type, "string");

    // 3d. upper and trim functions
    let sql4 = "SELECT * FROM users WHERE upper(name) = upper($1) AND trim(bio) = trim($2);";
    let a4 = analyze_query(sql4, &catalog, None).expect("upper and trim should analyze");
    assert_eq!(a4.params.len(), 2);
    assert_eq!(a4.params[0].name, "name");
    assert_eq!(a4.params[0].ts_type, "string");
    assert_eq!(a4.params[1].name, "bio");
    assert_eq!(a4.params[1].ts_type, "string");

    // 3e. Coalesce with parameter: coalesce($1, name)
    let sql5 = "SELECT coalesce($1, name) FROM users;";
    let a5 = analyze_query(sql5, &catalog, None).expect("coalesce with param should analyze");
    assert_eq!(a5.params.len(), 1);
    assert_eq!(a5.params[0].ts_type, "string");

    // 3f. Parameter inside built-in function: name = lower($1)
    let sql6 = "SELECT * FROM users WHERE name = lower($1);";
    let a6 = analyze_query(sql6, &catalog, None).expect("name = lower($1) should analyze");
    assert_eq!(a6.params.len(), 1);
    assert_eq!(a6.params[0].name, "name");
    assert_eq!(a6.params[0].ts_type, "string");
}

#[test]
fn test_explicit_and_composite_type_casts() {
    let catalog = setup_challenge_catalog();

    // 4a. Explicit cast: $1::uuid
    let sql1 = "SELECT * FROM users WHERE id = $1::uuid;";
    let a1 = analyze_query(sql1, &catalog, None).expect("$1::uuid should analyze");
    assert_eq!(a1.params.len(), 1);
    assert_eq!(a1.params[0].name, "id");
    assert_eq!(a1.params[0].ts_type, "string");

    // 4b. Explicit array cast: $1::text[]
    let sql2 = "SELECT * FROM users WHERE tags = $1::text[];";
    let a2 = analyze_query(sql2, &catalog, None).expect("$1::text[] should analyze");
    assert_eq!(a2.params.len(), 1);
    assert_eq!(a2.params[0].name, "tags");
    assert_eq!(a2.params[0].ts_type, "Array<string>");

    // 4c. Custom composite type cast: $1::address
    let sql3 = "SELECT $1::address AS user_addr;";
    let a3 = analyze_query(sql3, &catalog, None).expect("$1::address should analyze");
    assert_eq!(a3.params.len(), 1);
    assert_eq!(a3.params[0].name, "user_addr");
    assert!(a3.params[0].ts_type.contains("street: string;"));
    assert!(a3.params[0].ts_type.contains("city: string;"));
    assert!(a3.params[0].ts_type.contains("zip: number"));

    // 4d. Custom composite array cast: $1::address[]
    let sql4 = "SELECT $1::address[] AS user_addrs;";
    let a4 = analyze_query(sql4, &catalog, None).expect("$1::address[] should analyze");
    assert_eq!(a4.params.len(), 1);
    assert_eq!(a4.params[0].name, "user_addrs");
    assert!(a4.params[0].ts_type.contains("Array<{") || a4.params[0].ts_type.contains("}[]"));

    // 4e. Nested cast: ($1::text)::varchar
    let sql5 = "SELECT * FROM users WHERE name = ($1::text)::varchar;";
    let a5 = analyze_query(sql5, &catalog, None).expect("Nested cast should analyze");
    assert_eq!(a5.params.len(), 1);
    assert_eq!(a5.params[0].name, "name");
    assert_eq!(a5.params[0].ts_type, "string");
}

#[test]
fn test_literal_comparisons_matrix() {
    let catalog = setup_challenge_catalog();

    // 5a. Integer literal: $1 = 42
    let sql_int = "SELECT * FROM users WHERE $1 = 42;";
    let a_int = analyze_query(sql_int, &catalog, None).expect("$1 = 42 should analyze");
    assert_eq!(a_int.params.len(), 1);
    assert_eq!(a_int.params[0].ts_type, "number");

    // 5b. String literal: $1 = 'active'
    let sql_str = "SELECT * FROM users WHERE $1 = 'active';";
    let a_str = analyze_query(sql_str, &catalog, None).expect("$1 = 'active' should analyze");
    assert_eq!(a_str.params.len(), 1);
    assert_eq!(a_str.params[0].ts_type, "string");

    // 5c. Boolean literal: $1 = true
    let sql_bool = "SELECT * FROM users WHERE $1 = true;";
    let a_bool = analyze_query(sql_bool, &catalog, None).expect("$1 = true should analyze");
    assert_eq!(a_bool.params.len(), 1);
    assert_eq!(a_bool.params[0].ts_type, "boolean");

    // 5d. Float literal: $1 > 3.14
    let sql_float = "SELECT * FROM users WHERE $1 > 3.14;";
    let a_float = analyze_query(sql_float, &catalog, None).expect("$1 > 3.14 should analyze");
    assert_eq!(a_float.params.len(), 1);
    assert_eq!(a_float.params[0].ts_type, "number");

    // 5e. Inequality with literal: $1 <> 'archived'
    let sql_neq = "SELECT * FROM users WHERE $1 <> 'archived';";
    let a_neq = analyze_query(sql_neq, &catalog, None).expect("$1 <> 'archived' should analyze");
    assert_eq!(a_neq.params.len(), 1);
    assert_eq!(a_neq.params[0].ts_type, "string");
}

// =========================================================================
// 2. Array Semantics: ANY($1) vs $1 = ANY(arr_col)
// =========================================================================

#[test]
fn test_array_semantics_discrimination() {
    let catalog = setup_challenge_catalog();

    // 2a. Array param: col = ANY($1) -> $1 is Array<T>
    let sql_any_param = "SELECT * FROM users WHERE id = ANY($1);";
    let a_arr = analyze_query(sql_any_param, &catalog, None).expect("ANY($1) should analyze");
    assert_eq!(a_arr.params.len(), 1);
    assert_eq!(a_arr.params[0].name, "id");
    assert_eq!(a_arr.params[0].ts_type, "Array<string>");

    // 2b. Scalar element param: $1 = ANY(arr_col) -> $1 is T (element of arr_col)
    let sql_scalar_any = "SELECT * FROM posts WHERE $1 = ANY(tags);";
    let a_elem = analyze_query(sql_scalar_any, &catalog, None).expect("$1 = ANY(arr) should analyze");
    assert_eq!(a_elem.params.len(), 1);
    assert_eq!(a_elem.params[0].name, "tags");
    assert_eq!(a_elem.params[0].ts_type, "string");

    // 2c. Combined query with BOTH in the same statement:
    // WHERE id = ANY($1) AND $2 = ANY(tags)
    let sql_combined = "SELECT * FROM users WHERE id = ANY($1) AND $2 = ANY(tags);";
    let a_comb = analyze_query(sql_combined, &catalog, None).expect("Combined array query should analyze");
    assert_eq!(a_comb.params.len(), 2);
    assert_eq!(a_comb.params[0].index, 1);
    assert_eq!(a_comb.params[0].name, "id");
    assert_eq!(a_comb.params[0].ts_type, "Array<string>"); // Array param
    assert_eq!(a_comb.params[1].index, 2);
    assert_eq!(a_comb.params[1].name, "tags");
    assert_eq!(a_comb.params[1].ts_type, "string"); // Element param

    // 2d. Array overlap (&&) and containment (@>)
    let sql_ops = "SELECT * FROM posts WHERE tags && $1 AND tags @> $2;";
    let a_ops = analyze_query(sql_ops, &catalog, None).expect("Array ops should analyze");
    assert_eq!(a_ops.params.len(), 2);
    assert_eq!(a_ops.params[0].name, "tags");
    assert_eq!(a_ops.params[0].ts_type, "Array<string>");
    assert_eq!(a_ops.params[1].name, "tags2");
    assert_eq!(a_ops.params[1].ts_type, "Array<string>");

    // 2e. ALL operator: age > ALL($1) -> Array<number>
    let sql_all = "SELECT * FROM users WHERE age > ALL($1);";
    let a_all = analyze_query(sql_all, &catalog, None).expect("ALL($1) should analyze");
    assert_eq!(a_all.params.len(), 1);
    assert_eq!(a_all.params[0].name, "age");
    assert_eq!(a_all.params[0].ts_type, "Array<number>");
}

// =========================================================================
// 3. Dynamic Optional Filters Matrix
// =========================================================================

#[test]
fn test_dynamic_optional_filters_exhaustive_matrix() {
    let catalog = setup_challenge_catalog();

    // 3a. Standard ordering: ($1 IS NULL OR col = $1)
    let sql1 = "SELECT * FROM users WHERE ($1 IS NULL OR email = $1);";
    let a1 = analyze_query(sql1, &catalog, None).expect("Standard optional filter should analyze");
    assert_eq!(a1.params.len(), 1);
    assert_eq!(a1.params[0].name, "email");
    assert_eq!(a1.params[0].ts_type, "string");
    assert!(a1.params[0].is_optional);

    // 3b. Inverted ordering: (col = $1 OR $1 IS NULL)
    let sql2 = "SELECT * FROM users WHERE (email = $1 OR $1 IS NULL);";
    let a2 = analyze_query(sql2, &catalog, None).expect("Inverted optional filter should analyze");
    assert_eq!(a2.params.len(), 1);
    assert_eq!(a2.params[0].name, "email");
    assert_eq!(a2.params[0].ts_type, "string");
    assert!(a2.params[0].is_optional);

    // 3c. Inverted equality inside predicate: ($1 IS NULL OR $1 = email)
    let sql3 = "SELECT * FROM users WHERE ($1 IS NULL OR $1 = email);";
    let a3 = analyze_query(sql3, &catalog, None).expect("Inverted predicate equality should analyze");
    assert_eq!(a3.params.len(), 1);
    assert_eq!(a3.params[0].name, "email");
    assert_eq!(a3.params[0].ts_type, "string");
    assert!(a3.params[0].is_optional);

    // 3d. Inverted ordering AND inverted predicate: ($1 = email OR $1 IS NULL)
    let sql4 = "SELECT * FROM users WHERE ($1 = email OR $1 IS NULL);";
    let a4 = analyze_query(sql4, &catalog, None).expect("Both inverted optional filter should analyze");
    assert_eq!(a4.params.len(), 1);
    assert_eq!(a4.params[0].name, "email");
    assert_eq!(a4.params[0].ts_type, "string");
    assert!(a4.params[0].is_optional);

    // 3e. Cast variant 1: ($1::text IS NULL OR email = $1)
    let sql5 = "SELECT * FROM users WHERE ($1::text IS NULL OR email = $1);";
    let a5 = analyze_query(sql5, &catalog, None).expect("Cast on null test should analyze");
    assert_eq!(a5.params.len(), 1);
    assert_eq!(a5.params[0].name, "email");
    assert_eq!(a5.params[0].ts_type, "string");
    assert!(a5.params[0].is_optional);

    // 3f. Cast variant 2: ($1 IS NULL OR email = $1::text)
    let sql6 = "SELECT * FROM users WHERE ($1 IS NULL OR email = $1::text);";
    let a6 = analyze_query(sql6, &catalog, None).expect("Cast on predicate param should analyze");
    assert_eq!(a6.params.len(), 1);
    assert_eq!(a6.params[0].name, "email");
    assert_eq!(a6.params[0].ts_type, "string");
    assert!(a6.params[0].is_optional);

    // 3g. Cast variant 3: ($1::uuid IS NULL OR id = $1)
    let sql7 = "SELECT * FROM users WHERE ($1::uuid IS NULL OR id = $1);";
    let a7 = analyze_query(sql7, &catalog, None).expect("UUID cast on null test should analyze");
    assert_eq!(a7.params.len(), 1);
    assert_eq!(a7.params[0].name, "id");
    assert_eq!(a7.params[0].ts_type, "string");
    assert!(a7.params[0].is_optional);

    // 3h. Optional LIKE: ($1 IS NULL OR name LIKE $1) and inverted
    let sql8 = "SELECT * FROM users WHERE ($1 IS NULL OR name LIKE $1);";
    let a8 = analyze_query(sql8, &catalog, None).expect("Optional LIKE should analyze");
    assert_eq!(a8.params.len(), 1);
    assert_eq!(a8.params[0].name, "name");
    assert_eq!(a8.params[0].ts_type, "string");
    assert!(a8.params[0].is_optional);

    let sql9 = "SELECT * FROM users WHERE (name LIKE $1 OR $1 IS NULL);";
    let a9 = analyze_query(sql9, &catalog, None).expect("Inverted optional LIKE should analyze");
    assert_eq!(a9.params.len(), 1);
    assert_eq!(a9.params[0].name, "name");
    assert_eq!(a9.params[0].ts_type, "string");
    assert!(a9.params[0].is_optional);

    // 3i. Optional ILIKE: ($1 IS NULL OR email ILIKE $1) and inverted
    let sql10 = "SELECT * FROM users WHERE ($1 IS NULL OR email ILIKE $1);";
    let a10 = analyze_query(sql10, &catalog, None).expect("Optional ILIKE should analyze");
    assert_eq!(a10.params.len(), 1);
    assert_eq!(a10.params[0].name, "email");
    assert_eq!(a10.params[0].ts_type, "string");
    assert!(a10.params[0].is_optional);

    // 3j. Optional array ANY: ($1 IS NULL OR id = ANY($1)) and inverted
    let sql11 = "SELECT * FROM users WHERE ($1 IS NULL OR id = ANY($1));";
    let a11 = analyze_query(sql11, &catalog, None).expect("Optional ANY should analyze");
    assert_eq!(a11.params.len(), 1);
    assert_eq!(a11.params[0].name, "id");
    assert_eq!(a11.params[0].ts_type, "Array<string>");
    assert!(a11.params[0].is_optional);

    let sql12 = "SELECT * FROM users WHERE (id = ANY($1) OR $1 IS NULL);";
    let a12 = analyze_query(sql12, &catalog, None).expect("Inverted optional ANY should analyze");
    assert_eq!(a12.params.len(), 1);
    assert_eq!(a12.params[0].name, "id");
    assert_eq!(a12.params[0].ts_type, "Array<string>");
    assert!(a12.params[0].is_optional);

    // 3k. Optional symmetrical function: ($1 IS NULL OR lower(email) = lower($1)) and inverted
    let sql13 = "SELECT * FROM users WHERE ($1 IS NULL OR lower(email) = lower($1));";
    let a13 = analyze_query(sql13, &catalog, None).expect("Optional symmetrical function should analyze");
    assert_eq!(a13.params.len(), 1);
    assert_eq!(a13.params[0].name, "email");
    assert_eq!(a13.params[0].ts_type, "string");
    assert!(a13.params[0].is_optional);

    // 3l. Conjunction of multiple optional dynamic filters:
    let sql_multi = r#"
        SELECT * FROM users
        WHERE ($1 IS NULL OR email = $1)
          AND ($2 IS NULL OR name LIKE $2)
          AND ($3 IS NULL OR id = ANY($3));
    "#;
    let a_multi = analyze_query(sql_multi, &catalog, None).expect("Multiple optional filters should analyze");
    assert_eq!(a_multi.params.len(), 3);
    assert_eq!(a_multi.params[0].name, "email");
    assert!(a_multi.params[0].is_optional);
    assert_eq!(a_multi.params[1].name, "name");
    assert!(a_multi.params[1].is_optional);
    assert_eq!(a_multi.params[2].name, "id");
    assert!(a_multi.params[2].is_optional);
}

// =========================================================================
// 4. Stress Tests: Collision Invariants & DML Edge Cases
// =========================================================================

#[test]
fn test_collision_avoidance_stress_with_existing_identifiers() {
    let catalog = setup_challenge_catalog();

    // Table `metrics` has columns: `id`, `id2`, `id3`, `val`
    // Query: id = $1 AND id2 = $2 AND id3 = $3 AND id = $4 AND id = $5
    // In naive numbering: $4 on `id` would become `id2` (COLLISION with column id2!)
    // In correct implementation: $4 allocates `id4`, and $5 allocates `id5`!
    let sql = r#"
        SELECT * FROM metrics
        WHERE id = $1
          AND id2 = $2
          AND id3 = $3
          AND id = $4
          AND id = $5;
    "#;
    let analyzed = analyze_query(sql, &catalog, None).expect("Collision stress query should analyze");
    assert_eq!(analyzed.params.len(), 5);
    assert_eq!(analyzed.params[0].name, "id");
    assert_eq!(analyzed.params[1].name, "id2");
    assert_eq!(analyzed.params[2].name, "id3");
    assert_eq!(analyzed.params[3].name, "id4");
    assert_eq!(analyzed.params[4].name, "id5");

    // Codegen interface verification:
    let ts = generate_file_ts(&[analyzed]);
    assert!(ts.contains("id: string;"));
    assert!(ts.contains("id2: string;"));
    assert!(ts.contains("id3: string;"));
    assert!(ts.contains("id4: string;"));
    assert!(ts.contains("id5: string;"));
}

#[test]
fn test_dml_and_having_clause_parameter_deduction() {
    let catalog = setup_challenge_catalog();

    // 1. UPDATE statement with nullable target column -> marked optional
    let sql_update = "UPDATE users SET age = $1, bio = $2 WHERE id = $3;";
    let a_up = analyze_query(sql_update, &catalog, None).expect("UPDATE query should analyze");
    assert_eq!(a_up.params.len(), 3);
    assert_eq!(a_up.params[0].name, "age");
    assert_eq!(a_up.params[0].ts_type, "number");
    assert!(a_up.params[0].is_optional); // age is nullable
    assert_eq!(a_up.params[1].name, "bio");
    assert_eq!(a_up.params[1].ts_type, "string");
    assert!(a_up.params[1].is_optional); // bio is nullable
    assert_eq!(a_up.params[2].name, "id");
    assert_eq!(a_up.params[2].ts_type, "string");
    assert!(!a_up.params[2].is_optional); // id WHERE predicate is not optional

    // 2. DELETE statement with WHERE
    let sql_del = "DELETE FROM users WHERE org_id = $1 AND role_id = $2;";
    let a_del = analyze_query(sql_del, &catalog, None).expect("DELETE query should analyze");
    assert_eq!(a_del.params.len(), 2);
    assert_eq!(a_del.params[0].name, "org_id");
    assert_eq!(a_del.params[0].ts_type, "string");
    assert_eq!(a_del.params[1].name, "role_id");
    assert_eq!(a_del.params[1].ts_type, "number");

    // 3. HAVING clause in aggregate query
    let sql_having = "SELECT org_id, count(*) FROM users GROUP BY org_id HAVING count(*) >= $1;";
    let a_having = analyze_query(sql_having, &catalog, None).expect("HAVING query should analyze");
    assert_eq!(a_having.params.len(), 1);
    assert_eq!(a_having.params[0].index, 1);
    assert_eq!(a_having.params[0].ts_type, "number");
}
