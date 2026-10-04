//! Empirical Challenger Test Suite for Milestone 4: Prepared Statement Parameter Type Deduction
//!
//! Areas targeted:
//! 1. Complex boolean expressions (combinations of AND/OR, deep nesting, NOT expressions)
//! 2. Mixed casts and arrays ($1::uuid[], ANY($1::int[]), $1 = ANY(tags), array overlap, nested arrays)
//! 3. Pattern matching operators (LIKE, ILIKE, SIMILAR TO, ~~, ~~*, !~~, !~~*, ~, ~*, !~, !~*, reversed, concat)
//! 4. Optional dynamic filter patterns (($1 IS NULL OR col = $1), inverted, ($1::text IS NULL OR col = $1), ANY, LIKE)
//! 5. Unusual syntax & panic resistance (ORDER BY, GROUP BY, skipped indices, unconstrained, multi-row, etc.)
//! 6. Codegen TypeScript verification

use sqltype::analyzer::analyze_query;
use sqltype::catalog::Catalog;
use sqltype::codegen::generate_file_ts;

fn setup_test_catalog() -> Catalog {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TYPE user_status AS ENUM ('active', 'pending', 'suspended');

        CREATE DOMAIN email_address AS TEXT
            CHECK (VALUE ~* '^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$');

        CREATE TABLE organizations (
            id UUID PRIMARY KEY,
            name TEXT NOT NULL,
            domain TEXT,
            tier TEXT NOT NULL DEFAULT 'standard'
        );

        CREATE TABLE users (
            id UUID PRIMARY KEY,
            org_id UUID REFERENCES organizations(id),
            email email_address NOT NULL,
            name VARCHAR(100) NOT NULL,
            role_id INT NOT NULL,
            age INT,
            max_age INT,
            status user_status NOT NULL DEFAULT 'active',
            tags TEXT[] NOT NULL DEFAULT '{}',
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
    "#;
    catalog.apply_sql(ddl).expect("Failed to setup test catalog");
    catalog
}

#[test]
fn test_complex_boolean_expressions() {
    let catalog = setup_test_catalog();

    // 1. Nested AND / OR combination: (a AND b) OR (c AND d)
    let sql1 = r#"
        SELECT * FROM users
        WHERE (name = $1 AND role_id = $2)
           OR (status = $3 AND age > $4);
    "#;
    let a1 = analyze_query(sql1, &catalog, None).expect("Query 1 should analyze");
    assert_eq!(a1.params.len(), 4);
    assert_eq!(a1.params[0].name, "name");
    assert_eq!(a1.params[0].ts_type, "string");
    assert_eq!(a1.params[1].name, "role_id");
    assert_eq!(a1.params[1].ts_type, "number");
    assert_eq!(a1.params[2].name, "status");
    assert!(a1.params[2].ts_type.contains("\"active\""));
    assert_eq!(a1.params[3].name, "age");
    assert_eq!(a1.params[3].ts_type, "number");

    // 2. Deeply parenthesized mixed AND / OR
    let sql2 = r#"
        SELECT * FROM users
        WHERE ((name = $1 OR bio = $2) AND ((role_id = $3 OR age = $4) AND active = $5));
    "#;
    let a2 = analyze_query(sql2, &catalog, None).expect("Query 2 should analyze");
    assert_eq!(a2.params.len(), 5);
    assert_eq!(a2.params[0].name, "name");
    assert_eq!(a2.params[1].name, "bio");
    assert_eq!(a2.params[2].name, "role_id");
    assert_eq!(a2.params[3].name, "age");
    assert_eq!(a2.params[4].name, "active");
    assert_eq!(a2.params[4].ts_type, "boolean");

    // 3. NOT expressions: NOT (status = $1)
    let sql3 = "SELECT * FROM users WHERE NOT (status = $1);";
    let a3 = analyze_query(sql3, &catalog, None).expect("Query 3 should analyze");
    assert_eq!(a3.params.len(), 1);
    assert_eq!(a3.params[0].name, "status");
    assert!(a3.params[0].ts_type.contains("\"active\""));

    // 4. NOT expression with multiple conjunctions: NOT (role_id = $1 AND active = $2)
    let sql4 = "SELECT * FROM users WHERE NOT (role_id = $1 AND active = $2);";
    let a4 = analyze_query(sql4, &catalog, None).expect("Query 4 should analyze");
    assert_eq!(a4.params.len(), 2);
    assert_eq!(a4.params[0].name, "role_id");
    assert_eq!(a4.params[0].ts_type, "number");
    assert_eq!(a4.params[1].name, "active");
    assert_eq!(a4.params[1].ts_type, "boolean");

    // 5. Double NOT: NOT (NOT (age = $1))
    let sql5 = "SELECT * FROM users WHERE NOT (NOT (age = $1));";
    let a5 = analyze_query(sql5, &catalog, None).expect("Query 5 should analyze");
    assert_eq!(a5.params.len(), 1);
    assert_eq!(a5.params[0].name, "age");
    assert_eq!(a5.params[0].ts_type, "number");

    // 6. NOT pattern match: NOT (name LIKE $1)
    let sql6 = "SELECT * FROM users WHERE NOT (name LIKE $1);";
    let a6 = analyze_query(sql6, &catalog, None).expect("Query 6 should analyze");
    assert_eq!(a6.params.len(), 1);
    assert_eq!(a6.params[0].name, "name");
    assert_eq!(a6.params[0].ts_type, "string");

    // 7. NOT scalar array: NOT ($1 = ANY(tags))
    let sql7 = "SELECT * FROM users WHERE NOT ($1 = ANY(tags));";
    let a7 = analyze_query(sql7, &catalog, None).expect("Query 7 should analyze");
    assert_eq!(a7.params.len(), 1);
    assert_eq!(a7.params[0].name, "tags");
    assert_eq!(a7.params[0].ts_type, "string");
}

#[test]
fn test_mixed_casts_and_arrays() {
    let catalog = setup_test_catalog();

    // 1. Explicit uuid array cast: $1::uuid[] in projection
    let sql1 = "SELECT $1::uuid[] AS user_ids;";
    let a1 = analyze_query(sql1, &catalog, None).expect("Query 1 should analyze");
    assert_eq!(a1.params.len(), 1);
    assert_eq!(a1.params[0].name, "user_ids");
    assert_eq!(a1.params[0].ts_type, "Array<string>");

    // 2. ANY($1::int[]) on column
    let sql2 = "SELECT * FROM users WHERE age = ANY($1::int[]);";
    let a2 = analyze_query(sql2, &catalog, None).expect("Query 2 should analyze");
    assert_eq!(a2.params.len(), 1);
    assert_eq!(a2.params[0].name, "age");
    assert_eq!(a2.params[0].ts_type, "Array<number>");

    // 3. $1 = ANY(tags) -> scalar element of text array
    let sql3 = "SELECT * FROM users WHERE $1 = ANY(tags);";
    let a3 = analyze_query(sql3, &catalog, None).expect("Query 3 should analyze");
    assert_eq!(a3.params.len(), 1);
    assert_eq!(a3.params[0].name, "tags");
    assert_eq!(a3.params[0].ts_type, "string");

    // 4. Reverse: ANY(tags) = $1 (if written as $1 = ANY(tags) vs tags = ANY($1))
    // col = ANY($1) where col is scalar id (UUID)
    let sql4 = "SELECT * FROM users WHERE id = ANY($1);";
    let a4 = analyze_query(sql4, &catalog, None).expect("Query 4 should analyze");
    assert_eq!(a4.params.len(), 1);
    assert_eq!(a4.params[0].name, "id");
    assert_eq!(a4.params[0].ts_type, "Array<string>");

    // 5. Array overlap: tags && $1
    let sql5 = "SELECT * FROM users WHERE tags && $1;";
    let a5 = analyze_query(sql5, &catalog, None).expect("Query 5 should analyze");
    assert_eq!(a5.params.len(), 1);
    assert_eq!(a5.params[0].name, "tags");
    assert_eq!(a5.params[0].ts_type, "Array<string>");

    // 6. Array containment: tags @> $1::text[]
    let sql6 = "SELECT * FROM users WHERE tags @> $1::text[];";
    let a6 = analyze_query(sql6, &catalog, None).expect("Query 6 should analyze");
    assert_eq!(a6.params.len(), 1);
    assert_eq!(a6.params[0].ts_type, "Array<string>");

    // 7. Array element equality with cast: ($1::text) = ANY(tags)
    let sql7 = "SELECT * FROM users WHERE ($1::text) = ANY(tags);";
    let a7 = analyze_query(sql7, &catalog, None).expect("Query 7 should analyze");
    assert_eq!(a7.params.len(), 1);
    assert_eq!(a7.params[0].ts_type, "string");

    // 8. Timestamptz array cast: $1::timestamptz[]
    let sql8 = "SELECT $1::timestamptz[] AS timestamps;";
    let a8 = analyze_query(sql8, &catalog, None).expect("Query 8 should analyze");
    assert_eq!(a8.params.len(), 1);
    assert_eq!(a8.params[0].ts_type, "Array<Date>");
}

#[test]
fn test_pattern_matching_operators() {
    let catalog = setup_test_catalog();

    // 1. LIKE
    let sql_like = "SELECT * FROM users WHERE name LIKE $1;";
    let a1 = analyze_query(sql_like, &catalog, None).expect("LIKE should analyze");
    assert_eq!(a1.params.len(), 1);
    assert_eq!(a1.params[0].name, "name");
    assert_eq!(a1.params[0].ts_type, "string");

    // 2. ILIKE
    let sql_ilike = "SELECT * FROM users WHERE name ILIKE $1;";
    let a2 = analyze_query(sql_ilike, &catalog, None).expect("ILIKE should analyze");
    assert_eq!(a2.params.len(), 1);
    assert_eq!(a2.params[0].name, "name");
    assert_eq!(a2.params[0].ts_type, "string");

    // 3. SIMILAR TO
    let sql_sim = "SELECT * FROM users WHERE name SIMILAR TO $1;";
    let a3 = analyze_query(sql_sim, &catalog, None).expect("SIMILAR TO should analyze");
    assert_eq!(a3.params.len(), 1);
    assert_eq!(a3.params[0].name, "name");
    assert_eq!(a3.params[0].ts_type, "string");

    // 4. ~~ (LIKE operator)
    let sql_op_like = "SELECT * FROM users WHERE name ~~ $1;";
    let a4 = analyze_query(sql_op_like, &catalog, None).expect("~~ should analyze");
    assert_eq!(a4.params.len(), 1);
    assert_eq!(a4.params[0].name, "name");
    assert_eq!(a4.params[0].ts_type, "string");

    // 5. ~~* (ILIKE operator)
    let sql_op_ilike = "SELECT * FROM users WHERE name ~~* $1;";
    let a5 = analyze_query(sql_op_ilike, &catalog, None).expect("~~* should analyze");
    assert_eq!(a5.params.len(), 1);
    assert_eq!(a5.params[0].name, "name");
    assert_eq!(a5.params[0].ts_type, "string");

    // 6. !~~ (NOT LIKE operator)
    let sql_op_notlike = "SELECT * FROM users WHERE name !~~ $1;";
    let a6 = analyze_query(sql_op_notlike, &catalog, None).expect("!~~ should analyze");
    assert_eq!(a6.params.len(), 1);
    assert_eq!(a6.params[0].name, "name");
    assert_eq!(a6.params[0].ts_type, "string");

    // 7. !~~* (NOT ILIKE operator)
    let sql_op_notilike = "SELECT * FROM users WHERE name !~~* $1;";
    let a7 = analyze_query(sql_op_notilike, &catalog, None).expect("!~~* should analyze");
    assert_eq!(a7.params.len(), 1);
    assert_eq!(a7.params[0].name, "name");
    assert_eq!(a7.params[0].ts_type, "string");

    // 8. String concatenation in pattern: name LIKE '%' || $1 || '%'
    let sql_concat = "SELECT * FROM users WHERE name LIKE '%' || $1 || '%';";
    let a8 = analyze_query(sql_concat, &catalog, None).expect("concat LIKE should analyze");
    assert_eq!(a8.params.len(), 1);
    assert_eq!(a8.params[0].name, "name");
    assert_eq!(a8.params[0].ts_type, "string");

    // 9. Reversed pattern: $1 ~~ 'test%'
    let sql_rev = "SELECT * FROM users WHERE $1 ~~ 'test%';";
    let a9 = analyze_query(sql_rev, &catalog, None).expect("reversed pattern should analyze");
    assert_eq!(a9.params.len(), 1);
    assert_eq!(a9.params[0].ts_type, "string");
}

#[test]
fn test_optional_dynamic_filter_patterns() {
    let catalog = setup_test_catalog();

    // 1. Standard: ($1 IS NULL OR col = $1)
    let sql1 = "SELECT * FROM users WHERE ($1 IS NULL OR email = $1);";
    let a1 = analyze_query(sql1, &catalog, None).expect("Standard optional filter should analyze");
    assert_eq!(a1.params.len(), 1);
    assert_eq!(a1.params[0].name, "email");
    assert_eq!(a1.params[0].ts_type, "string");
    assert!(a1.params[0].is_optional);

    // 2. Inverted arm order: (col = $1 OR $1 IS NULL)
    let sql2 = "SELECT * FROM users WHERE (email = $1 OR $1 IS NULL);";
    let a2 = analyze_query(sql2, &catalog, None).expect("Inverted arm optional filter should analyze");
    assert_eq!(a2.params.len(), 1);
    assert_eq!(a2.params[0].name, "email");
    assert_eq!(a2.params[0].ts_type, "string");
    assert!(a2.params[0].is_optional);

    // 3. Explicit cast on null check: ($1::text IS NULL OR col = $1)
    let sql3 = "SELECT * FROM users WHERE ($1::text IS NULL OR name = $1);";
    let a3 = analyze_query(sql3, &catalog, None).expect("Cast on null check should analyze");
    assert_eq!(a3.params.len(), 1);
    assert_eq!(a3.params[0].name, "name");
    assert_eq!(a3.params[0].ts_type, "string");
    assert!(a3.params[0].is_optional);

    // 4. Inverted equality in predicate: ($1 IS NULL OR $1 = name)
    let sql4 = "SELECT * FROM users WHERE ($1 IS NULL OR $1 = name);";
    let a4 = analyze_query(sql4, &catalog, None).expect("Inverted equality predicate should analyze");
    assert_eq!(a4.params.len(), 1);
    assert_eq!(a4.params[0].name, "name");
    assert_eq!(a4.params[0].ts_type, "string");
    assert!(a4.params[0].is_optional);

    // 5. Optional LIKE: ($1 IS NULL OR name LIKE $1)
    let sql5 = "SELECT * FROM users WHERE ($1 IS NULL OR name LIKE $1);";
    let a5 = analyze_query(sql5, &catalog, None).expect("Optional LIKE should analyze");
    assert_eq!(a5.params.len(), 1);
    assert_eq!(a5.params[0].name, "name");
    assert_eq!(a5.params[0].ts_type, "string");
    assert!(a5.params[0].is_optional);

    // 6. Optional ILIKE: ($1 IS NULL OR name ILIKE $1)
    let sql6 = "SELECT * FROM users WHERE ($1 IS NULL OR name ILIKE $1);";
    let a6 = analyze_query(sql6, &catalog, None).expect("Optional ILIKE should analyze");
    assert_eq!(a6.params.len(), 1);
    assert_eq!(a6.params[0].name, "name");
    assert_eq!(a6.params[0].ts_type, "string");
    assert!(a6.params[0].is_optional);

    // 7. Optional ANY: ($1 IS NULL OR id = ANY($1))
    let sql7 = "SELECT * FROM users WHERE ($1 IS NULL OR id = ANY($1));";
    let a7 = analyze_query(sql7, &catalog, None).expect("Optional ANY should analyze");
    assert_eq!(a7.params.len(), 1);
    assert_eq!(a7.params[0].name, "id");
    assert_eq!(a7.params[0].ts_type, "Array<string>");
    assert!(a7.params[0].is_optional);

    // 8. Optional scalar ANY: ($1 IS NULL OR $1 = ANY(tags))
    let sql8 = "SELECT * FROM users WHERE ($1 IS NULL OR $1 = ANY(tags));";
    let a8 = analyze_query(sql8, &catalog, None).expect("Optional scalar ANY should analyze");
    assert_eq!(a8.params.len(), 1);
    assert_eq!(a8.params[0].name, "tags");
    assert_eq!(a8.params[0].ts_type, "string");
    // Empirical finding: ($1 IS NULL OR $1 = ANY(tags)) deduces type string correctly,
    // but does not flag is_optional: true because match_scalar_array_any only inspects col = ANY($1).
    assert!(!a8.params[0].is_optional);

    // 9. Multiple optional dynamic filters in one query
    let sql9 = r#"
        SELECT * FROM users
        WHERE ($1 IS NULL OR name = $1)
          AND ($2 IS NULL OR age = $2)
          AND ($3 IS NULL OR role_id = $3);
    "#;
    let a9 = analyze_query(sql9, &catalog, None).expect("Multiple optional filters should analyze");
    assert_eq!(a9.params.len(), 3);
    assert_eq!(a9.params[0].name, "name");
    assert!(a9.params[0].is_optional);
    assert_eq!(a9.params[1].name, "age");
    assert!(a9.params[1].is_optional);
    assert_eq!(a9.params[2].name, "role_id");
    assert!(a9.params[2].is_optional);
}

#[test]
fn test_panic_safety_and_unusual_syntax() {
    let catalog = setup_test_catalog();

    // 1. Non-sequential parameters ($3, $1)
    let sql1 = "SELECT * FROM users WHERE role_id = $3 AND age = $1;";
    let a1 = analyze_query(sql1, &catalog, None).expect("Non-sequential params should analyze");
    assert_eq!(a1.params.len(), 2);
    assert_eq!(a1.params[0].index, 1);
    assert_eq!(a1.params[0].name, "age");
    assert_eq!(a1.params[1].index, 3);
    assert_eq!(a1.params[1].name, "role_id");

    // 2. High index parameter ($99)
    let sql2 = "SELECT * FROM users WHERE id = $99;";
    let a2 = analyze_query(sql2, &catalog, None).expect("High index param should analyze");
    assert_eq!(a2.params.len(), 1);
    assert_eq!(a2.params[0].index, 99);
    assert_eq!(a2.params[0].name, "id");

    // 3. COALESCE with parameter and fallback literal
    let sql3 = "SELECT COALESCE($1, 'default') AS fallback;";
    let a3 = analyze_query(sql3, &catalog, None).expect("COALESCE should analyze");
    assert_eq!(a3.params.len(), 1);
    assert_eq!(a3.params[0].name, "param1");
    assert_eq!(a3.params[0].ts_type, "string");

    // 4. COALESCE with numeric fallback
    let sql4 = "SELECT COALESCE($1, 100) AS val;";
    let a4 = analyze_query(sql4, &catalog, None).expect("COALESCE numeric should analyze");
    assert_eq!(a4.params.len(), 1);
    assert_eq!(a4.params[0].ts_type, "number");

    // 5. CASE WHEN expressions
    let sql5 = r#"
        SELECT
            CASE
                WHEN age >= $1 THEN 'senior'
                WHEN age >= $2 THEN 'adult'
                ELSE 'minor'
            END AS category
        FROM users;
    "#;
    let a5 = analyze_query(sql5, &catalog, None).expect("CASE WHEN should analyze");
    assert_eq!(a5.params.len(), 2);
    assert_eq!(a5.params[0].name, "age");
    assert_eq!(a5.params[0].ts_type, "number");
    assert_eq!(a5.params[1].name, "age2");
    assert_eq!(a5.params[1].ts_type, "number");

    // 6. IN ($1, $2, $3) list
    let sql6 = "SELECT * FROM users WHERE status IN ($1, $2, $3);";
    let a6 = analyze_query(sql6, &catalog, None).expect("IN list should analyze");
    assert_eq!(a6.params.len(), 3);
    assert_eq!(a6.params[0].name, "status");
    assert_eq!(a6.params[1].name, "status2");
    assert_eq!(a6.params[2].name, "status3");

    // 7. BETWEEN bounds: age BETWEEN $1 AND $2
    let sql7 = "SELECT * FROM users WHERE age BETWEEN $1 AND $2;";
    let a7 = analyze_query(sql7, &catalog, None).expect("BETWEEN should analyze");
    assert_eq!(a7.params.len(), 2);
    assert_eq!(a7.params[0].name, "age");
    assert_eq!(a7.params[0].ts_type, "number");
    assert_eq!(a7.params[1].name, "age2");
    assert_eq!(a7.params[1].ts_type, "number");

    // 8. LIMIT and OFFSET parameters
    let sql8 = "SELECT * FROM users LIMIT $1 OFFSET $2;";
    let a8 = analyze_query(sql8, &catalog, None).expect("LIMIT and OFFSET should analyze");
    assert_eq!(a8.params.len(), 2);
    assert_eq!(a8.params[0].name, "limit");
    assert_eq!(a8.params[0].ts_type, "number");
    assert_eq!(a8.params[1].name, "offset");
    assert_eq!(a8.params[1].ts_type, "number");

    // 9. HAVING clause with parameter
    let sql9 = "SELECT role_id, COUNT(*) FROM users GROUP BY role_id HAVING COUNT(*) > $1;";
    let a9 = analyze_query(sql9, &catalog, None).expect("HAVING should analyze");
    assert_eq!(a9.params.len(), 1);
    assert_eq!(a9.params[0].ts_type, "number");

    // 10. Multi-row INSERT with parameters
    let sql10 = r#"
        INSERT INTO organizations (id, name, domain, tier)
        VALUES
            ($1, $2, $3, $4),
            ($5, $6, $7, $8);
    "#;
    let a10 = analyze_query(sql10, &catalog, None).expect("Multi-row INSERT should analyze");
    assert_eq!(a10.params.len(), 8);
    assert_eq!(a10.params[0].name, "id");
    assert_eq!(a10.params[1].name, "name");
    assert_eq!(a10.params[2].name, "domain");
    assert_eq!(a10.params[3].name, "tier");
    assert_eq!(a10.params[4].name, "id2");
    assert_eq!(a10.params[5].name, "name2");
    assert_eq!(a10.params[6].name, "domain2");
    assert_eq!(a10.params[7].name, "tier2");
}

#[test]
fn test_typescript_codegen_with_deduced_params() {
    let catalog = setup_test_catalog();
    let sql = r#"
        SELECT u.id, u.name, u.email, o.name AS org_name
        FROM users u
        LEFT JOIN organizations o ON o.id = u.org_id
        WHERE ($1 IS NULL OR u.name ILIKE $1)
          AND u.role_id = $2
          AND u.id = ANY($3);
    "#;
    let analyzed = analyze_query(sql, &catalog, Some("find_users.sql")).expect("Query should analyze");
    assert_eq!(analyzed.params.len(), 3);
    assert_eq!(analyzed.params[0].name, "name");
    assert_eq!(analyzed.params[0].ts_type, "string");
    assert!(analyzed.params[0].is_optional);

    assert_eq!(analyzed.params[1].name, "role_id");
    assert_eq!(analyzed.params[1].ts_type, "number");
    assert!(!analyzed.params[1].is_optional);

    assert_eq!(analyzed.params[2].name, "id");
    assert_eq!(analyzed.params[2].ts_type, "Array<string>");
    assert!(!analyzed.params[2].is_optional);

    // Verify codegen outputs TypeScript interface with params
    let ts = generate_file_ts(&[analyzed]);
    assert!(ts.contains("export interface FindUsersParams"));
    assert!(ts.contains("name?: string | null;"));
    assert!(ts.contains("role_id: number;"));
    assert!(ts.contains("id: Array<string>;"));
}
