use pg_query::NodeEnum;
use pg_query::protobuf::{AlterTableType, ConstrType};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::analyzer::PgType;
use crate::catalog::extensions::{ExtensionFunction, ExtensionOperator};

pub mod extensions;
pub mod types;

pub use types::*;

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub tables: HashMap<String, TableMetadata>,
    pub enums: HashMap<String, Vec<String>>,
    pub composite_types: HashMap<String, CompositeTypeMetadata>,
    pub domains: HashMap<String, DomainMetadata>,
    pub driver: DriverTarget,
    pub type_overrides: HashMap<String, String>,
    pub extensions: HashSet<String>,
    pub extension_types: HashMap<String, PgType>,
    pub extension_operators: Vec<ExtensionOperator>,
    pub extension_functions: HashMap<String, Vec<ExtensionFunction>>,
}

pub type SchemaCatalog = Catalog;

pub fn is_type_compatible(actual: &PgType, expected: &PgType) -> bool {
    if actual == expected {
        return true;
    }
    if *actual == PgType::Unknown || *expected == PgType::Unknown {
        return true;
    }
    match (actual, expected) {
        (PgType::Vector(_), PgType::Vector(_)) => true,
        (PgType::HalfVec(_), PgType::HalfVec(_)) => true,
        (PgType::Geometry, PgType::Geometry) => true,
        (PgType::Geography, PgType::Geography) => true,
        (PgType::Box2D, PgType::Box2D) => true,
        (PgType::Box3D, PgType::Box3D) => true,
        (a, PgType::Float8) if crate::analyzer::is_numeric_type(a) => true,
        (a, PgType::Numeric) if crate::analyzer::is_numeric_type(a) => true,
        (a, PgType::Int4) if crate::analyzer::is_numeric_type(a) => true,
        (PgType::Text | PgType::Varchar, PgType::Text) => true,
        _ => false,
    }
}

/// Normalizes PostgreSQL types to TypeScript primitives according to driver target specifications.
pub fn normalize_pg_type_to_ts(pg_type: &str, driver: DriverTarget) -> String {
    let lower = pg_type.to_ascii_lowercase();
    let lower = lower.trim();

    // Check array suffix e.g. text[] or int[]
    if let Some(inner) = lower.strip_suffix("[]") {
        let inner_ts = normalize_pg_type_to_ts(inner, driver);
        return format!("{}[]", inner_ts);
    }

    let base = if let Some((head, _)) = lower.split_once('(') {
        head.trim()
    } else {
        lower
    };

    let base = base.strip_prefix("pg_catalog.").unwrap_or(base);

    match base {
        // int2, int4, float4, float8 -> number
        "int2" | "smallint" | "smallserial" => "number".to_string(),
        "int4" | "integer" | "int" | "serial" => "number".to_string(),
        "float4" | "real" => "number".to_string(),
        "float8" | "double precision" => "number".to_string(),
        "numeric" | "decimal" => "number".to_string(),

        // int8, bigint -> string for postgres/pg, bigint for bun
        "int8" | "bigint" | "bigserial" | "serial8" => match driver {
            DriverTarget::Bun => "bigint".to_string(),
            DriverTarget::Postgres | DriverTarget::Pg => "string".to_string(),
        },

        // bytea -> Buffer for postgres/pg, Uint8Array for bun
        "bytea" => match driver {
            DriverTarget::Bun => "Uint8Array".to_string(),
            DriverTarget::Postgres | DriverTarget::Pg => "Buffer".to_string(),
        },

        // text, varchar, uuid -> string
        "text" | "varchar" | "character varying" | "char" | "character" | "bpchar" | "uuid"
        | "citext" => "string".to_string(),

        // bool -> boolean
        "bool" | "boolean" => "boolean".to_string(),

        // date -> string ("YYYY-MM-DD") across all drivers
        "date" => "string".to_string(),

        // timestamp, timestamptz -> Date across all drivers
        "timestamptz"
        | "timestamp with time zone"
        | "timestamp"
        | "timestamp without time zone" => "Date".to_string(),

        "time" | "timetz" | "time with time zone" | "time without time zone" => {
            "string".to_string()
        }

        // json, jsonb -> unknown
        "json" | "jsonb" => "unknown".to_string(),

        // pgvector
        "vector" | "halfvec" => "number[]".to_string(),

        // PostGIS
        "geometry" | "geography" => "GeoJSON.Geometry".to_string(),

        // Spatial box types
        "box2d" | "box3d" => "string".to_string(),

        _ => "unknown".to_string(),
    }
}

/// Extracts a structured qualified type name from a TypeName AST node.
pub fn extract_qualified_type_name(type_name: &pg_query::protobuf::TypeName) -> QualifiedTypeName {
    let mut names = Vec::new();
    for node in &type_name.names {
        if let Some(NodeEnum::String(s)) = &node.node {
            names.push(s.sval.as_str());
        }
    }

    let (schema, name) = if names.len() > 1 {
        let s = names[0].to_ascii_lowercase();
        let n = names.last().unwrap().to_ascii_lowercase();
        (Some(s), n)
    } else if let Some(&n) = names.last() {
        (None, n.to_ascii_lowercase())
    } else {
        (None, "text".to_string())
    };

    let mut typmod_str = None;
    if !type_name.typmods.is_empty()
        && (name.eq_ignore_ascii_case("vector") || name.eq_ignore_ascii_case("halfvec"))
    {
        let mut parts = Vec::new();
        for tm in &type_name.typmods {
            if let Some(NodeEnum::AConst(ac)) = &tm.node
                && let Some(val) = &ac.val
            {
                match val {
                    pg_query::protobuf::a_const::Val::Ival(i) => parts.push(i.ival.to_string()),
                    pg_query::protobuf::a_const::Val::Sval(s) => parts.push(s.sval.clone()),
                    _ => {}
                }
            }
        }
        if !parts.is_empty() {
            typmod_str = Some(format!("({})", parts.join(", ")));
        }
    }

    QualifiedTypeName {
        schema,
        name,
        is_array: !type_name.array_bounds.is_empty(),
        typmod: typmod_str,
    }
}

/// Extracts the normalized PostgreSQL type name from a TypeName AST node.
pub fn extract_type_name(type_name: &pg_query::protobuf::TypeName) -> String {
    let qname = extract_qualified_type_name(type_name);
    qname.to_type_string()
}

impl Catalog {
    pub fn new(driver: DriverTarget) -> Self {
        Self {
            tables: HashMap::new(),
            enums: HashMap::new(),
            composite_types: HashMap::new(),
            domains: HashMap::new(),
            driver,
            type_overrides: HashMap::new(),
            extensions: HashSet::new(),
            extension_types: HashMap::new(),
            extension_operators: Vec::new(),
            extension_functions: HashMap::new(),
        }
    }

    pub fn with_overrides(mut self, overrides: HashMap<String, String>) -> Self {
        self.type_overrides = overrides;
        self
    }

    pub fn get_domain(&self, name: &str) -> Option<&DomainMetadata> {
        let lower = name.to_ascii_lowercase();
        let trimmed = lower.strip_suffix("[]").unwrap_or(&lower);
        if let Some(d) = self.domains.get(trimmed) {
            return Some(d);
        }
        if let Some((schema, domain_name)) = trimmed.split_once('.') {
            if schema == "public"
                && let Some(d) = self.domains.get(domain_name)
            {
                return Some(d);
            }
        } else {
            let public_key = format!("public.{}", trimmed);
            if let Some(d) = self.domains.get(&public_key) {
                return Some(d);
            }
        }
        None
    }

    pub fn get_composite_type(&self, name: &str) -> Option<&CompositeTypeMetadata> {
        let lower = name.to_ascii_lowercase();
        let trimmed = lower.strip_suffix("[]").unwrap_or(&lower);
        if let Some(ct) = self.composite_types.get(trimmed) {
            return Some(ct);
        }
        if let Some((schema, comp_name)) = trimmed.split_once('.') {
            if schema == "public"
                && let Some(ct) = self.composite_types.get(comp_name)
            {
                return Some(ct);
            }
        } else {
            let public_key = format!("public.{}", trimmed);
            if let Some(ct) = self.composite_types.get(&public_key) {
                return Some(ct);
            }
        }
        None
    }

    pub fn resolve_operator(
        &self,
        op_name: &str,
        left: &PgType,
        right: &PgType,
    ) -> Option<&ExtensionOperator> {
        self.extension_operators.iter().find(|op| {
            op.name == op_name
                && is_type_compatible(left, &op.left)
                && is_type_compatible(right, &op.right)
        })
    }

    pub fn resolve_function(&self, name: &str, arg_count: usize) -> Option<&ExtensionFunction> {
        if let Some(funcs) = self.extension_functions.get(&name.to_ascii_lowercase()) {
            funcs
                .iter()
                .find(|f| f.variadic || f.params.len() == arg_count)
        } else {
            None
        }
    }

    pub fn resolve_function_with_args(
        &self,
        name: &str,
        arg_types: &[PgType],
    ) -> Option<&ExtensionFunction> {
        if let Some(funcs) = self.extension_functions.get(&name.to_ascii_lowercase()) {
            for f in funcs {
                if f.variadic || f.params.len() == arg_types.len() {
                    let matches = f
                        .params
                        .iter()
                        .zip(arg_types.iter())
                        .all(|(expected, actual)| is_type_compatible(actual, expected));
                    if matches {
                        return Some(f);
                    }
                }
            }
            funcs
                .iter()
                .find(|f| f.variadic || f.params.len() == arg_types.len())
        } else {
            None
        }
    }

    /// Resolves a PostgreSQL type name to its TypeScript type representation,
    /// checking for registered custom ENUM types, domains, and composite types before falling back to default primitives.
    pub fn resolve_type(&self, pg_type: &str) -> String {
        let lower = pg_type.to_ascii_lowercase();
        let lower = lower.trim();

        if let Some(inner) = lower.strip_suffix("[]") {
            let inner_ts = self.resolve_type(inner);
            if inner_ts.starts_with('{') && inner_ts.ends_with('}') {
                return format!("Array<{}>", inner_ts);
            } else if inner_ts.contains('|') {
                return format!("({})[]", inner_ts);
            } else {
                return format!("{}[]", inner_ts);
            }
        }

        let base = if let Some((head, _)) = lower.split_once('(') {
            head.trim()
        } else {
            lower
        };

        // Strict schema qualification: "pg_catalog.x" MUST resolve to standard PostgreSQL built-ins only!
        if let Some(builtin_name) = base.strip_prefix("pg_catalog.") {
            return normalize_pg_type_to_ts(builtin_name, self.driver);
        }

        if let Some(override_ts) = self
            .type_overrides
            .get(base)
            .or_else(|| self.type_overrides.get(lower))
        {
            return override_ts.clone();
        }

        if let Some(domain) = self.get_domain(base) {
            return domain.ts_type.clone();
        }

        if let Some(comp) = self.get_composite_type(base) {
            return comp.to_ts();
        }

        let enum_vals = self.enums.get(base).or_else(|| {
            if let Some((schema, enum_name)) = base.split_once('.') {
                if schema == "public" {
                    self.enums.get(enum_name)
                } else {
                    None
                }
            } else {
                let public_key = format!("public.{}", base);
                self.enums.get(&public_key)
            }
        });
        if let Some(vals) = enum_vals {
            if vals.is_empty() {
                return "string".to_string();
            }
            return vals
                .iter()
                .map(|v| format!("\"{}\"", v))
                .collect::<Vec<_>>()
                .join(" | ");
        }

        if let Some((schema, _)) = base.split_once('.')
            && schema != "public"
            && schema != "pg_catalog"
        {
            return "unknown".to_string();
        }

        normalize_pg_type_to_ts(base, self.driver)
    }

    pub fn resolve_qualified_type(&self, qname: &QualifiedTypeName) -> String {
        self.resolve_type(&qname.to_type_string())
    }

    pub fn with_driver(mut self, driver: DriverTarget) -> Self {
        self.driver = driver;
        self
    }

    /// Loads and executes all `.sql` migration files in `dir` sorted alphanumerically with specified driver target.
    pub fn load_from_dir_with_driver<P: AsRef<Path>>(
        dir: P,
        driver: DriverTarget,
    ) -> Result<Self, String> {
        let dir_path = dir.as_ref();
        if !dir_path.exists() {
            return Err(format!(
                "Migrations directory does not exist: {}",
                dir_path.display()
            ));
        }

        let mut catalog = Catalog::new(driver);
        let mut files: Vec<PathBuf> = Vec::new();

        for entry in walkdir::WalkDir::new(dir_path).follow_links(true) {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_file()
                && let Some(ext) = path.extension()
                && ext.eq_ignore_ascii_case("sql")
            {
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

    /// Loads and executes all `.sql` migration files in `dir` sorted alphanumerically.
    pub fn load_from_dir<P: AsRef<Path>>(dir: P) -> Result<Self, String> {
        Self::load_from_dir_with_driver(dir, DriverTarget::default())
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
                    Some(NodeEnum::CreateEnumStmt(create_enum)) => {
                        self.handle_create_enum_stmt(create_enum)?;
                    }
                    Some(NodeEnum::CompositeTypeStmt(comp_stmt)) => {
                        self.handle_composite_type_stmt(comp_stmt)?;
                    }
                    Some(NodeEnum::CreateDomainStmt(domain_stmt)) => {
                        self.handle_create_domain_stmt(domain_stmt)?;
                    }
                    Some(NodeEnum::CreateExtensionStmt(create_ext)) => {
                        self.handle_create_extension_stmt(create_ext)?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn handle_composite_type_stmt(
        &mut self,
        stmt: &pg_query::protobuf::CompositeTypeStmt,
    ) -> Result<(), String> {
        let typevar = match &stmt.typevar {
            Some(tv) => tv,
            None => return Ok(()),
        };

        let name = typevar.relname.to_ascii_lowercase();
        let schema = if typevar.schemaname.is_empty() {
            None
        } else {
            Some(typevar.schemaname.to_ascii_lowercase())
        };

        let mut attributes = Vec::new();
        for col_node in &stmt.coldeflist {
            if let Some(NodeEnum::ColumnDef(col)) = &col_node.node {
                let attr_name = col.colname.clone();
                let pg_type = col
                    .type_name
                    .as_ref()
                    .map(extract_type_name)
                    .unwrap_or_else(|| "text".to_string());
                let ts_type = self.resolve_type(&pg_type);
                attributes.push(CompositeTypeAttribute {
                    name: attr_name,
                    pg_type,
                    ts_type,
                });
            }
        }

        let meta = CompositeTypeMetadata {
            name: name.clone(),
            schema: schema.clone(),
            attributes,
        };

        if let Some(s) = &schema {
            self.composite_types
                .insert(format!("{}.{}", s, name), meta.clone());
            if s == "public" {
                self.composite_types.insert(name, meta);
            }
        } else {
            self.composite_types.insert(name.clone(), meta.clone());
            self.composite_types
                .insert(format!("public.{}", name), meta);
        }

        Ok(())
    }

    fn handle_create_domain_stmt(
        &mut self,
        stmt: &pg_query::protobuf::CreateDomainStmt,
    ) -> Result<(), String> {
        let mut name_parts = Vec::new();
        for node in &stmt.domainname {
            if let Some(NodeEnum::String(s)) = &node.node {
                name_parts.push(s.sval.as_str());
            }
        }

        let (schema, domain_name) = match name_parts.len() {
            0 => return Ok(()),
            1 => (None, name_parts[0].to_ascii_lowercase()),
            _ => (
                Some(name_parts[0].to_ascii_lowercase()),
                name_parts.last().unwrap().to_ascii_lowercase(),
            ),
        };

        let base_type = stmt
            .type_name
            .as_ref()
            .map(extract_type_name)
            .unwrap_or_else(|| "text".to_string());
        let ts_type = self.resolve_type(&base_type);

        let mut is_not_null = false;
        let mut has_default = false;
        for c in &stmt.constraints {
            if let Some(NodeEnum::Constraint(constr)) = &c.node {
                if constr.contype == ConstrType::ConstrNotnull as i32 {
                    is_not_null = true;
                } else if constr.contype == ConstrType::ConstrDefault as i32 {
                    has_default = true;
                }
            }
        }

        let domain_meta = DomainMetadata {
            name: domain_name.clone(),
            schema: schema.clone(),
            base_type,
            ts_type,
            is_not_null,
            has_default,
        };

        if let Some(s) = &schema {
            self.domains
                .insert(format!("{}.{}", s, domain_name), domain_meta.clone());
            if s == "public" {
                self.domains.insert(domain_name, domain_meta);
            }
        } else {
            self.domains
                .insert(domain_name.clone(), domain_meta.clone());
            self.domains
                .insert(format!("public.{}", domain_name), domain_meta);
        }

        Ok(())
    }

    fn handle_create_extension_stmt(
        &mut self,
        stmt: &pg_query::protobuf::CreateExtensionStmt,
    ) -> Result<(), String> {
        let name = stmt.extname.to_ascii_lowercase();
        if self.extensions.contains(&name) {
            if stmt.if_not_exists {
                return Ok(());
            } else {
                return Err(format!("Extension \"{}\" already exists", name));
            }
        }

        let registry = extensions::ExtensionRegistry::default();
        if let Some(ext_def) = registry.get(&name) {
            self.extensions.insert(name);
            for ext_type in ext_def.types() {
                self.extension_types
                    .insert(ext_type.name.to_ascii_lowercase(), ext_type.pg_type.clone());
                for alias in ext_type.aliases {
                    self.extension_types
                        .insert(alias.to_ascii_lowercase(), ext_type.pg_type.clone());
                }
            }
            for op in ext_def.operators() {
                self.extension_operators.push(op);
            }
            for func in ext_def.functions() {
                self.extension_functions
                    .entry(func.name.to_ascii_lowercase())
                    .or_default()
                    .push(func);
            }
        } else {
            self.extensions.insert(name);
        }
        Ok(())
    }

    fn handle_create_enum_stmt(
        &mut self,
        stmt: &pg_query::protobuf::CreateEnumStmt,
    ) -> Result<(), String> {
        let mut name_parts = Vec::new();
        for node in &stmt.type_name {
            if let Some(NodeEnum::String(s)) = &node.node {
                name_parts.push(s.sval.as_str());
            }
        }
        let (schema, enum_name) = match name_parts.len() {
            0 => return Ok(()),
            1 => (None, name_parts[0].to_ascii_lowercase()),
            _ => (
                Some(name_parts[0].to_ascii_lowercase()),
                name_parts.last().unwrap().to_ascii_lowercase(),
            ),
        };

        let mut values = Vec::new();
        for node in &stmt.vals {
            if let Some(NodeEnum::String(s)) = &node.node {
                values.push(s.sval.clone());
            }
        }

        if let Some(s) = &schema {
            self.enums
                .insert(format!("{}.{}", s, enum_name), values.clone());
            if s == "public" {
                self.enums.insert(enum_name, values);
            }
        } else {
            self.enums.insert(enum_name.clone(), values.clone());
            self.enums.insert(format!("public.{}", enum_name), values);
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
                    let ts_type = self.resolve_type(&pg_type);

                    let mut is_not_null = col.is_not_null
                        || matches!(
                            pg_type.to_ascii_lowercase().as_str(),
                            "serial"
                                | "bigserial"
                                | "smallserial"
                                | "serial8"
                                | "serial4"
                                | "serial2"
                        );
                    let mut has_default = matches!(
                        pg_type.to_ascii_lowercase().as_str(),
                        "serial" | "bigserial" | "smallserial" | "serial8" | "serial4" | "serial2"
                    );

                    // Inherit domain constraints (NOT NULL, DEFAULT)
                    if let Some(domain) = self.get_domain(&pg_type) {
                        if domain.is_not_null {
                            is_not_null = true;
                        }
                        if domain.has_default {
                            has_default = true;
                        }
                    }

                    let mut is_pk = false;
                    for c in &col.constraints {
                        if let Some(NodeEnum::Constraint(constr)) = &c.node {
                            if constr.contype == ConstrType::ConstrPrimary as i32 {
                                is_not_null = true;
                                is_pk = true;
                                if !table_pk_cols.iter().any(|k| k.eq_ignore_ascii_case(&col_name)) {
                                    table_pk_cols.push(col_name.clone());
                                }
                            } else if constr.contype == ConstrType::ConstrNotnull as i32 {
                                is_not_null = true;
                            } else if constr.contype == ConstrType::ConstrDefault as i32 {
                                has_default = true;
                            }
                        }
                    }

                    columns.push(ColumnMetadata {
                        name: col_name,
                        pg_type,
                        ts_type,
                        is_nullable: !is_not_null,
                        has_default,
                        is_primary_key: is_pk,
                    });
                }
                Some(NodeEnum::Constraint(constr))
                    if constr.contype == ConstrType::ConstrPrimary as i32 =>
                {
                    for key in &constr.keys {
                        if let Some(NodeEnum::String(s)) = &key.node
                            && !table_pk_cols.iter().any(|k| k.eq_ignore_ascii_case(&s.sval))
                        {
                            table_pk_cols.push(s.sval.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        // Apply table-level primary key constraints
        for pk_col in &table_pk_cols {
            if let Some(col) = columns
                .iter_mut()
                .find(|c| c.name.eq_ignore_ascii_case(pk_col))
            {
                col.is_nullable = false;
                col.is_primary_key = true;
            }
        }

        let table_meta = TableMetadata {
            name: table_name.clone(),
            schema: schema.clone(),
            columns,
            primary_keys: table_pk_cols,
        };

        if let Some(s) = &schema {
            self.tables.insert(format!("{}.{}", s, table_name), table_meta.clone());
            if s == "public" {
                self.tables.insert(table_name, table_meta);
            }
        } else {
            self.tables.insert(table_name.clone(), table_meta.clone());
            self.tables.insert(format!("public.{}", table_name), table_meta);
        }

        Ok(())
    }

    fn handle_alter_table_stmt(
        &mut self,
        stmt: &pg_query::protobuf::AlterTableStmt,
    ) -> Result<(), String> {
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

        let target_key = if let Some(s) = &schema {
            let qualified = format!("{}.{}", s, table_name);
            if self.tables.contains_key(&qualified) {
                qualified
            } else if s == "public" && self.tables.contains_key(&table_name) {
                table_name.clone()
            } else {
                return Ok(());
            }
        } else if self.tables.contains_key(&table_name) {
            table_name.clone()
        } else if self.tables.contains_key(&format!("public.{}", table_name)) {
            format!("public.{}", table_name)
        } else {
            return Ok(());
        };

        let mut table = match self.tables.get(&target_key) {
            Some(t) => t.clone(),
            None => return Ok(()),
        };

        for cmd_node in &stmt.cmds {
            if let Some(NodeEnum::AlterTableCmd(cmd)) = &cmd_node.node {
                if cmd.subtype == AlterTableType::AtAddColumn as i32 {
                    if let Some(def_node) = &cmd.def
                        && let Some(NodeEnum::ColumnDef(col)) = &def_node.node
                    {
                        let col_name = col.colname.clone();
                        let pg_type = col
                            .type_name
                            .as_ref()
                            .map(extract_type_name)
                            .unwrap_or_else(|| "text".to_string());
                        let ts_type = self.resolve_type(&pg_type);
                        let mut is_not_null = col.is_not_null
                            || matches!(
                                pg_type.to_ascii_lowercase().as_str(),
                                "serial"
                                    | "bigserial"
                                    | "smallserial"
                                    | "serial8"
                                    | "serial4"
                                    | "serial2"
                            );
                        let mut has_default = matches!(
                            pg_type.to_ascii_lowercase().as_str(),
                            "serial"
                                | "bigserial"
                                | "smallserial"
                                | "serial8"
                                | "serial4"
                                | "serial2"
                        );

                        if let Some(domain) = self.get_domain(&pg_type) {
                            if domain.is_not_null {
                                is_not_null = true;
                            }
                            if domain.has_default {
                                has_default = true;
                            }
                        }

                        let mut is_pk = false;
                        for c in &col.constraints {
                            if let Some(NodeEnum::Constraint(constr)) = &c.node {
                                if constr.contype == ConstrType::ConstrPrimary as i32 {
                                    is_not_null = true;
                                    is_pk = true;
                                    if !table.primary_keys.iter().any(|k| k.eq_ignore_ascii_case(&col_name)) {
                                        table.primary_keys.push(col_name.clone());
                                    }
                                } else if constr.contype == ConstrType::ConstrNotnull as i32 {
                                    is_not_null = true;
                                } else if constr.contype == ConstrType::ConstrDefault as i32 {
                                    has_default = true;
                                }
                            }
                        }

                        table
                            .columns
                            .retain(|c| !c.name.eq_ignore_ascii_case(&col_name));
                        table.columns.push(ColumnMetadata {
                            name: col_name,
                            pg_type,
                            ts_type,
                            is_nullable: !is_not_null,
                            has_default,
                            is_primary_key: is_pk,
                        });
                    }
                } else if cmd.subtype == AlterTableType::AtDropColumn as i32 {
                    let col_name = &cmd.name;
                    table
                        .columns
                        .retain(|c| !c.name.eq_ignore_ascii_case(col_name));
                    table
                        .primary_keys
                        .retain(|k| !k.eq_ignore_ascii_case(col_name));
                } else if cmd.subtype == AlterTableType::AtAlterColumnType as i32 {
                    let col_name = &cmd.name;
                    if let Some(def_node) = &cmd.def
                        && let Some(NodeEnum::ColumnDef(col)) = &def_node.node
                    {
                        let pg_type = col
                            .type_name
                            .as_ref()
                            .map(extract_type_name)
                            .unwrap_or_else(|| "text".to_string());
                        let ts_type = self.resolve_type(&pg_type);
                        if let Some(existing_col) = table
                            .columns
                            .iter_mut()
                            .find(|c| c.name.eq_ignore_ascii_case(col_name))
                        {
                            existing_col.pg_type = pg_type;
                            existing_col.ts_type = ts_type;
                        }
                    }
                } else if cmd.subtype == AlterTableType::AtSetNotNull as i32 {
                    let col_name = &cmd.name;
                    if let Some(existing_col) = table
                        .columns
                        .iter_mut()
                        .find(|c| c.name.eq_ignore_ascii_case(col_name))
                    {
                        existing_col.is_nullable = false;
                    }
                } else if cmd.subtype == AlterTableType::AtDropNotNull as i32 {
                    let col_name = &cmd.name;
                    if let Some(existing_col) = table
                        .columns
                        .iter_mut()
                        .find(|c| c.name.eq_ignore_ascii_case(col_name))
                    {
                        existing_col.is_nullable = true;
                    }
                } else if cmd.subtype == AlterTableType::AtColumnDefault as i32 {
                    let col_name = &cmd.name;
                    if let Some(existing_col) = table
                        .columns
                        .iter_mut()
                        .find(|c| c.name.eq_ignore_ascii_case(col_name))
                    {
                        existing_col.has_default = cmd.def.is_some();
                    }
                } else if cmd.subtype == AlterTableType::AtAddConstraint as i32
                    && let Some(def_node) = &cmd.def
                    && let Some(NodeEnum::Constraint(constr)) = &def_node.node
                    && constr.contype == ConstrType::ConstrPrimary as i32
                {
                    for key in &constr.keys {
                        if let Some(NodeEnum::String(s)) = &key.node {
                            let pk_col = &s.sval;
                            if !table.primary_keys.iter().any(|k| k.eq_ignore_ascii_case(pk_col)) {
                                table.primary_keys.push(pk_col.clone());
                            }
                            if let Some(existing_col) = table
                                    .columns
                                    .iter_mut()
                                    .find(|c| c.name.eq_ignore_ascii_case(pk_col))
                            {
                                existing_col.is_primary_key = true;
                                existing_col.is_nullable = false;
                            }
                        }
                    }
                }
            }
        }

        if let Some(s) = &table.schema {
            self.tables
                .insert(format!("{}.{}", s, table.name), table.clone());
            if s == "public" {
                self.tables.insert(table.name.clone(), table);
            }
        } else {
            self.tables.insert(table.name.clone(), table.clone());
            self.tables.insert(format!("public.{}", table.name), table);
        }

        Ok(())
    }

    /// Looks up a table in the catalog by table name (case-insensitive) or schema-qualified name.
    /// Strict schema separation: "pg_catalog.users" will NOT match a table in "public".
    pub fn get_table(&self, name: &str) -> Option<&TableMetadata> {
        let lower = name.to_ascii_lowercase();
        if let Some((schema_part, table_part)) = lower.split_once('.') {
            // Qualified lookup e.g. "public.users", "pg_catalog.users", "schema1.t"
            if let Some(t) = self.tables.get(&lower) {
                if let Some(ref tbl_schema) = t.schema {
                    if tbl_schema.eq_ignore_ascii_case(schema_part) {
                        return Some(t);
                    }
                } else if schema_part == "public" {
                    return Some(t);
                }
                return None;
            }
            if schema_part == "public"
                && let Some(t) = self.tables.get(table_part)
                && t.schema.as_deref().unwrap_or("public").eq_ignore_ascii_case("public")
            {
                return Some(t);
            }
            None
        } else {
            // Unqualified lookup e.g. "users", "t"
            if let Some(t) = self.tables.get(&lower) {
                return Some(t);
            }
            let public_key = format!("public.{}", lower);
            if let Some(t) = self.tables.get(&public_key) {
                return Some(t);
            }
            for t in self.tables.values() {
                if t.name.eq_ignore_ascii_case(&lower)
                    && t.schema.as_deref().unwrap_or("public").eq_ignore_ascii_case("public")
                {
                    return Some(t);
                }
            }
            None
        }
    }

    /// Looks up a table with an optional explicit schema.
    pub fn get_table_qualified(&self, schema: Option<&str>, name: &str) -> Option<&TableMetadata> {
        if let Some(s) = schema {
            let qualified = format!("{}.{}", s.to_ascii_lowercase(), name.to_ascii_lowercase());
            self.get_table(&qualified)
        } else {
            self.get_table(name)
        }
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

        let table = catalog
            .get_table("users")
            .expect("users table should exist");
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
        assert_eq!(created_col.ts_type, "Date");
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
        let views_col = catalog
            .get_table("products")
            .unwrap()
            .get_column("views")
            .unwrap();
        assert!(views_col.is_nullable);

        catalog
            .apply_sql("ALTER TABLE products ALTER COLUMN views SET NOT NULL;")
            .unwrap();
        let views_col = catalog
            .get_table("products")
            .unwrap()
            .get_column("views")
            .unwrap();
        assert!(!views_col.is_nullable);

        catalog
            .apply_sql("ALTER TABLE products ALTER COLUMN views DROP NOT NULL;")
            .unwrap();
        let views_col = catalog
            .get_table("products")
            .unwrap()
            .get_column("views")
            .unwrap();
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
        let temp_dir = std::env::temp_dir().join(format!(
            "sqltype_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp_dir).unwrap();

        // Write migration 002 first, then 001
        fs::write(
            temp_dir.join("002_add_col.sql"),
            "ALTER TABLE items ADD COLUMN price INT NOT NULL;",
        )
        .unwrap();
        fs::write(
            temp_dir.join("001_create.sql"),
            "CREATE TABLE items (id UUID PRIMARY KEY);",
        )
        .unwrap();

        let catalog = Catalog::load_from_dir(&temp_dir).unwrap();
        let table = catalog
            .get_table("items")
            .expect("items table should exist");
        assert_eq!(table.columns.len(), 2);
        assert!(!table.get_column("id").unwrap().is_nullable);
        assert!(!table.get_column("price").unwrap().is_nullable);

        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_create_enum_type() {
        let sql = "
            CREATE TYPE user_role AS ENUM ('admin', 'editor', 'viewer');
            CREATE TABLE team_members (
                id UUID PRIMARY KEY,
                role user_role NOT NULL,
                backup_roles user_role[]
            );
        ";
        let mut catalog = Catalog::default();
        catalog.apply_sql(sql).unwrap();

        assert_eq!(
            catalog.resolve_type("user_role"),
            "\"admin\" | \"editor\" | \"viewer\""
        );
        assert_eq!(
            catalog.resolve_type("user_role[]"),
            "(\"admin\" | \"editor\" | \"viewer\")[]"
        );

        let table = catalog.get_table("team_members").unwrap();
        let role_col = table.get_column("role").unwrap();
        assert_eq!(role_col.ts_type, "\"admin\" | \"editor\" | \"viewer\"");
        assert!(!role_col.is_nullable);

        let backup_col = table.get_column("backup_roles").unwrap();
        assert_eq!(
            backup_col.ts_type,
            "(\"admin\" | \"editor\" | \"viewer\")[]"
        );
        assert!(backup_col.is_nullable);
    }

    #[test]
    fn test_composite_type_ddl_and_array_columns() {
        let sql = "
            CREATE TYPE address AS (street text, city varchar(100), zip text);
            CREATE TYPE geo_point AS (lat float8, lng float8);
            CREATE TABLE places (
                id UUID PRIMARY KEY,
                home address,
                branches address[],
                loc geo_point,
                waypoints geo_point[]
            );
        ";
        let mut catalog = Catalog::default();
        catalog.apply_sql(sql).unwrap();

        assert!(catalog.composite_types.contains_key("address"));
        assert!(catalog.composite_types.contains_key("geo_point"));

        let addr_meta = &catalog.composite_types["address"];
        assert_eq!(addr_meta.attributes.len(), 3);
        assert_eq!(addr_meta.attributes[0].name, "street");
        assert_eq!(addr_meta.attributes[0].ts_type, "string");
        assert_eq!(addr_meta.attributes[1].name, "city");
        assert_eq!(addr_meta.attributes[1].ts_type, "string");
        assert_eq!(addr_meta.attributes[2].name, "zip");
        assert_eq!(addr_meta.attributes[2].ts_type, "string");

        assert_eq!(
            catalog.resolve_type("address"),
            "{ street: string; city: string; zip: string }"
        );
        assert_eq!(
            catalog.resolve_type("address[]"),
            "Array<{ street: string; city: string; zip: string }>"
        );
        assert_eq!(
            catalog.resolve_type("geo_point"),
            "{ lat: number; lng: number }"
        );
        assert_eq!(
            catalog.resolve_type("geo_point[]"),
            "Array<{ lat: number; lng: number }>"
        );

        let table = catalog.get_table("places").unwrap();
        let home_col = table.get_column("home").unwrap();
        assert_eq!(
            home_col.ts_type,
            "{ street: string; city: string; zip: string }"
        );
        assert!(home_col.is_nullable);

        let branches_col = table.get_column("branches").unwrap();
        assert_eq!(
            branches_col.ts_type,
            "Array<{ street: string; city: string; zip: string }>"
        );
        assert!(branches_col.is_nullable);

        let loc_col = table.get_column("loc").unwrap();
        assert_eq!(loc_col.ts_type, "{ lat: number; lng: number }");
        assert!(loc_col.is_nullable);

        let waypoints_col = table.get_column("waypoints").unwrap();
        assert_eq!(
            waypoints_col.ts_type,
            "Array<{ lat: number; lng: number }>"
        );
        assert!(waypoints_col.is_nullable);
    }

    #[test]
    fn test_domain_type_ddl_and_not_null_inheritance() {
        let sql = "
            CREATE DOMAIN email_addr AS text;
            CREATE DOMAIN non_empty_str AS varchar(255) NOT NULL;
            CREATE DOMAIN positive_num AS int DEFAULT 1;
            CREATE DOMAIN uuid_key AS uuid NOT NULL;
            CREATE TABLE accounts (
                id uuid_key,
                contact email_addr,
                title non_empty_str,
                score positive_num,
                backup_email email_addr NOT NULL
            );
        ";
        let mut catalog = Catalog::default();
        catalog.apply_sql(sql).unwrap();

        assert!(catalog.domains.contains_key("email_addr"));
        assert!(catalog.domains.contains_key("non_empty_str"));
        assert!(catalog.domains.contains_key("positive_num"));
        assert!(catalog.domains.contains_key("uuid_key"));

        let email_dom = &catalog.domains["email_addr"];
        assert_eq!(email_dom.base_type, "text");
        assert_eq!(email_dom.ts_type, "string");
        assert!(!email_dom.is_not_null);
        assert!(!email_dom.has_default);

        let non_empty_dom = &catalog.domains["non_empty_str"];
        assert_eq!(non_empty_dom.base_type, "varchar");
        assert_eq!(non_empty_dom.ts_type, "string");
        assert!(non_empty_dom.is_not_null);

        let pos_dom = &catalog.domains["positive_num"];
        assert_eq!(pos_dom.ts_type, "number");
        assert!(!pos_dom.is_not_null);
        assert!(pos_dom.has_default);

        assert_eq!(catalog.resolve_type("email_addr"), "string");
        assert_eq!(catalog.resolve_type("email_addr[]"), "string[]");
        assert_eq!(catalog.resolve_type("positive_num"), "number");

        let table = catalog.get_table("accounts").unwrap();

        // id has uuid_key NOT NULL -> inherits non-nullability
        let id_col = table.get_column("id").unwrap();
        assert_eq!(id_col.ts_type, "string");
        assert!(!id_col.is_nullable);

        // contact is regular domain without NOT NULL -> nullable
        let contact_col = table.get_column("contact").unwrap();
        assert_eq!(contact_col.ts_type, "string");
        assert!(contact_col.is_nullable);

        // title has non_empty_str NOT NULL -> inherits non-nullability
        let title_col = table.get_column("title").unwrap();
        assert_eq!(title_col.ts_type, "string");
        assert!(!title_col.is_nullable);

        // score has positive_num with DEFAULT -> inherits default
        let score_col = table.get_column("score").unwrap();
        assert_eq!(score_col.ts_type, "number");
        assert!(score_col.is_nullable);
        assert!(score_col.has_default);

        // backup_email has explicit NOT NULL
        let backup_col = table.get_column("backup_email").unwrap();
        assert_eq!(backup_col.ts_type, "string");
        assert!(!backup_col.is_nullable);
    }

    #[test]
    fn test_primary_key_metadata_tracking() {
        let sql = "
            CREATE TABLE single_pk (
                id UUID PRIMARY KEY,
                name TEXT NOT NULL
            );
            CREATE TABLE composite_pk (
                tenant_id UUID,
                user_id INT,
                role TEXT,
                PRIMARY KEY (tenant_id, user_id)
            );
            CREATE TABLE alter_pk (
                id INT NOT NULL,
                name TEXT
            );
        ";
        let mut catalog = Catalog::default();
        catalog.apply_sql(sql).unwrap();

        // 1. Single inline PK
        let single = catalog.get_table("single_pk").unwrap();
        assert_eq!(single.primary_keys, vec!["id"]);
        assert!(single.get_column("id").unwrap().is_primary_key);
        assert!(!single.get_column("id").unwrap().is_nullable);
        assert!(!single.get_column("name").unwrap().is_primary_key);

        // 2. Composite table-level PK
        let comp = catalog.get_table("composite_pk").unwrap();
        assert_eq!(comp.primary_keys, vec!["tenant_id", "user_id"]);
        let tenant_col = comp.get_column("tenant_id").unwrap();
        assert!(tenant_col.is_primary_key);
        assert!(!tenant_col.is_nullable);
        let user_col = comp.get_column("user_id").unwrap();
        assert!(user_col.is_primary_key);
        assert!(!user_col.is_nullable);
        assert!(!comp.get_column("role").unwrap().is_primary_key);

        // 3. Alter table add PK
        let alter = catalog.get_table("alter_pk").unwrap();
        assert!(alter.primary_keys.is_empty());
        assert!(!alter.get_column("id").unwrap().is_primary_key);

        catalog
            .apply_sql("ALTER TABLE alter_pk ADD PRIMARY KEY (id);")
            .unwrap();
        let alter = catalog.get_table("alter_pk").unwrap();
        assert_eq!(alter.primary_keys, vec!["id"]);
        assert!(alter.get_column("id").unwrap().is_primary_key);
        assert!(!alter.get_column("id").unwrap().is_nullable);

        // 4. Alter table add composite PK constraint
        catalog
            .apply_sql("CREATE TABLE tag_mapping (tag_id INT NOT NULL, post_id INT NOT NULL);")
            .unwrap();
        catalog
            .apply_sql("ALTER TABLE tag_mapping ADD CONSTRAINT pk_tag_map PRIMARY KEY (tag_id, post_id);")
            .unwrap();
        let tag_map = catalog.get_table("tag_mapping").unwrap();
        assert_eq!(tag_map.primary_keys, vec!["tag_id", "post_id"]);
        assert!(tag_map.get_column("tag_id").unwrap().is_primary_key);
        assert!(tag_map.get_column("post_id").unwrap().is_primary_key);
    }

    #[test]
    fn test_strict_schema_qualification_resolution() {
        let sql = "
            CREATE DOMAIN public.int4 AS text;
            CREATE TABLE test_types (
                builtin_val pg_catalog.int4,
                domain_val public.int4
            );
            CREATE TABLE schema1.t (
                id UUID PRIMARY KEY,
                col_one TEXT NOT NULL
            );
            CREATE TABLE schema2.t (
                id INT PRIMARY KEY,
                col_two INT NOT NULL
            );
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                name TEXT
            );
        ";
        let mut catalog = Catalog::default();
        catalog.apply_sql(sql).unwrap();

        // pg_catalog.int4 vs public.int4 resolution
        assert_eq!(catalog.resolve_type("pg_catalog.int4"), "number");
        assert_eq!(catalog.resolve_type("public.int4"), "string");

        let tt = catalog.get_table("test_types").unwrap();
        let b_col = tt.get_column("builtin_val").unwrap();
        assert_eq!(b_col.ts_type, "number");
        let d_col = tt.get_column("domain_val").unwrap();
        assert_eq!(d_col.ts_type, "string");

        // schema1.t vs schema2.t
        let s1 = catalog.get_table("schema1.t").unwrap();
        assert_eq!(s1.name, "t");
        assert_eq!(s1.schema.as_deref(), Some("schema1"));
        assert!(s1.get_column("col_one").is_some());
        assert!(s1.get_column("col_two").is_none());

        let s2 = catalog.get_table("schema2.t").unwrap();
        assert_eq!(s2.name, "t");
        assert_eq!(s2.schema.as_deref(), Some("schema2"));
        assert!(s2.get_column("col_two").is_some());
        assert!(s2.get_column("col_one").is_none());

        assert_eq!(
            catalog.get_table_qualified(Some("schema1"), "t").unwrap().columns[1].name,
            "col_one"
        );
        assert_eq!(
            catalog.get_table_qualified(Some("schema2"), "t").unwrap().columns[1].name,
            "col_two"
        );
        assert!(catalog.get_table_qualified(Some("schema3"), "t").is_none());

        // Strict schema lookup rejection:
        // "users" is in public / unqualified
        assert!(catalog.get_table("users").is_some());
        assert!(catalog.get_table("public.users").is_some());
        assert!(catalog.get_table("pg_catalog.users").is_none());
        assert!(catalog.get_table("other.users").is_none());
    }

    #[test]
    fn test_alter_table_with_composite_and_domain_types() {
        let sql = "
            CREATE TYPE money_amount AS (currency text, amount numeric);
            CREATE DOMAIN postal_code AS varchar(10) NOT NULL;
            CREATE TABLE accounts (
                id UUID PRIMARY KEY
            );
        ";
        let mut catalog = Catalog::default();
        catalog.apply_sql(sql).unwrap();

        // 1. Add column with composite type
        catalog
            .apply_sql("ALTER TABLE accounts ADD COLUMN balance money_amount;")
            .unwrap();
        let table = catalog.get_table("accounts").unwrap();
        let bal_col = table.get_column("balance").unwrap();
        assert_eq!(bal_col.ts_type, "{ currency: string; amount: number }");
        assert!(bal_col.is_nullable);

        // 2. Add column with domain type having NOT NULL
        catalog
            .apply_sql("ALTER TABLE accounts ADD COLUMN zip postal_code;")
            .unwrap();
        let table = catalog.get_table("accounts").unwrap();
        let zip_col = table.get_column("zip").unwrap();
        assert_eq!(zip_col.ts_type, "string");
        assert!(!zip_col.is_nullable);

        // 3. Alter column type
        catalog
            .apply_sql("ALTER TABLE accounts ALTER COLUMN zip TYPE text;")
            .unwrap();
        let table = catalog.get_table("accounts").unwrap();
        let zip_col = table.get_column("zip").unwrap();
        assert_eq!(zip_col.ts_type, "string");
        assert_eq!(zip_col.pg_type, "text");

        // 4. Drop column
        catalog
            .apply_sql("ALTER TABLE accounts DROP COLUMN balance;")
            .unwrap();
        let table = catalog.get_table("accounts").unwrap();
        assert!(table.get_column("balance").is_none());
    }
}

