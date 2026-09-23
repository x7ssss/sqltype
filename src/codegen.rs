use crate::analyzer::{AnalyzedQuery, PgType};
use crate::catalog::{Catalog, DriverTarget};

/// Renders a PgType into its corresponding TypeScript representation.
pub fn render_pg_type_to_ts(pg_type: &PgType, catalog: &Catalog) -> String {
    pg_type.to_ts(catalog)
}

/// Converts PascalCase or general string to camelCase.
pub fn to_camel_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut result = first.to_lowercase().to_string();
            result.extend(chars);
            result
        }
    }
}

/// Checks if a string is a valid JavaScript / TypeScript identifier.
pub fn is_valid_js_identifier(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_alphabetic() && first != '_' && first != '$' {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

pub fn format_property_key(name: &str) -> String {
    if is_valid_js_identifier(name) {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\\\""))
    }
}

fn sanitize_sql_template(sql: &str) -> String {
    // Strip leading comment lines like '-- name: ...' and leading empty lines
    let mut content_lines = Vec::new();
    let mut skipping_leading = true;

    for line in sql.lines() {
        let trimmed = line.trim();
        if skipping_leading {
            if trimmed.is_empty() || trimmed.starts_with("--") {
                continue;
            }
            skipping_leading = false;
        }
        content_lines.push(line);
    }

    let joined = content_lines.join("\n");
    // Escape backticks and ${} template expressions
    joined.replace('`', "\\`").replace("${", "\\${")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CodegenOptions {
    pub driver: DriverTarget,
    pub wrappers: bool,
}

impl CodegenOptions {
    pub fn new(driver: DriverTarget, wrappers: bool) -> Self {
        Self { driver, wrappers }
    }
}

/// Formats the argument expressions to pass to the query execution wrapper in strict $1, $2, ... numerical order.
fn format_wrapper_param_args(query: &AnalyzedQuery) -> String {
    let mut sorted_params = query.params.clone();
    sorted_params.sort_by_key(|p| p.index);

    let args: Vec<String> = sorted_params
        .iter()
        .map(|param| {
            let key = if is_valid_js_identifier(&param.name) {
                format!("params.{}", param.name)
            } else {
                format!("params[\"{}\"]", param.name.replace('"', "\\\""))
            };
            if param.is_optional {
                format!("{} ?? null", key)
            } else {
                key
            }
        })
        .collect();

    args.join(", ")
}

/// Generates a typed query execution wrapper function for a single analyzed query.
pub fn generate_query_wrapper(query: &AnalyzedQuery, driver: DriverTarget) -> String {
    let name = &query.name;
    let camel_name = to_camel_case(name);
    let sql_const_name = format!("{}Sql", camel_name);

    let has_rows = !query.fields.is_empty();
    let has_params = !query.params.is_empty();

    let client_arg = match driver {
        DriverTarget::Postgres => "sql: postgres.Sql",
        DriverTarget::Pg => "client: pg.ClientBase | pg.Pool",
        DriverTarget::Bun => "sql: import(\"bun\").SQL",
    };

    let params_arg = if has_params {
        format!(", params: {}Params", name)
    } else {
        String::new()
    };

    let return_type = if has_rows {
        format!("Promise<{}Row[]>", name)
    } else {
        "Promise<void>".to_string()
    };

    let args_str = format_wrapper_param_args(query);

    let mut out = String::new();
    let full_args = format!("{}{}", client_arg, params_arg);
    out.push_str(&format!(
        "export async function {}({}): {} {{\n",
        camel_name, full_args, return_type
    ));

    match driver {
        DriverTarget::Postgres => {
            if has_rows {
                out.push_str(&format!(
                    "  return await sql<{}Row[]>`${{sql.unsafe({}, [{}])}}`;\n",
                    name, sql_const_name, args_str
                ));
            } else {
                out.push_str(&format!(
                    "  await sql.unsafe({}, [{}]);\n",
                    sql_const_name, args_str
                ));
            }
        }
        DriverTarget::Pg => {
            if has_rows {
                out.push_str(&format!(
                    "  const res = await client.query<{}Row>({}, [{}]);\n  return res.rows;\n",
                    name, sql_const_name, args_str
                ));
            } else {
                out.push_str(&format!(
                    "  await client.query({}, [{}]);\n",
                    sql_const_name, args_str
                ));
            }
        }
        DriverTarget::Bun => {
            if has_rows {
                out.push_str(&format!(
                    "  return await sql<{}Row[]>`${{sql.raw({}, [{}])}}`;\n",
                    name, sql_const_name, args_str
                ));
            } else {
                out.push_str(&format!(
                    "  await sql.raw({}, [{}]);\n",
                    sql_const_name, args_str
                ));
            }
        }
    }

    out.push_str("}\n");
    out
}

/// Generates TypeScript code for a single analyzed query.
pub fn generate_query_ts(query: &AnalyzedQuery) -> String {
    generate_query_ts_with_options(query, &CodegenOptions::default())
}

/// Generates TypeScript code for a single analyzed query with specified options.
pub fn generate_query_ts_with_options(query: &AnalyzedQuery, options: &CodegenOptions) -> String {
    let name = &query.name;
    let camel_name = to_camel_case(name);
    let sql_const_name = format!("{}Sql", camel_name);

    let mut out = String::new();

    // 1. Params Interface
    out.push_str(&format!("export interface {}Params {{\n", name));
    for param in &query.params {
        let key = format_property_key(&param.name);
        if param.is_optional {
            let ts_type = if param.ts_type.contains("| null") {
                param.ts_type.clone()
            } else {
                format!("{} | null", param.ts_type)
            };
            out.push_str(&format!("  {}?: {};\n", key, ts_type));
        } else {
            out.push_str(&format!("  {}: {};\n", key, param.ts_type));
        }
    }
    out.push_str("}\n\n");

    // 2. Row Interface
    let has_row = !query.fields.is_empty();
    if has_row {
        out.push_str(&format!("export interface {}Row {{\n", name));
        for field in &query.fields {
            let key = format_property_key(&field.name);
            out.push_str(&format!("  {}: {};\n", key, field.ts_type));
        }
        out.push_str("}\n\n");
    }

    // 3. SQL string constant
    let clean_sql = sanitize_sql_template(&query.raw_sql);
    let clean_sql_trimmed = clean_sql.trim();
    out.push_str(&format!(
        "export const {} = `\n  {}\n`;\n\n",
        sql_const_name,
        clean_sql_trimmed.lines().collect::<Vec<_>>().join("\n  ")
    ));

    // 4. Query Type
    out.push_str(&format!("export type {}Query = {{\n", name));
    out.push_str("  sql: string;\n");
    out.push_str(&format!("  params: {}Params;\n", name));
    if has_row {
        out.push_str(&format!("  row: {}Row;\n", name));
    }
    out.push_str("};\n");

    // 5. Execution Wrapper (if enabled)
    if options.wrappers {
        out.push('\n');
        out.push_str(&generate_query_wrapper(query, options.driver));
    }

    out
}

/// Generates a complete TypeScript file for one or more analyzed queries.
pub fn generate_file_ts(queries: &[AnalyzedQuery]) -> String {
    generate_file_ts_with_options(queries, &CodegenOptions::default())
}

/// Generates a complete TypeScript file for one or more analyzed queries with specified options.
pub fn generate_file_ts_with_options(
    queries: &[AnalyzedQuery],
    options: &CodegenOptions,
) -> String {
    let mut out = String::new();
    out.push_str("// Autogenerated by sqltype. DO NOT EDIT.\n\n");

    for (i, query) in queries.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&generate_query_ts_with_options(query, options));
    }

    out
}

/// Helper to determine the output file name for a given query source file.
/// For TS/JS files (.ts, .tsx, .js), returns sibling declaration file with ".sqltype.ts".
/// For .sql files, returns file with ".ts".
pub fn get_output_file_name(source_file: &std::path::Path) -> std::path::PathBuf {
    if crate::ts_scanner::is_ts_js_file(source_file) {
        source_file.with_extension("sqltype.ts")
    } else {
        source_file.with_extension("ts")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{QueryField, QueryParam};

    #[test]
    fn test_codegen_matches_prompt_spec() {
        let query = AnalyzedQuery {
            name: "GetUserWithPosts".to_string(),
            raw_sql: "
-- name: GetUserWithPosts
SELECT 
  u.id, 
  u.email, 
  p.title AS post_title,
  COUNT(c.id) AS comment_count
FROM users u
LEFT JOIN posts p ON p.user_id = u.id
LEFT JOIN comments c ON c.post_id = p.id
WHERE u.id = $1
GROUP BY u.id, u.email, p.title;
"
            .to_string(),
            params: vec![QueryParam {
                index: 1,
                name: "id".to_string(),
                ts_type: "string".to_string(),
                is_optional: false,
            }],
            fields: vec![
                QueryField {
                    name: "id".to_string(),
                    ts_type: "string".to_string(),
                },
                QueryField {
                    name: "email".to_string(),
                    ts_type: "string".to_string(),
                },
                QueryField {
                    name: "post_title".to_string(),
                    ts_type: "string | null".to_string(),
                },
                QueryField {
                    name: "comment_count".to_string(),
                    ts_type: "number".to_string(),
                },
            ],
        };

        let ts = generate_file_ts(&[query]);
        println!("{}", ts);

        assert!(ts.contains("// Autogenerated by sqltype. DO NOT EDIT."));
        assert!(ts.contains("export interface GetUserWithPostsParams {\n  id: string;\n}"));
        assert!(ts.contains(
            "export interface GetUserWithPostsRow {\n  id: string;\n  email: string;\n  post_title: string | null;\n  comment_count: number;\n}"
        ));
        assert!(ts.contains("export const getUserWithPostsSql = `"));
        assert!(ts.contains("export type GetUserWithPostsQuery = {"));
        assert!(ts.contains("  params: GetUserWithPostsParams;"));
        assert!(ts.contains("  row: GetUserWithPostsRow;"));
    }

    #[test]
    fn test_empty_params() {
        let query = AnalyzedQuery {
            name: "GetAllUsers".to_string(),
            raw_sql: "SELECT id FROM users;".to_string(),
            params: vec![],
            fields: vec![QueryField {
                name: "id".to_string(),
                ts_type: "string".to_string(),
            }],
        };
        let ts = generate_file_ts(&[query]);
        assert!(ts.contains("export interface GetAllUsersParams {\n}"));
    }

    #[test]
    fn test_optional_param_codegen() {
        let mut catalog = crate::catalog::Catalog::default();
        catalog
            .apply_sql(
                "
                CREATE TABLE users (
                    id UUID PRIMARY KEY,
                    name TEXT NOT NULL
                );
            ",
            )
            .unwrap();

        let query_sql = "
            SELECT id, name FROM users WHERE ($1::text IS NULL OR name = $1) AND id = $2;
        ";
        let analyzed =
            crate::analyzer::analyze_query(query_sql, &catalog, Some("get_users_by_filter.sql"))
                .unwrap();
        let ts = generate_file_ts(&[analyzed]);

        assert!(ts.contains(
            "export interface GetUsersByFilterParams {\n  name?: string | null;\n  id: string;\n}"
        ));
    }

    #[test]
    fn test_driver_profiles_codegen() {
        use crate::catalog::{Catalog, DriverTarget};

        let ddl = "
            CREATE TABLE data_records (
                id BIGINT PRIMARY KEY,
                payload BYTEA NOT NULL,
                active_date DATE NOT NULL,
                created_at TIMESTAMPTZ NOT NULL
            );
        ";

        // Bun driver
        let mut bun_catalog = Catalog::new(DriverTarget::Bun);
        bun_catalog.apply_sql(ddl).unwrap();
        let bun_query = crate::analyzer::analyze_query(
            "SELECT id, payload, active_date, created_at FROM data_records;",
            &bun_catalog,
            Some("get_records.sql"),
        )
        .unwrap();
        let bun_ts = generate_file_ts(&[bun_query]);
        assert!(bun_ts.contains("id: bigint;"));
        assert!(bun_ts.contains("payload: Uint8Array;"));
        assert!(bun_ts.contains("active_date: string;"));
        assert!(bun_ts.contains("created_at: Date;"));

        // Postgres / pg driver
        let mut pg_catalog = Catalog::new(DriverTarget::Postgres);
        pg_catalog.apply_sql(ddl).unwrap();
        let pg_query = crate::analyzer::analyze_query(
            "SELECT id, payload, active_date, created_at FROM data_records;",
            &pg_catalog,
            Some("get_records.sql"),
        )
        .unwrap();
        let pg_ts = generate_file_ts(&[pg_query]);
        assert!(pg_ts.contains("id: string;"));
        assert!(pg_ts.contains("payload: Buffer;"));
        assert!(pg_ts.contains("active_date: string;"));
        assert!(pg_ts.contains("created_at: Date;"));
    }

    #[test]
    fn test_dml_codegen_insert_returning() {
        use crate::catalog::{Catalog, DriverTarget};

        let ddl = "
            CREATE TABLE users (
                id BIGINT PRIMARY KEY,
                name TEXT NOT NULL,
                email VARCHAR(255) NOT NULL,
                created_at TIMESTAMPTZ NOT NULL
            );
        ";

        // Postgres driver
        let mut pg_catalog = Catalog::new(DriverTarget::Postgres);
        pg_catalog.apply_sql(ddl).unwrap();
        let query_sql = "
            -- name: CreateUser
            INSERT INTO users (name, email) VALUES ($1, $2) RETURNING id, created_at;
        ";
        let pg_analyzed = crate::analyzer::analyze_query(query_sql, &pg_catalog, None).unwrap();
        let pg_ts = generate_file_ts(&[pg_analyzed]);

        assert!(
            pg_ts.contains(
                "export interface CreateUserParams {\n  name: string;\n  email: string;\n}"
            )
        );
        assert!(
            pg_ts.contains(
                "export interface CreateUserRow {\n  id: string;\n  created_at: Date;\n}"
            )
        );
        assert!(pg_ts.contains("export const createUserSql = `"));
        assert!(pg_ts.contains("export type CreateUserQuery = {\n  sql: string;\n  params: CreateUserParams;\n  row: CreateUserRow;\n};"));

        // Bun driver: id should be bigint
        let mut bun_catalog = Catalog::new(DriverTarget::Bun);
        bun_catalog.apply_sql(ddl).unwrap();
        let bun_analyzed = crate::analyzer::analyze_query(query_sql, &bun_catalog, None).unwrap();
        let bun_ts = generate_file_ts(&[bun_analyzed]);
        assert!(
            bun_ts.contains(
                "export interface CreateUserRow {\n  id: bigint;\n  created_at: Date;\n}"
            )
        );
    }

    #[test]
    fn test_dml_codegen_update_returning() {
        let mut catalog = crate::catalog::Catalog::default();
        catalog
            .apply_sql(
                "
                CREATE TABLE users (
                    id UUID PRIMARY KEY,
                    name TEXT NOT NULL
                );
            ",
            )
            .unwrap();

        let query_sql = "
            -- name: UpdateUser
            UPDATE users SET name = $1 WHERE id = $2 RETURNING id, name;
        ";
        let analyzed = crate::analyzer::analyze_query(query_sql, &catalog, None).unwrap();
        let ts = generate_file_ts(&[analyzed]);

        assert!(
            ts.contains("export interface UpdateUserParams {\n  name: string;\n  id: string;\n}")
        );
        assert!(ts.contains("export interface UpdateUserRow {\n  id: string;\n  name: string;\n}"));
        assert!(ts.contains("export type UpdateUserQuery = {\n  sql: string;\n  params: UpdateUserParams;\n  row: UpdateUserRow;\n};"));
    }

    #[test]
    fn test_dml_codegen_delete_without_returning() {
        let mut catalog = crate::catalog::Catalog::default();
        catalog
            .apply_sql(
                "
                CREATE TABLE users (
                    id UUID PRIMARY KEY
                );
            ",
            )
            .unwrap();

        let query_sql = "
            -- name: DeleteUser
            DELETE FROM users WHERE id = $1;
        ";
        let analyzed = crate::analyzer::analyze_query(query_sql, &catalog, None).unwrap();
        let ts = generate_file_ts(&[analyzed]);

        assert!(ts.contains("export interface DeleteUserParams {\n  id: string;\n}"));
        // No row return type or interface required
        assert!(!ts.contains("DeleteUserRow"));
        assert!(!ts.contains("row:"));
        assert!(ts.contains(
            "export type DeleteUserQuery = {\n  sql: string;\n  params: DeleteUserParams;\n};"
        ));
    }

    #[test]
    fn test_wrappers_postgres_driver() {
        use crate::catalog::DriverTarget;

        let query_with_rows = AnalyzedQuery {
            name: "GetUser".to_string(),
            raw_sql: "SELECT id, name FROM users WHERE id = $1;".to_string(),
            params: vec![QueryParam {
                index: 1,
                name: "id".to_string(),
                ts_type: "string".to_string(),
                is_optional: false,
            }],
            fields: vec![
                QueryField {
                    name: "id".to_string(),
                    ts_type: "string".to_string(),
                },
                QueryField {
                    name: "name".to_string(),
                    ts_type: "string".to_string(),
                },
            ],
        };

        let options = CodegenOptions::new(DriverTarget::Postgres, true);
        let ts = generate_file_ts_with_options(&[query_with_rows], &options);

        let expected_wrapper = "export async function getUser(sql: postgres.Sql, params: GetUserParams): Promise<GetUserRow[]> {\n  return await sql<GetUserRow[]>`${sql.unsafe(getUserSql, [params.id])}`;\n}";
        assert!(
            ts.contains(expected_wrapper),
            "Expected wrapper:\n{}\n\nFound:\n{}",
            expected_wrapper,
            ts
        );

        // Mutation without rows
        let query_mutation = AnalyzedQuery {
            name: "DeleteUser".to_string(),
            raw_sql: "DELETE FROM users WHERE id = $1;".to_string(),
            params: vec![QueryParam {
                index: 1,
                name: "id".to_string(),
                ts_type: "string".to_string(),
                is_optional: false,
            }],
            fields: vec![],
        };

        let ts_mut = generate_file_ts_with_options(&[query_mutation], &options);
        let expected_mut_wrapper = "export async function deleteUser(sql: postgres.Sql, params: DeleteUserParams): Promise<void> {\n  await sql.unsafe(deleteUserSql, [params.id]);\n}";
        assert!(
            ts_mut.contains(expected_mut_wrapper),
            "Expected wrapper:\n{}\n\nFound:\n{}",
            expected_mut_wrapper,
            ts_mut
        );
    }

    #[test]
    fn test_wrappers_pg_driver() {
        use crate::catalog::DriverTarget;

        let query_with_rows = AnalyzedQuery {
            name: "GetUser".to_string(),
            raw_sql: "SELECT id FROM users WHERE id = $1;".to_string(),
            params: vec![QueryParam {
                index: 1,
                name: "id".to_string(),
                ts_type: "string".to_string(),
                is_optional: false,
            }],
            fields: vec![QueryField {
                name: "id".to_string(),
                ts_type: "string".to_string(),
            }],
        };

        let options = CodegenOptions::new(DriverTarget::Pg, true);
        let ts = generate_file_ts_with_options(&[query_with_rows], &options);

        let expected_wrapper = "export async function getUser(client: pg.ClientBase | pg.Pool, params: GetUserParams): Promise<GetUserRow[]> {\n  const res = await client.query<GetUserRow>(getUserSql, [params.id]);\n  return res.rows;\n}";
        assert!(
            ts.contains(expected_wrapper),
            "Expected wrapper:\n{}\n\nFound:\n{}",
            expected_wrapper,
            ts
        );

        // Mutation without rows
        let query_mutation = AnalyzedQuery {
            name: "DeleteUser".to_string(),
            raw_sql: "DELETE FROM users WHERE id = $1;".to_string(),
            params: vec![QueryParam {
                index: 1,
                name: "id".to_string(),
                ts_type: "string".to_string(),
                is_optional: false,
            }],
            fields: vec![],
        };

        let ts_mut = generate_file_ts_with_options(&[query_mutation], &options);
        let expected_mut_wrapper = "export async function deleteUser(client: pg.ClientBase | pg.Pool, params: DeleteUserParams): Promise<void> {\n  await client.query(deleteUserSql, [params.id]);\n}";
        assert!(
            ts_mut.contains(expected_mut_wrapper),
            "Expected wrapper:\n{}\n\nFound:\n{}",
            expected_mut_wrapper,
            ts_mut
        );
    }

    #[test]
    fn test_wrappers_bun_driver() {
        use crate::catalog::DriverTarget;

        let query_with_rows = AnalyzedQuery {
            name: "GetUser".to_string(),
            raw_sql: "SELECT id FROM users WHERE id = $1;".to_string(),
            params: vec![QueryParam {
                index: 1,
                name: "id".to_string(),
                ts_type: "string".to_string(),
                is_optional: false,
            }],
            fields: vec![QueryField {
                name: "id".to_string(),
                ts_type: "string".to_string(),
            }],
        };

        let options = CodegenOptions::new(DriverTarget::Bun, true);
        let ts = generate_file_ts_with_options(&[query_with_rows], &options);

        let expected_wrapper = "export async function getUser(sql: import(\"bun\").SQL, params: GetUserParams): Promise<GetUserRow[]> {\n  return await sql<GetUserRow[]>`${sql.raw(getUserSql, [params.id])}`;\n}";
        assert!(
            ts.contains(expected_wrapper),
            "Expected wrapper:\n{}\n\nFound:\n{}",
            expected_wrapper,
            ts
        );

        // Mutation without rows
        let query_mutation = AnalyzedQuery {
            name: "DeleteUser".to_string(),
            raw_sql: "DELETE FROM users WHERE id = $1;".to_string(),
            params: vec![QueryParam {
                index: 1,
                name: "id".to_string(),
                ts_type: "string".to_string(),
                is_optional: false,
            }],
            fields: vec![],
        };

        let ts_mut = generate_file_ts_with_options(&[query_mutation], &options);
        let expected_mut_wrapper = "export async function deleteUser(sql: import(\"bun\").SQL, params: DeleteUserParams): Promise<void> {\n  await sql.raw(deleteUserSql, [params.id]);\n}";
        assert!(
            ts_mut.contains(expected_mut_wrapper),
            "Expected wrapper:\n{}\n\nFound:\n{}",
            expected_mut_wrapper,
            ts_mut
        );
    }

    #[test]
    fn test_wrappers_no_params() {
        use crate::catalog::DriverTarget;

        let query = AnalyzedQuery {
            name: "GetActiveUsers".to_string(),
            raw_sql: "SELECT id FROM users WHERE active = true;".to_string(),
            params: vec![],
            fields: vec![QueryField {
                name: "id".to_string(),
                ts_type: "string".to_string(),
            }],
        };

        let options = CodegenOptions::new(DriverTarget::Postgres, true);
        let ts = generate_file_ts_with_options(&[query], &options);

        let expected_wrapper = "export async function getActiveUsers(sql: postgres.Sql): Promise<GetActiveUsersRow[]> {\n  return await sql<GetActiveUsersRow[]>`${sql.unsafe(getActiveUsersSql, [])}`;\n}";
        assert!(
            ts.contains(expected_wrapper),
            "Expected wrapper:\n{}\n\nFound:\n{}",
            expected_wrapper,
            ts
        );
    }

    #[test]
    fn test_wrappers_optional_param_and_ordering() {
        use crate::catalog::DriverTarget;

        let query = AnalyzedQuery {
            name: "FindUsers".to_string(),
            raw_sql: "SELECT id FROM users WHERE ($1::text IS NULL OR name = $1) AND id = $2;"
                .to_string(),
            // Deliberately put param 2 before param 1 to test numerical sorting
            params: vec![
                QueryParam {
                    index: 2,
                    name: "id".to_string(),
                    ts_type: "string".to_string(),
                    is_optional: false,
                },
                QueryParam {
                    index: 1,
                    name: "name".to_string(),
                    ts_type: "string".to_string(),
                    is_optional: true,
                },
            ],
            fields: vec![QueryField {
                name: "id".to_string(),
                ts_type: "string".to_string(),
            }],
        };

        let options = CodegenOptions::new(DriverTarget::Postgres, true);
        let ts = generate_file_ts_with_options(&[query], &options);

        // Strict $1, $2 ordering with params.name ?? null for optional param
        let expected_wrapper = "export async function findUsers(sql: postgres.Sql, params: FindUsersParams): Promise<FindUsersRow[]> {\n  return await sql<FindUsersRow[]>`${sql.unsafe(findUsersSql, [params.name ?? null, params.id])}`;\n}";
        assert!(
            ts.contains(expected_wrapper),
            "Expected wrapper:\n{}\n\nFound:\n{}",
            expected_wrapper,
            ts
        );
    }

    #[test]
    fn test_codegen_json_types() {
        let query = AnalyzedQuery {
            name: "GetUserPayload".to_string(),
            raw_sql: "SELECT jsonb_build_object('id', id, 'meta', jsonb_build_object('active', true)) AS payload FROM users;".to_string(),
            params: vec![],
            fields: vec![
                QueryField {
                    name: "payload".to_string(),
                    ts_type: "{ id: number; meta: { active: boolean } }".to_string(),
                },
                QueryField {
                    name: "tags".to_string(),
                    ts_type: "Array<string>".to_string(),
                },
                QueryField {
                    name: "dynamic_data".to_string(),
                    ts_type: "Record<string, unknown>".to_string(),
                },
                QueryField {
                    name: "raw_json".to_string(),
                    ts_type: "unknown".to_string(),
                },
            ],
        };

        let ts = generate_file_ts(&[query]);
        assert!(ts.contains("export interface GetUserPayloadRow {"));
        assert!(ts.contains("payload: { id: number; meta: { active: boolean } };"));
        assert!(ts.contains("tags: Array<string>;"));
        assert!(ts.contains("dynamic_data: Record<string, unknown>;"));
        assert!(ts.contains("raw_json: unknown;"));
    }
}
