//! Adversarial Empirical Challenge Suite for Milestone 2 (Schema DDL Catalog)
//! Tests schema qualification, cross-schema lookups, constraint invariants, and type resolution.

use sqltype::catalog::{Catalog, DriverTarget};

#[test]
fn test_cross_schema_lookup_and_isolation() {
    let sql = "
        -- 1. Table in public (unqualified DDL)
        CREATE TABLE users (
            id UUID PRIMARY KEY,
            name TEXT NOT NULL
        );

        -- 2. Table with same name in explicit pg_catalog schema
        CREATE TABLE pg_catalog.users (
            id INT PRIMARY KEY,
            sys_name TEXT NOT NULL
        );

        -- 3. Table with same name in custom auth schema
        CREATE TABLE auth.users (
            id BIGINT PRIMARY KEY,
            email TEXT NOT NULL
        );

        -- 4. Table with same name in custom audit schema
        CREATE TABLE audit.users (
            id SERIAL PRIMARY KEY,
            log_entry TEXT NOT NULL
        );

        -- 5. Isolated table only in auth schema
        CREATE TABLE auth.tokens (
            token_id UUID PRIMARY KEY,
            secret TEXT NOT NULL
        );

        -- 6. Isolated table only in pg_catalog
        CREATE TABLE pg_catalog.sys_config (
            cfg_id INT PRIMARY KEY,
            cfg_val TEXT NOT NULL
        );
    ";

    let mut catalog = Catalog::default();
    catalog.apply_sql(sql).expect("DDL script should parse successfully");

    // === Test 1: Unqualified lookups ===
    // "users" must resolve to public.users (UUID id), NEVER to pg_catalog, auth, or audit
    let unqualified_users = catalog.get_table("users").expect("users must exist in public");
    assert_eq!(unqualified_users.name, "users");
    assert_eq!(unqualified_users.columns[0].pg_type, "uuid");
    assert_eq!(unqualified_users.columns[0].ts_type, "string");

    let qualified_none_users = catalog.get_table_qualified(None, "users").expect("users must exist with None schema");
    assert_eq!(qualified_none_users.columns[0].pg_type, "uuid");

    // === Test 2: Explicit public qualification ===
    let public_users = catalog.get_table("public.users").expect("public.users must exist");
    assert_eq!(public_users.columns[0].pg_type, "uuid");

    let qual_public_users = catalog.get_table_qualified(Some("public"), "users").expect("qualified public.users must exist");
    assert_eq!(qual_public_users.columns[0].pg_type, "uuid");

    // === Test 3: Explicit pg_catalog qualification ===
    let pg_users = catalog.get_table("pg_catalog.users").expect("pg_catalog.users must exist");
    assert_eq!(pg_users.schema.as_deref(), Some("pg_catalog"));
    assert_eq!(pg_users.columns[0].ts_type, "number");
    assert_eq!(pg_users.columns[1].name, "sys_name");
    assert_eq!(pg_users.columns[1].ts_type, "string");

    let qual_pg_users = catalog.get_table_qualified(Some("pg_catalog"), "users").expect("qualified pg_catalog.users must exist");
    assert_eq!(qual_pg_users.columns[0].ts_type, "number");
    assert_eq!(qual_pg_users.columns[1].name, "sys_name");

    // === Test 4: Explicit auth qualification ===
    let auth_users = catalog.get_table("auth.users").expect("auth.users must exist");
    assert_eq!(auth_users.schema.as_deref(), Some("auth"));
    assert_eq!(auth_users.columns[0].ts_type, "string"); // bigint -> string for Postgres driver
    assert_eq!(auth_users.columns[1].name, "email");
    assert_eq!(auth_users.columns[1].ts_type, "string");

    let qual_auth_users = catalog.get_table_qualified(Some("auth"), "users").expect("qualified auth.users must exist");
    assert_eq!(qual_auth_users.columns[0].ts_type, "string");

    // === Test 5: Explicit audit qualification ===
    let audit_users = catalog.get_table("audit.users").expect("audit.users must exist");
    assert_eq!(audit_users.schema.as_deref(), Some("audit"));
    assert_eq!(audit_users.columns[0].ts_type, "number"); // serial -> number
    assert_eq!(audit_users.columns[1].name, "log_entry");
    assert_eq!(audit_users.columns[1].ts_type, "string");

    let qual_audit_users = catalog.get_table_qualified(Some("audit"), "users").expect("qualified audit.users must exist");
    assert_eq!(qual_audit_users.columns[0].ts_type, "number");

    // === Test 6: Cross-schema non-matching lookups ===
    // Negative lookups: looking for non-existent schemas or cross-polluted lookups
    assert!(catalog.get_table("other.users").is_none());
    assert!(catalog.get_table_qualified(Some("other"), "users").is_none());
    assert!(catalog.get_table_qualified(Some("nonexistent"), "users").is_none());

    // === Test 7: Isolated schema tables unqualified lookup behavior ===
    // "tokens" only exists in "auth": unqualified "tokens" must NOT resolve to auth.tokens
    assert!(catalog.get_table("tokens").is_none(), "Unqualified lookup must NOT leak into auth schema");
    assert!(catalog.get_table_qualified(None, "tokens").is_none());
    assert!(catalog.get_table("public.tokens").is_none());
    assert!(catalog.get_table("pg_catalog.tokens").is_none());
    assert!(catalog.get_table("auth.tokens").is_some());
    assert!(catalog.get_table_qualified(Some("auth"), "tokens").is_some());

    // "sys_config" only exists in "pg_catalog": unqualified must NOT leak into pg_catalog
    assert!(catalog.get_table("sys_config").is_none(), "Unqualified lookup must NOT leak into pg_catalog");
    assert!(catalog.get_table("public.sys_config").is_none());
    assert!(catalog.get_table("auth.sys_config").is_none());
    assert!(catalog.get_table("pg_catalog.sys_config").is_some());
    assert!(catalog.get_table_qualified(Some("pg_catalog"), "sys_config").is_some());

    // === Test 8: Case insensitivity in schema and table lookup ===
    assert!(catalog.get_table("PG_CATALOG.USERS").is_some());
    assert_eq!(catalog.get_table("PG_CATALOG.USERS").unwrap().columns[0].ts_type, "number");
    assert!(catalog.get_table_qualified(Some("PG_CATALOG"), "USERS").is_some());
    assert!(catalog.get_table_qualified(Some("AuTh"), "UsErS").is_some());
    assert_eq!(catalog.get_table_qualified(Some("AuTh"), "UsErS").unwrap().columns[0].ts_type, "string");
}

#[test]
fn test_primary_key_and_constraint_invariants() {
    let sql = "
        -- 1. Inline PK
        CREATE TABLE pk_single (
            id UUID PRIMARY KEY,
            val TEXT
        );

        -- 2. Multi-column table PK
        CREATE TABLE pk_composite (
            tenant_id UUID,
            user_id INT,
            data TEXT,
            PRIMARY KEY (tenant_id, user_id)
        );

        -- 3. Named table constraint PK
        CREATE TABLE pk_named (
            org_id INT,
            dept_id INT,
            info TEXT,
            CONSTRAINT pk_org_dept PRIMARY KEY (org_id, dept_id)
        );

        -- 4. Serial types implicit NOT NULL and DEFAULT
        CREATE TABLE serial_invariants (
            s_small SMALLSERIAL,
            s_norm SERIAL,
            s_big BIGSERIAL,
            regular_int INT
        );

        -- 5. Domain inheritance with NOT NULL and DEFAULT
        CREATE DOMAIN constrained_status AS TEXT NOT NULL DEFAULT 'pending';
        CREATE DOMAIN unconstrained_status AS TEXT;
        CREATE TABLE domain_invariants (
            id INT PRIMARY KEY,
            st_req constrained_status,
            st_opt unconstrained_status,
            st_overridden constrained_status
        );
    ";

    let mut catalog = Catalog::default();
    catalog.apply_sql(sql).unwrap();

    // Verify pk_single
    let single = catalog.get_table("pk_single").unwrap();
    assert_eq!(single.primary_keys, vec!["id"]);
    let id_col = single.get_column("id").unwrap();
    assert!(id_col.is_primary_key);
    assert!(!id_col.is_nullable);
    let val_col = single.get_column("val").unwrap();
    assert!(!val_col.is_primary_key);
    assert!(val_col.is_nullable);

    // Verify pk_composite
    let comp = catalog.get_table("pk_composite").unwrap();
    assert_eq!(comp.primary_keys, vec!["tenant_id", "user_id"]);
    let tenant_col = comp.get_column("tenant_id").unwrap();
    assert!(tenant_col.is_primary_key);
    assert!(!tenant_col.is_nullable);
    let user_col = comp.get_column("user_id").unwrap();
    assert!(user_col.is_primary_key);
    assert!(!user_col.is_nullable);
    let data_col = comp.get_column("data").unwrap();
    assert!(!data_col.is_primary_key);
    assert!(data_col.is_nullable);

    // Verify pk_named
    let named = catalog.get_table("pk_named").unwrap();
    assert_eq!(named.primary_keys, vec!["org_id", "dept_id"]);
    assert!(named.get_column("org_id").unwrap().is_primary_key);
    assert!(!named.get_column("org_id").unwrap().is_nullable);
    assert!(named.get_column("dept_id").unwrap().is_primary_key);
    assert!(!named.get_column("dept_id").unwrap().is_nullable);
    assert!(!named.get_column("info").unwrap().is_primary_key);

    // Verify serial invariants
    let serial_t = catalog.get_table("serial_invariants").unwrap();
    let s_small = serial_t.get_column("s_small").unwrap();
    assert!(!s_small.is_nullable, "smallserial must be NOT NULL");
    assert!(s_small.has_default, "smallserial must have DEFAULT");

    let s_norm = serial_t.get_column("s_norm").unwrap();
    assert!(!s_norm.is_nullable, "serial must be NOT NULL");
    assert!(s_norm.has_default, "serial must have DEFAULT");

    let s_big = serial_t.get_column("s_big").unwrap();
    assert!(!s_big.is_nullable, "bigserial must be NOT NULL");
    assert!(s_big.has_default, "bigserial must have DEFAULT");

    let reg = serial_t.get_column("regular_int").unwrap();
    assert!(reg.is_nullable, "regular int should be nullable by default");
    assert!(!reg.has_default, "regular int should not have default");

    // Verify domain constraint inheritance
    let dom_t = catalog.get_table("domain_invariants").unwrap();
    let st_req = dom_t.get_column("st_req").unwrap();
    assert!(!st_req.is_nullable, "constrained domain must propagate NOT NULL");
    assert!(st_req.has_default, "constrained domain must propagate DEFAULT");

    let st_opt = dom_t.get_column("st_opt").unwrap();
    assert!(st_opt.is_nullable, "unconstrained domain must remain nullable");
    assert!(!st_opt.has_default, "unconstrained domain must not have default");

    // === Test 6: Alter Table Add/Drop PK and Columns ===
    catalog.apply_sql("CREATE TABLE alter_invariants (a INT, b INT, c TEXT);").unwrap();
    let alt_init = catalog.get_table("alter_invariants").unwrap();
    assert!(alt_init.primary_keys.is_empty());

    // Add composite PK via ALTER TABLE
    catalog.apply_sql("ALTER TABLE alter_invariants ADD CONSTRAINT pk_ab PRIMARY KEY (a, b);").unwrap();
    let alt_pk = catalog.get_table("alter_invariants").unwrap();
    assert_eq!(alt_pk.primary_keys, vec!["a", "b"]);
    assert!(alt_pk.get_column("a").unwrap().is_primary_key);
    assert!(!alt_pk.get_column("a").unwrap().is_nullable);
    assert!(alt_pk.get_column("b").unwrap().is_primary_key);
    assert!(!alt_pk.get_column("b").unwrap().is_nullable);

    // Drop column that is part of PK
    catalog.apply_sql("ALTER TABLE alter_invariants DROP COLUMN b;").unwrap();
    let alt_drop = catalog.get_table("alter_invariants").unwrap();
    assert_eq!(alt_drop.primary_keys, vec!["a"], "Primary keys must remove dropped column");
    assert!(alt_drop.get_column("b").is_none());
    assert!(alt_drop.get_column("a").unwrap().is_primary_key);

    // Add new column with serial type via ALTER TABLE
    catalog.apply_sql("ALTER TABLE alter_invariants ADD COLUMN seq_num SERIAL;").unwrap();
    let alt_add_serial = catalog.get_table("alter_invariants").unwrap();
    let seq_col = alt_add_serial.get_column("seq_num").unwrap();
    assert!(!seq_col.is_nullable);
    assert!(seq_col.has_default);
}

#[test]
fn test_strict_type_resolution_and_driver_matrix() {
    let sql = "
        CREATE TYPE custom_role AS ENUM ('admin', 'viewer');
        CREATE TYPE audit.custom_role AS ENUM ('auditor', 'secops');

        CREATE TYPE geo_coord AS (lat float8, lng float8);
        CREATE TYPE custom_schema.geo_coord AS (x int4, y int4, z int4);

        CREATE DOMAIN public.phone_num AS text NOT NULL;
        CREATE DOMAIN custom_schema.phone_num AS varchar(20);
    ";

    // Test with Default/Postgres driver
    let mut cat_pg = Catalog::new(DriverTarget::Postgres);
    cat_pg.apply_sql(sql).unwrap();

    // 1. Built-in types with pg_catalog prefix
    assert_eq!(cat_pg.resolve_type("pg_catalog.int4"), "number");
    assert_eq!(cat_pg.resolve_type("pg_catalog.int8"), "string"); // Postgres driver -> string
    assert_eq!(cat_pg.resolve_type("pg_catalog.text"), "string");
    assert_eq!(cat_pg.resolve_type("pg_catalog.bool"), "boolean");
    assert_eq!(cat_pg.resolve_type("pg_catalog.timestamptz"), "Date");
    assert_eq!(cat_pg.resolve_type("pg_catalog.bytea"), "Buffer"); // Postgres driver -> Buffer
    assert_eq!(cat_pg.resolve_type("pg_catalog.jsonb"), "unknown");
    assert_eq!(cat_pg.resolve_type("pg_catalog.vector"), "number[]");

    // 2. Test Bun driver variations
    let mut cat_bun = Catalog::new(DriverTarget::Bun);
    cat_bun.apply_sql(sql).unwrap();
    assert_eq!(cat_bun.resolve_type("pg_catalog.int8"), "bigint"); // Bun driver -> bigint
    assert_eq!(cat_bun.resolve_type("pg_catalog.bytea"), "Uint8Array"); // Bun driver -> Uint8Array

    // 3. Enum resolution across schemas
    assert_eq!(cat_pg.resolve_type("custom_role"), "\"admin\" | \"viewer\"");
    assert_eq!(cat_pg.resolve_type("public.custom_role"), "\"admin\" | \"viewer\"");
    assert_eq!(cat_pg.resolve_type("audit.custom_role"), "\"auditor\" | \"secops\"");
    assert_eq!(cat_pg.resolve_type("pg_catalog.custom_role"), "unknown"); // pg_catalog does not have custom_role!

    // 4. Composite type resolution across schemas
    assert_eq!(cat_pg.resolve_type("geo_coord"), "{ lat: number; lng: number }");
    assert_eq!(cat_pg.resolve_type("public.geo_coord"), "{ lat: number; lng: number }");
    assert_eq!(cat_pg.resolve_type("custom_schema.geo_coord"), "{ x: number; y: number; z: number }");
    assert_eq!(cat_pg.resolve_type("geo_coord[]"), "Array<{ lat: number; lng: number }>");
    assert_eq!(cat_pg.resolve_type("custom_schema.geo_coord[]"), "Array<{ x: number; y: number; z: number }>");
    assert_eq!(cat_pg.resolve_type("pg_catalog.geo_coord"), "unknown");

    // 5. Domain resolution across schemas
    assert_eq!(cat_pg.resolve_type("phone_num"), "string");
    assert_eq!(cat_pg.resolve_type("public.phone_num"), "string");
    assert_eq!(cat_pg.resolve_type("custom_schema.phone_num"), "string");
    assert_eq!(cat_pg.resolve_type("phone_num[]"), "string[]");
    assert_eq!(cat_pg.resolve_type("pg_catalog.phone_num"), "unknown");

    // 6. Unknown types in foreign schemas
    assert_eq!(cat_pg.resolve_type("unknown_schema.some_type"), "unknown");
}

#[test]
fn test_nested_composite_types() {
    let sql = "
        CREATE TYPE point_2d AS (x float8, y float8);
        CREATE TYPE shape_metadata AS (name text, center point_2d);
        CREATE TABLE shapes (
            id INT PRIMARY KEY,
            meta shape_metadata,
            points point_2d[]
        );
    ";

    let mut catalog = Catalog::default();
    catalog.apply_sql(sql).unwrap();

    let shapes = catalog.get_table("shapes").unwrap();
    let meta_col = shapes.get_column("meta").unwrap();
    assert_eq!(
        meta_col.ts_type,
        "{ name: string; center: { x: number; y: number } }"
    );

    let points_col = shapes.get_column("points").unwrap();
    assert_eq!(
        points_col.ts_type,
        "Array<{ x: number; y: number }>"
    );
}

#[test]
fn test_domain_transitive_inheritance_boundary() {
    let sql = "
        CREATE DOMAIN code_level_1 AS VARCHAR(10) NOT NULL;
        CREATE DOMAIN code_level_2 AS code_level_1 DEFAULT 'DEFAULT_CODE';
        CREATE TABLE direct_holder (
            id INT PRIMARY KEY,
            code code_level_1
        );
        CREATE TABLE transitive_holder (
            id INT PRIMARY KEY,
            code code_level_2
        );
    ";

    let mut catalog = Catalog::default();
    catalog.apply_sql(sql).unwrap();

    // 1. Direct domain inheritance: NOT NULL is properly inherited
    let direct = catalog.get_table("direct_holder").unwrap();
    let direct_col = direct.get_column("code").unwrap();
    assert_eq!(direct_col.ts_type, "string");
    assert!(!direct_col.is_nullable, "Direct domain column must inherit NOT NULL");

    // 2. Transitive domain boundary:
    // code_level_2 resolves base ts_type to string and has DEFAULT
    let trans = catalog.get_table("transitive_holder").unwrap();
    let trans_col = trans.get_column("code").unwrap();
    assert_eq!(trans_col.ts_type, "string");
    assert!(trans_col.has_default, "Transitive domain column preserves DEFAULT");

    // FINDING: handle_create_domain_stmt only checks AST constraints for the current domain;
    // it does not look up get_domain(base_type) to recursively inherit NOT NULL.
    // Therefore, code_level_2 is not flagged is_not_null unless explicitly specified.
    let d2 = catalog.get_domain("code_level_2").unwrap();
    assert!(!d2.is_not_null, "Transitive domain constraint inheritance is non-recursive in M2");
    assert!(trans_col.is_nullable);
}

#[test]
fn test_alter_table_comprehensive_lifecycle() {
    let sql = "
        CREATE TABLE lifecycle (
            id INT,
            val TEXT
        );
    ";
    let mut catalog = Catalog::default();
    catalog.apply_sql(sql).unwrap();

    // 1. ADD COLUMN
    catalog.apply_sql("ALTER TABLE lifecycle ADD COLUMN count INT;").unwrap();
    let t = catalog.get_table("lifecycle").unwrap();
    assert_eq!(t.columns.len(), 3);
    assert!(t.get_column("count").unwrap().is_nullable);

    // 2. ALTER COLUMN TYPE
    catalog.apply_sql("ALTER TABLE lifecycle ALTER COLUMN count TYPE numeric;").unwrap();
    let t = catalog.get_table("lifecycle").unwrap();
    assert_eq!(t.get_column("count").unwrap().ts_type, "number");

    // 3. SET NOT NULL
    catalog.apply_sql("ALTER TABLE lifecycle ALTER COLUMN count SET NOT NULL;").unwrap();
    let t = catalog.get_table("lifecycle").unwrap();
    assert!(!t.get_column("count").unwrap().is_nullable);

    // 4. DROP NOT NULL
    catalog.apply_sql("ALTER TABLE lifecycle ALTER COLUMN count DROP NOT NULL;").unwrap();
    let t = catalog.get_table("lifecycle").unwrap();
    assert!(t.get_column("count").unwrap().is_nullable);

    // 5. SET DEFAULT
    catalog.apply_sql("ALTER TABLE lifecycle ALTER COLUMN count SET DEFAULT 0;").unwrap();
    let t = catalog.get_table("lifecycle").unwrap();
    assert!(t.get_column("count").unwrap().has_default);

    // 6. DROP COLUMN
    catalog.apply_sql("ALTER TABLE lifecycle DROP COLUMN count;").unwrap();
    let t = catalog.get_table("lifecycle").unwrap();
    assert_eq!(t.columns.len(), 2);
    assert!(t.get_column("count").is_none());

    // 7. ADD PRIMARY KEY
    catalog.apply_sql("ALTER TABLE lifecycle ADD PRIMARY KEY (id);").unwrap();
    let t = catalog.get_table("lifecycle").unwrap();
    assert_eq!(t.primary_keys, vec!["id"]);
    assert!(t.get_column("id").unwrap().is_primary_key);
    assert!(!t.get_column("id").unwrap().is_nullable);
}

