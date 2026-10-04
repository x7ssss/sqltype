//! Adversarial Challenger 2 Stress Suite for Milestone 4: Prepared Statement Parameter Type Deduction
//!
//! Thoroughly challenges:
//! 1. Statement families:
//!    - INSERT INTO ... ON CONFLICT (...) DO UPDATE SET ... RETURNING *
//!    - UPDATE ... SET ... WHERE ... RETURNING * (including arithmetic & multi-table FROM)
//!    - DELETE ... WHERE ... RETURNING * (including USING and subqueries)
//!    - Subqueries and CTEs (chained CTEs, scalar subqueries in projections, SubLink in WHERE)
//!    - HAVING clauses and LIMIT / OFFSET parameterization
//! 2. Parameter numbering & identifier disambiguation:
//!    - Out of order parameters ($2 before $1)
//!    - Duplicate column references (id = $1 OR id = $2 OR id = $3)
//!    - Interleaved base identifier collisions
//!    - Multi-table join column collisions (t1.id, t2.id, t3.id)
//!    - Special/quoted identifier names in SQL & codegen
//!    - Reused parameter numbers ($1 referenced multiple times)

use sqltype::analyzer::analyze_query;
use sqltype::catalog::Catalog;
use sqltype::codegen::{generate_file_ts_with_options, CodegenOptions};
use sqltype::DriverTarget;

fn setup_adv_catalog() -> Catalog {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TYPE account_tier AS ENUM ('free', 'bronze', 'silver', 'gold', 'platinum');

        CREATE TABLE accounts (
            id UUID PRIMARY KEY,
            account_no INT NOT NULL,
            name TEXT NOT NULL,
            tier account_tier NOT NULL DEFAULT 'free',
            balance NUMERIC(14, 2) NOT NULL DEFAULT 0.00,
            is_active BOOLEAN NOT NULL DEFAULT true,
            created_at TIMESTAMPTZ NOT NULL
        );

        CREATE TABLE members (
            id UUID PRIMARY KEY,
            account_id UUID REFERENCES accounts(id),
            role_id INT NOT NULL,
            email TEXT NOT NULL,
            score FLOAT8,
            notes TEXT,
            skills TEXT[] NOT NULL DEFAULT '{}',
            joined_at TIMESTAMPTZ NOT NULL
        );

        CREATE TABLE audit_logs (
            id UUID PRIMARY KEY,
            account_id UUID REFERENCES accounts(id),
            action TEXT NOT NULL,
            payload JSONB,
            logged_at TIMESTAMPTZ NOT NULL
        );

        CREATE TABLE collision_table (
            id UUID PRIMARY KEY,
            id2 TEXT NOT NULL,
            id3 TEXT NOT NULL,
            "user-id" TEXT NOT NULL,
            "default" INT NOT NULL
        );
    "#;
    catalog.apply_sql(ddl).expect("Failed to setup stress catalog");
    catalog
}

#[test]
fn test_insert_on_conflict_do_update_full_lifecycle() {
    let catalog = setup_adv_catalog();

    // 1. Full ON CONFLICT DO UPDATE SET with EXCLUDED, WHERE and RETURNING *
    let sql = r#"
        INSERT INTO accounts (id, account_no, name, tier, balance, is_active, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (id) DO UPDATE
        SET name = $8,
            balance = EXCLUDED.balance + $9
        WHERE accounts.balance >= $10
        RETURNING *;
    "#;

    let analyzed = analyze_query(sql, &catalog, Some("UpsertAccount")).expect("Insert query should analyze");

    assert_eq!(analyzed.params.len(), 10, "Expected exactly 10 parameters");

    // $1 -> id: UUID
    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "id");
    assert_eq!(analyzed.params[0].ts_type, "string");

    // $2 -> account_no: INT
    assert_eq!(analyzed.params[1].index, 2);
    assert_eq!(analyzed.params[1].name, "account_no");
    assert_eq!(analyzed.params[1].ts_type, "number");

    // $3 -> name: TEXT
    assert_eq!(analyzed.params[2].index, 3);
    assert_eq!(analyzed.params[2].name, "name");
    assert_eq!(analyzed.params[2].ts_type, "string");

    // $4 -> tier: account_tier ENUM
    assert_eq!(analyzed.params[3].index, 4);
    assert_eq!(analyzed.params[3].name, "tier");
    assert!(analyzed.params[3].ts_type.contains("\"free\""));
    assert!(analyzed.params[3].ts_type.contains("\"platinum\""));

    // $5 -> balance: NUMERIC
    assert_eq!(analyzed.params[4].index, 5);
    assert_eq!(analyzed.params[4].name, "balance");
    assert_eq!(analyzed.params[4].ts_type, "number");

    // $6 -> is_active: BOOLEAN
    assert_eq!(analyzed.params[5].index, 6);
    assert_eq!(analyzed.params[5].name, "is_active");
    assert_eq!(analyzed.params[5].ts_type, "boolean");

    // $7 -> created_at: TIMESTAMPTZ
    assert_eq!(analyzed.params[6].index, 7);
    assert_eq!(analyzed.params[6].name, "created_at");
    assert_eq!(analyzed.params[6].ts_type, "Date");

    // $8 -> name in DO UPDATE SET: disambiguated name
    assert_eq!(analyzed.params[7].index, 8);
    assert_eq!(analyzed.params[7].name, "name2");
    assert_eq!(analyzed.params[7].ts_type, "string");

    // $9 -> balance in DO UPDATE SET: disambiguated balance
    assert_eq!(analyzed.params[8].index, 9);
    assert_eq!(analyzed.params[8].name, "balance2");
    assert_eq!(analyzed.params[8].ts_type, "number");

    // $10 -> balance in conflict WHERE accounts.balance >= $10
    assert_eq!(analyzed.params[9].index, 10);
    assert_eq!(analyzed.params[9].name, "balance3");
    assert_eq!(analyzed.params[9].ts_type, "number");

    // Verify RETURNING * fields
    assert_eq!(analyzed.fields.len(), 7);

    // Verify TS codegen compiles cleanly
    let ts_code = generate_file_ts_with_options(&[analyzed], &CodegenOptions::new(DriverTarget::Pg, true));
    assert!(ts_code.contains("export interface UpsertAccountParams {"));
    assert!(ts_code.contains("  name2: string;"));
    assert!(ts_code.contains("  balance2: number;"));
    assert!(ts_code.contains("  balance3: number;"));
    assert!(ts_code.contains("export interface UpsertAccountRow {"));
}

#[test]
fn test_insert_on_conflict_do_nothing_and_multi_row() {
    let catalog = setup_adv_catalog();

    // Multi-row INSERT with ON CONFLICT DO NOTHING
    let sql = r#"
        INSERT INTO accounts (id, account_no, name, created_at)
        VALUES ($1, $2, $3, $4),
               ($5, $6, $7, $8)
        ON CONFLICT (id) DO NOTHING
        RETURNING id, name;
    "#;

    let analyzed = analyze_query(sql, &catalog, Some("BatchInsert")).expect("Batch insert should analyze");
    assert_eq!(analyzed.params.len(), 8);

    assert_eq!(analyzed.params[0].name, "id");
    assert_eq!(analyzed.params[1].name, "account_no");
    assert_eq!(analyzed.params[2].name, "name");
    assert_eq!(analyzed.params[3].name, "created_at");

    assert_eq!(analyzed.params[4].name, "id2");
    assert_eq!(analyzed.params[5].name, "account_no2");
    assert_eq!(analyzed.params[6].name, "name2");
    assert_eq!(analyzed.params[7].name, "created_at2");

    assert_eq!(analyzed.fields.len(), 2);
    assert_eq!(analyzed.fields[0].name, "id");
    assert_eq!(analyzed.fields[1].name, "name");
}

#[test]
fn test_update_statements_with_arithmetic_joins_and_returning() {
    let catalog = setup_adv_catalog();

    // 1. UPDATE with arithmetic SET and WHERE
    let sql1 = r#"
        UPDATE accounts
        SET name = $1,
            balance = balance + $2,
            is_active = $3
        WHERE id = $4 AND is_active = $5
        RETURNING id, balance, is_active;
    "#;
    let a1 = analyze_query(sql1, &catalog, Some("UpdateAccount")).expect("Update query should analyze");
    assert_eq!(a1.params.len(), 5);
    assert_eq!(a1.params[0].name, "name");
    assert_eq!(a1.params[0].ts_type, "string");
    assert_eq!(a1.params[1].name, "balance");
    assert_eq!(a1.params[1].ts_type, "number");
    assert_eq!(a1.params[2].name, "is_active");
    assert_eq!(a1.params[2].ts_type, "boolean");
    assert_eq!(a1.params[3].name, "id");
    assert_eq!(a1.params[3].ts_type, "string");
    assert_eq!(a1.params[4].name, "is_active2");
    assert_eq!(a1.params[4].ts_type, "boolean");

    // 2. UPDATE with FROM clause joining another table
    let sql2 = r#"
        UPDATE members m
        SET notes = $1, score = score + $2
        FROM accounts a
        WHERE m.account_id = a.id AND a.tier = $3 AND m.role_id = $4
        RETURNING m.id, m.score, a.name;
    "#;
    let a2 = analyze_query(sql2, &catalog, Some("UpdateMemberWithFrom")).expect("Update from should analyze");
    assert_eq!(a2.params.len(), 4);
    assert_eq!(a2.params[0].name, "notes");
    assert_eq!(a2.params[0].ts_type, "string");
    assert_eq!(a2.params[1].name, "score");
    assert_eq!(a2.params[1].ts_type, "number");
    assert_eq!(a2.params[2].name, "tier");
    assert!(a2.params[2].ts_type.contains("\"free\""));
    assert_eq!(a2.params[3].name, "role_id");
    assert_eq!(a2.params[3].ts_type, "number");
    assert_eq!(a2.fields.len(), 3);
}

#[test]
fn test_delete_statements_with_using_and_returning() {
    let catalog = setup_adv_catalog();

    // 1. DELETE with multiple WHERE predicates and RETURNING *
    let sql1 = r#"
        DELETE FROM accounts
        WHERE balance <= $1 AND is_active = $2
        RETURNING *;
    "#;
    let a1 = analyze_query(sql1, &catalog, Some("DeleteInactiveAccounts")).expect("Delete should analyze");
    assert_eq!(a1.params.len(), 2);
    assert_eq!(a1.params[0].name, "balance");
    assert_eq!(a1.params[0].ts_type, "number");
    assert_eq!(a1.params[1].name, "is_active");
    assert_eq!(a1.params[1].ts_type, "boolean");
    assert_eq!(a1.fields.len(), 7);

    // 2. DELETE with USING clause
    let sql2 = r#"
        DELETE FROM audit_logs l
        USING accounts a
        WHERE l.account_id = a.id AND a.account_no = $1 AND l.action = $2
        RETURNING l.id, l.logged_at;
    "#;
    let a2 = analyze_query(sql2, &catalog, Some("DeleteAuditLogs")).expect("Delete with using should analyze");
    assert_eq!(a2.params.len(), 2);
    assert_eq!(a2.params[0].name, "account_no");
    assert_eq!(a2.params[0].ts_type, "number");
    assert_eq!(a2.params[1].name, "action");
    assert_eq!(a2.params[1].ts_type, "string");
    assert_eq!(a2.fields.len(), 2);
}

#[test]
fn test_ctes_and_subqueries_comprehensive() {
    let catalog = setup_adv_catalog();

    // 1. Chained CTEs with parameters in both CTEs and outer query
    let sql1 = r#"
        WITH active_accounts AS (
            SELECT id, name, tier, balance
            FROM accounts
            WHERE is_active = $1 AND balance > $2
        ),
        top_members AS (
            SELECT account_id, sum(score) as total_score
            FROM members
            WHERE joined_at >= $3
            GROUP BY account_id
            HAVING sum(score) >= $4
        )
        SELECT aa.name, aa.tier, tm.total_score
        FROM active_accounts aa
        JOIN top_members tm ON tm.account_id = aa.id
        WHERE aa.tier = $5 AND tm.total_score > $6;
    "#;

    let a1 = analyze_query(sql1, &catalog, Some("ChainedCteQuery")).expect("Chained CTE should analyze");
    assert_eq!(a1.params.len(), 6);
    assert_eq!(a1.params[0].name, "is_active");
    assert_eq!(a1.params[0].ts_type, "boolean");
    assert_eq!(a1.params[1].name, "balance");
    assert_eq!(a1.params[1].ts_type, "number");
    assert_eq!(a1.params[2].name, "joined_at");
    assert_eq!(a1.params[2].ts_type, "Date");
    assert_eq!(a1.params[3].name, "param4");
    assert_eq!(a1.params[3].ts_type, "number");
    assert_eq!(a1.params[4].name, "tier");
    assert!(a1.params[4].ts_type.contains("\"free\""));
    assert_eq!(a1.params[5].name, "total_score");
    assert_eq!(a1.params[5].ts_type, "number");

    // 2. Subquery in WHERE with IN and EXISTS
    let sql2 = r#"
        SELECT a.name, a.balance
        FROM accounts a
        WHERE a.id IN (
            SELECT m.account_id FROM members m WHERE m.score > $1
        ) AND EXISTS (
            SELECT 1 FROM audit_logs l WHERE l.account_id = a.id AND l.action = $2
        ) AND a.tier = $3;
    "#;
    let a2 = analyze_query(sql2, &catalog, Some("SubqueriesInWhere")).expect("Subqueries in where should analyze");
    assert_eq!(a2.params.len(), 3);
    assert_eq!(a2.params[0].name, "score");
    assert_eq!(a2.params[0].ts_type, "number");
    assert_eq!(a2.params[1].name, "action");
    assert_eq!(a2.params[1].ts_type, "string");
    assert_eq!(a2.params[2].name, "tier");
    assert!(a2.params[2].ts_type.contains("\"free\""));

    // 3. Scalar subquery in target_list
    let sql3 = r#"
        SELECT a.name, (
            SELECT count(*) FROM members m WHERE m.account_id = a.id AND m.score >= $1
        ) AS high_score_count
        FROM accounts a
        WHERE a.balance >= $2;
    "#;
    let a3 = analyze_query(sql3, &catalog, Some("ScalarSubqueryProjection")).expect("Scalar subquery should analyze");
    assert_eq!(a3.params.len(), 2);
    assert_eq!(a3.params[0].name, "score");
    assert_eq!(a3.params[0].ts_type, "number");
    assert_eq!(a3.params[1].name, "balance");
    assert_eq!(a3.params[1].ts_type, "number");
}

#[test]
fn test_having_and_limit_offset_parameterization() {
    let catalog = setup_adv_catalog();

    // Combined HAVING clause and LIMIT / OFFSET
    let sql = r#"
        SELECT role_id, count(*) as member_count, avg(score) as avg_score
        FROM members
        WHERE joined_at >= $1
        GROUP BY role_id
        HAVING role_id = $2 AND count(*) >= $3 AND avg(score) > $4
        ORDER BY member_count DESC
        LIMIT $5 OFFSET $6;
    "#;

    let analyzed = analyze_query(sql, &catalog, Some("AggregatedMembers")).expect("Having & limit should analyze");
    assert_eq!(analyzed.params.len(), 6);

    // $1 in WHERE joined_at >= $1
    assert_eq!(analyzed.params[0].index, 1);
    assert_eq!(analyzed.params[0].name, "joined_at");
    assert_eq!(analyzed.params[0].ts_type, "Date");

    // $2 in HAVING role_id = $2
    assert_eq!(analyzed.params[1].index, 2);
    assert_eq!(analyzed.params[1].name, "role_id");
    assert_eq!(analyzed.params[1].ts_type, "number");

    // $3 in HAVING count(*) >= $3
    assert_eq!(analyzed.params[2].index, 3);
    assert_eq!(analyzed.params[2].ts_type, "number");

    // $4 in HAVING avg(score) > $4
    assert_eq!(analyzed.params[3].index, 4);
    assert_eq!(analyzed.params[3].ts_type, "number");

    // $5 in LIMIT $5
    assert_eq!(analyzed.params[4].index, 5);
    assert_eq!(analyzed.params[4].name, "limit");
    assert_eq!(analyzed.params[4].ts_type, "number");

    // $6 in OFFSET $6
    assert_eq!(analyzed.params[5].index, 6);
    assert_eq!(analyzed.params[5].name, "offset");
    assert_eq!(analyzed.params[5].ts_type, "number");
}

#[test]
fn test_parameter_numbering_out_of_order_and_multi_reference() {
    let catalog = setup_adv_catalog();

    // 1. Out of order parameters: $2 appears before $1 in SQL
    let sql1 = "SELECT * FROM accounts WHERE balance >= $2 AND name = $1;";
    let a1 = analyze_query(sql1, &catalog, Some("OutOfOrder")).expect("Out of order should analyze");
    assert_eq!(a1.params.len(), 2);
    assert_eq!(a1.params[0].index, 1);
    assert_eq!(a1.params[0].name, "name");
    assert_eq!(a1.params[0].ts_type, "string");
    assert_eq!(a1.params[1].index, 2);
    assert_eq!(a1.params[1].name, "balance");
    assert_eq!(a1.params[1].ts_type, "number");

    // 2. Same parameter $1 referenced multiple times in same query
    let sql2 = "SELECT * FROM accounts WHERE name = $1 OR tier::text = $1;";
    let a2 = analyze_query(sql2, &catalog, Some("MultiRefSameParam")).expect("Multiple ref should analyze");
    assert_eq!(a2.params.len(), 1, "Should deduplicate parameter 1");
    assert_eq!(a2.params[0].index, 1);
    assert_eq!(a2.params[0].name, "name");
    assert_eq!(a2.params[0].ts_type, "string");
}

#[test]
fn test_property_name_disambiguation_under_extreme_collisions() {
    let catalog = setup_adv_catalog();

    // 1. Multiple references to same column producing unique names
    let sql1 = r#"
        SELECT * FROM accounts
        WHERE name = $1 OR name = $2 OR name = $3 OR name = $4;
    "#;
    let a1 = analyze_query(sql1, &catalog, Some("QuadNameCollision")).expect("Quad collision should analyze");
    assert_eq!(a1.params.len(), 4);
    assert_eq!(a1.params[0].name, "name");
    assert_eq!(a1.params[1].name, "name2");
    assert_eq!(a1.params[2].name, "name3");
    assert_eq!(a1.params[3].name, "name4");
    for p in &a1.params {
        assert_eq!(p.ts_type, "string");
    }

    // 2. Interleaved pre-existing numbered column names
    let sql2 = r#"
        SELECT * FROM collision_table
        WHERE id = $1 AND id2 = $2 AND id3 = $3 AND id = $4 AND id2 = $5 AND id = $6;
    "#;
    let a2 = analyze_query(sql2, &catalog, Some("InterleavedCollisions")).expect("Interleaved should analyze");
    assert_eq!(a2.params.len(), 6);
    let names: Vec<String> = a2.params.iter().map(|p| p.name.clone()).collect();
    let unique_names: std::collections::HashSet<String> = names.iter().cloned().collect();
    assert_eq!(unique_names.len(), 6, "All 6 parameter names must be globally unique: {:?}", names);

    // 3. Multi-table join column collisions: a.id vs m.id vs l.id
    let sql3 = r#"
        SELECT a.name, m.email, l.action
        FROM accounts a
        JOIN members m ON m.account_id = a.id
        JOIN audit_logs l ON l.account_id = a.id
        WHERE a.id = $1 AND m.id = $2 AND l.id = $3;
    "#;
    let a3 = analyze_query(sql3, &catalog, Some("MultiTableIdCollision")).expect("Multi table should analyze");
    assert_eq!(a3.params.len(), 3);
    assert_eq!(a3.params[0].name, "id");
    assert_eq!(a3.params[1].name, "id2");
    assert_eq!(a3.params[2].name, "id3");

    // 4. Special characters in column names and TypeScript reserved words
    let sql4 = r#"
        SELECT * FROM collision_table
        WHERE "user-id" = $1 OR "user-id" = $2 OR "default" = $3 OR "default" = $4;
    "#;
    let a4 = analyze_query(sql4, &catalog, Some("SpecialCharsAndKeywords")).expect("Special chars should analyze");
    assert_eq!(a4.params.len(), 4);
    assert_eq!(a4.params[0].name, "user-id");
    assert_eq!(a4.params[1].name, "user-id2");
    assert_eq!(a4.params[2].name, "default");
    assert_eq!(a4.params[3].name, "default2");

    // Codegen must correctly quote non-identifier property keys
    let ts_code = generate_file_ts_with_options(&[a4], &CodegenOptions::new(DriverTarget::Pg, true));
    assert!(ts_code.contains(r#""user-id": string;"#));
    assert!(ts_code.contains(r#""user-id2": string;"#));
    assert!(ts_code.contains(r#"params["user-id"]"#));
    assert!(ts_code.contains(r#"params["user-id2"]"#));
}

#[test]
fn test_advanced_set_operations_and_subqueries_in_dml() {
    let catalog = setup_adv_catalog();

    // 1. UNION with parameters in both arms
    let sql1 = r#"
        (SELECT id, name FROM accounts WHERE balance > $1)
        UNION ALL
        (SELECT id, name FROM accounts WHERE tier = $2);
    "#;
    let a1 = analyze_query(sql1, &catalog, Some("UnionParams")).expect("Union should analyze");
    assert_eq!(a1.params.len(), 2);
    assert_eq!(a1.params[0].index, 1);
    assert_eq!(a1.params[0].name, "balance");
    assert_eq!(a1.params[0].ts_type, "number");
    assert_eq!(a1.params[1].index, 2);
    assert_eq!(a1.params[1].name, "tier");
    assert!(a1.params[1].ts_type.contains("\"free\""));

    // 2. CTE with set operation and downstream query
    let sql2 = r#"
        WITH combined AS (
            SELECT id, name, balance FROM accounts WHERE balance > $1
            UNION ALL
            SELECT a.id, a.name, a.balance FROM accounts a WHERE a.tier = $2
        )
        SELECT * FROM combined WHERE name LIKE $3;
    "#;
    let a2 = analyze_query(sql2, &catalog, Some("CteUnionParams")).expect("CTE Union should analyze");
    assert_eq!(a2.params.len(), 3);
    assert_eq!(a2.params[0].name, "balance");
    assert_eq!(a2.params[0].ts_type, "number");
    assert_eq!(a2.params[1].name, "tier");
    assert_eq!(a2.params[2].name, "name");
    assert_eq!(a2.params[2].ts_type, "string");

    // 3. Subquery inside UPDATE SET clause
    let sql3 = r#"
        UPDATE accounts
        SET balance = (
            SELECT coalesce(avg(m.score), 0)
            FROM members m
            WHERE m.account_id = accounts.id AND m.score >= $1
        )
        WHERE id = $2
        RETURNING id, balance;
    "#;
    let a3 = analyze_query(sql3, &catalog, Some("UpdateSubquerySet")).expect("Update with subquery set should analyze");
    assert_eq!(a3.params.len(), 2);
    assert_eq!(a3.params[0].index, 1);
    assert_eq!(a3.params[0].name, "score");
    assert_eq!(a3.params[0].ts_type, "number");
    assert_eq!(a3.params[1].index, 2);
    assert_eq!(a3.params[1].name, "id");
    assert_eq!(a3.params[1].ts_type, "string");

    // 4. Parameter in RETURNING clause expression
    let sql4 = r#"
        DELETE FROM accounts
        WHERE id = $1
        RETURNING id, (balance * $2::numeric) AS scaled_balance;
    "#;
    let a4 = analyze_query(sql4, &catalog, Some("DeleteReturningExpr")).expect("Delete returning expr should analyze");
    assert_eq!(a4.params.len(), 2);
    assert_eq!(a4.params[0].name, "id");
    assert_eq!(a4.params[0].ts_type, "string");
    assert_eq!(a4.params[1].index, 2);
    assert_eq!(a4.params[1].ts_type, "number");

    // 5. IN and BETWEEN parameter lists
    let sql5 = r#"
        SELECT * FROM accounts
        WHERE id IN ($1, $2, $3) AND balance BETWEEN $4 AND $5;
    "#;
    let a5 = analyze_query(sql5, &catalog, Some("InBetweenParams")).expect("In between params should analyze");
    assert_eq!(a5.params.len(), 5);
    assert_eq!(a5.params[0].name, "id");
    assert_eq!(a5.params[1].name, "id2");
    assert_eq!(a5.params[2].name, "id3");
    assert_eq!(a5.params[3].name, "balance");
    assert_eq!(a5.params[4].name, "balance2");
    assert_eq!(a5.params[0].ts_type, "string");
    assert_eq!(a5.params[1].ts_type, "string");
    assert_eq!(a5.params[2].ts_type, "string");
    assert_eq!(a5.params[3].ts_type, "number");
    assert_eq!(a5.params[4].ts_type, "number");

    // 6. Chained CTE used inside UPDATE
    let sql6 = r#"
        WITH target_accounts AS (
            SELECT id FROM accounts WHERE balance < $1
        )
        UPDATE members
        SET notes = $2
        WHERE account_id IN (SELECT id FROM target_accounts) AND role_id = $3
        RETURNING id, notes;
    "#;
    let a6 = analyze_query(sql6, &catalog, Some("UpdateCteTargets")).expect("Update CTE targets should analyze");
    assert_eq!(a6.params.len(), 3);
    assert_eq!(a6.params[0].name, "balance");
    assert_eq!(a6.params[0].ts_type, "number");
    assert_eq!(a6.params[1].name, "notes");
    assert_eq!(a6.params[1].ts_type, "string");
    assert_eq!(a6.params[2].name, "role_id");
    assert_eq!(a6.params[2].ts_type, "number");
}

