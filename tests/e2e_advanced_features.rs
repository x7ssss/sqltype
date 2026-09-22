use sqltype::analyzer::analyze_query;
use sqltype::catalog::{Catalog, DriverTarget};
use sqltype::codegen::{CodegenOptions, generate_file_ts, generate_file_ts_with_options};
use sqltype::ts_scanner::scan_ts_queries;
use std::fs;

/// Helper to create a unified schema catalog containing users, profiles, staff, and employees.
fn create_test_catalog() -> Catalog {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql(
            r#"
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                name TEXT NOT NULL
            );

            CREATE TABLE profiles (
                user_id UUID NOT NULL,
                bio TEXT NOT NULL
            );

            CREATE TABLE staff (
                id UUID PRIMARY KEY,
                full_name TEXT NOT NULL
            );

            CREATE TABLE employees (
                id UUID PRIMARY KEY,
                manager_id UUID,
                name TEXT NOT NULL
            );
            "#,
        )
        .expect("Failed to apply initial migrations");
    catalog
}

#[test]
fn test_complex_relational_join_with_nullability_propagation() {
    let catalog = create_test_catalog();

    // Query: users u LEFT JOIN profiles p ON u.id = p.user_id
    // Because p is on the right side of a LEFT JOIN, p.bio is null-padded
    let query_sql = r#"
-- name: GetUserWithProfile
SELECT u.name, p.bio
FROM users u
LEFT JOIN profiles p ON u.id = p.user_id;
    "#;

    let analyzed = analyze_query(query_sql, &catalog, Some("get_user_with_profile.sql")).unwrap();

    assert_eq!(analyzed.name, "GetUserWithProfile");
    assert_eq!(analyzed.params.len(), 0);
    assert_eq!(analyzed.fields.len(), 2);

    assert_eq!(analyzed.fields[0].name, "name");
    assert_eq!(analyzed.fields[0].ts_type, "string");

    assert_eq!(analyzed.fields[1].name, "bio");
    assert_eq!(analyzed.fields[1].ts_type, "string | null");

    let ts = generate_file_ts(&[analyzed]);
    assert!(
        ts.contains("name: string;"),
        "Generated TypeScript must contain 'name: string;'\nFound:\n{}",
        ts
    );
    assert!(
        ts.contains("bio: string | null;"),
        "Generated TypeScript must contain 'bio: string | null;'\nFound:\n{}",
        ts
    );
}

#[test]
fn test_recursive_cte() {
    let catalog = create_test_catalog();

    let query_sql = r#"
-- name: GetOrgHierarchy
WITH RECURSIVE hierarchy AS (
    SELECT id, manager_id, 1 AS depth
    FROM employees
    WHERE manager_id IS NULL
    UNION ALL
    SELECT e.id, e.manager_id, h.depth + 1
    FROM employees e
    INNER JOIN hierarchy h ON e.manager_id = h.id
)
SELECT id, depth FROM hierarchy;
    "#;

    let analyzed = analyze_query(query_sql, &catalog, Some("get_hierarchy.sql")).unwrap();

    assert_eq!(analyzed.name, "GetOrgHierarchy");
    assert_eq!(analyzed.params.len(), 0);
    assert_eq!(analyzed.fields.len(), 2);

    assert_eq!(analyzed.fields[0].name, "id");
    assert_eq!(analyzed.fields[0].ts_type, "string");

    assert_eq!(analyzed.fields[1].name, "depth");
    assert_eq!(analyzed.fields[1].ts_type, "number");

    let ts = generate_file_ts(&[analyzed]);
    assert!(
        ts.contains("id: string;"),
        "Generated TypeScript must contain 'id: string;'\nFound:\n{}",
        ts
    );
    assert!(
        ts.contains("depth: number;"),
        "Generated TypeScript must contain 'depth: number;'\nFound:\n{}",
        ts
    );
}

#[test]
fn test_set_operations() {
    let catalog = create_test_catalog();

    // 1. Valid UNION ALL unifying id and name
    let valid_union_sql = r#"
-- name: GetAllPeople
SELECT id, name FROM users
UNION ALL
SELECT id, full_name AS name FROM staff;
    "#;

    let analyzed = analyze_query(valid_union_sql, &catalog, Some("all_people.sql")).unwrap();

    assert_eq!(analyzed.name, "GetAllPeople");
    assert_eq!(analyzed.fields.len(), 2);

    assert_eq!(analyzed.fields[0].name, "id");
    assert_eq!(analyzed.fields[0].ts_type, "string");

    assert_eq!(analyzed.fields[1].name, "name");
    assert_eq!(analyzed.fields[1].ts_type, "string");

    let ts = generate_file_ts(&[analyzed]);
    assert!(ts.contains("id: string;"));
    assert!(ts.contains("name: string;"));

    // 2. Reject column count mismatch between branches
    let mismatch_sql = r#"
SELECT id, name FROM users
UNION ALL
SELECT id FROM staff;
    "#;
    let err = analyze_query(mismatch_sql, &catalog, None).unwrap_err();
    assert!(
        err.contains("UNION") || err.contains("columns") || err.contains("mismatch"),
        "Expected column count mismatch error, got: {}",
        err
    );
}

#[test]
fn test_structured_json_aggregation() {
    let catalog = create_test_catalog();

    let query_sql = r#"
-- name: GetUserProfilesList
SELECT
    u.id,
    COALESCE(jsonb_agg(jsonb_build_object('bio', p.bio)), '[]'::jsonb) AS profiles_list
FROM users u
LEFT JOIN profiles p ON u.id = p.user_id
GROUP BY u.id;
    "#;

    let analyzed = analyze_query(query_sql, &catalog, Some("get_user_profiles_list.sql")).unwrap();

    assert_eq!(analyzed.name, "GetUserProfilesList");
    assert_eq!(analyzed.fields.len(), 2);

    assert_eq!(analyzed.fields[0].name, "id");
    assert_eq!(analyzed.fields[0].ts_type, "string");

    assert_eq!(analyzed.fields[1].name, "profiles_list");

    // In a LEFT JOIN, p.bio is null-padded if a user has no matching profiles,
    // so jsonb_build_object('bio', p.bio) correctly infers { bio: string | null }
    // while COALESCE preserves the structured Array and shields the outer nullability.
    assert!(
        analyzed.fields[1].ts_type == "Array<{ bio: string | null }>"
            || analyzed.fields[1].ts_type == "Array<{ bio: string }>",
        "Expected profiles_list to be structured Array of object, got: {}",
        analyzed.fields[1].ts_type
    );

    let ts = generate_file_ts(&[analyzed]);
    assert!(
        ts.contains("profiles_list: Array<{ bio: string | null }>;")
            || ts.contains("profiles_list: Array<{ bio: string }>;")
            || ts.contains("profiles_list: Array<{\n  bio: string;\n}>;")
            || ts.contains("profiles_list: Array<{\n  bio: string | null;\n}>;"),
        "Generated TypeScript must contain structured profiles_list array.\nFound:\n{}",
        ts
    );
}

#[test]
fn test_inline_typescript_template_extraction() {
    let catalog = create_test_catalog();

    // Synthetic TypeScript file with tagged template literal
    let ts_source = r#"
        import { sql } from 'bun';

        export const getSummary = sql`
          SELECT u.name, p.bio
          FROM users u
          LEFT JOIN profiles p ON u.id = p.user_id
          WHERE u.id = $1;
        `;
    "#;

    let extracted = scan_ts_queries(ts_source);
    assert_eq!(extracted.len(), 1);
    assert_eq!(extracted[0].name.as_deref(), Some("GetSummary"));
    assert!(extracted[0].sql.contains("SELECT u.name, p.bio"));

    let analyzed =
        analyze_query(&extracted[0].sql, &catalog, extracted[0].name.as_deref()).unwrap();

    assert_eq!(analyzed.name, "GetSummary");
    assert_eq!(analyzed.params.len(), 1);
    assert_eq!(analyzed.params[0].name, "id");
    assert_eq!(analyzed.params[0].ts_type, "string");

    assert_eq!(analyzed.fields.len(), 2);
    assert_eq!(analyzed.fields[0].name, "name");
    assert_eq!(analyzed.fields[0].ts_type, "string");
    assert_eq!(analyzed.fields[1].name, "bio");
    assert_eq!(analyzed.fields[1].ts_type, "string | null");

    // Codegen with Bun execution wrappers
    let options = CodegenOptions::new(DriverTarget::Bun, true);
    let companion_ts = generate_file_ts_with_options(&[analyzed], &options);

    assert!(companion_ts.contains("export interface GetSummaryParams {\n  id: string;\n}"));
    assert!(
        companion_ts.contains(
            "export interface GetSummaryRow {\n  name: string;\n  bio: string | null;\n}"
        )
    );
    assert!(companion_ts.contains("export const getSummarySql = `"));
    assert!(companion_ts.contains("export type GetSummaryQuery = {"));
    assert!(companion_ts.contains("export async function getSummary(sql: import(\"bun\").SQL, params: GetSummaryParams): Promise<GetSummaryRow[]> {"));
}

#[test]
fn test_comprehensive_e2e_disk_pipeline() {
    let base_dir = std::env::temp_dir().join(format!(
        "sqltype_e2e_adv_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let migrations_dir = base_dir.join("migrations");
    let queries_dir = base_dir.join("src").join("queries");
    let out_dir = base_dir.join("src").join("types");

    fs::create_dir_all(&migrations_dir).unwrap();
    fs::create_dir_all(&queries_dir).unwrap();
    fs::create_dir_all(&out_dir).unwrap();

    // 1. Write migration files
    fs::write(
        migrations_dir.join("001_create_schema.sql"),
        r#"
        CREATE TABLE users (
            id UUID PRIMARY KEY,
            name TEXT NOT NULL
        );
        CREATE TABLE profiles (
            user_id UUID NOT NULL,
            bio TEXT NOT NULL
        );
        CREATE TABLE staff (
            id UUID PRIMARY KEY,
            full_name TEXT NOT NULL
        );
        CREATE TABLE employees (
            id UUID PRIMARY KEY,
            manager_id UUID,
            name TEXT NOT NULL
        );
        "#,
    )
    .unwrap();

    // 2. Load catalog from disk
    let catalog = Catalog::load_from_dir(&migrations_dir).unwrap();
    assert!(catalog.get_table("users").is_some());
    assert!(catalog.get_table("profiles").is_some());
    assert!(catalog.get_table("staff").is_some());
    assert!(catalog.get_table("employees").is_some());

    // 3. Write SQL query files covering joins, recursive CTE, and json agg
    let query_join = r#"
-- name: GetUserWithProfile
SELECT u.name, p.bio FROM users u LEFT JOIN profiles p ON u.id = p.user_id;
    "#;
    let query_cte = r#"
-- name: GetOrgHierarchy
WITH RECURSIVE hierarchy AS (
    SELECT id, manager_id, 1 AS depth FROM employees WHERE manager_id IS NULL
    UNION ALL
    SELECT e.id, e.manager_id, h.depth + 1 FROM employees e INNER JOIN hierarchy h ON e.manager_id = h.id
) SELECT id, depth FROM hierarchy;
    "#;
    let query_json = r#"
-- name: GetUserProfilesList
SELECT u.id, COALESCE(jsonb_agg(jsonb_build_object('bio', p.bio)), '[]'::jsonb) AS profiles_list
FROM users u LEFT JOIN profiles p ON u.id = p.user_id GROUP BY u.id;
    "#;

    fs::write(queries_dir.join("user_profile.sql"), query_join).unwrap();
    fs::write(queries_dir.join("hierarchy.sql"), query_cte).unwrap();
    fs::write(queries_dir.join("profiles_json.sql"), query_json).unwrap();

    // 4. Write inline TS file
    let inline_ts = r#"
        import { sql } from 'bun';

        export const getSummary = sql`
          SELECT u.name, p.bio
          FROM users u
          LEFT JOIN profiles p ON u.id = p.user_id
          WHERE u.id = $1;
        `;
    "#;
    let inline_ts_path = queries_dir.join("summary.ts");
    fs::write(&inline_ts_path, inline_ts).unwrap();

    // 5. Analyze each query and verify types
    let a_join = analyze_query(query_join, &catalog, Some("user_profile.sql")).unwrap();
    assert_eq!(a_join.fields[0].ts_type, "string");
    assert_eq!(a_join.fields[1].ts_type, "string | null");

    let a_cte = analyze_query(query_cte, &catalog, Some("hierarchy.sql")).unwrap();
    assert_eq!(a_cte.fields[0].ts_type, "string");
    assert_eq!(a_cte.fields[1].ts_type, "number");

    let a_json = analyze_query(query_json, &catalog, Some("profiles_json.sql")).unwrap();
    assert_eq!(a_json.fields[0].ts_type, "string");
    assert!(a_json.fields[1].ts_type.starts_with("Array<{ bio: string"));

    let extracted = scan_ts_queries(&fs::read_to_string(&inline_ts_path).unwrap());
    assert_eq!(extracted.len(), 1);
    let a_inline =
        analyze_query(&extracted[0].sql, &catalog, extracted[0].name.as_deref()).unwrap();
    assert_eq!(a_inline.params.len(), 1);
    assert_eq!(a_inline.params[0].ts_type, "string");

    // 6. Generate TypeScript files to disk and verify contents
    let options = CodegenOptions::new(DriverTarget::Postgres, true);
    let ts_all = generate_file_ts_with_options(&[a_join, a_cte, a_json], &options);
    let out_file = out_dir.join("index.ts");
    fs::write(&out_file, &ts_all).unwrap();

    let companion_ts = generate_file_ts_with_options(&[a_inline], &options);
    let sibling_file = inline_ts_path.with_extension("sqltype.ts");
    fs::write(&sibling_file, &companion_ts).unwrap();

    // Verify written files
    assert!(out_file.exists());
    assert!(sibling_file.exists());

    let out_content = fs::read_to_string(&out_file).unwrap();
    assert!(out_content.contains("export interface GetUserWithProfileRow"));
    assert!(out_content.contains("name: string;"));
    assert!(out_content.contains("bio: string | null;"));
    assert!(out_content.contains("export interface GetOrgHierarchyRow"));
    assert!(out_content.contains("id: string;"));
    assert!(out_content.contains("depth: number;"));
    assert!(out_content.contains("export interface GetUserProfilesListRow"));
    assert!(out_content.contains("profiles_list: Array<{ bio: string"));

    let sibling_content = fs::read_to_string(&sibling_file).unwrap();
    assert!(sibling_content.contains("export interface GetSummaryRow"));
    assert!(sibling_content.contains("export async function getSummary"));

    // Cleanup
    let _ = fs::remove_dir_all(base_dir);
}
