//! Comprehensive Integration Test Suite for Milestone 4: Prepared Statement Parameter Type Deduction
//!
//! Tests:
//! 1. Binary comparison operations and literal counterparts (AConst: int, float, string, bool)
//! 2. Pattern matching operations (LIKE, ILIKE, SIMILAR TO, ~~, ~~*, NOT LIKE, NOT ILIKE, string concat ||)
//! 3. Array and set operations (ANY($1), $1 = ANY(arr), ALL($1), IN ($1, $2), NOT IN, BETWEEN bounds, array overlap &&, contains @>)
//! 4. Type casts and function calls (explicit casts, array casts, domain/enum casts, symmetrical functions, coalesce, limit/offset)
//! 5. Optional dynamic filter idioms (($1 IS NULL OR col = $1), inverted, LIKE, ILIKE, ANY($1), symmetrical functions)
//! 6. Deterministic parameter sorting and collision-free disambiguation (allocated_names invariant: id, id2, id3)
//! 7. DML statement analysis (multi-row INSERT, ON CONFLICT, UPDATE, DELETE)
//! 8. HAVING clause parameter deduction
//! 9. Standalone deduce_query_params API and TypeScript codegen verification

use sqltype::analyzer::analyze_query;
use sqltype::catalog::Catalog;
use sqltype::codegen::generate_file_ts;
use sqltype::{QueryScope, deduce_query_params};

fn setup_test_catalog() -> Catalog {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TYPE user_status AS ENUM ('active', 'pending', 'suspended');

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
            max_age INT,
            status user_status NOT NULL DEFAULT 'active',
            tags TEXT[] NOT NULL DEFAULT '{}',
            bio TEXT,
            active BOOLEAN NOT NULL DEFAULT true,
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
    "#;
    catalog
        .apply_sql(ddl)
        .expect("Failed to setup test catalog");
    catalog
}

#[test]
fn test_param_col_equality_and_reverse() {
    let catalog = setup_test_catalog();

    // Standard LHS column = RHS param
    let sql1 = "SELECT * FROM users WHERE id = $1;";
    let analyzed1 = analyze_query(sql1, &catalog, None).expect("Query should analyze");
    assert_eq!(analyzed1.params.len(), 1);
    assert_eq!(analyzed1.params[0].index, 1);
    assert_eq!(analyzed1.params[0].name, "id");
    assert_eq!(analyzed1.params[0].ts_type, "string");
    assert!(!analyzed1.params[0].is_optional);

    // Reversed: LHS param = RHS column
    let sql2 = "SELECT * FROM users WHERE $1 = id;";
    let analyzed2 = analyze_query(sql2, &catalog, None).expect("Query should analyze");
    assert_eq!(analyzed2.params.len(), 1);
    assert_eq!(analyzed2.params[0].index, 1);
    assert_eq!(analyzed2.params[0].name, "id");
    assert_eq!(analyzed2.params[0].ts_type, "string");
    assert!(!analyzed2.params[0].is_optional);
}

#[test]
fn test_param_literal_counterparts() {
    let catalog = setup_test_catalog();

    // Integer literal counterpart: $1 = 42
    let sql_int = "SELECT * FROM users WHERE $1 = 42;";
    let analyzed_int = analyze_query(sql_int, &catalog, None).expect("Query should analyze");
    assert_eq!(analyzed_int.params.len(), 1);
    assert_eq!(analyzed_int.params[0].index, 1);
    assert_eq!(analyzed_int.params[0].name, "param1");
    assert_eq!(analyzed_int.params[0].ts_type, "number");

    // String literal counterpart: $1 = 'active'
    let sql_str = "SELECT * FROM users WHERE $1 = 'active';";
    let analyzed_str = analyze_query(sql_str, &catalog, None).expect("Query should analyze");
    assert_eq!(analyzed_str.params.len(), 1);
    assert_eq!(analyzed_str.params[0].index, 1);
    assert_eq!(analyzed_str.params[0].name, "param1");
    assert_eq!(analyzed_str.params[0].ts_type, "string");

    // Boolean literal counterpart: $1 = true
    let sql_bool = "SELECT * FROM users WHERE $1 = true;";
    let analyzed_bool = analyze_query(sql_bool, &catalog, None).expect("Query should analyze");
    assert_eq!(analyzed_bool.params.len(), 1);
    assert_eq!(analyzed_bool.params[0].index, 1);
    assert_eq!(analyzed_bool.params[0].name, "param1");
    assert_eq!(analyzed_bool.params[0].ts_type, "boolean");

    // Float literal counterpart: $1 > 3.14
    let sql_float = "SELECT * FROM users WHERE $1 > 3.14;";
    let analyzed_float = analyze_query(sql_float, &catalog, None).expect("Query should analyze");
    assert_eq!(analyzed_float.params.len(), 1);
    assert_eq!(analyzed_float.params[0].index, 1);
    assert_eq!(analyzed_float.params[0].name, "param1");
    assert_eq!(analyzed_float.params[0].ts_type, "number");
}

#[test]
fn test_param_aliased_column_lookup() {
    let catalog = setup_test_catalog();
    let sql = "SELECT * FROM users u WHERE u.age >= $1;";
    let analyzed = analyze_query(sql, &catalog, None).expect("Query should analyze");
    assert_eq!(analyzed.params.len(), 1);
    assert_eq!(analyzed.params[0].name, "age");
    assert_eq!(analyzed.params[0].ts_type, "number");
}

#[test]
fn test_param_pattern_matching_operators() {
    let catalog = setup_test_catalog();

    // LIKE
    let sql_like = "SELECT * FROM users WHERE name LIKE $1;";
    let a_like = analyze_query(sql_like, &catalog, None).expect("LIKE should analyze");
    assert_eq!(a_like.params.len(), 1);
    assert_eq!(a_like.params[0].name, "name");
    assert_eq!(a_like.params[0].ts_type, "string");

    // ILIKE
    let sql_ilike = "SELECT * FROM users WHERE email ILIKE $1;";
    let a_ilike = analyze_query(sql_ilike, &catalog, None).expect("ILIKE should analyze");
    assert_eq!(a_ilike.params.len(), 1);
    assert_eq!(a_ilike.params[0].name, "email");
    assert_eq!(a_ilike.params[0].ts_type, "string");

    // NOT LIKE
    let sql_not_like = "SELECT * FROM users WHERE name NOT LIKE $1;";
    let a_not_like = analyze_query(sql_not_like, &catalog, None).expect("NOT LIKE should analyze");
    assert_eq!(a_not_like.params.len(), 1);
    assert_eq!(a_not_like.params[0].name, "name");
    assert_eq!(a_not_like.params[0].ts_type, "string");

    // NOT ILIKE
    let sql_not_ilike = "SELECT * FROM users WHERE email NOT ILIKE $1;";
    let a_not_ilike =
        analyze_query(sql_not_ilike, &catalog, None).expect("NOT ILIKE should analyze");
    assert_eq!(a_not_ilike.params.len(), 1);
    assert_eq!(a_not_ilike.params[0].name, "email");
    assert_eq!(a_not_ilike.params[0].ts_type, "string");

    // SIMILAR TO
    let sql_similar = "SELECT * FROM users WHERE name SIMILAR TO $1;";
    let a_similar = analyze_query(sql_similar, &catalog, None).expect("SIMILAR TO should analyze");
    assert_eq!(a_similar.params.len(), 1);
    assert_eq!(a_similar.params[0].name, "name");
    assert_eq!(a_similar.params[0].ts_type, "string");

    // Raw operators: ~~ and ~~*
    let sql_raw_like = "SELECT * FROM users WHERE name ~~ $1;";
    let a_raw_like = analyze_query(sql_raw_like, &catalog, None).expect("~~ should analyze");
    assert_eq!(a_raw_like.params.len(), 1);
    assert_eq!(a_raw_like.params[0].name, "name");
    assert_eq!(a_raw_like.params[0].ts_type, "string");

    let sql_raw_ilike = "SELECT * FROM users WHERE email ~~* $1;";
    let a_raw_ilike = analyze_query(sql_raw_ilike, &catalog, None).expect("~~* should analyze");
    assert_eq!(a_raw_ilike.params.len(), 1);
    assert_eq!(a_raw_ilike.params[0].name, "email");
    assert_eq!(a_raw_ilike.params[0].ts_type, "string");

    // Parameter on LHS: $1 LIKE 'prefix%'
    let sql_lhs = "SELECT * FROM users WHERE $1 LIKE 'prefix%';";
    let a_lhs = analyze_query(sql_lhs, &catalog, None).expect("LHS LIKE should analyze");
    assert_eq!(a_lhs.params.len(), 1);
    assert_eq!(a_lhs.params[0].name, "param1");
    assert_eq!(a_lhs.params[0].ts_type, "string");

    // String concatenation in pattern: name LIKE '%' || $1 || '%'
    let sql_concat = "SELECT * FROM users WHERE name LIKE '%' || $1 || '%';";
    let a_concat =
        analyze_query(sql_concat, &catalog, None).expect("Concat pattern should analyze");
    assert_eq!(a_concat.params.len(), 1);
    assert_eq!(a_concat.params[0].name, "name");
    assert_eq!(a_concat.params[0].ts_type, "string");
}

#[test]
fn test_param_array_and_set_operations() {
    let catalog = setup_test_catalog();

    // 1. col = ANY($1) -> $1 is Array<string>
    let sql_any = "SELECT * FROM users WHERE id = ANY($1);";
    let a_any = analyze_query(sql_any, &catalog, None).expect("ANY($1) should analyze");
    assert_eq!(a_any.params.len(), 1);
    assert_eq!(a_any.params[0].name, "id");
    assert_eq!(a_any.params[0].ts_type, "Array<string>");

    // 2. $1 = ANY(tags) -> $1 is scalar element type string
    let sql_scalar_any = "SELECT * FROM posts WHERE $1 = ANY(tags);";
    let a_scalar_any =
        analyze_query(sql_scalar_any, &catalog, None).expect("Scalar ANY should analyze");
    assert_eq!(a_scalar_any.params.len(), 1);
    assert_eq!(a_scalar_any.params[0].name, "tags");
    assert_eq!(a_scalar_any.params[0].ts_type, "string");

    // 3. age > ALL($1) -> $1 is Array<number>
    let sql_all = "SELECT * FROM users WHERE age > ALL($1);";
    let a_all = analyze_query(sql_all, &catalog, None).expect("ALL($1) should analyze");
    assert_eq!(a_all.params.len(), 1);
    assert_eq!(a_all.params[0].name, "age");
    assert_eq!(a_all.params[0].ts_type, "Array<number>");

    // 4. IN list: id IN ($1, $2)
    let sql_in = "SELECT * FROM users WHERE id IN ($1, $2);";
    let a_in = analyze_query(sql_in, &catalog, None).expect("IN list should analyze");
    assert_eq!(a_in.params.len(), 2);
    assert_eq!(a_in.params[0].index, 1);
    assert_eq!(a_in.params[0].name, "id");
    assert_eq!(a_in.params[0].ts_type, "string");
    assert_eq!(a_in.params[1].index, 2);
    assert_eq!(a_in.params[1].name, "id2");
    assert_eq!(a_in.params[1].ts_type, "string");

    // 5. IN list multiple int: role_id IN ($1, $2, $3)
    let sql_in_int = "SELECT * FROM users WHERE role_id IN ($1, $2, $3);";
    let a_in_int = analyze_query(sql_in_int, &catalog, None).expect("IN list int should analyze");
    assert_eq!(a_in_int.params.len(), 3);
    assert_eq!(a_in_int.params[0].name, "role_id");
    assert_eq!(a_in_int.params[0].ts_type, "number");
    assert_eq!(a_in_int.params[1].name, "role_id2");
    assert_eq!(a_in_int.params[1].ts_type, "number");
    assert_eq!(a_in_int.params[2].name, "role_id3");
    assert_eq!(a_in_int.params[2].ts_type, "number");

    // 6. NOT IN list: status NOT IN ($1, $2)
    let sql_not_in = "SELECT * FROM users WHERE status NOT IN ($1, $2);";
    let a_not_in = analyze_query(sql_not_in, &catalog, None).expect("NOT IN should analyze");
    assert_eq!(a_not_in.params.len(), 2);
    assert_eq!(a_not_in.params[0].name, "status");
    assert_eq!(a_not_in.params[1].name, "status2");

    // 7. BETWEEN: age BETWEEN $1 AND $2
    let sql_between = "SELECT * FROM users WHERE age BETWEEN $1 AND $2;";
    let a_between = analyze_query(sql_between, &catalog, None).expect("BETWEEN should analyze");
    assert_eq!(a_between.params.len(), 2);
    assert_eq!(a_between.params[0].name, "age");
    assert_eq!(a_between.params[0].ts_type, "number");
    assert_eq!(a_between.params[1].name, "age2");
    assert_eq!(a_between.params[1].ts_type, "number");

    // 8. NOT BETWEEN: created_at NOT BETWEEN $1 AND $2
    let sql_not_between = "SELECT * FROM users WHERE created_at NOT BETWEEN $1 AND $2;";
    let a_not_between =
        analyze_query(sql_not_between, &catalog, None).expect("NOT BETWEEN should analyze");
    assert_eq!(a_not_between.params.len(), 2);
    assert_eq!(a_not_between.params[0].name, "created_at");
    assert_eq!(a_not_between.params[0].ts_type, "Date");
    assert_eq!(a_not_between.params[1].name, "created_at2");
    assert_eq!(a_not_between.params[1].ts_type, "Date");

    // 9. Parameter subject in BETWEEN: $1 BETWEEN age AND max_age
    let sql_between_subj = "SELECT * FROM users WHERE $1 BETWEEN age AND max_age;";
    let a_between_subj =
        analyze_query(sql_between_subj, &catalog, None).expect("BETWEEN subject should analyze");
    assert_eq!(a_between_subj.params.len(), 1);
    assert_eq!(a_between_subj.params[0].ts_type, "number");

    // 10. Array overlap && and containment @>
    let sql_overlap = "SELECT * FROM posts WHERE tags && $1;";
    let a_overlap =
        analyze_query(sql_overlap, &catalog, None).expect("Array overlap should analyze");
    assert_eq!(a_overlap.params.len(), 1);
    assert_eq!(a_overlap.params[0].name, "tags");
    assert_eq!(a_overlap.params[0].ts_type, "Array<string>");

    let sql_contains = "SELECT * FROM posts WHERE tags @> $1;";
    let a_contains =
        analyze_query(sql_contains, &catalog, None).expect("Array containment should analyze");
    assert_eq!(a_contains.params.len(), 1);
    assert_eq!(a_contains.params[0].name, "tags");
    assert_eq!(a_contains.params[0].ts_type, "Array<string>");
}

#[test]
fn test_param_type_casts_and_functions() {
    let catalog = setup_test_catalog();

    // 1. Explicit cast: $1::timestamptz
    let sql_cast = "SELECT * FROM users WHERE created_at > $1::timestamptz;";
    let a_cast = analyze_query(sql_cast, &catalog, None).expect("Explicit cast should analyze");
    assert_eq!(a_cast.params.len(), 1);
    assert_eq!(a_cast.params[0].name, "created_at");
    assert_eq!(a_cast.params[0].ts_type, "Date");

    // 2. Explicit array cast: $1::uuid[]
    let sql_arr_cast = "SELECT * FROM users WHERE id = ANY($1::uuid[]);";
    let a_arr_cast =
        analyze_query(sql_arr_cast, &catalog, None).expect("Array cast should analyze");
    assert_eq!(a_arr_cast.params.len(), 1);
    assert_eq!(a_arr_cast.params[0].name, "id");
    assert_eq!(a_arr_cast.params[0].ts_type, "Array<string>");

    // 3. Nested cast: ($1::text)::varchar
    let sql_nested = "SELECT * FROM users WHERE name = ($1::text)::varchar;";
    let a_nested = analyze_query(sql_nested, &catalog, None).expect("Nested cast should analyze");
    assert_eq!(a_nested.params.len(), 1);
    assert_eq!(a_nested.params[0].name, "name");
    assert_eq!(a_nested.params[0].ts_type, "string");

    // 4. Built-in lower function: email = lower($1)
    let sql_lower = "SELECT * FROM users WHERE email = lower($1);";
    let a_lower = analyze_query(sql_lower, &catalog, None).expect("lower function should analyze");
    assert_eq!(a_lower.params.len(), 1);
    assert_eq!(a_lower.params[0].name, "email");
    assert_eq!(a_lower.params[0].ts_type, "string");

    // 5. Symmetrical function: lower(email) = lower($1)
    let sql_sym = "SELECT * FROM users WHERE lower(email) = lower($1);";
    let a_sym =
        analyze_query(sql_sym, &catalog, None).expect("Symmetrical function should analyze");
    assert_eq!(a_sym.params.len(), 1);
    assert_eq!(a_sym.params[0].name, "email");
    assert_eq!(a_sym.params[0].ts_type, "string");

    // 6. Target list parameter casts: SELECT $1::uuid AS user_id, $2::int4 AS rank
    let sql_target = "SELECT $1::uuid AS user_id, $2::int4 AS rank;";
    let a_target = analyze_query(sql_target, &catalog, None).expect("Target list should analyze");
    assert_eq!(a_target.params.len(), 2);
    assert_eq!(a_target.params[0].name, "user_id");
    assert_eq!(a_target.params[0].ts_type, "string");
    assert_eq!(a_target.params[1].name, "rank");
    assert_eq!(a_target.params[1].ts_type, "number");

    // 7. Domain type cast: $1::email_address
    let sql_domain = "SELECT * FROM users WHERE email = $1::email_address;";
    let a_domain = analyze_query(sql_domain, &catalog, None).expect("Domain cast should analyze");
    assert_eq!(a_domain.params.len(), 1);
    assert_eq!(a_domain.params[0].name, "email");
    assert_eq!(a_domain.params[0].ts_type, "string");

    // 8. Enum type cast: $1::user_status
    let sql_enum = "SELECT * FROM users WHERE status = $1::user_status;";
    let a_enum = analyze_query(sql_enum, &catalog, None).expect("Enum cast should analyze");
    assert_eq!(a_enum.params.len(), 1);
    assert_eq!(a_enum.params[0].name, "status");
    assert!(a_enum.params[0].ts_type.contains("\"active\""));

    // 9. Coalesce parameter: coalesce($1, name)
    let sql_coalesce = "SELECT coalesce($1, name) FROM users;";
    let a_coalesce = analyze_query(sql_coalesce, &catalog, None).expect("Coalesce should analyze");
    assert_eq!(a_coalesce.params.len(), 1);
    assert_eq!(a_coalesce.params[0].ts_type, "string");

    // 10. LIMIT and OFFSET parameters
    let sql_limit_offset = "SELECT * FROM users LIMIT $1 OFFSET $2;";
    let a_lo =
        analyze_query(sql_limit_offset, &catalog, None).expect("LIMIT OFFSET should analyze");
    assert_eq!(a_lo.params.len(), 2);
    assert_eq!(a_lo.params[0].index, 1);
    assert_eq!(a_lo.params[0].name, "limit");
    assert_eq!(a_lo.params[0].ts_type, "number");
    assert_eq!(a_lo.params[1].index, 2);
    assert_eq!(a_lo.params[1].name, "offset");
    assert_eq!(a_lo.params[1].ts_type, "number");
}

#[test]
fn test_param_optional_dynamic_filter_patterns() {
    let catalog = setup_test_catalog();

    // 1. Standard optional equality: ($1::text IS NULL OR email = $1)
    let sql_opt_eq = "SELECT * FROM users WHERE ($1::text IS NULL OR email = $1);";
    let a_opt_eq =
        analyze_query(sql_opt_eq, &catalog, None).expect("Optional equality should analyze");
    assert_eq!(a_opt_eq.params.len(), 1);
    assert_eq!(a_opt_eq.params[0].name, "email");
    assert_eq!(a_opt_eq.params[0].ts_type, "string");
    assert!(a_opt_eq.params[0].is_optional);

    // 2. Inverted order: (email = $1 OR $1 IS NULL)
    let sql_opt_inv = "SELECT * FROM users WHERE (email = $1 OR $1 IS NULL);";
    let a_opt_inv =
        analyze_query(sql_opt_inv, &catalog, None).expect("Inverted optional should analyze");
    assert_eq!(a_opt_inv.params.len(), 1);
    assert_eq!(a_opt_inv.params[0].name, "email");
    assert_eq!(a_opt_inv.params[0].ts_type, "string");
    assert!(a_opt_inv.params[0].is_optional);

    // 3. Optional LIKE: ($1 IS NULL OR name LIKE $1)
    let sql_opt_like = "SELECT * FROM users WHERE ($1 IS NULL OR name LIKE $1);";
    let a_opt_like =
        analyze_query(sql_opt_like, &catalog, None).expect("Optional LIKE should analyze");
    assert_eq!(a_opt_like.params.len(), 1);
    assert_eq!(a_opt_like.params[0].name, "name");
    assert_eq!(a_opt_like.params[0].ts_type, "string");
    assert!(a_opt_like.params[0].is_optional);

    // 4. Optional ILIKE: ($1 IS NULL OR email ILIKE $1)
    let sql_opt_ilike = "SELECT * FROM users WHERE ($1 IS NULL OR email ILIKE $1);";
    let a_opt_ilike =
        analyze_query(sql_opt_ilike, &catalog, None).expect("Optional ILIKE should analyze");
    assert_eq!(a_opt_ilike.params.len(), 1);
    assert_eq!(a_opt_ilike.params[0].name, "email");
    assert_eq!(a_opt_ilike.params[0].ts_type, "string");
    assert!(a_opt_ilike.params[0].is_optional);

    // 5. Optional ANY: ($1 IS NULL OR id = ANY($1))
    let sql_opt_any = "SELECT * FROM users WHERE ($1 IS NULL OR id = ANY($1));";
    let a_opt_any =
        analyze_query(sql_opt_any, &catalog, None).expect("Optional ANY should analyze");
    assert_eq!(a_opt_any.params.len(), 1);
    assert_eq!(a_opt_any.params[0].name, "id");
    assert_eq!(a_opt_any.params[0].ts_type, "Array<string>");
    assert!(a_opt_any.params[0].is_optional);

    // 6. Optional symmetrical function: ($1 IS NULL OR lower(email) = lower($1))
    let sql_opt_sym = "SELECT * FROM users WHERE ($1 IS NULL OR lower(email) = lower($1));";
    let a_opt_sym =
        analyze_query(sql_opt_sym, &catalog, None).expect("Optional sym func should analyze");
    assert_eq!(a_opt_sym.params.len(), 1);
    assert_eq!(a_opt_sym.params[0].name, "email");
    assert_eq!(a_opt_sym.params[0].ts_type, "string");
    assert!(a_opt_sym.params[0].is_optional);

    // 7. Multi-filter optional conjunction:
    let sql_multi = r#"
        SELECT * FROM users
        WHERE ($1 IS NULL OR email = $1)
          AND ($2 IS NULL OR name LIKE $2)
          AND ($3 IS NULL OR id = ANY($3));
    "#;
    let a_multi = analyze_query(sql_multi, &catalog, None).expect("Multi optional should analyze");
    assert_eq!(a_multi.params.len(), 3);
    assert_eq!(a_multi.params[0].name, "email");
    assert!(a_multi.params[0].is_optional);
    assert_eq!(a_multi.params[1].name, "name");
    assert!(a_multi.params[1].is_optional);
    assert_eq!(a_multi.params[2].name, "id");
    assert!(a_multi.params[2].is_optional);
}

#[test]
fn test_param_collision_free_naming_and_disambiguation() {
    let catalog = setup_test_catalog();

    // 1. Standard duplicate names: WHERE id = $1 OR id = $2 -> id, id2
    let sql_dup = "SELECT * FROM users WHERE id = $1 OR id = $2;";
    let a_dup = analyze_query(sql_dup, &catalog, None).expect("Duplicate names should analyze");
    assert_eq!(a_dup.params.len(), 2);
    assert_eq!(a_dup.params[0].name, "id");
    assert_eq!(a_dup.params[1].name, "id2");

    // 2. Critical collision avoidance invariant:
    // WHERE id = $1 AND id2 = $2 AND id = $3
    // In buggy code: $1 is "id", $2 is "id2", and $3 was formatted as "id" + 2 = "id2" (collision!)
    // In our implementation: $3 detects "id2" is already allocated and allocates "id3"!
    let mut collision_catalog = Catalog::default();
    let collision_ddl = r#"
        CREATE TABLE metrics (
            id UUID PRIMARY KEY,
            id2 TEXT NOT NULL,
            val INT NOT NULL
        );
    "#;
    collision_catalog.apply_sql(collision_ddl).unwrap();

    let sql_coll = "SELECT * FROM metrics WHERE id = $1 AND id2 = $2 AND id = $3;";
    let a_coll =
        analyze_query(sql_coll, &collision_catalog, None).expect("Collision query should analyze");
    assert_eq!(a_coll.params.len(), 3);
    assert_eq!(a_coll.params[0].name, "id");
    assert_eq!(a_coll.params[1].name, "id2");
    assert_eq!(a_coll.params[2].name, "id3");

    // 3. Multiple references to the SAME parameter index: WHERE id = $1 OR parent_id = $1
    let sql_same = "SELECT * FROM users WHERE id = $1 OR parent_id = $1;";
    let a_same = analyze_query(sql_same, &catalog, None).expect("Same param query should analyze");
    assert_eq!(a_same.params.len(), 1);
    assert_eq!(a_same.params[0].index, 1);
    assert_eq!(a_same.params[0].name, "id");
    assert_eq!(a_same.params[0].ts_type, "string");

    // 4. Out-of-order parameter references in SQL text: WHERE name = $2 AND id = $1
    let sql_order = "SELECT * FROM users WHERE name = $2 AND id = $1;";
    let a_order =
        analyze_query(sql_order, &catalog, None).expect("Out-of-order query should analyze");
    assert_eq!(a_order.params.len(), 2);
    assert_eq!(a_order.params[0].index, 1);
    assert_eq!(a_order.params[0].name, "id");
    assert_eq!(a_order.params[1].index, 2);
    assert_eq!(a_order.params[1].name, "name");
}

#[test]
fn test_param_dml_statements() {
    let catalog = setup_test_catalog();

    // 1. Multi-row INSERT statement:
    let sql_multi_insert = r#"
        INSERT INTO users (name, email)
        VALUES ($1, $2), ($3, $4);
    "#;
    let a_mi =
        analyze_query(sql_multi_insert, &catalog, None).expect("Multi-row INSERT should analyze");
    assert_eq!(a_mi.params.len(), 4);
    assert_eq!(a_mi.params[0].name, "name");
    assert_eq!(a_mi.params[1].name, "email");
    assert_eq!(a_mi.params[2].name, "name2");
    assert_eq!(a_mi.params[3].name, "email2");

    // 2. INSERT with ON CONFLICT DO UPDATE:
    let sql_on_conflict = r#"
        INSERT INTO users (id, name, email, role_id)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (id) DO UPDATE
        SET name = $5
        WHERE users.active = $6;
    "#;
    let a_oc = analyze_query(sql_on_conflict, &catalog, None).expect("ON CONFLICT should analyze");
    assert_eq!(a_oc.params.len(), 6);
    assert_eq!(a_oc.params[0].name, "id");
    assert_eq!(a_oc.params[1].name, "name");
    assert_eq!(a_oc.params[2].name, "email");
    assert_eq!(a_oc.params[3].name, "role_id");
    assert_eq!(a_oc.params[4].name, "name2");
    assert_eq!(a_oc.params[5].name, "active");
    assert_eq!(a_oc.params[5].ts_type, "boolean");

    // 3. UPDATE statement with SET target list and WHERE clause:
    let sql_update = "UPDATE users SET name = $1, age = $2 WHERE id = $3;";
    let a_up = analyze_query(sql_update, &catalog, None).expect("UPDATE should analyze");
    assert_eq!(a_up.params.len(), 3);
    assert_eq!(a_up.params[0].name, "name");
    assert_eq!(a_up.params[0].ts_type, "string");
    assert_eq!(a_up.params[1].name, "age");
    assert_eq!(a_up.params[1].ts_type, "number");
    assert!(a_up.params[1].is_optional); // age is nullable in users table
    assert_eq!(a_up.params[2].name, "id");
    assert_eq!(a_up.params[2].ts_type, "string");

    // 4. DELETE statement with WHERE clause:
    let sql_del = "DELETE FROM users WHERE id = $1 AND active = $2;";
    let a_del = analyze_query(sql_del, &catalog, None).expect("DELETE should analyze");
    assert_eq!(a_del.params.len(), 2);
    assert_eq!(a_del.params[0].name, "id");
    assert_eq!(a_del.params[0].ts_type, "string");
    assert_eq!(a_del.params[1].name, "active");
    assert_eq!(a_del.params[1].ts_type, "boolean");
}

#[test]
fn test_param_select_having_clause() {
    let catalog = setup_test_catalog();
    let sql = "SELECT org_id, count(*) FROM users GROUP BY org_id HAVING count(*) > $1;";
    let analyzed = analyze_query(sql, &catalog, None).expect("HAVING query should analyze");
    assert_eq!(analyzed.params.len(), 1);
    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "param1");
    assert_eq!(analyzed.params[0].ts_type, "number");
}

#[test]
fn test_standalone_deduce_query_params_api() {
    let catalog = setup_test_catalog();
    let sql = "SELECT * FROM users WHERE email = $1 AND role_id = $2;";
    let parsed = pg_query::parse(sql).expect("Parse should succeed");
    let root = parsed.protobuf.stmts.first().expect("Statement required");
    let actual_node = root.stmt.as_deref().expect("RawStmt has stmt");

    let params = deduce_query_params(actual_node, &catalog, &QueryScope::default())
        .expect("deduce_query_params should succeed");
    assert_eq!(params.len(), 2);
    assert_eq!(params[0].name, "email");
    assert_eq!(params[0].ts_type, "string");
    assert_eq!(params[1].name, "role_id");
    assert_eq!(params[1].ts_type, "number");
}

#[test]
fn test_codegen_with_prepared_parameters() {
    let catalog = setup_test_catalog();
    let sql = r#"
-- name: FindUsersByTag
SELECT id, name, email
FROM users
WHERE tags @> $1 AND ($2 IS NULL OR status = $2);
    "#;

    let analyzed = analyze_query(sql, &catalog, Some("find_users_by_tag.sql")).unwrap();
    assert_eq!(analyzed.name, "FindUsersByTag");
    assert_eq!(analyzed.params.len(), 2);
    assert_eq!(analyzed.params[0].name, "tags");
    assert_eq!(analyzed.params[0].ts_type, "Array<string>");
    assert_eq!(analyzed.params[1].name, "status");
    assert!(analyzed.params[1].is_optional);

    let ts_code = generate_file_ts(&[analyzed]);
    assert!(ts_code.contains("export interface FindUsersByTagParams {"));
    assert!(ts_code.contains("tags: Array<string>;"));
    assert!(ts_code.contains("status?: \"active\" | \"pending\" | \"suspended\" | null;"));
    assert!(ts_code.contains("export interface FindUsersByTagRow {"));
}
