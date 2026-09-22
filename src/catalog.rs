use pg_query::protobuf::{AlterTableType, ConstrType};
use pg_query::NodeEnum;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMetadata {
    pub name: String,
    pub pg_type: String,
    pub ts_type: String,
    pub is_nullable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableMetadata {
    pub name: String,
    pub schema: Option<String>,
    pub columns: Vec<ColumnMetadata>,
}

impl TableMetadata {
    pub fn get_column(&self, name: &str) -> Option<&ColumnMetadata> {
        self.columns.iter().find(|c| c.name.eq_ignore_ascii_case(name))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub tables: HashMap<String, TableMetadata>,
}

/// Normalizes PostgreSQL types to TypeScript primitives according to project specifications.
pub fn normalize_pg_type_to_ts(pg_type: &str) -> String {
    let lower = pg_type.to_ascii_lowercase();
    let lower = lower.trim();

    // Check array suffix e.g. text[] or int[]
    if let Some(inner) = lower.strip_suffix("[]") {
        let inner_ts = normalize_pg_type_to_ts(inner);
        return format!("{}[]", inner_ts);
    }

    match lower {
        // int2, int4, float4, float8 -> number
        "int2" | "smallint" | "smallserial" => "number".to_string(),
        "int4" | "integer" | "int" | "serial" => "number".to_string(),
        "float4" | "real" => "number".to_string(),
        "float8" | "double precision" => "number".to_string(),
        "numeric" | "decimal" => "number".to_string(),

        // int8, bigint -> string
        "int8" | "bigint" | "bigserial" => "string".to_string(),

        // text, varchar, uuid -> string
        "text" | "varchar" | "character varying" | "char" | "character" | "bpchar" | "uuid" | "citext" => {
            "string".to_string()
        }

        // bool -> boolean
        "bool" | "boolean" => "boolean".to_string(),

        // timestamptz, timestamp, date -> Date | string
        "timestamptz"
        | "timestamp with time zone"
        | "timestamp"
        | "timestamp without time zone"
        | "date"
        | "time"
        | "timetz"
        | "time with time zone"
        | "time without time zone" => "Date | string".to_string(),

        // json, jsonb -> unknown
        "json" | "jsonb" => "unknown".to_string(),

        _ => "unknown".to_string(),
    }
}

/// Extracts the normalized PostgreSQL type name from a TypeName AST node.
pub fn extract_type_name(type_name: &pg_query::protobuf::TypeName) -> String {
    let mut names = Vec::new();
    for node in &type_name.names {
        if let Some(NodeEnum::String(s)) = &node.node {
            names.push(s.sval.as_str());
        }
    }

    // In pg_query, names may be ["pg_catalog", "int4"] or ["uuid"] or ["public", "my_type"]
    let base_name = if let Some(&last) = names.last() {
        last
    } else {
        "text"
    };

    if !type_name.array_bounds.is_empty() {
        format!("{}[]", base_name)
    } else {
        base_name.to_string()
    }
}

impl Catalog {
    /// Loads and executes all `.sql` migration files in `dir` sorted alphanumerically.
    pub fn load_from_dir<P: AsRef<Path>>(dir: P) -> Result<Self, String> {
        let dir_path = dir.as_ref();
        if !dir_path.exists() {
            return Err(format!("Migrations directory does not exist: {}", dir_path.display()));
        }

        let mut catalog = Catalog::default();
        let mut files: Vec<PathBuf> = Vec::new();

        for entry in walkdir::WalkDir::new(dir_path).follow_links(true) {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_file()
                && let Some(ext) = path.extension()
                    && ext.eq_ignore_ascii_case("sql") {
                        files.push(path.to_path_buf());
                    }
        }

        // Migrations MUST be sorted deterministically by filename/path before building the catalog
        files.sort();

        for file_path in files {
            let content = std::fs::read_to_string(&file_path)
                .map_err(|e| format!("Failed to read migration {}: {}", file_path.display(), e))?;
            catalog
                .apply_sql(&content)
                .map_err(|e| format!("Failed to parse migration {}: {}", file_path.display(), e))?;
        }

        Ok(catalog)
    }

    /// Parses SQL string and applies DDL statements to the catalog state.
    pub fn apply_sql(&mut self, sql: &str) -> Result<(), String> {
        let parsed = pg_query::parse(sql).map_err(|e| e.to_string())?;
        self.apply_parsed(&parsed.protobuf)?;
        Ok(())
    }

    /// Applies parsed statements to the catalog state.
    pub fn apply_parsed(&mut self, result: &pg_query::protobuf::ParseResult) -> Result<(), String> {
        for stmt in &result.stmts {
            if let Some(node) = &stmt.stmt {
                match &node.node {
                    Some(NodeEnum::CreateStmt(create_stmt)) => {
                        self.handle_create_stmt(create_stmt)?;
                    }
                    Some(NodeEnum::AlterTableStmt(alter_stmt)) => {
                        self.handle_alter_table_stmt(alter_stmt)?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn handle_create_stmt(&mut self, stmt: &pg_query::protobuf::CreateStmt) -> Result<(), String> {
        let rel = match &stmt.relation {
            Some(r) => r,
            None => return Ok(()),
        };

        let table_name = rel.relname.to_ascii_lowercase();
        let schema = if rel.schemaname.is_empty() {
            None
        } else {
            Some(rel.schemaname.to_ascii_lowercase())
        };

        let mut columns: Vec<ColumnMetadata> = Vec::new();
        let mut table_pk_cols: Vec<String> = Vec::new();

        for elt in &stmt.table_elts {
            match &elt.node {
                Some(NodeEnum::ColumnDef(col)) => {
                    let col_name = col.colname.clone();
                    let pg_type = col
                        .type_name
                        .as_ref()
                        .map(extract_type_name)
                        .unwrap_or_else(|| "text".to_string());
                    let ts_type = normalize_pg_type_to_ts(&pg_type);

                    let mut is_not_null = col.is_not_null;
                    for c in &col.constraints {
                        if let Some(NodeEnum::Constraint(constr)) = &c.node
                            && (constr.contype == ConstrType::ConstrPrimary as i32
                                || constr.contype == ConstrType::ConstrNotnull as i32)
                            {
                                is_not_null = true;
                            }
                    }

                    columns.push(ColumnMetadata {
                        name: col_name,
                        pg_type,
                        ts_type,
                        is_nullable: !is_not_null,
                    });
                }
                Some(NodeEnum::Constraint(constr))
                    if constr.contype == ConstrType::ConstrPrimary as i32 => {
                        for key in &constr.keys {
                            if let Some(NodeEnum::String(s)) = &key.node {
                                table_pk_cols.push(s.sval.clone());
                            }
                        }
                    }
                _ => {}
            }
        }

        // Apply table-level primary key constraints
        for pk_col in table_pk_cols {
            if let Some(col) = columns.iter_mut().find(|c| c.name.eq_ignore_ascii_case(&pk_col)) {
                col.is_nullable = false;
            }
        }

        let table_meta = TableMetadata {
            name: table_name.clone(),
            schema,
            columns,
        };

        self.tables.insert(table_name, table_meta);
        Ok(())
    }

    fn handle_alter_table_stmt(&mut self, stmt: &pg_query::protobuf::AlterTableStmt) -> Result<(), String> {
        let rel = match &stmt.relation {
            Some(r) => r,
            None => return Ok(()),
        };

        let table_name = rel.relname.to_ascii_lowercase();
        let table = match self.tables.get_mut(&table_name) {
            Some(t) => t,
            None => return Ok(()),
        };

        for cmd_node in &stmt.cmds {
            if let Some(NodeEnum::AlterTableCmd(cmd)) = &cmd_node.node {
                if cmd.subtype == AlterTableType::AtAddColumn as i32 {
                    if let Some(def_node) = &cmd.def
                        && let Some(NodeEnum::ColumnDef(col)) = &def_node.node {
                            let col_name = col.colname.clone();
                            let pg_type = col
                                .type_name
                                .as_ref()
                                .map(extract_type_name)
                                .unwrap_or_else(|| "text".to_string());
                            let ts_type = normalize_pg_type_to_ts(&pg_type);

                            let mut is_not_null = col.is_not_null;
                            for c in &col.constraints {
                                if let Some(NodeEnum::Constraint(constr)) = &c.node
                                    && (constr.contype == ConstrType::ConstrPrimary as i32
                                        || constr.contype == ConstrType::ConstrNotnull as i32)
                                    {
                                        is_not_null = true;
                                    }
                            }

                            // If column existed previously, replace it; otherwise append
                            table.columns.retain(|c| !c.name.eq_ignore_ascii_case(&col_name));
                            table.columns.push(ColumnMetadata {
                                name: col_name,
                                pg_type,
                                ts_type,
                                is_nullable: !is_not_null,
                            });
                        }
                } else if cmd.subtype == AlterTableType::AtDropColumn as i32 {
                    let col_name = &cmd.name;
                    table.columns.retain(|c| !c.name.eq_ignore_ascii_case(col_name));
                } else if cmd.subtype == AlterTableType::AtAlterColumnType as i32 {
                    let col_name = &cmd.name;
                    if let Some(def_node) = &cmd.def
                        && let Some(NodeEnum::ColumnDef(col)) = &def_node.node {
                            let pg_type = col
                                .type_name
                                .as_ref()
                                .map(extract_type_name)
                                .unwrap_or_else(|| "text".to_string());
                            let ts_type = normalize_pg_type_to_ts(&pg_type);
                            if let Some(existing_col) =
                                table.columns.iter_mut().find(|c| c.name.eq_ignore_ascii_case(col_name))
                            {
                                existing_col.pg_type = pg_type;
                                existing_col.ts_type = ts_type;
                            }
                        }
                } else if cmd.subtype == AlterTableType::AtSetNotNull as i32 {
                    let col_name = &cmd.name;
                    if let Some(existing_col) =
                        table.columns.iter_mut().find(|c| c.name.eq_ignore_ascii_case(col_name))
                    {
                        existing_col.is_nullable = false;
                    }
                } else if cmd.subtype == AlterTableType::AtDropNotNull as i32 {
                    let col_name = &cmd.name;
                    if let Some(existing_col) =
                        table.columns.iter_mut().find(|c| c.name.eq_ignore_ascii_case(col_name))
                    {
                        existing_col.is_nullable = true;
                    }
                }
            }
        }
        Ok(())
    }

    /// Looks up a table in the catalog by table name (case-insensitive) or schema-qualified name.
    pub fn get_table(&self, name: &str) -> Option<&TableMetadata> {
        let lower = name.to_ascii_lowercase();
        if let Some(t) = self.tables.get(&lower) {
            return Some(t);
        }
        if let Some((_, table_part)) = lower.rsplit_once('.')
            && let Some(t) = self.tables.get(table_part) {
                return Some(t);
            }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_table_parsing() {
        let sql = "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                email TEXT NOT NULL,
                age INT,
                bio VARCHAR(500),
                is_admin BOOLEAN NOT NULL DEFAULT false,
                metadata JSONB,
                created_at TIMESTAMPTZ NOT NULL
            );
        ";
        let mut catalog = Catalog::default();
        catalog.apply_sql(sql).unwrap();

        let table = catalog.get_table("users").expect("users table should exist");
        assert_eq!(table.name, "users");
        assert_eq!(table.columns.len(), 7);

        let id_col = table.get_column("id").unwrap();
        assert_eq!(id_col.ts_type, "string");
        assert!(!id_col.is_nullable);

        let email_col = table.get_column("email").unwrap();
        assert_eq!(email_col.ts_type, "string");
        assert!(!email_col.is_nullable);

        let age_col = table.get_column("age").unwrap();
        assert_eq!(age_col.ts_type, "number");
        assert!(age_col.is_nullable);

        let bio_col = table.get_column("bio").unwrap();
        assert_eq!(bio_col.ts_type, "string");
        assert!(bio_col.is_nullable);

        let admin_col = table.get_column("is_admin").unwrap();
        assert_eq!(admin_col.ts_type, "boolean");
        assert!(!admin_col.is_nullable);

        let meta_col = table.get_column("metadata").unwrap();
        assert_eq!(meta_col.ts_type, "unknown");
        assert!(meta_col.is_nullable);

        let created_col = table.get_column("created_at").unwrap();
        assert_eq!(created_col.ts_type, "Date | string");
        assert!(!created_col.is_nullable);
    }

    #[test]
    fn test_sequential_ddl_alters() {
        let mut catalog = Catalog::default();

        // 1. Initial CREATE TABLE
        catalog
            .apply_sql("CREATE TABLE products (id UUID PRIMARY KEY, name TEXT NOT NULL);")
            .unwrap();
        let table = catalog.get_table("products").unwrap();
        assert_eq!(table.columns.len(), 2);

        // 2. ALTER TABLE ADD COLUMN
        catalog
            .apply_sql("ALTER TABLE products ADD COLUMN price NUMERIC NOT NULL;")
            .unwrap();
        let table = catalog.get_table("products").unwrap();
        assert_eq!(table.columns.len(), 3);
        let price_col = table.get_column("price").unwrap();
        assert_eq!(price_col.ts_type, "number");
        assert!(!price_col.is_nullable);

        // 3. ALTER TABLE ALTER COLUMN TYPE
        catalog
            .apply_sql("ALTER TABLE products ALTER COLUMN name TYPE VARCHAR(255);")
            .unwrap();
        let table = catalog.get_table("products").unwrap();
        let name_col = table.get_column("name").unwrap();
        assert_eq!(name_col.ts_type, "string");
        assert_eq!(name_col.pg_type, "varchar");
        assert!(!name_col.is_nullable);

        // 4. ALTER TABLE DROP COLUMN
        catalog
            .apply_sql("ALTER TABLE products DROP COLUMN price;")
            .unwrap();
        let table = catalog.get_table("products").unwrap();
        assert_eq!(table.columns.len(), 2);
        assert!(table.get_column("price").is_none());

        // 5. ALTER TABLE ADD COLUMN and ALTER NOT NULL
        catalog
            .apply_sql("ALTER TABLE products ADD COLUMN views INT;")
            .unwrap();
        let views_col = catalog.get_table("products").unwrap().get_column("views").unwrap();
        assert!(views_col.is_nullable);

        catalog
            .apply_sql("ALTER TABLE products ALTER COLUMN views SET NOT NULL;")
            .unwrap();
        let views_col = catalog.get_table("products").unwrap().get_column("views").unwrap();
        assert!(!views_col.is_nullable);

        catalog
            .apply_sql("ALTER TABLE products ALTER COLUMN views DROP NOT NULL;")
            .unwrap();
        let views_col = catalog.get_table("products").unwrap().get_column("views").unwrap();
        assert!(views_col.is_nullable);
    }

    #[test]
    fn test_table_level_primary_key() {
        let sql = "
            CREATE TABLE order_items (
                order_id UUID,
                item_id INT,
                quantity INT NOT NULL,
                PRIMARY KEY (order_id, item_id)
            );
        ";
        let mut catalog = Catalog::default();
        catalog.apply_sql(sql).unwrap();

        let table = catalog.get_table("order_items").unwrap();
        assert!(!table.get_column("order_id").unwrap().is_nullable);
        assert!(!table.get_column("item_id").unwrap().is_nullable);
        assert!(!table.get_column("quantity").unwrap().is_nullable);
    }

    #[test]
    fn test_load_from_dir_deterministic_sorting() {
        use std::fs;
        let temp_dir = std::env::temp_dir().join(format!("sqltype_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&temp_dir).unwrap();

        // Write migration 002 first, then 001
        fs::write(temp_dir.join("002_add_col.sql"), "ALTER TABLE items ADD COLUMN price INT NOT NULL;").unwrap();
        fs::write(temp_dir.join("001_create.sql"), "CREATE TABLE items (id UUID PRIMARY KEY);").unwrap();

        let catalog = Catalog::load_from_dir(&temp_dir).unwrap();
        let table = catalog.get_table("items").expect("items table should exist");
        assert_eq!(table.columns.len(), 2);
        assert!(!table.get_column("id").unwrap().is_nullable);
        assert!(!table.get_column("price").unwrap().is_nullable);

        let _ = fs::remove_dir_all(temp_dir);
    }
}
