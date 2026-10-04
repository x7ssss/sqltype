//! Empirical Adversarial Stress Test Suite: Scope Isolation & Cross-Schema Behavior
//!
//! Focus Areas:
//! 1. Catalog mutation isolation (aliased joins, range subqueries, column aliases, multi-threaded safety)
//! 2. Cross-schema ALTER TABLE behavior and isolation
//! 3. Ambiguous column reference detection on aliased joins

use sqltype::analyzer::analyze_query;
use sqltype::catalog::Catalog;
use std::sync::Arc;
use std::thread;

// =========================================================================
// Area 1: Catalog Mutation Isolation
// =========================================================================

#[test]
fn test_adversarial_aliased_join_catalog_immutability() {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql(
            r#"
            CREATE TABLE dept (dept_id INT PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE emp (emp_id INT PRIMARY KEY, dept_id INT NOT NULL, salary NUMERIC NOT NULL);
            "#,
        )
        .expect("DDL setup must succeed");

    let initial_table_count = catalog.tables.len();
    assert!(catalog.get_table("dept").is_some());
    assert!(catalog.get_table("emp").is_some());
    assert!(catalog.get_table("j").is_none());

    // 1. Run query with aliased join: (dept d JOIN emp e) AS j
    let query = r#"
        SELECT
            j.emp_id,
            j.name,
            j.salary
        FROM (dept d JOIN emp e ON d.dept_id = e.dept_id) AS j;
    "#;
    let analyzed = analyze_query(query, &catalog, None).expect("Aliased join query must analyze cleanly");
    assert_eq!(analyzed.fields.len(), 3);

    // Verify catalog state is completely unchanged
    assert_eq!(catalog.tables.len(), initial_table_count, "Catalog table count must not increase");
    assert!(catalog.get_table("j").is_none(), "Alias 'j' must not exist in catalog");
    assert!(!catalog.tables.contains_key("j"), "catalog.tables must not contain key 'j'");
    assert!(!catalog.tables.contains_key("public.j"), "catalog.tables must not contain 'public.j'");

    // 2. Subsequent query attempting to reference 'j' must fail cleanly with table not found
    let invalid_subsequent = "SELECT id FROM j;";
    let err = analyze_query(invalid_subsequent, &catalog, None).unwrap_err();
    assert!(
        err.contains("does not exist"),
        "Subsequent query querying leaked alias 'j' must fail with 'does not exist', got: {}",
        err
    );
}

#[test]
fn test_adversarial_range_subquery_catalog_immutability() {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql(
            r#"
            CREATE TABLE metrics (
                id INT PRIMARY KEY,
                category TEXT NOT NULL,
                val DOUBLE PRECISION NOT NULL
            );
            "#,
        )
        .expect("DDL setup must succeed");

    let initial_count = catalog.tables.len();
    assert!(catalog.get_table("sub").is_none());

    // 1. Run query with range subquery: (SELECT ...) AS sub
    let query = r#"
        SELECT
            sub.category,
            sub.val
        FROM (SELECT category, val FROM metrics WHERE val > 10.0) AS sub;
    "#;
    let analyzed = analyze_query(query, &catalog, None).expect("Range subselect query must analyze cleanly");
    assert_eq!(analyzed.fields.len(), 2);
    assert_eq!(analyzed.fields[0].name, "category");
    assert_eq!(analyzed.fields[1].name, "val");

    // Verify catalog state is completely unchanged
    assert_eq!(catalog.tables.len(), initial_count);
    assert!(catalog.get_table("sub").is_none(), "Range subquery alias 'sub' must not exist in catalog");
    assert!(!catalog.tables.contains_key("sub"));
    assert!(!catalog.tables.contains_key("public.sub"));

    // 2. Subsequent query attempting to reference 'sub' must fail cleanly
    let invalid_subsequent = "SELECT category FROM sub;";
    let err = analyze_query(invalid_subsequent, &catalog, None).unwrap_err();
    assert!(
        err.contains("does not exist"),
        "Subsequent query querying leaked alias 'sub' must fail with 'does not exist', got: {}",
        err
    );
}

#[test]
fn test_adversarial_column_alias_catalog_immutability() {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql(
            r#"
            CREATE TABLE accounts (
                acc_id INT PRIMARY KEY,
                holder_name TEXT NOT NULL,
                balance NUMERIC NOT NULL
            );
            "#,
        )
        .expect("DDL setup must succeed");

    // Verify initial column names in catalog
    let tbl_before = catalog.get_table("accounts").unwrap();
    let col_names_before: Vec<String> = tbl_before.columns.iter().map(|c| c.name.clone()).collect();
    assert_eq!(col_names_before, vec!["acc_id", "holder_name", "balance"]);

    // 1. Query with column aliases: accounts a(c1, c2, c3)
    let query = r#"
        SELECT
            a.c1,
            a.c2,
            a.c3
        FROM accounts a(c1, c2, c3);
    "#;
    let analyzed = analyze_query(query, &catalog, None).expect("Column aliased query must analyze cleanly");
    assert_eq!(analyzed.fields.len(), 3);
    assert_eq!(analyzed.fields[0].name, "c1");
    assert_eq!(analyzed.fields[1].name, "c2");
    assert_eq!(analyzed.fields[2].name, "c3");

    // 2. Critical catalog inspection: Catalog table accounts MUST NOT have columns mutated!
    let tbl_after = catalog.get_table("accounts").unwrap();
    let col_names_after: Vec<String> = tbl_after.columns.iter().map(|c| c.name.clone()).collect();
    assert_eq!(
        col_names_after,
        vec!["acc_id", "holder_name", "balance"],
        "Column aliases in query must NOT overwrite underlying catalog column metadata"
    );
    assert!(tbl_after.get_column("c1").is_none());
    assert!(tbl_after.get_column("c2").is_none());
    assert!(tbl_after.get_column("c3").is_none());

    // 3. Subsequent query without column aliases must see original column names
    let q_orig = "SELECT acc_id, holder_name FROM accounts;";
    let res_orig = analyze_query(q_orig, &catalog, None).expect("Original columns must still be valid");
    assert_eq!(res_orig.fields[0].name, "acc_id");
    assert_eq!(res_orig.fields[1].name, "holder_name");

    // 4. Subsequent query trying to query aliased names c1, c2 without aliasing must fail
    let q_invalid = "SELECT c1 FROM accounts;";
    let err = analyze_query(q_invalid, &catalog, None).unwrap_err();
    assert!(
        err.contains("not found"),
        "Referencing transient column alias c1 in unaliased query must fail, got: {}",
        err
    );
}

#[test]
fn test_adversarial_multithreaded_concurrent_query_isolation() {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql(
            r#"
            CREATE TABLE products (prod_id INT PRIMARY KEY, title TEXT NOT NULL, price NUMERIC NOT NULL);
            CREATE TABLE reviews (review_id INT PRIMARY KEY, product_id INT NOT NULL, rating INT NOT NULL);
            "#,
        )
        .expect("DDL setup must succeed");

    let initial_count = catalog.tables.len();
    let arc_catalog = Arc::new(catalog);

    // Spawn 16 concurrent threads performing aliased joins, range subqueries, and column aliases
    let mut handles = Vec::new();
    for i in 0..16 {
        let cat = Arc::clone(&arc_catalog);
        let handle = thread::spawn(move || {
            // Case A: Aliased join with thread-specific alias name
            let q_join = format!(
                "SELECT j{0}.prod_id, j{0}.title, j{0}.rating FROM (products p JOIN reviews r ON p.prod_id = r.product_id) AS j{0};",
                i
            );
            let res_join = analyze_query(&q_join, &cat, None).expect("Concurrent aliased join failed");
            assert_eq!(res_join.fields.len(), 3);

            // Case B: Range subselect with thread-specific alias
            let q_sub = format!(
                "SELECT s{0}.title FROM (SELECT title FROM products WHERE price > 5.0) AS s{0};",
                i
            );
            let res_sub = analyze_query(&q_sub, &cat, None).expect("Concurrent subquery failed");
            assert_eq!(res_sub.fields[0].name, "title");

            // Case C: Column alias
            let q_col = format!(
                "SELECT p{0}.custom_p_id, p{0}.custom_p_title FROM products p{0}(custom_p_id, custom_p_title, custom_p_price);",
                i
            );
            let res_col = analyze_query(&q_col, &cat, None).expect("Concurrent column aliasing failed");
            assert_eq!(res_col.fields[0].name, "custom_p_id");
            assert_eq!(res_col.fields[1].name, "custom_p_title");
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread should not panic");
    }

    // Verify catalog immutability after all concurrent queries have finished
    assert_eq!(
        arc_catalog.tables.len(),
        initial_count,
        "Catalog tables must not be modified by concurrent threads"
    );
    for i in 0..16 {
        assert!(arc_catalog.get_table(&format!("j{}", i)).is_none());
        assert!(arc_catalog.get_table(&format!("s{}", i)).is_none());
        assert!(arc_catalog.get_table(&format!("p{}", i)).is_none());
    }
}

// =========================================================================
// Area 2: Cross-Schema ALTER TABLE Isolation
// =========================================================================

#[test]
fn test_adversarial_cross_schema_alter_table_isolation() {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql(
            r#"
            CREATE TABLE users (
                id INT PRIMARY KEY,
                email TEXT NOT NULL
            );
            CREATE TABLE other_schema.users (
                id INT PRIMARY KEY,
                username TEXT NOT NULL,
                bio TEXT
            );
            "#,
        )
        .expect("DDL setup must succeed");

    // 1. Initial verification of both tables
    let public_users = catalog.get_table("users").expect("public.users must exist");
    assert_eq!(public_users.columns.len(), 2);
    assert!(public_users.get_column("email").is_some());
    assert!(public_users.get_column("username").is_none());

    let other_users = catalog.get_table("other_schema.users").expect("other_schema.users must exist");
    assert_eq!(other_users.columns.len(), 3);
    assert!(other_users.get_column("username").is_some());
    assert!(other_users.get_column("email").is_none());

    // 2. ALTER other_schema.users: ADD COLUMN status TEXT;
    catalog
        .apply_sql("ALTER TABLE other_schema.users ADD COLUMN status TEXT;")
        .expect("ALTER TABLE other_schema.users must succeed");

    // Verification: other_schema.users MUST have 'status', public.users MUST NOT!
    let other_after_add = catalog.get_table("other_schema.users").unwrap();
    assert!(
        other_after_add.get_column("status").is_some(),
        "other_schema.users must have added status column"
    );

    let public_after_add = catalog.get_table("users").unwrap();
    assert!(
        public_after_add.get_column("status").is_none(),
        "public.users must NOT have status column added to other_schema.users"
    );

    // 3. ALTER other_schema.users: DROP COLUMN bio;
    catalog
        .apply_sql("ALTER TABLE other_schema.users DROP COLUMN bio;")
        .expect("DROP COLUMN on other_schema.users must succeed");

    let other_after_drop = catalog.get_table("other_schema.users").unwrap();
    assert!(other_after_drop.get_column("bio").is_none());

    let public_after_drop = catalog.get_table("users").unwrap();
    assert_eq!(public_after_drop.columns.len(), 2, "public.users columns count must be completely unaffected");

    // 4. Non-existent foreign schema ALTER must NOT alter public.users
    catalog
        .apply_sql("ALTER TABLE completely_unknown_schema.users ADD COLUMN malicious TEXT;")
        .expect("ALTER on unknown schema should be a safe no-op in catalog");

    let public_check_malicious = catalog.get_table("users").unwrap();
    assert!(
        public_check_malicious.get_column("malicious").is_none(),
        "ALTER TABLE on unknown schema must never fall back to public.users"
    );

    // 5. Unqualified ALTER TABLE users ADD COLUMN phone TEXT must alter public.users only
    catalog
        .apply_sql("ALTER TABLE users ADD COLUMN phone TEXT;")
        .expect("Unqualified ALTER TABLE users must succeed");

    let public_has_phone = catalog.get_table("users").unwrap();
    assert!(public_has_phone.get_column("phone").is_some(), "public.users must have phone column");

    let other_check_phone = catalog.get_table("other_schema.users").unwrap();
    assert!(
        other_check_phone.get_column("phone").is_none(),
        "other_schema.users must NOT receive column added to unqualified users"
    );
}

// =========================================================================
// Area 3: Ambiguous Column References on Aliased Joins
// =========================================================================

#[test]
fn test_adversarial_ambiguous_column_in_aliased_join_triggers_clean_error() {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql(
            r#"
            CREATE TABLE t_left (
                id INT PRIMARY KEY,
                name TEXT NOT NULL,
                common_tag TEXT NOT NULL,
                left_unique TEXT NOT NULL
            );
            CREATE TABLE t_right (
                id INT PRIMARY KEY,
                email TEXT NOT NULL,
                common_tag TEXT NOT NULL,
                right_unique TEXT NOT NULL
            );
            "#,
        )
        .expect("DDL setup must succeed");

    // Case 1: Direct qualified ambiguous reference `j.id`
    let q_ambig_id = r#"
        SELECT j.id
        FROM (t_left l JOIN t_right r ON l.id = r.id) AS j;
    "#;
    let res_ambig_id = analyze_query(q_ambig_id, &catalog, None);
    assert!(res_ambig_id.is_err(), "Referencing j.id on aliased join must return error");
    let err_msg = res_ambig_id.unwrap_err();
    assert!(
        err_msg.to_ascii_lowercase().contains("ambiguous"),
        "Expected error message mentioning ambiguous column, got: {}",
        err_msg
    );

    // Case 2: Direct qualified ambiguous reference `j.common_tag`
    let q_ambig_tag = r#"
        SELECT j.common_tag
        FROM (t_left l JOIN t_right r ON l.id = r.id) AS j;
    "#;
    let res_ambig_tag = analyze_query(q_ambig_tag, &catalog, None);
    assert!(res_ambig_tag.is_err(), "Referencing j.common_tag must return error");
    assert!(
        res_ambig_tag.unwrap_err().to_ascii_lowercase().contains("ambiguous"),
        "Expected error message mentioning ambiguous"
    );

    // Case 3: Unqualified ambiguous reference `id`
    let q_unqual_id = r#"
        SELECT id
        FROM (t_left l JOIN t_right r ON l.id = r.id) AS j;
    "#;
    let res_unqual_id = analyze_query(q_unqual_id, &catalog, None);
    assert!(res_unqual_id.is_err(), "Referencing unqualified ambiguous id must return error");
    assert!(
        res_unqual_id.unwrap_err().to_ascii_lowercase().contains("ambiguous"),
        "Expected error mentioning ambiguous"
    );

    // Case 4: Non-ambiguous unique columns resolve cleanly
    let q_unique = r#"
        SELECT
            j.name,
            j.email,
            j.left_unique,
            j.right_unique
        FROM (t_left l JOIN t_right r ON l.id = r.id) AS j;
    "#;
    let res_unique = analyze_query(q_unique, &catalog, None).expect("Unique columns in aliased join must resolve cleanly");
    assert_eq!(res_unique.fields.len(), 4);
    assert_eq!(res_unique.fields[0].name, "name");
    assert_eq!(res_unique.fields[1].name, "email");
    assert_eq!(res_unique.fields[2].name, "left_unique");
    assert_eq!(res_unique.fields[3].name, "right_unique");

    // Case 5: Unqualified unique columns also resolve cleanly
    let q_unique_unqual = r#"
        SELECT name, email FROM (t_left l JOIN t_right r ON l.id = r.id) AS j;
    "#;
    let res_unique_unqual = analyze_query(q_unique_unqual, &catalog, None).expect("Unqualified unique columns must resolve");
    assert_eq!(res_unique_unqual.fields[0].name, "name");
    assert_eq!(res_unique_unqual.fields[1].name, "email");

    // Case 6: Attempting to reference inner table alias 'l' after it is wrapped in 'AS j' must fail cleanly
    let q_hidden_l = r#"
        SELECT l.id FROM (t_left l JOIN t_right r ON l.id = r.id) AS j;
    "#;
    let res_hidden_l = analyze_query(q_hidden_l, &catalog, None);
    assert!(
        res_hidden_l.is_err(),
        "Referencing hidden alias 'l' outside of aliased join 'j' must fail"
    );
}

#[test]
fn test_adversarial_nested_aliased_joins_ambiguity_propagation() {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql(
            r#"
            CREATE TABLE part_a (id INT PRIMARY KEY, shared_code TEXT NOT NULL, a_desc TEXT NOT NULL);
            CREATE TABLE part_b (id INT PRIMARY KEY, shared_code TEXT NOT NULL, b_desc TEXT NOT NULL);
            CREATE TABLE part_c (id INT PRIMARY KEY, c_desc TEXT NOT NULL);
            "#,
        )
        .expect("DDL setup must succeed");

    // ((part_a a JOIN part_b b ON a.id = b.id) AS j1 JOIN part_c c ON j1.a_desc = c.c_desc) AS j2
    // Here:
    // in j1: 'id' and 'shared_code' are ambiguous.
    // in j2: 'id' is ambiguous from j1 and part_c.
    // Unique columns: j2.a_desc, j2.b_desc, j2.c_desc.
    let sql_ambig = r#"
        SELECT j2.id
        FROM ((part_a a JOIN part_b b ON a.id = b.id) AS j1
              JOIN part_c c ON j1.a_desc = c.c_desc) AS j2;
    "#;
    let res_ambig = analyze_query(sql_ambig, &catalog, None);
    assert!(res_ambig.is_err(), "j2.id must be rejected as ambiguous");
    assert!(res_ambig.unwrap_err().to_ascii_lowercase().contains("ambiguous"));

    let sql_ok = r#"
        SELECT
            j2.a_desc,
            j2.b_desc,
            j2.c_desc
        FROM ((part_a a JOIN part_b b ON a.id = b.id) AS j1
              JOIN part_c c ON j1.a_desc = c.c_desc) AS j2;
    "#;
    let res_ok = analyze_query(sql_ok, &catalog, None).expect("Non-ambiguous columns in nested aliased join must succeed");
    assert_eq!(res_ok.fields.len(), 3);
    assert_eq!(res_ok.fields[0].name, "a_desc");
    assert_eq!(res_ok.fields[1].name, "b_desc");
    assert_eq!(res_ok.fields[2].name, "c_desc");
}

#[test]
fn test_adversarial_three_way_cross_schema_alter_matrix() {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql(
            r#"
            CREATE TABLE public.users (
                id INT PRIMARY KEY,
                public_col TEXT NOT NULL
            );
            CREATE TABLE tenant_a.users (
                id INT PRIMARY KEY,
                tenant_a_col TEXT NOT NULL
            );
            CREATE TABLE tenant_b.users (
                id INT PRIMARY KEY,
                tenant_b_col TEXT NOT NULL
            );
            "#,
        )
        .expect("DDL setup must succeed");

    // Alter tenant_a only
    catalog
        .apply_sql("ALTER TABLE tenant_a.users ADD COLUMN extra_a TEXT;")
        .unwrap();

    // Alter tenant_b only
    catalog
        .apply_sql("ALTER TABLE tenant_b.users ADD COLUMN extra_b INT;")
        .unwrap();

    // Alter public only (unqualified)
    catalog
        .apply_sql("ALTER TABLE users ADD COLUMN extra_public BOOLEAN;")
        .unwrap();

    // Alter non-existent tenant_c
    catalog
        .apply_sql("ALTER TABLE tenant_c.users ADD COLUMN ghost TEXT;")
        .unwrap();

    // Strict assertions on each table's columns:
    let pub_tbl = catalog.get_table("users").unwrap();
    assert!(pub_tbl.get_column("public_col").is_some());
    assert!(pub_tbl.get_column("extra_public").is_some());
    assert!(pub_tbl.get_column("extra_a").is_none(), "tenant_a column must not leak to public");
    assert!(pub_tbl.get_column("extra_b").is_none(), "tenant_b column must not leak to public");
    assert!(pub_tbl.get_column("ghost").is_none(), "ghost column must not leak to public");
    assert_eq!(pub_tbl.columns.len(), 3);

    let a_tbl = catalog.get_table("tenant_a.users").unwrap();
    assert!(a_tbl.get_column("tenant_a_col").is_some());
    assert!(a_tbl.get_column("extra_a").is_some());
    assert!(a_tbl.get_column("extra_b").is_none());
    assert!(a_tbl.get_column("extra_public").is_none());
    assert!(a_tbl.get_column("ghost").is_none());
    assert_eq!(a_tbl.columns.len(), 3);

    let b_tbl = catalog.get_table("tenant_b.users").unwrap();
    assert!(b_tbl.get_column("tenant_b_col").is_some());
    assert!(b_tbl.get_column("extra_b").is_some());
    assert!(b_tbl.get_column("extra_a").is_none());
    assert!(b_tbl.get_column("extra_public").is_none());
    assert!(b_tbl.get_column("ghost").is_none());
    assert_eq!(b_tbl.columns.len(), 3);

    assert!(catalog.get_table("tenant_c.users").is_none());
}

#[test]
fn test_adversarial_nested_aliased_join_nullability_and_type_projection() {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql(
            r#"
            CREATE TABLE src_a (a_id INT PRIMARY KEY, a_val TEXT NOT NULL);
            CREATE TABLE src_b (b_id INT PRIMARY KEY, b_val TEXT NOT NULL);
            CREATE TABLE src_c (c_id INT PRIMARY KEY, c_val TEXT NOT NULL);
            "#,
        )
        .expect("DDL setup must succeed");

    // (src_a a LEFT JOIN src_b b ON a.a_id = b.b_id) AS j1
    // In j1: a_val is NOT NULL (string), b_val is nullable (string | null)
    // Then: (j1 FULL JOIN src_c c ON j1.a_id = c.c_id) AS j_outer
    // In j_outer: FULL JOIN marks j1 as null-producing!
    // Therefore, in j_outer:
    // - a_val becomes nullable (string | null)!
    // - b_val remains nullable (string | null)!
    // - c_val becomes nullable (string | null)!
    let sql = r#"
        SELECT
            j_outer.a_val,
            j_outer.b_val,
            j_outer.c_val
        FROM ((src_a a LEFT JOIN src_b b ON a.a_id = b.b_id) AS j1
              FULL JOIN src_c c ON j1.a_id = c.c_id) AS j_outer;
    "#;

    let analyzed = analyze_query(sql, &catalog, None).expect("Nested aliased outer join query must analyze");
    assert_eq!(analyzed.fields.len(), 3);

    assert_eq!(analyzed.fields[0].name, "a_val");
    assert_eq!(
        analyzed.fields[0].ts_type, "string | null",
        "j_outer.a_val must be nullable because j1 is on nullable side of FULL JOIN"
    );

    assert_eq!(analyzed.fields[1].name, "b_val");
    assert_eq!(analyzed.fields[1].ts_type, "string | null");

    assert_eq!(analyzed.fields[2].name, "c_val");
    assert_eq!(analyzed.fields[2].ts_type, "string | null");

    // Verify catalog immutability
    assert!(catalog.get_table("j1").is_none());
    assert!(catalog.get_table("j_outer").is_none());
}

#[test]
fn test_adversarial_self_join_aliased_ambiguity_detection() {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql(
            r#"
            CREATE TABLE staff (
                staff_id INT PRIMARY KEY,
                name TEXT NOT NULL,
                mentor_id INT
            );
            "#,
        )
        .expect("DDL setup must succeed");

    // Self join wrapped in alias `pair`:
    // (staff s1 JOIN staff s2 ON s1.mentor_id = s2.staff_id) AS pair
    // Both sides have staff_id, name, mentor_id.
    // Querying pair.name MUST be rejected as ambiguous!
    let sql_ambig = r#"
        SELECT pair.name
        FROM (staff s1 JOIN staff s2 ON s1.mentor_id = s2.staff_id) AS pair;
    "#;
    let err = analyze_query(sql_ambig, &catalog, None).unwrap_err();
    assert!(
        err.to_ascii_lowercase().contains("ambiguous"),
        "Self-join with duplicate column names in alias must be rejected as ambiguous, got: {}",
        err
    );

    // If subqueries rename the columns, then querying the alias MUST succeed!
    let sql_clean = r#"
        SELECT
            pair.junior_name,
            pair.senior_name
        FROM (
            (SELECT staff_id AS s1_id, name AS junior_name, mentor_id FROM staff) s1
            JOIN
            (SELECT staff_id AS s2_id, name AS senior_name FROM staff) s2
            ON s1.mentor_id = s2.s2_id
        ) AS pair;
    "#;
    let analyzed = analyze_query(sql_clean, &catalog, None).expect("Disambiguated self join must succeed");
    assert_eq!(analyzed.fields.len(), 2);
    assert_eq!(analyzed.fields[0].name, "junior_name");
    assert_eq!(analyzed.fields[1].name, "senior_name");
}

#[test]
fn test_adversarial_partial_column_aliasing() {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql(
            r#"
            CREATE TABLE inventory (
                sku TEXT PRIMARY KEY,
                qty INT NOT NULL,
                loc TEXT NOT NULL
            );
            "#,
        )
        .expect("DDL setup must succeed");

    // Alias only first 2 columns: inv(custom_sku, custom_qty)
    // 3rd column retains 'loc'
    let sql = r#"
        SELECT
            i.custom_sku,
            i.custom_qty,
            i.loc
        FROM inventory i(custom_sku, custom_qty);
    "#;
    let analyzed = analyze_query(sql, &catalog, None).expect("Partial column aliasing must succeed");
    assert_eq!(analyzed.fields.len(), 3);
    assert_eq!(analyzed.fields[0].name, "custom_sku");
    assert_eq!(analyzed.fields[1].name, "custom_qty");
    assert_eq!(analyzed.fields[2].name, "loc");

    // Catalog must remain unchanged
    let inv_table = catalog.get_table("inventory").unwrap();
    assert!(inv_table.get_column("sku").is_some());
    assert!(inv_table.get_column("custom_sku").is_none());
}
