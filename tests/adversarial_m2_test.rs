use sqltype::analyzer::analyze_query;
use sqltype::catalog::{Catalog, DriverTarget, QualifiedTypeName};
use sqltype::codegen::generate_file_ts;

#[test]
fn test_stress_composite_types_nested_and_arrays() {
    let mut catalog = Catalog::default();
    let sql = r#"
        CREATE TYPE point_2d AS (x double precision, y double precision);
        CREATE TYPE bounding_box AS (min_pt point_2d, max_pt point_2d);
        CREATE TYPE polygon AS (vertices point_2d[], label text);
        CREATE TABLE shapes (
            id UUID PRIMARY KEY,
            center point_2d NOT NULL,
            bbox bounding_box,
            polys polygon[]
        );
    "#;
    catalog.apply_sql(sql).expect("Failed to apply DDL");

    // 1. Basic composite type resolution
    assert_eq!(catalog.resolve_type("point_2d"), "{ x: number; y: number }");
    assert_eq!(
        catalog.resolve_type("point_2d[]"),
        "Array<{ x: number; y: number }>"
    );
    assert_eq!(
        catalog.resolve_type("point_2d[][]"),
        "Array<{ x: number; y: number }>[]"
    );

    // 2. Nested composite type resolution
    assert_eq!(
        catalog.resolve_type("bounding_box"),
        "{ min_pt: { x: number; y: number }; max_pt: { x: number; y: number } }"
    );
    assert_eq!(
        catalog.resolve_type("bounding_box[]"),
        "Array<{ min_pt: { x: number; y: number }; max_pt: { x: number; y: number } }>"
    );

    // 3. Composite type with internal array attribute
    assert_eq!(
        catalog.resolve_type("polygon"),
        "{ vertices: Array<{ x: number; y: number }>; label: string }"
    );
    assert_eq!(
        catalog.resolve_type("polygon[]"),
        "Array<{ vertices: Array<{ x: number; y: number }>; label: string }>"
    );

    // 4. Table column verification
    let table = catalog
        .get_table("shapes")
        .expect("shapes table must exist");
    let center = table.get_column("center").unwrap();
    assert_eq!(center.ts_type, "{ x: number; y: number }");
    assert!(!center.is_nullable);

    let bbox = table.get_column("bbox").unwrap();
    assert_eq!(
        bbox.ts_type,
        "{ min_pt: { x: number; y: number }; max_pt: { x: number; y: number } }"
    );
    assert!(bbox.is_nullable);

    let polys = table.get_column("polys").unwrap();
    assert_eq!(
        polys.ts_type,
        "Array<{ vertices: Array<{ x: number; y: number }>; label: string }>"
    );
    assert!(polys.is_nullable);
}

#[test]
fn test_stress_composite_query_analysis_and_codegen() {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TYPE coordinate AS (lat float8, lng float8);
        CREATE TYPE route AS (name text, waypoints coordinate[]);
        CREATE TABLE trips (
            id serial PRIMARY KEY,
            destination coordinate NOT NULL,
            active_route route
        );
    "#;
    catalog.apply_sql(ddl).unwrap();

    let query = "SELECT id, destination, active_route FROM trips;";
    let analyzed = analyze_query(query, &catalog, Some("get_trips.sql")).unwrap();

    assert_eq!(analyzed.name, "GetTrips");
    assert_eq!(analyzed.fields.len(), 3);

    // id is serial primary key -> number
    assert_eq!(analyzed.fields[0].name, "id");
    assert_eq!(analyzed.fields[0].ts_type, "number");

    // destination is NOT NULL coordinate
    assert_eq!(analyzed.fields[1].name, "destination");
    assert_eq!(analyzed.fields[1].ts_type, "{ lat: number; lng: number }");

    // active_route is nullable route
    assert_eq!(analyzed.fields[2].name, "active_route");
    assert_eq!(
        analyzed.fields[2].ts_type,
        "{ name: string; waypoints: Array<{ lat: number; lng: number }> } | null"
    );

    // Code generation output check
    let ts = generate_file_ts(&[analyzed]);
    assert!(
        ts.contains("destination: { lat: number; lng: number };"),
        "Generated TypeScript must contain destination with exact interface:\n{}",
        ts
    );
    assert!(
        ts.contains(
            "active_route: { name: string; waypoints: Array<{ lat: number; lng: number }> } | null;"
        ),
        "Generated TypeScript must contain active_route with exact interface | null:\n{}",
        ts
    );
}

#[test]
fn test_stress_composite_types_driver_variants() {
    let sql = r#"
        CREATE TYPE device_stat AS (device_id bigint, raw_payload bytea, recorded_at timestamptz);
        CREATE TABLE telemetry (
            id serial PRIMARY KEY,
            stat device_stat NOT NULL
        );
    "#;

    // Postgres driver target: bigint -> string, bytea -> Buffer
    let mut pg_cat = Catalog::new(DriverTarget::Postgres);
    pg_cat.apply_sql(sql).unwrap();
    assert_eq!(
        pg_cat.resolve_type("device_stat"),
        "{ device_id: string; raw_payload: Buffer; recorded_at: Date }"
    );

    // Bun driver target: bigint -> bigint, bytea -> Uint8Array
    let mut bun_cat = Catalog::new(DriverTarget::Bun);
    bun_cat.apply_sql(sql).unwrap();
    assert_eq!(
        bun_cat.resolve_type("device_stat"),
        "{ device_id: bigint; raw_payload: Uint8Array; recorded_at: Date }"
    );
}

#[test]
fn test_stress_composite_types_property_escaping() {
    let mut catalog = Catalog::default();
    let sql = r#"
        CREATE TYPE special_props AS (
            "k-e-y" text,
            "123numeric" int,
            "valid_ident" bool,
            "with space" float8
        );
    "#;
    catalog.apply_sql(sql).unwrap();

    let expected =
        r#"{ "k-e-y": string; "123numeric": number; valid_ident: boolean; "with space": number }"#;
    assert_eq!(catalog.resolve_type("special_props"), expected);
}

#[test]
fn test_stress_domain_types_and_inheritance() {
    let mut catalog = Catalog::default();
    let sql = r#"
        CREATE DOMAIN non_empty_name AS varchar(100) NOT NULL;
        CREATE DOMAIN priority_level AS integer DEFAULT 10;
        CREATE DOMAIN strict_status AS text NOT NULL DEFAULT 'pending';
        CREATE DOMAIN nullable_tag AS text;
        CREATE TABLE tasks (
            id uuid PRIMARY KEY,
            title non_empty_name,
            priority priority_level,
            status strict_status,
            tag nullable_tag,
            explicit_null_override non_empty_name
        );
    "#;
    catalog.apply_sql(sql).unwrap();

    let table = catalog.get_table("tasks").unwrap();

    // title inherits NOT NULL
    let title = table.get_column("title").unwrap();
    assert_eq!(title.ts_type, "string");
    assert!(!title.is_nullable);
    assert!(!title.has_default);

    // priority inherits DEFAULT
    let priority = table.get_column("priority").unwrap();
    assert_eq!(priority.ts_type, "number");
    assert!(priority.is_nullable);
    assert!(priority.has_default);

    // status inherits both NOT NULL and DEFAULT
    let status = table.get_column("status").unwrap();
    assert_eq!(status.ts_type, "string");
    assert!(!status.is_nullable);
    assert!(status.has_default);

    // tag has neither NOT NULL nor DEFAULT
    let tag = table.get_column("tag").unwrap();
    assert_eq!(tag.ts_type, "string");
    assert!(tag.is_nullable);
    assert!(!tag.has_default);
}

#[test]
fn test_stress_domain_over_composite_and_arrays() {
    let mut catalog = Catalog::default();
    let sql = r#"
        CREATE TYPE money_spec AS (amount numeric, currency varchar(3));
        CREATE DOMAIN not_null_money AS money_spec NOT NULL;
        CREATE DOMAIN money_list AS money_spec[];
        CREATE TABLE ledger (
            id serial PRIMARY KEY,
            entry not_null_money,
            history money_list
        );
    "#;
    catalog.apply_sql(sql).unwrap();

    assert_eq!(
        catalog.resolve_type("not_null_money"),
        "{ amount: number; currency: string }"
    );
    assert_eq!(
        catalog.resolve_type("money_list"),
        "Array<{ amount: number; currency: string }>"
    );

    let table = catalog.get_table("ledger").unwrap();
    let entry = table.get_column("entry").unwrap();
    assert_eq!(entry.ts_type, "{ amount: number; currency: string }");
    assert!(!entry.is_nullable);

    let history = table.get_column("history").unwrap();
    assert_eq!(
        history.ts_type,
        "Array<{ amount: number; currency: string }>"
    );
    assert!(history.is_nullable);
}

#[test]
fn test_stress_domain_with_enums() {
    let mut catalog = Catalog::default();
    let sql = r#"
        CREATE TYPE state_enum AS ENUM ('init', 'running', 'stopped');
        CREATE DOMAIN active_state AS state_enum NOT NULL;
        CREATE TABLE processes (
            pid int PRIMARY KEY,
            current_state active_state
        );
    "#;
    catalog.apply_sql(sql).unwrap();

    assert_eq!(
        catalog.resolve_type("active_state"),
        "\"init\" | \"running\" | \"stopped\""
    );

    let table = catalog.get_table("processes").unwrap();
    let state_col = table.get_column("current_state").unwrap();
    assert_eq!(state_col.ts_type, "\"init\" | \"running\" | \"stopped\"");
    assert!(!state_col.is_nullable);
}

#[test]
fn test_stress_primary_key_tracking_variations() {
    let mut catalog = Catalog::default();
    let sql = r#"
        CREATE TABLE pk_single (
            id bigint PRIMARY KEY,
            val text
        );
        CREATE TABLE pk_composite (
            tenant_id uuid NOT NULL,
            account_id int NOT NULL,
            display_name text,
            CONSTRAINT pk_composite_tenant_account PRIMARY KEY (tenant_id, account_id)
        );
        CREATE TABLE pk_alter (
            org_id uuid NOT NULL,
            project_id int NOT NULL,
            metadata jsonb
        );
    "#;
    catalog.apply_sql(sql).unwrap();

    // 1. Single inline PK
    let single = catalog.get_table("pk_single").unwrap();
    assert_eq!(single.primary_keys, vec!["id"]);
    assert!(single.get_column("id").unwrap().is_primary_key);
    assert!(!single.get_column("id").unwrap().is_nullable);
    assert!(!single.get_column("val").unwrap().is_primary_key);

    // 2. Composite table constraint PK
    let comp = catalog.get_table("pk_composite").unwrap();
    assert_eq!(comp.primary_keys, vec!["tenant_id", "account_id"]);
    assert!(comp.get_column("tenant_id").unwrap().is_primary_key);
    assert!(!comp.get_column("tenant_id").unwrap().is_nullable);
    assert!(comp.get_column("account_id").unwrap().is_primary_key);
    assert!(!comp.get_column("account_id").unwrap().is_nullable);
    assert!(!comp.get_column("display_name").unwrap().is_primary_key);

    // 3. ALTER TABLE ADD CONSTRAINT PRIMARY KEY
    catalog
        .apply_sql(
            "ALTER TABLE pk_alter ADD CONSTRAINT pk_alter_const PRIMARY KEY (org_id, project_id);",
        )
        .unwrap();
    let alter = catalog.get_table("pk_alter").unwrap();
    assert_eq!(alter.primary_keys, vec!["org_id", "project_id"]);
    assert!(alter.get_column("org_id").unwrap().is_primary_key);
    assert!(!alter.get_column("org_id").unwrap().is_nullable);
    assert!(alter.get_column("project_id").unwrap().is_primary_key);
    assert!(!alter.get_column("project_id").unwrap().is_nullable);

    // 4. Drop column that was part of PK
    catalog
        .apply_sql("ALTER TABLE pk_alter DROP COLUMN project_id;")
        .unwrap();
    let alter_after_drop = catalog.get_table("pk_alter").unwrap();
    assert_eq!(alter_after_drop.primary_keys, vec!["org_id"]);
    assert!(alter_after_drop.get_column("project_id").is_none());
}

#[test]
fn test_stress_strict_schema_qualification() {
    let mut catalog = Catalog::default();
    let sql = r#"
        CREATE TYPE custom_app.status AS ENUM ('open', 'closed');
        CREATE TYPE custom_app.geo_loc AS (lat float8, lon float8);
        CREATE DOMAIN custom_app.short_code AS varchar(5) NOT NULL;

        CREATE TABLE custom_app.tickets (
            ticket_id uuid PRIMARY KEY,
            state custom_app.status NOT NULL,
            loc custom_app.geo_loc,
            code custom_app.short_code
        );

        CREATE TABLE public.tickets (
            id serial PRIMARY KEY,
            title text NOT NULL
        );
    "#;
    catalog.apply_sql(sql).unwrap();

    // 1. Strict table resolution
    let custom_ticket = catalog.get_table("custom_app.tickets").unwrap();
    assert_eq!(custom_ticket.schema.as_deref(), Some("custom_app"));
    assert_eq!(custom_ticket.columns.len(), 4);
    assert!(custom_ticket.get_column("ticket_id").is_some());

    let public_ticket = catalog.get_table("tickets").unwrap();
    assert_eq!(public_ticket.schema.as_deref(), Some("public"));
    assert_eq!(public_ticket.columns.len(), 2);
    assert!(public_ticket.get_column("title").is_some());

    // 2. Negative lookups
    assert!(catalog.get_table("pg_catalog.tickets").is_none());
    assert!(catalog.get_table("other.tickets").is_none());
    assert!(
        catalog
            .get_table_qualified(Some("other"), "tickets")
            .is_none()
    );

    // 3. Schema-qualified type resolution
    assert_eq!(
        catalog.resolve_type("custom_app.status"),
        "\"open\" | \"closed\""
    );
    assert_eq!(
        catalog.resolve_type("custom_app.geo_loc"),
        "{ lat: number; lon: number }"
    );
    assert_eq!(
        catalog.resolve_type("custom_app.geo_loc[]"),
        "Array<{ lat: number; lon: number }>"
    );
    assert_eq!(catalog.resolve_type("custom_app.short_code"), "string");

    // 4. pg_catalog built-in immunity against custom shadowing
    assert_eq!(catalog.resolve_type("pg_catalog.int4"), "number");
    assert_eq!(catalog.resolve_type("pg_catalog.text"), "string");
    assert_eq!(catalog.resolve_type("pg_catalog.bool"), "boolean");
    assert_eq!(catalog.resolve_type("pg_catalog.timestamptz"), "Date");

    // QualifiedTypeName helper
    let q_int4 = QualifiedTypeName {
        schema: Some("pg_catalog".to_string()),
        name: "int4".to_string(),
        is_array: false,
        typmod: None,
    };
    assert_eq!(catalog.resolve_qualified_type(&q_int4), "number");

    let q_int4_arr = QualifiedTypeName {
        schema: Some("pg_catalog".to_string()),
        name: "int4".to_string(),
        is_array: true,
        typmod: None,
    };
    assert_eq!(catalog.resolve_qualified_type(&q_int4_arr), "number[]");
}

#[test]
fn test_adversarial_alter_table_cross_schema_pollution() {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql("CREATE TABLE users (id int, name text);")
        .unwrap();

    let users_before = catalog.get_table("users").unwrap();
    assert_eq!(users_before.columns.len(), 2);

    // Now alter other_schema.users (does not exist in catalog)
    catalog
        .apply_sql("ALTER TABLE other_schema.users ADD COLUMN hacked text;")
        .unwrap();

    let users_after = catalog.get_table("users").unwrap();
    let has_hacked = users_after.get_column("hacked").is_some();
    // Catalog hardening verification:
    // In src/catalog/mod.rs:804, if `other_schema.users` does not exist,
    // it must not fall back to mutating `public.users`.
    assert!(
        !has_hacked,
        "Hardened catalog: ALTER TABLE other_schema.users must not mutate public.users"
    );
}
