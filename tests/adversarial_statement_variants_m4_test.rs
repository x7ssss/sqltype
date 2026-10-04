//! Adversarial Test Suite 2 for Milestone 4: Prepared Statement Parameter Type Deduction
//!
//! Empirical challenges targeting:
//! 1. Deep CTEs with parameter bindings across CTE boundaries (multi-tier, aliasing, recursive, DML)
//! 2. DML statements: INSERT with ON CONFLICT DO UPDATE and RETURNING parameters
//! 3. UPDATE and DELETE with complex WHERE clauses and RETURNING
//! 4. Multiple identical column comparisons (collision-free finalize_params and TypeScript codegen)
//! 5. HAVING clause parameters and limit/offset parameters

use sqltype::analyzer::analyze_query;
use sqltype::catalog::Catalog;
use sqltype::codegen::generate_file_ts;

fn setup_adv2_catalog() -> Catalog {
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
            author_id UUID NOT NULL REFERENCES users(id),
            title VARCHAR(255) NOT NULL,
            tags TEXT[] NOT NULL DEFAULT '{}',
            view_count INT NOT NULL DEFAULT 0,
            published BOOLEAN NOT NULL DEFAULT false,
            created_at TIMESTAMPTZ NOT NULL
        );
    "#;
    catalog.apply_sql(ddl).expect("Failed to setup test catalog");
    catalog
}

// =====================================================================================
// Category 1: Deep CTEs with Parameter Bindings Across CTE Boundaries
// =====================================================================================

#[test]
fn test_adv_deep_cte_multi_tier_pipeline() {
    let catalog = setup_adv2_catalog();

    // 4-tier CTE pipeline chaining parameters and projections
    let sql = r#"
        WITH tier1 AS (
            SELECT id, email, role_id, score
            FROM users
            WHERE role_id = $1
        ),
        tier2 AS (
            SELECT id, email, score
            FROM tier1
            WHERE email = $2
        ),
        tier3 AS (
            SELECT t2.id, t2.email, p.view_count, p.title
            FROM tier2 t2
            INNER JOIN posts p ON t2.id = p.author_id
            WHERE p.view_count > $3
        ),
        tier4 AS (
            SELECT id, email, view_count, title, $4::int AS bonus_rank
            FROM tier3
            WHERE view_count < $5
        )
        SELECT id, email, bonus_rank
        FROM tier4
        WHERE email LIKE $6
        LIMIT $7
        OFFSET $8;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("Deep CTE pipeline should analyze");
    assert_eq!(analyzed.params.len(), 8);

    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "role_id");
    assert_eq!(analyzed.params[0].ts_type, "number");

    assert_eq!(analyzed.params[1].index, 2);
    assert_eq!(analyzed.params[1].name, "email");
    assert_eq!(analyzed.params[1].ts_type, "string");

    assert_eq!(analyzed.params[2].index, 3);
    assert_eq!(analyzed.params[2].name, "view_count");
    assert_eq!(analyzed.params[2].ts_type, "number");

    assert_eq!(analyzed.params[3].index, 4);
    assert_eq!(analyzed.params[3].ts_type, "number");

    assert_eq!(analyzed.params[4].index, 5);
    assert_eq!(analyzed.params[4].name, "view_count2");
    assert_eq!(analyzed.params[4].ts_type, "number");

    assert_eq!(analyzed.params[5].index, 6);
    assert_eq!(analyzed.params[5].name, "email2");
    assert_eq!(analyzed.params[5].ts_type, "string");

    assert_eq!(analyzed.params[6].index, 7);
    assert_eq!(analyzed.params[6].name, "limit");
    assert_eq!(analyzed.params[6].ts_type, "number");

    assert_eq!(analyzed.params[7].index, 8);
    assert_eq!(analyzed.params[7].name, "offset");
    assert_eq!(analyzed.params[7].ts_type, "number");
}

#[test]
fn test_adv_cte_column_alias_propagation_and_cross_boundary_params() {
    let catalog = setup_adv2_catalog();

    let sql = r#"
        WITH renamed_cols(user_id, contact, user_score) AS (
            SELECT id, email, score
            FROM users
            WHERE score >= $1
        ),
        filtered AS (
            SELECT user_id, contact
            FROM renamed_cols
            WHERE contact = $2 AND user_score < $3
        )
        SELECT *
        FROM filtered
        WHERE contact LIKE $4;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("CTE alias propagation should analyze");
    assert_eq!(analyzed.params.len(), 4);

    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "score");
    assert_eq!(analyzed.params[0].ts_type, "number");

    assert_eq!(analyzed.params[1].index, 2);
    assert_eq!(analyzed.params[1].name, "contact");
    assert_eq!(analyzed.params[1].ts_type, "string");

    assert_eq!(analyzed.params[2].index, 3);
    assert_eq!(analyzed.params[2].name, "user_score");
    assert_eq!(analyzed.params[2].ts_type, "number");

    assert_eq!(analyzed.params[3].index, 4);
    assert_eq!(analyzed.params[3].name, "contact2");
    assert_eq!(analyzed.params[3].ts_type, "string");
}

#[test]
fn test_adv_recursive_cte_with_params_in_anchor_and_recursive_term() {
    let catalog = setup_adv2_catalog();

    let sql = r#"
        WITH RECURSIVE tree AS (
            SELECT id, name, parent_id, role_id, 1 as depth
            FROM users
            WHERE parent_id = $1
            UNION ALL
            SELECT u.id, u.name, u.parent_id, u.role_id, t.depth + 1
            FROM users u
            INNER JOIN tree t ON u.parent_id = t.id
            WHERE u.role_id = $2 AND t.depth < $3
        )
        SELECT *
        FROM tree
        WHERE name = $4;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("Recursive CTE with params should analyze");
    assert_eq!(analyzed.params.len(), 4);

    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "parent_id");
    assert_eq!(analyzed.params[0].ts_type, "string");

    assert_eq!(analyzed.params[1].index, 2);
    assert_eq!(analyzed.params[1].name, "role_id");
    assert_eq!(analyzed.params[1].ts_type, "number");

    assert_eq!(analyzed.params[2].index, 3);
    assert_eq!(analyzed.params[2].ts_type, "number");

    assert_eq!(analyzed.params[3].index, 4);
    assert_eq!(analyzed.params[3].name, "name");
    assert_eq!(analyzed.params[3].ts_type, "string");
}

#[test]
fn test_adv_cte_inside_dml_statement() {
    let catalog = setup_adv2_catalog();

    let sql = r#"
        WITH target_orgs AS (
            SELECT id FROM organizations WHERE domain = $1
        )
        INSERT INTO users (id, name, email, role_id, org_id)
        VALUES ($2, $3, $4, $5, (SELECT id FROM target_orgs LIMIT 1));
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("CTE inside INSERT should analyze");
    assert_eq!(analyzed.params.len(), 5);

    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "domain");
    assert_eq!(analyzed.params[0].ts_type, "string");

    assert_eq!(analyzed.params[1].index, 2);
    assert_eq!(analyzed.params[1].name, "id");
    assert_eq!(analyzed.params[1].ts_type, "string");

    assert_eq!(analyzed.params[2].index, 3);
    assert_eq!(analyzed.params[2].name, "name");
    assert_eq!(analyzed.params[2].ts_type, "string");

    assert_eq!(analyzed.params[3].index, 4);
    assert_eq!(analyzed.params[3].name, "email");
    assert_eq!(analyzed.params[3].ts_type, "string");

    assert_eq!(analyzed.params[4].index, 5);
    assert_eq!(analyzed.params[4].name, "role_id");
    assert_eq!(analyzed.params[4].ts_type, "number");
}

// =====================================================================================
// Category 2: DML Statements: INSERT with ON CONFLICT and RETURNING Parameters
// =====================================================================================

#[test]
fn test_adv_insert_on_conflict_do_update_and_returning_params() {
    let catalog = setup_adv2_catalog();

    let sql = r#"
        INSERT INTO users (id, name, email, role_id, score)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (id) DO UPDATE
        SET name = $6,
            score = EXCLUDED.score + $7,
            bio = $8
        WHERE users.active = $9
        RETURNING id, name, ($10::text) AS custom_label, (score > $11) AS is_high_score;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("INSERT ON CONFLICT with RETURNING should analyze");
    assert_eq!(analyzed.params.len(), 11);

    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "id");
    assert_eq!(analyzed.params[0].ts_type, "string");

    assert_eq!(analyzed.params[1].index, 2);
    assert_eq!(analyzed.params[1].name, "name");
    assert_eq!(analyzed.params[1].ts_type, "string");

    assert_eq!(analyzed.params[2].index, 3);
    assert_eq!(analyzed.params[2].name, "email");
    assert_eq!(analyzed.params[2].ts_type, "string");

    assert_eq!(analyzed.params[3].index, 4);
    assert_eq!(analyzed.params[3].name, "role_id");
    assert_eq!(analyzed.params[3].ts_type, "number");

    // $5 from VALUES -> score
    assert_eq!(analyzed.params[4].index, 5);
    assert_eq!(analyzed.params[4].name, "score");
    assert_eq!(analyzed.params[4].ts_type, "number");

    // $6 from DO UPDATE SET name = $6 -> name2
    assert_eq!(analyzed.params[5].index, 6);
    assert_eq!(analyzed.params[5].name, "name2");
    assert_eq!(analyzed.params[5].ts_type, "string");

    // $7 from DO UPDATE SET score = EXCLUDED.score + $7 -> score2
    assert_eq!(analyzed.params[6].index, 7);
    assert_eq!(analyzed.params[6].name, "score2");
    assert_eq!(analyzed.params[6].ts_type, "number");

    // $8 from DO UPDATE SET bio = $8 -> bio (optional because bio column is nullable)
    assert_eq!(analyzed.params[7].index, 8);
    assert_eq!(analyzed.params[7].name, "bio");
    assert_eq!(analyzed.params[7].ts_type, "string");
    assert!(analyzed.params[7].is_optional);

    // $9 from WHERE users.active = $9 -> active
    assert_eq!(analyzed.params[8].index, 9);
    assert_eq!(analyzed.params[8].name, "active");
    assert_eq!(analyzed.params[8].ts_type, "boolean");

    // $10 from RETURNING ($10::text) AS custom_label
    assert_eq!(analyzed.params[9].index, 10);
    assert_eq!(analyzed.params[9].ts_type, "string");

    // $11 from RETURNING (score > $11) -> score3
    assert_eq!(analyzed.params[10].index, 11);
    assert_eq!(analyzed.params[10].name, "score3");
    assert_eq!(analyzed.params[10].ts_type, "number");
}

#[test]
fn test_adv_insert_on_conflict_partial_index_and_do_update() {
    let catalog = setup_adv2_catalog();

    let sql = r#"
        INSERT INTO users (id, name, email, role_id)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (email) WHERE active = $5
        DO UPDATE SET role_id = $6
        RETURNING id, email;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("Partial index ON CONFLICT should analyze");
    assert_eq!(analyzed.params.len(), 6);

    assert_eq!(analyzed.params[0].name, "id");
    assert_eq!(analyzed.params[1].name, "name");
    assert_eq!(analyzed.params[2].name, "email");
    assert_eq!(analyzed.params[3].name, "role_id");
    assert_eq!(analyzed.params[4].name, "active");
    assert_eq!(analyzed.params[4].ts_type, "boolean");
    assert_eq!(analyzed.params[5].name, "role_id2");
    assert_eq!(analyzed.params[5].ts_type, "number");
}

#[test]
fn test_adv_multi_row_insert_with_on_conflict() {
    let catalog = setup_adv2_catalog();

    let sql = r#"
        INSERT INTO posts (id, author_id, title, tags, view_count, created_at)
        VALUES ($1, $2, $3, $4, $5, $6),
               ($7, $8, $9, $10, $11, $12)
        ON CONFLICT (id) DO UPDATE
        SET title = EXCLUDED.title,
            view_count = posts.view_count + $13
        RETURNING id, title, view_count, ($14::boolean) AS was_updated;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("Multi-row INSERT ON CONFLICT should analyze");
    assert_eq!(analyzed.params.len(), 14);

    assert_eq!(analyzed.params[0].name, "id");
    assert_eq!(analyzed.params[1].name, "author_id");
    assert_eq!(analyzed.params[2].name, "title");
    assert_eq!(analyzed.params[3].name, "tags");
    assert_eq!(analyzed.params[3].ts_type, "string[]");
    assert_eq!(analyzed.params[4].name, "view_count");
    assert_eq!(analyzed.params[4].ts_type, "number");
    assert_eq!(analyzed.params[5].name, "created_at");
    assert_eq!(analyzed.params[5].ts_type, "Date");

    assert_eq!(analyzed.params[6].name, "id2");
    assert_eq!(analyzed.params[7].name, "author_id2");
    assert_eq!(analyzed.params[8].name, "title2");
    assert_eq!(analyzed.params[9].name, "tags2");
    assert_eq!(analyzed.params[9].ts_type, "string[]");
    assert_eq!(analyzed.params[10].name, "view_count2");
    assert_eq!(analyzed.params[11].name, "created_at2");

    assert_eq!(analyzed.params[12].ts_type, "number");
    assert_eq!(analyzed.params[13].ts_type, "boolean");
}

// =====================================================================================
// Category 3: UPDATE and DELETE with Complex WHERE Clauses and RETURNING
// =====================================================================================

#[test]
fn test_adv_update_from_clause_and_complex_where_and_returning() {
    let catalog = setup_adv2_catalog();

    let sql = r#"
        UPDATE users u
        SET name = $1, age = $2
        FROM organizations o
        WHERE u.org_id = o.id
          AND o.name = $3
          AND (u.status = $4 OR u.score >= $5)
        RETURNING u.id, u.name, o.name AS org_name, ($6::timestamptz) AS sync_time;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("UPDATE with FROM and WHERE should analyze");
    assert_eq!(analyzed.params.len(), 6);

    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "name");
    assert_eq!(analyzed.params[0].ts_type, "string");

    assert_eq!(analyzed.params[1].index, 2);
    assert_eq!(analyzed.params[1].name, "age");
    assert_eq!(analyzed.params[1].ts_type, "number");
    assert!(analyzed.params[1].is_optional);

    assert_eq!(analyzed.params[2].index, 3);
    assert_eq!(analyzed.params[2].name, "name2");
    assert_eq!(analyzed.params[2].ts_type, "string");

    assert_eq!(analyzed.params[3].index, 4);
    assert_eq!(analyzed.params[3].name, "status");
    assert!(analyzed.params[3].ts_type.contains("\"active\""));

    assert_eq!(analyzed.params[4].index, 5);
    assert_eq!(analyzed.params[4].name, "score");
    assert_eq!(analyzed.params[4].ts_type, "number");

    assert_eq!(analyzed.params[5].index, 6);
    assert_eq!(analyzed.params[5].ts_type, "Date");
}

#[test]
fn test_adv_delete_using_clause_and_complex_where_and_returning() {
    let catalog = setup_adv2_catalog();

    let sql = r#"
        DELETE FROM posts p
        USING users u, organizations o
        WHERE p.author_id = u.id
          AND u.org_id = o.id
          AND o.domain = $1
          AND (p.published = $2 AND p.view_count < $3)
          AND (u.created_at < $4::timestamptz)
        RETURNING p.id, p.title, ($5::text) AS deletion_reason;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("DELETE with USING and WHERE should analyze");
    assert_eq!(analyzed.params.len(), 5);

    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "domain");
    assert_eq!(analyzed.params[0].ts_type, "string");

    assert_eq!(analyzed.params[1].index, 2);
    assert_eq!(analyzed.params[1].name, "published");
    assert_eq!(analyzed.params[1].ts_type, "boolean");

    assert_eq!(analyzed.params[2].index, 3);
    assert_eq!(analyzed.params[2].name, "view_count");
    assert_eq!(analyzed.params[2].ts_type, "number");

    assert_eq!(analyzed.params[3].index, 4);
    assert_eq!(analyzed.params[3].name, "created_at");
    assert_eq!(analyzed.params[3].ts_type, "Date");

    assert_eq!(analyzed.params[4].index, 5);
    assert_eq!(analyzed.params[4].ts_type, "string");
}

#[test]
fn test_adv_dml_with_dynamic_optional_filter_and_array_ops() {
    let catalog = setup_adv2_catalog();

    // 1. UPDATE with optional filter in WHERE: ($3 IS NULL OR status = $3)
    let sql_up = r#"
        UPDATE users
        SET active = $1
        WHERE id = $2 AND ($3 IS NULL OR status = $3);
    "#;
    let a_up = analyze_query(sql_up, &catalog, None).expect("UPDATE with optional filter should analyze");
    assert_eq!(a_up.params.len(), 3);
    assert_eq!(a_up.params[0].name, "active");
    assert_eq!(a_up.params[1].name, "id");
    assert_eq!(a_up.params[2].name, "status");
    assert!(a_up.params[2].is_optional);

    // 2. DELETE with optional array filter in WHERE: ($2 IS NULL OR id = ANY($2))
    let sql_del = r#"
        DELETE FROM posts
        WHERE author_id = $1 AND ($2 IS NULL OR id = ANY($2));
    "#;
    let a_del = analyze_query(sql_del, &catalog, None).expect("DELETE with optional array filter should analyze");
    assert_eq!(a_del.params.len(), 2);
    assert_eq!(a_del.params[0].name, "author_id");
    assert_eq!(a_del.params[1].name, "id");
    assert_eq!(a_del.params[1].ts_type, "Array<string>");
    assert!(a_del.params[1].is_optional);

    // 3. UPDATE SET tags with array overlap in WHERE
    let sql_arr = r#"
        UPDATE posts
        SET tags = $1
        WHERE tags && $2
        RETURNING id, tags;
    "#;
    let a_arr = analyze_query(sql_arr, &catalog, None).expect("UPDATE array overlap should analyze");
    assert_eq!(a_arr.params.len(), 2);
    assert_eq!(a_arr.params[0].name, "tags");
    assert_eq!(a_arr.params[0].ts_type, "string[]"); // from column metadata posts.tags
    assert_eq!(a_arr.params[1].name, "tags2");
    assert_eq!(a_arr.params[1].ts_type, "Array<string>"); // from operator &&
}

// =====================================================================================
// Category 4: Multiple Identical Column Comparisons & Collision-Free Disambiguation
// =====================================================================================

#[test]
fn test_adv_identical_column_comparisons_and_typescript_codegen_safety() {
    let catalog = setup_adv2_catalog();

    // Query with 9 parameter comparisons against id, id2, id3
    let sql = r#"
        -- name: StressCollisions
        SELECT * FROM metrics
        WHERE id = $1
          AND id2 = $2
          AND id = $3
          AND id = $4
          AND id2 = $5
          AND id3 = $6
          AND id = $7
          AND id3 = $8
          AND id = $9;
    "#;

    let analyzed = analyze_query(sql, &catalog, Some("stress_collisions.sql")).expect("Collision stress query should analyze");
    assert_eq!(analyzed.params.len(), 9);

    let names: Vec<String> = analyzed.params.iter().map(|p| p.name.clone()).collect();
    let unique_names: std::collections::HashSet<String> = names.iter().cloned().collect();
    assert_eq!(names.len(), unique_names.len(), "Every parameter name MUST be distinct! Got: {:?}", names);

    assert_eq!(analyzed.params[0].name, "id");
    assert_eq!(analyzed.params[1].name, "id2");
    assert_eq!(analyzed.params[2].name, "id3");
    assert_eq!(analyzed.params[3].name, "id4");
    assert_eq!(analyzed.params[4].name, "id22");
    assert_eq!(analyzed.params[5].name, "id32");
    assert_eq!(analyzed.params[6].name, "id5");
    assert_eq!(analyzed.params[7].name, "id33");
    assert_eq!(analyzed.params[8].name, "id6");

    // Verify TypeScript codegen contains all unique properties
    let ts_code = generate_file_ts(&[analyzed]);
    assert!(ts_code.contains("export interface StressCollisionsParams {"));
    for name in &names {
        assert!(ts_code.contains(&format!("{}: ", name)), "TypeScript interface missing property {}", name);
    }
}

#[test]
fn test_adv_inverted_identical_column_order_stress() {
    let catalog = setup_adv2_catalog();

    let sql = r#"
        SELECT * FROM users
        WHERE id = $9
          AND id = $1
          AND id = $7
          AND id = $3
          AND id = $5
          AND id = $2
          AND id = $8
          AND id = $4
          AND id = $6;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("Inverted order collision query should analyze");
    assert_eq!(analyzed.params.len(), 9);

    for (i, param) in analyzed.params.iter().enumerate() {
        assert_eq!(param.index, i + 1, "Parameters must be sorted by numerical index 1..=9");
        let expected_name = if i == 0 { "id".to_string() } else { format!("id{}", i + 1) };
        assert_eq!(param.name, expected_name);
        assert_eq!(param.ts_type, "string");
    }
}

// =====================================================================================
// Category 5: HAVING Clause and LIMIT / OFFSET Parameters
// =====================================================================================

#[test]
fn test_adv_having_clause_with_aggregates_and_limit_offset() {
    let catalog = setup_adv2_catalog();

    let sql = r#"
        SELECT org_id, count(*) as cnt, avg(age) as avg_age, sum(score) as total_score
        FROM users
        GROUP BY org_id
        HAVING count(*) >= $1
           AND avg(age) < $2
           AND sum(score) > $3
           AND max(created_at) <= $4::timestamptz
        ORDER BY total_score DESC
        LIMIT $5
        OFFSET $6;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("HAVING with aggregates should analyze");
    assert_eq!(analyzed.params.len(), 6);

    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].ts_type, "number");

    assert_eq!(analyzed.params[1].index, 2);
    assert_eq!(analyzed.params[1].ts_type, "number");

    assert_eq!(analyzed.params[2].index, 3);
    assert_eq!(analyzed.params[2].ts_type, "number");

    assert_eq!(analyzed.params[3].index, 4);
    assert_eq!(analyzed.params[3].ts_type, "Date");

    assert_eq!(analyzed.params[4].index, 5);
    assert_eq!(analyzed.params[4].name, "limit");
    assert_eq!(analyzed.params[4].ts_type, "number");

    assert_eq!(analyzed.params[5].index, 6);
    assert_eq!(analyzed.params[5].name, "offset");
    assert_eq!(analyzed.params[5].ts_type, "number");
}

#[test]
fn test_adv_having_clause_with_optional_filter_and_between() {
    let catalog = setup_adv2_catalog();

    // 1. Optional filter on grouped column in HAVING: ($1 IS NULL OR org_id = $1)
    let sql_opt = r#"
        SELECT org_id, count(*)
        FROM users
        GROUP BY org_id
        HAVING ($1 IS NULL OR org_id = $1)
        LIMIT $2;
    "#;
    let a_opt = analyze_query(sql_opt, &catalog, None).expect("Optional filter on grouped column in HAVING should analyze");
    assert_eq!(a_opt.params.len(), 2);
    assert_eq!(a_opt.params[0].index, 1);
    assert_eq!(a_opt.params[0].name, "org_id");
    assert_eq!(a_opt.params[0].ts_type, "string");
    assert!(a_opt.params[0].is_optional);
    assert_eq!(a_opt.params[1].index, 2);
    assert_eq!(a_opt.params[1].name, "limit");
    assert_eq!(a_opt.params[1].ts_type, "number");

    // 2. BETWEEN on aggregate in HAVING
    let sql_between = r#"
        SELECT org_id, count(*)
        FROM users
        GROUP BY org_id
        HAVING count(*) BETWEEN $1 AND $2;
    "#;
    let a_between = analyze_query(sql_between, &catalog, None).expect("HAVING BETWEEN should analyze");
    assert_eq!(a_between.params.len(), 2);
    assert_eq!(a_between.params[0].ts_type, "number");
    assert_eq!(a_between.params[1].ts_type, "number");

    // 3. Boolean comparison expression in HAVING: (count(*) > 10) = $1
    let sql_bool = r#"
        SELECT org_id
        FROM users
        GROUP BY org_id
        HAVING (count(*) > 10) = $1;
    "#;
    let a_bool = analyze_query(sql_bool, &catalog, None).expect("HAVING boolean comparison should analyze");
    assert_eq!(a_bool.params.len(), 1);
    assert_eq!(a_bool.params[0].ts_type, "boolean");

    // 4. Custom/unregistered function with explicit cast in HAVING: bool_and(active) = $1::boolean
    let sql_cast = r#"
        SELECT org_id
        FROM users
        GROUP BY org_id
        HAVING bool_and(active) = $1::boolean;
    "#;
    let a_cast = analyze_query(sql_cast, &catalog, None).expect("HAVING cast should analyze");
    assert_eq!(a_cast.params.len(), 1);
    assert_eq!(a_cast.params[0].ts_type, "boolean");

    // 5. Unregistered function without cast defaults to unknown
    let sql_unregistered = r#"
        SELECT org_id
        FROM users
        GROUP BY org_id
        HAVING bool_and(active) = $1;
    "#;
    let a_unreg = analyze_query(sql_unregistered, &catalog, None).expect("HAVING unregistered func should analyze");
    assert_eq!(a_unreg.params.len(), 1);
    assert_eq!(a_unreg.params[0].ts_type, "unknown");
}

#[test]
fn test_adv_set_operations_with_branch_and_global_limit_offset() {
    let catalog = setup_adv2_catalog();

    let sql = r#"
        (SELECT id, name FROM users WHERE role_id = $1 LIMIT $2 OFFSET $3)
        UNION ALL
        (SELECT id, name FROM users WHERE role_id = $4 LIMIT $5 OFFSET $6)
        ORDER BY name
        LIMIT $7
        OFFSET $8;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("UNION with branch and global limits should analyze");
    assert_eq!(analyzed.params.len(), 8);

    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "role_id");
    assert_eq!(analyzed.params[0].ts_type, "number");

    assert_eq!(analyzed.params[1].index, 2);
    assert_eq!(analyzed.params[1].name, "limit");
    assert_eq!(analyzed.params[1].ts_type, "number");

    assert_eq!(analyzed.params[2].index, 3);
    assert_eq!(analyzed.params[2].name, "offset");
    assert_eq!(analyzed.params[2].ts_type, "number");

    assert_eq!(analyzed.params[3].index, 4);
    assert_eq!(analyzed.params[3].name, "role_id2");
    assert_eq!(analyzed.params[3].ts_type, "number");

    assert_eq!(analyzed.params[4].index, 5);
    assert_eq!(analyzed.params[4].name, "limit2");
    assert_eq!(analyzed.params[4].ts_type, "number");

    assert_eq!(analyzed.params[5].index, 6);
    assert_eq!(analyzed.params[5].name, "offset2");
    assert_eq!(analyzed.params[5].ts_type, "number");

    assert_eq!(analyzed.params[6].index, 7);
    assert_eq!(analyzed.params[6].name, "limit3");
    assert_eq!(analyzed.params[6].ts_type, "number");

    assert_eq!(analyzed.params[7].index, 8);
    assert_eq!(analyzed.params[7].name, "offset3");
    assert_eq!(analyzed.params[7].ts_type, "number");
}
