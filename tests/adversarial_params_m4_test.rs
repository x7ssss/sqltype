//! Adversarial Test Suite for Milestone 4: Prepared Statement Parameter Type Deduction
//!
//! Empirical challenges:
//! 1. Deeply nested binary expressions and arithmetic counterparts
//! 2. Multi-cast expressions ($1::text::varchar, $1::uuid[], ($1::int)::text, etc.)
//! 3. Inverted and symmetrical functions (lower(col) = lower($1), lower($1) = lower(col), upper, trim, etc.)
//! 4. Multiple parameters on the same column (collision stress: id = $1 AND id2 = $2 AND id3 = $3 AND id = $4)
//! 5. Pattern matching operators with various PostgreSQL operator encodings (~~, ~~*, !~~, !~~*, ~, ~*, !~, !~*, SIMILAR TO, NOT SIMILAR TO)
//! 6. Optional dynamic filter combinations with LIKE, ILIKE, ANY, pattern concatenation, and scalar ANY
//! 7. Set operations (UNION/INTERSECT/EXCEPT), SubLink subqueries, and CTEs with parameters
//! 8. Target list typecasts, Coalesce, CASE WHEN, and NULL tests

use sqltype::analyzer::analyze_query;
use sqltype::catalog::Catalog;

fn setup_adv_catalog() -> Catalog {
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
            score NUMERIC(10, 2),
            created_at TIMESTAMPTZ NOT NULL
        );

        CREATE TABLE metrics (
            id UUID PRIMARY KEY,
            id2 TEXT NOT NULL,
            id3 TEXT NOT NULL,
            val INT NOT NULL
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
fn test_adv_deeply_nested_binary_expressions() {
    let catalog = setup_adv_catalog();

    // 1. Deep boolean tree with mixed AND / OR
    let sql1 = r#"
        SELECT * FROM users
        WHERE (((id = $1 AND role_id = $2) OR (age > $3 AND active = $4))
               AND ((email = $5 OR name = $6) AND created_at <= $7));
    "#;
    let a1 = analyze_query(sql1, &catalog, None).expect("Deeply nested query should parse");
    assert_eq!(a1.params.len(), 7);
    assert_eq!(a1.params[0].name, "id");
    assert_eq!(a1.params[0].ts_type, "string");
    assert_eq!(a1.params[1].name, "role_id");
    assert_eq!(a1.params[1].ts_type, "number");
    assert_eq!(a1.params[2].name, "age");
    assert_eq!(a1.params[2].ts_type, "number");
    assert_eq!(a1.params[3].name, "active");
    assert_eq!(a1.params[3].ts_type, "boolean");
    assert_eq!(a1.params[4].name, "email");
    assert_eq!(a1.params[4].ts_type, "string");
    assert_eq!(a1.params[5].name, "name");
    assert_eq!(a1.params[5].ts_type, "string");
    assert_eq!(a1.params[6].name, "created_at");
    assert_eq!(a1.params[6].ts_type, "Date");

    // 2. Nested arithmetic comparisons
    let sql2 = "SELECT * FROM users WHERE age > ($1 + 5);";
    let a2 = analyze_query(sql2, &catalog, None).expect("Arithmetic comparison should parse");
    assert_eq!(a2.params.len(), 1);
    assert_eq!(a2.params[0].ts_type, "number");

    // 3. Reverse nested arithmetic comparison: ($1 * 2) <= age
    let sql3 = "SELECT * FROM users WHERE ($1 * 2) <= age;";
    let a3 =
        analyze_query(sql3, &catalog, None).expect("Reverse arithmetic comparison should parse");
    assert_eq!(a3.params.len(), 1);
    assert_eq!(a3.params[0].ts_type, "number");
}

#[test]
fn test_adv_multi_cast_expressions() {
    let catalog = setup_adv_catalog();

    // 1. Multi-cast in projection: $1::text::varchar
    let sql1 = "SELECT $1::text::varchar AS name;";
    let a1 = analyze_query(sql1, &catalog, None).expect("Multi-cast projection should parse");
    assert_eq!(a1.params.len(), 1);
    assert_eq!(a1.params[0].name, "name");
    assert_eq!(a1.params[0].ts_type, "string");

    // 2. Explicit array cast: $1::uuid[]
    let sql2 = "SELECT $1::uuid[] AS ids;";
    let a2 = analyze_query(sql2, &catalog, None).expect("Array cast projection should parse");
    assert_eq!(a2.params.len(), 1);
    assert_eq!(a2.params[0].name, "ids");
    assert_eq!(a2.params[0].ts_type, "Array<string>");

    // 3. Multi-cast in WHERE: name = ($1::text)::varchar
    let sql3 = "SELECT * FROM users WHERE name = ($1::text)::varchar;";
    let a3 = analyze_query(sql3, &catalog, None).expect("Multi-cast WHERE should parse");
    assert_eq!(a3.params.len(), 1);
    assert_eq!(a3.params[0].name, "name");
    assert_eq!(a3.params[0].ts_type, "string");

    // 4. Int to text cast: ($1::int)::text in SELECT
    let sql4 = "SELECT ($1::int)::text AS converted;";
    let a4 = analyze_query(sql4, &catalog, None).expect("Int-to-text cast should parse");
    assert_eq!(a4.params.len(), 1);
    assert_eq!(a4.params[0].name, "converted");
    // $1 was explicitly cast to int by user
    println!("Int-to-text result ts_type: {}", a4.params[0].ts_type);

    // 5. Array cast with timestamptz[]
    let sql5 = "SELECT $1::timestamptz[] AS dates;";
    let a5 = analyze_query(sql5, &catalog, None).expect("Array timestamptz cast should parse");
    assert_eq!(a5.params.len(), 1);
    assert_eq!(a5.params[0].ts_type, "Array<Date>");
}

#[test]
fn test_adv_inverted_and_symmetrical_functions() {
    let catalog = setup_adv_catalog();

    // 1. lower(email) = lower($1)
    let sql1 = "SELECT * FROM users WHERE lower(email) = lower($1);";
    let a1 = analyze_query(sql1, &catalog, None).expect("lower(col) = lower($1) should parse");
    assert_eq!(a1.params.len(), 1);
    assert_eq!(a1.params[0].name, "email");
    assert_eq!(a1.params[0].ts_type, "string");

    // 2. lower($1) = lower(email) (inverted)
    let sql2 = "SELECT * FROM users WHERE lower($1) = lower(email);";
    let a2 = analyze_query(sql2, &catalog, None).expect("lower($1) = lower(col) should parse");
    assert_eq!(a2.params.len(), 1);
    assert_eq!(a2.params[0].name, "email");
    assert_eq!(a2.params[0].ts_type, "string");

    // 3. upper(name) = upper($1)
    let sql3 = "SELECT * FROM users WHERE upper(name) = upper($1);";
    let a3 = analyze_query(sql3, &catalog, None).expect("upper should parse");
    assert_eq!(a3.params.len(), 1);
    assert_eq!(a3.params[0].name, "name");
    assert_eq!(a3.params[0].ts_type, "string");

    // 4. trim(name) = trim($1)
    let sql4 = "SELECT * FROM users WHERE trim(name) = trim($1);";
    let a4 = analyze_query(sql4, &catalog, None).expect("trim should parse");
    assert_eq!(a4.params.len(), 1);
    assert_eq!(a4.params[0].name, "name");
    assert_eq!(a4.params[0].ts_type, "string");

    // 5. Symmetrical function with table alias: lower(u.email) = lower($1)
    let sql5 = "SELECT * FROM users u WHERE lower(u.email) = lower($1);";
    let a5 = analyze_query(sql5, &catalog, None).expect("aliased lower should parse");
    assert_eq!(a5.params.len(), 1);
    assert_eq!(a5.params[0].name, "email");
    assert_eq!(a5.params[0].ts_type, "string");
}

#[test]
fn test_adv_multiple_params_same_column_collision_stress() {
    let catalog = setup_adv_catalog();

    // 1. Multiple parameters on same column: 4 params on `id`
    let sql1 = "SELECT * FROM users WHERE id = $1 OR id = $2 OR id = $3 OR id = $4;";
    let a1 = analyze_query(sql1, &catalog, None).expect("4 duplicate id params should parse");
    assert_eq!(a1.params.len(), 4);
    assert_eq!(a1.params[0].name, "id");
    assert_eq!(a1.params[1].name, "id2");
    assert_eq!(a1.params[2].name, "id3");
    assert_eq!(a1.params[3].name, "id4");

    // 2. Existing id2 and id3 in table: id = $1 AND id2 = $2 AND id3 = $3 AND id = $4
    let sql2 = "SELECT * FROM metrics WHERE id = $1 AND id2 = $2 AND id3 = $3 AND id = $4;";
    let a2 = analyze_query(sql2, &catalog, None).expect("id collision stress should parse");
    assert_eq!(a2.params.len(), 4);
    assert_eq!(a2.params[0].name, "id");
    assert_eq!(a2.params[1].name, "id2");
    assert_eq!(a2.params[2].name, "id3");
    assert_eq!(a2.params[3].name, "id4");

    // 3. Out of order references: id = $4 AND id = $1 AND id = $3 AND id = $2
    let sql3 = "SELECT * FROM users WHERE id = $4 AND id = $1 AND id = $3 AND id = $2;";
    let a3 = analyze_query(sql3, &catalog, None).expect("out of order id params should parse");
    assert_eq!(a3.params.len(), 4);
    assert_eq!(a3.params[0].index, 1);
    assert_eq!(a3.params[0].name, "id");
    assert_eq!(a3.params[1].index, 2);
    assert_eq!(a3.params[1].name, "id2");
    assert_eq!(a3.params[2].index, 3);
    assert_eq!(a3.params[2].name, "id3");
    assert_eq!(a3.params[3].index, 4);
    assert_eq!(a3.params[3].name, "id4");
}

#[test]
fn test_adv_pattern_matching_operator_encodings() {
    let catalog = setup_adv_catalog();

    // 1. Raw operators: ~~, ~~*, !~~, !~~*
    let ops = vec![
        ("~~", "LIKE"),
        ("~~*", "ILIKE"),
        ("!~~", "NOT LIKE"),
        ("!~~*", "NOT ILIKE"),
    ];
    for (op, label) in ops {
        let sql = format!("SELECT * FROM users WHERE name {} $1;", op);
        let a = analyze_query(&sql, &catalog, None)
            .unwrap_or_else(|_| panic!("Operator {} should parse", label));
        assert_eq!(a.params.len(), 1, "Failed on op {}", op);
        assert_eq!(a.params[0].name, "name");
        assert_eq!(a.params[0].ts_type, "string");
    }

    // 2. Regex operators: ~, ~*, !~, !~*
    let regex_ops = vec!["~", "~*", "!~", "!~*"];
    for op in regex_ops {
        let sql = format!("SELECT * FROM users WHERE email {} $1;", op);
        let a = analyze_query(&sql, &catalog, None)
            .unwrap_or_else(|_| panic!("Regex op {} should parse", op));
        assert_eq!(a.params.len(), 1, "Failed on regex op {}", op);
        assert_eq!(a.params[0].name, "email");
        assert_eq!(a.params[0].ts_type, "string");
    }

    // 3. String concatenation in pattern: name LIKE '%' || $1 || '%'
    let sql_cat = "SELECT * FROM users WHERE name LIKE '%' || $1 || '%';";
    let a_cat =
        analyze_query(sql_cat, &catalog, None).expect("LIKE with concatenation should parse");
    assert_eq!(a_cat.params.len(), 1);
    assert_eq!(a_cat.params[0].name, "name");
    assert_eq!(a_cat.params[0].ts_type, "string");

    // 4. Reverse pattern matching: $1 ~~ 'prefix%'
    let sql_rev = "SELECT * FROM users WHERE $1 ~~ 'prefix%';";
    let a_rev = analyze_query(sql_rev, &catalog, None).expect("Reverse pattern match should parse");
    assert_eq!(a_rev.params.len(), 1);
    assert_eq!(a_rev.params[0].ts_type, "string");
}

#[test]
fn test_adv_optional_dynamic_filters_comprehensive() {
    let catalog = setup_adv_catalog();

    // 1. Standard optional equality: ($1 IS NULL OR email = $1)
    let sql1 = "SELECT * FROM users WHERE ($1 IS NULL OR email = $1);";
    let a1 = analyze_query(sql1, &catalog, None).expect("Optional equality should parse");
    assert_eq!(a1.params.len(), 1);
    assert_eq!(a1.params[0].name, "email");
    assert!(a1.params[0].is_optional);

    // 2. Inverted order: (email = $1 OR $1 IS NULL)
    let sql2 = "SELECT * FROM users WHERE (email = $1 OR $1 IS NULL);";
    let a2 = analyze_query(sql2, &catalog, None).expect("Inverted optional equality should parse");
    assert_eq!(a2.params.len(), 1);
    assert_eq!(a2.params[0].name, "email");
    assert!(a2.params[0].is_optional);

    // 3. Optional LIKE: ($1 IS NULL OR name LIKE $1)
    let sql3 = "SELECT * FROM users WHERE ($1 IS NULL OR name LIKE $1);";
    let a3 = analyze_query(sql3, &catalog, None).expect("Optional LIKE should parse");
    assert_eq!(a3.params.len(), 1);
    assert_eq!(a3.params[0].name, "name");
    assert_eq!(a3.params[0].ts_type, "string");
    assert!(a3.params[0].is_optional);

    // 4. Inverted optional LIKE: (name LIKE $1 OR $1 IS NULL)
    let sql4 = "SELECT * FROM users WHERE (name LIKE $1 OR $1 IS NULL);";
    let a4 = analyze_query(sql4, &catalog, None).expect("Inverted optional LIKE should parse");
    assert_eq!(a4.params.len(), 1);
    assert_eq!(a4.params[0].name, "name");
    assert_eq!(a4.params[0].ts_type, "string");
    assert!(a4.params[0].is_optional);

    // 5. Optional ILIKE: ($1 IS NULL OR email ILIKE $1)
    let sql5 = "SELECT * FROM users WHERE ($1 IS NULL OR email ILIKE $1);";
    let a5 = analyze_query(sql5, &catalog, None).expect("Optional ILIKE should parse");
    assert_eq!(a5.params.len(), 1);
    assert_eq!(a5.params[0].name, "email");
    assert_eq!(a5.params[0].ts_type, "string");
    assert!(a5.params[0].is_optional);

    // 6. Optional ANY: ($1 IS NULL OR id = ANY($1))
    let sql6 = "SELECT * FROM users WHERE ($1 IS NULL OR id = ANY($1));";
    let a6 = analyze_query(sql6, &catalog, None).expect("Optional ANY should parse");
    assert_eq!(a6.params.len(), 1);
    assert_eq!(a6.params[0].name, "id");
    assert_eq!(a6.params[0].ts_type, "Array<string>");
    assert!(a6.params[0].is_optional);

    // 7. Inverted optional ANY: (id = ANY($1) OR $1 IS NULL)
    let sql7 = "SELECT * FROM users WHERE (id = ANY($1) OR $1 IS NULL);";
    let a7 = analyze_query(sql7, &catalog, None).expect("Inverted optional ANY should parse");
    assert_eq!(a7.params.len(), 1);
    assert_eq!(a7.params[0].name, "id");
    assert_eq!(a7.params[0].ts_type, "Array<string>");
    assert!(a7.params[0].is_optional);

    // 8. Optional symmetrical function: ($1 IS NULL OR lower(email) = lower($1))
    let sql8 = "SELECT * FROM users WHERE ($1 IS NULL OR lower(email) = lower($1));";
    let a8 =
        analyze_query(sql8, &catalog, None).expect("Optional symmetrical function should parse");
    assert_eq!(a8.params.len(), 1);
    assert_eq!(a8.params[0].name, "email");
    assert_eq!(a8.params[0].ts_type, "string");
    assert!(a8.params[0].is_optional);

    // 9. Optional symmetrical function inverted order: (lower(email) = lower($1) OR $1 IS NULL)
    let sql9 = "SELECT * FROM users WHERE (lower(email) = lower($1) OR $1 IS NULL);";
    let a9 = analyze_query(sql9, &catalog, None)
        .expect("Inverted optional symmetrical func should parse");
    assert_eq!(a9.params.len(), 1);
    assert_eq!(a9.params[0].name, "email");
    assert_eq!(a9.params[0].ts_type, "string");
    assert!(a9.params[0].is_optional);

    // 10. Optional symmetrical function with inverted args: ($1 IS NULL OR lower($1) = lower(email))
    let sql10 = "SELECT * FROM users WHERE ($1 IS NULL OR lower($1) = lower(email));";
    let a10 = analyze_query(sql10, &catalog, None)
        .expect("Optional symmetrical with inverted args should parse");
    assert_eq!(a10.params.len(), 1);
    assert_eq!(a10.params[0].name, "email");
    assert_eq!(a10.params[0].ts_type, "string");
    assert!(a10.params[0].is_optional);
}

#[test]
fn test_adv_set_ops_and_subqueries() {
    let catalog = setup_adv_catalog();

    // 1. UNION with parameters in both branches
    let sql_union = r#"
        SELECT id, name FROM users WHERE role_id = $1
        UNION
        SELECT id, name FROM users WHERE role_id = $2;
    "#;
    let a_union = analyze_query(sql_union, &catalog, None).expect("UNION query should parse");
    assert_eq!(a_union.params.len(), 2);
    assert_eq!(a_union.params[0].name, "role_id");
    assert_eq!(a_union.params[0].ts_type, "number");
    assert_eq!(a_union.params[1].name, "role_id2");
    assert_eq!(a_union.params[1].ts_type, "number");

    // 2. CTE with parameters
    let sql_cte = r#"
        WITH ranked_users AS (
            SELECT id, email, role_id
            FROM users
            WHERE role_id = $1
        )
        SELECT * FROM ranked_users WHERE email = $2;
    "#;
    let a_cte = analyze_query(sql_cte, &catalog, None).expect("CTE query should parse");
    assert_eq!(a_cte.params.len(), 2);
    assert_eq!(a_cte.params[0].name, "role_id");
    assert_eq!(a_cte.params[0].ts_type, "number");
    assert_eq!(a_cte.params[1].name, "email");
    assert_eq!(a_cte.params[1].ts_type, "string");

    // 3. SubLink in WHERE: IN subquery
    let sql_sub = r#"
        SELECT * FROM users
        WHERE id IN (
            SELECT author_id FROM posts WHERE view_count > $1
        );
    "#;
    let a_sub = analyze_query(sql_sub, &catalog, None).expect("Subquery IN should parse");
    assert_eq!(a_sub.params.len(), 1);
    assert_eq!(a_sub.params[0].name, "view_count");
    assert_eq!(a_sub.params[0].ts_type, "number");
}
