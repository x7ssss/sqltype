use crate::catalog::{Catalog, ColumnMetadata, DriverTarget, TableMetadata, extract_type_name};
use pg_query::NodeEnum;
use pg_query::protobuf::{
    AExprKind, BoolExprType, JoinType, NullTestType, OnConflictAction, SetOperation, SubLinkType,
};
use std::collections::HashMap;

fn format_nullable(ts_type: &str) -> String {
    if ts_type == "unknown" {
        return "unknown".to_string();
    }
    if ts_type.contains("| null") {
        ts_type.to_string()
    } else {
        format!("{} | null", ts_type)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InferredType {
    pub pg_type: PgType,
    pub is_nullable: bool,
}

pub type InferredExpr = InferredType;

impl InferredType {
    pub fn to_typescript(&self, catalog: &Catalog) -> String {
        let base = self.pg_type.to_ts(catalog);
        if self.is_nullable {
            format_nullable(&base)
        } else {
            base
        }
    }

    pub fn to_ts(&self, catalog: &Catalog) -> String {
        self.to_typescript(catalog)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PgType {
    Unknown,
    Bool,
    Int2,
    Int4,
    Int8,
    Float4,
    Float8,
    Numeric,
    Text,
    Varchar,
    Uuid,
    Date,
    Timestamp,
    Timestamptz,
    Time,
    Timetz,
    Bytea,
    JsonRaw,
    JsonbRaw,
    JsonObject(Vec<(String, InferredType)>),
    JsonArray(Box<InferredType>),
    JsonDynamicObject,
    Custom(String),
    Array(Box<PgType>),
}

impl PgType {
    pub fn from_pg_str(s: &str) -> Self {
        let lower = s.to_ascii_lowercase();
        let trimmed = lower.trim();
        if let Some(inner) = trimmed.strip_suffix("[]") {
            return PgType::Array(Box::new(PgType::from_pg_str(inner)));
        }
        match trimmed {
            "int2" | "smallint" | "smallserial" => PgType::Int2,
            "int4" | "integer" | "int" | "serial" => PgType::Int4,
            "int8" | "bigint" | "bigserial" | "serial8" => PgType::Int8,
            "float4" | "real" => PgType::Float4,
            "float8" | "double precision" => PgType::Float8,
            "numeric" | "decimal" => PgType::Numeric,
            "bool" | "boolean" => PgType::Bool,
            "text" | "citext" => PgType::Text,
            "varchar" | "character varying" | "char" | "character" | "bpchar" => PgType::Varchar,
            "uuid" => PgType::Uuid,
            "date" => PgType::Date,
            "timestamp" | "timestamp without time zone" => PgType::Timestamp,
            "timestamptz" | "timestamp with time zone" => PgType::Timestamptz,
            "time" | "time without time zone" => PgType::Time,
            "timetz" | "time with time zone" => PgType::Timetz,
            "bytea" => PgType::Bytea,
            "json" => PgType::JsonRaw,
            "jsonb" => PgType::JsonbRaw,
            "unknown" => PgType::Unknown,
            other => PgType::Custom(other.to_string()),
        }
    }

    pub fn element_type(&self) -> Option<&PgType> {
        match self {
            PgType::Array(inner) => Some(inner.as_ref()),
            _ => None,
        }
    }

    pub fn to_array(&self) -> PgType {
        PgType::Array(Box::new(self.clone()))
    }

    pub fn to_ts(&self, catalog: &Catalog) -> String {
        match self {
            PgType::Unknown => "unknown".to_string(),
            PgType::Bool => "boolean".to_string(),
            PgType::Int2 | PgType::Int4 | PgType::Float4 | PgType::Float8 | PgType::Numeric => {
                "number".to_string()
            }
            PgType::Int8 => match catalog.driver {
                DriverTarget::Bun => "bigint".to_string(),
                DriverTarget::Postgres | DriverTarget::Pg => "string".to_string(),
            },
            PgType::Bytea => match catalog.driver {
                DriverTarget::Bun => "Uint8Array".to_string(),
                DriverTarget::Postgres | DriverTarget::Pg => "Buffer".to_string(),
            },
            PgType::Text | PgType::Varchar | PgType::Uuid => "string".to_string(),
            PgType::Date => "string".to_string(),
            PgType::Timestamp | PgType::Timestamptz => "Date".to_string(),
            PgType::Time | PgType::Timetz => "string".to_string(),
            PgType::JsonRaw | PgType::JsonbRaw => "unknown".to_string(),
            PgType::JsonDynamicObject => "Record<string, unknown>".to_string(),
            PgType::JsonArray(inner) => format!("Array<{}>", inner.to_typescript(catalog)),
            PgType::JsonObject(fields) => {
                if fields.is_empty() {
                    "Record<string, never>".to_string()
                } else {
                    let inner = fields
                        .iter()
                        .map(|(k, v)| {
                            format!(
                                "{}: {}",
                                crate::codegen::format_property_key(k),
                                v.to_typescript(catalog)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    format!("{{ {} }}", inner)
                }
            }
            PgType::Custom(name) => catalog.resolve_type(name),
            PgType::Array(inner) => {
                format!("Array<{}>", inner.to_ts(catalog))
            }
        }
    }

    pub fn to_pg_str(&self) -> String {
        match self {
            PgType::Unknown => "unknown".to_string(),
            PgType::Bool => "bool".to_string(),
            PgType::Int2 => "int2".to_string(),
            PgType::Int4 => "int4".to_string(),
            PgType::Int8 => "int8".to_string(),
            PgType::Float4 => "float4".to_string(),
            PgType::Float8 => "float8".to_string(),
            PgType::Numeric => "numeric".to_string(),
            PgType::Text => "text".to_string(),
            PgType::Varchar => "varchar".to_string(),
            PgType::Uuid => "uuid".to_string(),
            PgType::Date => "date".to_string(),
            PgType::Timestamp => "timestamp".to_string(),
            PgType::Timestamptz => "timestamptz".to_string(),
            PgType::Time => "time".to_string(),
            PgType::Timetz => "timetz".to_string(),
            PgType::Bytea => "bytea".to_string(),
            PgType::JsonRaw => "json".to_string(),
            PgType::JsonbRaw
            | PgType::JsonObject(_)
            | PgType::JsonArray(_)
            | PgType::JsonDynamicObject => "jsonb".to_string(),
            PgType::Custom(name) => name.clone(),
            PgType::Array(inner) => format!("{}[]", inner.to_pg_str()),
        }
    }
}

pub fn is_numeric_type(t: &PgType) -> bool {
    matches!(
        t,
        PgType::Int2
            | PgType::Int4
            | PgType::Int8
            | PgType::Float4
            | PgType::Float8
            | PgType::Numeric
    )
}

fn unify_numeric_types(a: &PgType, b: &PgType) -> PgType {
    if a == b {
        return a.clone();
    }
    if *a == PgType::Numeric || *b == PgType::Numeric {
        return PgType::Numeric;
    }

    let is_int_a = matches!(a, PgType::Int2 | PgType::Int4 | PgType::Int8);
    let is_int_b = matches!(b, PgType::Int2 | PgType::Int4 | PgType::Int8);

    if is_int_a && is_int_b {
        if *a == PgType::Int8 || *b == PgType::Int8 {
            return PgType::Int8;
        }
        if *a == PgType::Int4 || *b == PgType::Int4 {
            return PgType::Int4;
        }
        return PgType::Int2;
    }

    let is_float_a = matches!(a, PgType::Float4 | PgType::Float8);
    let is_float_b = matches!(b, PgType::Float4 | PgType::Float8);

    if is_float_a && is_float_b {
        return PgType::Float8;
    }

    if *a == PgType::Float8 || *b == PgType::Float8 {
        return PgType::Float8;
    }
    let int_type = if is_int_a { a } else { b };
    if *int_type == PgType::Int2 {
        PgType::Float4
    } else {
        PgType::Float8
    }
}

pub fn unify_types(a: &PgType, b: &PgType) -> Result<PgType, String> {
    if a == b {
        return Ok(a.clone());
    }

    // Unknown unifies to the concrete companion type
    if *a == PgType::Unknown {
        return Ok(b.clone());
    }
    if *b == PgType::Unknown {
        return Ok(a.clone());
    }

    // String hierarchy: Varchar / Uuid widen to Text
    let is_string_a = matches!(a, PgType::Text | PgType::Varchar | PgType::Uuid);
    let is_string_b = matches!(b, PgType::Text | PgType::Varchar | PgType::Uuid);
    if is_string_a && is_string_b {
        return Ok(PgType::Text);
    }

    // Datetime hierarchy: Date -> Timestamp -> Timestamptz
    let is_dt_a = matches!(a, PgType::Date | PgType::Timestamp | PgType::Timestamptz);
    let is_dt_b = matches!(b, PgType::Date | PgType::Timestamp | PgType::Timestamptz);
    if is_dt_a && is_dt_b {
        if *a == PgType::Timestamptz || *b == PgType::Timestamptz {
            return Ok(PgType::Timestamptz);
        }
        if *a == PgType::Timestamp || *b == PgType::Timestamp {
            return Ok(PgType::Timestamp);
        }
        return Ok(PgType::Date);
    }

    // Numeric hierarchy: Int2 -> Int4 -> Int8 -> Numeric; Float4 -> Float8 -> Numeric
    if is_numeric_type(a) && is_numeric_type(b) {
        return Ok(unify_numeric_types(a, b));
    }

    // Array types
    if let (PgType::Array(inner_a), PgType::Array(inner_b)) = (a, b) {
        let inner = unify_types(inner_a, inner_b)?;
        return Ok(PgType::Array(Box::new(inner)));
    }

    // JSON types hierarchy & unification
    if is_json_type(a) && is_json_type(b) {
        return unify_json_types(a, b);
    }

    Err(format!(
        "Cannot unify incompatible types: {:?} and {:?}",
        a, b
    ))
}

fn is_json_type(t: &PgType) -> bool {
    matches!(
        t,
        PgType::JsonRaw
            | PgType::JsonbRaw
            | PgType::JsonObject(_)
            | PgType::JsonArray(_)
            | PgType::JsonDynamicObject
    )
}

fn unify_json_types(a: &PgType, b: &PgType) -> Result<PgType, String> {
    if a == b {
        return Ok(a.clone());
    }

    // JsonArray with JsonArray
    if let (PgType::JsonArray(inner_a), PgType::JsonArray(inner_b)) = (a, b) {
        let unified_inner_pg = unify_types(&inner_a.pg_type, &inner_b.pg_type)?;
        let is_nullable = inner_a.is_nullable || inner_b.is_nullable;
        return Ok(PgType::JsonArray(Box::new(InferredType {
            pg_type: unified_inner_pg,
            is_nullable,
        })));
    }

    // JsonArray with raw json literal fallback (e.g. COALESCE(json_agg(...), '[]'::jsonb))
    if matches!(a, PgType::JsonArray(_)) && matches!(b, PgType::JsonRaw | PgType::JsonbRaw) {
        return Ok(a.clone());
    }
    if matches!(b, PgType::JsonArray(_)) && matches!(a, PgType::JsonRaw | PgType::JsonbRaw) {
        return Ok(b.clone());
    }

    // JsonObject with JsonObject
    if let (PgType::JsonObject(f_a), PgType::JsonObject(f_b)) = (a, b) {
        // If keys and lengths match, unify fields
        if f_a.len() == f_b.len() {
            let mut unified_fields = Vec::with_capacity(f_a.len());
            let mut all_matched = true;
            for (k_a, val_a) in f_a {
                if let Some((_, val_b)) = f_b.iter().find(|(k_b, _)| k_b == k_a) {
                    if let Ok(unified_val_pg) = unify_types(&val_a.pg_type, &val_b.pg_type) {
                        unified_fields.push((
                            k_a.clone(),
                            InferredType {
                                pg_type: unified_val_pg,
                                is_nullable: val_a.is_nullable || val_b.is_nullable,
                            },
                        ));
                    } else {
                        all_matched = false;
                        break;
                    }
                } else {
                    all_matched = false;
                    break;
                }
            }
            if all_matched {
                return Ok(PgType::JsonObject(unified_fields));
            }
        }
        return Ok(PgType::JsonDynamicObject);
    }

    // JsonObject with raw json fallback (e.g. COALESCE(obj, '{}'::jsonb))
    if matches!(a, PgType::JsonObject(_)) && matches!(b, PgType::JsonRaw | PgType::JsonbRaw) {
        return Ok(a.clone());
    }
    if matches!(b, PgType::JsonObject(_)) && matches!(a, PgType::JsonRaw | PgType::JsonbRaw) {
        return Ok(b.clone());
    }

    // JsonDynamicObject with JsonObject or raw
    if matches!(a, PgType::JsonDynamicObject)
        && matches!(
            b,
            PgType::JsonObject(_) | PgType::JsonRaw | PgType::JsonbRaw
        )
    {
        return Ok(PgType::JsonDynamicObject);
    }
    if matches!(b, PgType::JsonDynamicObject)
        && matches!(
            a,
            PgType::JsonObject(_) | PgType::JsonRaw | PgType::JsonbRaw
        )
    {
        return Ok(PgType::JsonDynamicObject);
    }

    // JsonRaw with JsonbRaw -> widen to JsonbRaw
    if matches!(a, PgType::JsonRaw | PgType::JsonbRaw)
        && matches!(b, PgType::JsonRaw | PgType::JsonbRaw)
    {
        if *a == PgType::JsonbRaw || *b == PgType::JsonbRaw {
            return Ok(PgType::JsonbRaw);
        }
        return Ok(PgType::JsonRaw);
    }

    // Any other mixed JSON types fall back to JsonbRaw
    Ok(PgType::JsonbRaw)
}

pub fn unify_cte_types(anchor: &PgType, rec: &PgType) -> Result<PgType, String> {
    unify_types(anchor, rec)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryParam {
    pub index: usize,
    pub name: String,
    pub ts_type: String,
    pub is_optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryField {
    pub name: String,
    pub ts_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzedQuery {
    pub name: String,
    pub raw_sql: String,
    pub params: Vec<QueryParam>,
    pub fields: Vec<QueryField>,
}

pub type ColumnMeta = ColumnMetadata;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    Inner,
    Left,
    Right,
    Full,
    Semi,
    Anti,
}

impl JoinKind {
    pub fn from_join_type(jt: JoinType) -> Self {
        match jt {
            JoinType::JoinInner => JoinKind::Inner,
            JoinType::JoinLeft => JoinKind::Left,
            JoinType::JoinRight => JoinKind::Right,
            JoinType::JoinFull => JoinKind::Full,
            JoinType::JoinSemi => JoinKind::Semi,
            JoinType::JoinAnti => JoinKind::Anti,
            _ => JoinKind::Inner,
        }
    }

    pub fn from_i32(val: i32) -> Self {
        if val == JoinType::JoinLeft as i32 {
            JoinKind::Left
        } else if val == JoinType::JoinRight as i32 {
            JoinKind::Right
        } else if val == JoinType::JoinFull as i32 {
            JoinKind::Full
        } else if val == JoinType::JoinSemi as i32 {
            JoinKind::Semi
        } else if val == JoinType::JoinAnti as i32 {
            JoinKind::Anti
        } else {
            JoinKind::Inner
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOpKind {
    None,
    Union,
    Intersect,
    Except,
}

impl From<SetOperation> for SetOpKind {
    fn from(op: SetOperation) -> Self {
        match op {
            SetOperation::SetopNone | SetOperation::Undefined => SetOpKind::None,
            SetOperation::SetopUnion => SetOpKind::Union,
            SetOperation::SetopIntersect => SetOpKind::Intersect,
            SetOperation::SetopExcept => SetOpKind::Except,
        }
    }
}

impl SetOpKind {
    pub fn from_i32(val: i32) -> Self {
        match SetOperation::try_from(val) {
            Ok(op) => SetOpKind::from(op),
            Err(_) => SetOpKind::None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TableBinding {
    pub base_table: String,
    pub exposed_name: String,
    pub has_explicit_alias: bool,
    pub is_null_padded: bool,
    pub columns: HashMap<String, ColumnMeta>,
}

impl TableBinding {
    pub fn get_column(&self, name: &str) -> Option<&ColumnMeta> {
        self.columns.get(name).or_else(|| {
            self.columns
                .values()
                .find(|c| c.name.eq_ignore_ascii_case(name))
        })
    }
}

pub type ScopeContext = QueryScope;

#[derive(Default, Debug, Clone)]
pub struct QueryScope {
    pub bindings: HashMap<String, TableBinding>,
    pub binding_order: Vec<String>,
    pub ctes: HashMap<String, TableBinding>,
    pub tables: HashMap<String, TableBinding>,
    pub cte_column_orders: HashMap<String, Vec<String>>,
}

impl QueryScope {
    pub fn with_ctes_from(active: &QueryScope) -> Self {
        Self {
            ctes: active.ctes.clone(),
            cte_column_orders: active.cte_column_orders.clone(),
            ..Default::default()
        }
    }

    pub fn force_all_nullable(&mut self) {
        for binding in self.bindings.values_mut() {
            binding.is_null_padded = true;
            for col in binding.columns.values_mut() {
                col.is_nullable = true;
            }
        }
        for binding in self.tables.values_mut() {
            binding.is_null_padded = true;
            for col in binding.columns.values_mut() {
                col.is_nullable = true;
            }
        }
    }

    pub fn add_binding(&mut self, binding: TableBinding) -> Result<(), String> {
        let key = binding.exposed_name.to_ascii_lowercase();
        if self.bindings.contains_key(&key) {
            return Err(format!(
                "Table name or alias \"{}\" specified more than once",
                binding.exposed_name
            ));
        }
        self.binding_order.push(binding.exposed_name.clone());
        self.bindings.insert(key.clone(), binding.clone());
        self.tables.insert(key, binding);
        Ok(())
    }

    pub fn register_cte(&mut self, binding: TableBinding) -> Result<(), String> {
        let key = binding.exposed_name.to_ascii_lowercase();
        if self.ctes.contains_key(&key) {
            return Err(format!(
                "WITH query name \"{}\" specified more than once",
                binding.exposed_name
            ));
        }
        self.ctes.insert(key, binding);
        Ok(())
    }

    pub fn merge(&mut self, other: QueryScope) -> Result<(), String> {
        for name in other.binding_order {
            if let Some(binding) = other.bindings.get(&name.to_ascii_lowercase()) {
                self.add_binding(binding.clone())?;
            }
        }
        for (k, v) in other.ctes {
            self.ctes.entry(k).or_insert(v);
        }
        for (k, v) in other.cte_column_orders {
            self.cte_column_orders.entry(k).or_insert(v);
        }
        Ok(())
    }

    pub fn resolve_column_ref(
        &self,
        cr: &pg_query::protobuf::ColumnRef,
        catalog: &Catalog,
    ) -> Result<(&ColumnMeta, bool), String> {
        if cr.fields.len() == 1 {
            let col_str = extract_string(&cr.fields[0])
                .ok_or_else(|| "Invalid column reference name".to_string())?;

            let mut matched: Vec<(&TableBinding, &ColumnMeta)> = Vec::new();
            for name in &self.binding_order {
                if let Some(binding) = self.bindings.get(&name.to_ascii_lowercase())
                    && let Some(col) = binding.get_column(&col_str)
                {
                    matched.push((binding, col));
                }
            }

            if matched.is_empty() {
                return Err(format!(
                    "Column \"{}\" not found in any table in scope",
                    col_str
                ));
            }
            if matched.len() > 1 {
                let non_excluded: Vec<_> = matched
                    .iter()
                    .filter(|(b, _)| !b.exposed_name.eq_ignore_ascii_case("excluded"))
                    .collect();
                if non_excluded.len() == 1 {
                    let (binding, col) = non_excluded[0];
                    let is_nullable = col.is_nullable || binding.is_null_padded;
                    return Ok((col, is_nullable));
                }

                let tables: Vec<String> = matched
                    .iter()
                    .map(|(b, _)| b.exposed_name.clone())
                    .collect();
                return Err(format!(
                    "Column reference \"{}\" is ambiguous. Present in tables: {}",
                    col_str,
                    tables.join(", ")
                ));
            }

            let (binding, col) = matched[0];
            let is_nullable = col.is_nullable || binding.is_null_padded;
            Ok((col, is_nullable))
        } else if cr.fields.len() == 2 {
            let target = extract_string(&cr.fields[0])
                .ok_or_else(|| "Invalid column reference alias".to_string())?;
            let col_str = extract_string(&cr.fields[1])
                .ok_or_else(|| "Invalid column reference name".to_string())?;

            // Check alias hiding: Cannot reference base table if it has an explicit alias
            for binding in self.bindings.values() {
                if binding.has_explicit_alias
                    && binding.base_table.eq_ignore_ascii_case(&target)
                    && !binding.exposed_name.eq_ignore_ascii_case(&target)
                {
                    return Err(format!(
                        "Cannot reference base table \"{}\" because it is aliased as \"{}\"",
                        target, binding.exposed_name
                    ));
                }
            }

            let binding = self
                .bindings
                .get(&target.to_ascii_lowercase())
                .ok_or_else(|| format!("Unknown table alias \"{}\" in column reference", target))?;

            let col = binding.get_column(&col_str).ok_or_else(|| {
                format!(
                    "Column \"{}\" not found on table \"{}\"",
                    col_str, binding.exposed_name
                )
            })?;

            let is_nullable = col.is_nullable || binding.is_null_padded;
            Ok((col, is_nullable))
        } else if cr.fields.len() == 3 {
            let schema_str = extract_string(&cr.fields[0])
                .ok_or_else(|| "Invalid schema in column reference".to_string())?;
            let table_str = extract_string(&cr.fields[1])
                .ok_or_else(|| "Invalid table in column reference".to_string())?;
            let col_str = extract_string(&cr.fields[2])
                .ok_or_else(|| "Invalid column name in column reference".to_string())?;

            // Check alias hiding: schema-qualified reference disallowed if table has an explicit alias
            for binding in self.bindings.values() {
                if binding.has_explicit_alias
                    && binding.base_table.eq_ignore_ascii_case(&table_str)
                    && !binding.exposed_name.eq_ignore_ascii_case(&table_str)
                {
                    return Err(format!(
                        "Cannot reference base table \"{}\" because it is aliased as \"{}\"",
                        table_str, binding.exposed_name
                    ));
                }
            }

            let binding = self
                .bindings
                .get(&table_str.to_ascii_lowercase())
                .ok_or_else(|| {
                    format!("Table \"{}.{}\" not found in scope", schema_str, table_str)
                })?;

            if let Some(table_meta) = catalog.get_table(&binding.base_table) {
                if let Some(tbl_schema) = &table_meta.schema {
                    if !tbl_schema.eq_ignore_ascii_case(&schema_str) {
                        return Err(format!(
                            "Schema mismatch for table \"{}\": expected \"{}\", got \"{}\"",
                            table_str, tbl_schema, schema_str
                        ));
                    }
                } else if !schema_str.eq_ignore_ascii_case("public") {
                    return Err(format!(
                        "Schema mismatch for table \"{}\": expected public, got \"{}\"",
                        table_str, schema_str
                    ));
                }
            }

            let col = binding.get_column(&col_str).ok_or_else(|| {
                format!(
                    "Column \"{}\" not found on table \"{}\"",
                    col_str, binding.exposed_name
                )
            })?;

            let is_nullable = col.is_nullable || binding.is_null_padded;
            Ok((col, is_nullable))
        } else {
            Err("Unsupported column reference format".to_string())
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ParamInfo {
    suggested_name: Option<String>,
    inferred_type: Option<String>,
    is_optional: bool,
}

/// Converts a string (e.g. "get_user_with_posts.sql" or "get_user") to PascalCase ("GetUserWithPosts").
pub fn to_pascal_case(s: &str) -> String {
    let s = s.strip_suffix(".sql").unwrap_or(s);
    let mut result = String::new();
    let mut capitalize_next = true;

    for c in s.chars() {
        if c == '_' || c == '-' || c == ' ' || c == '.' {
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }

    if result.is_empty() {
        "Query".to_string()
    } else {
        result
    }
}

/// Extracts query name from leading SQL comment `-- name: <Name>`, defaulting to filename or AnonymousQuery.
pub fn extract_query_name(sql: &str, fallback_filename: Option<&str>) -> String {
    for line in sql.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("--") {
            let after_dashes = trimmed.trim_start_matches('-').trim();
            if let Some(name_part) = after_dashes.strip_prefix("name:") {
                let name = name_part.trim();
                let query_name: String = name
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !query_name.is_empty() {
                    return query_name;
                }
            }
        } else if !trimmed.is_empty() {
            break;
        }
    }

    if let Some(filename) = fallback_filename {
        to_pascal_case(filename)
    } else {
        "AnonymousQuery".to_string()
    }
}

fn extract_string(node: &pg_query::protobuf::Node) -> Option<String> {
    if let Some(NodeEnum::String(s)) = &node.node {
        Some(s.sval.clone())
    } else {
        None
    }
}

fn extract_const_string(node: &pg_query::protobuf::Node) -> Option<String> {
    match &node.node {
        Some(NodeEnum::String(s)) => Some(s.sval.clone()),
        Some(NodeEnum::AConst(ac)) => {
            if let Some(pg_query::protobuf::a_const::Val::Sval(s)) = &ac.val {
                Some(s.sval.clone())
            } else {
                None
            }
        }
        Some(NodeEnum::TypeCast(tc)) => {
            if let Some(arg) = &tc.arg {
                extract_const_string(arg)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn extract_static_string_array(node: &pg_query::protobuf::Node) -> Option<Vec<String>> {
    match &node.node {
        Some(NodeEnum::AArrayExpr(aae)) => {
            let mut result = Vec::new();
            for el in &aae.elements {
                let s = extract_const_string(el)?;
                result.push(s);
            }
            Some(result)
        }
        Some(NodeEnum::TypeCast(tc)) => {
            if let Some(arg) = &tc.arg {
                extract_static_string_array(arg)
            } else {
                None
            }
        }
        Some(NodeEnum::AConst(ac)) => {
            if let Some(pg_query::protobuf::a_const::Val::Sval(s)) = &ac.val {
                let trimmed = s.sval.trim();
                if trimmed.starts_with('{') && trimmed.ends_with('}') {
                    let inner = &trimmed[1..trimmed.len() - 1];
                    let keys: Vec<String> = inner
                        .split(',')
                        .map(|k| k.trim().trim_matches('\"').trim_matches('\'').to_string())
                        .filter(|k| !k.is_empty())
                        .collect();
                    Some(keys)
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

fn extract_func_name(funcnames: &[pg_query::protobuf::Node]) -> String {
    let mut names = Vec::new();
    for f in funcnames {
        if let Some(s) = extract_string(f) {
            names.push(s);
        }
    }
    names
        .last()
        .cloned()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Analyzes an application SQL query file against the catalog.
pub fn analyze_query(
    sql: &str,
    catalog: &Catalog,
    fallback_filename: Option<&str>,
) -> Result<AnalyzedQuery, String> {
    let query_name = extract_query_name(sql, fallback_filename);
    let parsed = pg_query::parse(sql).map_err(|e| format!("Query parse error: {}", e))?;

    // Find the query statement
    let mut root_stmt: Option<&pg_query::protobuf::Node> = None;
    for stmt in &parsed.protobuf.stmts {
        if let Some(node) = &stmt.stmt
            && matches!(
                &node.node,
                Some(
                    NodeEnum::SelectStmt(_)
                        | NodeEnum::InsertStmt(_)
                        | NodeEnum::UpdateStmt(_)
                        | NodeEnum::DeleteStmt(_)
                )
            )
        {
            root_stmt = Some(node);
            break;
        }
    }

    let root = root_stmt.ok_or_else(|| {
        "No supported statement (SELECT, INSERT, UPDATE, DELETE) found in query".to_string()
    })?;
    let mut param_map: HashMap<i32, ParamInfo> = HashMap::new();

    let fields = match &root.node {
        Some(NodeEnum::SelectStmt(select)) => analyze_select_stmt(select, catalog, &mut param_map)?,
        Some(NodeEnum::InsertStmt(insert)) => analyze_insert_stmt(insert, catalog, &mut param_map)?,
        Some(NodeEnum::UpdateStmt(update)) => analyze_update_stmt(update, catalog, &mut param_map)?,
        Some(NodeEnum::DeleteStmt(delete)) => analyze_delete_stmt(delete, catalog, &mut param_map)?,
        _ => unreachable!(),
    };

    // Sort parameters deterministically by index (1..=N)
    let mut param_indices: Vec<i32> = param_map.keys().copied().collect();
    param_indices.sort();

    let mut params = Vec::new();
    let mut used_names: HashMap<String, usize> = HashMap::new();

    for idx in param_indices {
        let info = param_map.get(&idx).unwrap();
        let base_name = info
            .suggested_name
            .clone()
            .unwrap_or_else(|| format!("param{}", idx));

        // Deduplicate parameter names if multiple parameters share the same column name
        let count = used_names.entry(base_name.clone()).or_insert(0);
        *count += 1;
        let final_name = if *count > 1 {
            format!("{}{}", base_name, count)
        } else {
            base_name
        };

        params.push(QueryParam {
            index: idx as usize,
            name: final_name,
            ts_type: info
                .inferred_type
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            is_optional: info.is_optional,
        });
    }

    Ok(AnalyzedQuery {
        name: query_name,
        raw_sql: sql.to_string(),
        params,
        fields,
    })
}

pub fn process_with_clause(
    with_clause: &pg_query::protobuf::WithClause,
    scope: &mut ScopeContext,
    catalog: &Catalog,
) -> Result<(), String> {
    let mut param_map = HashMap::new();
    process_with_clause_with_params(with_clause, scope, catalog, &mut param_map)
}

fn process_with_clause_with_params(
    with_clause: &pg_query::protobuf::WithClause,
    scope: &mut ScopeContext,
    catalog: &Catalog,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    let mut seen_in_this_with = std::collections::HashSet::new();

    for cte_node in &with_clause.ctes {
        if let Some(NodeEnum::CommonTableExpr(cte)) = &cte_node.node {
            let cte_name = cte.ctename.to_ascii_lowercase();
            if !seen_in_this_with.insert(cte_name.clone()) {
                return Err(format!(
                    "WITH query name \"{}\" specified more than once",
                    cte.ctename
                ));
            }

            let query_node = cte
                .ctequery
                .as_ref()
                .ok_or_else(|| format!("WITH query \"{}\" missing query body", cte.ctename))?;
            let cte_select = match &query_node.node {
                Some(NodeEnum::SelectStmt(s)) => s,
                _ => {
                    return Err(format!(
                        "WITH query \"{}\" body is not a SelectStmt",
                        cte.ctename
                    ));
                }
            };

            // Check if recursive CTE: with_clause.recursive is true and query is a SetopUnion with larg & rarg
            let is_recursive_cte = with_clause.recursive
                && SetOpKind::from(cte_select.op()) == SetOpKind::Union
                && cte_select.larg.is_some()
                && cte_select.rarg.is_some();

            if is_recursive_cte {
                let anchor_stmt = cte_select.larg.as_ref().unwrap();
                let rec_stmt = cte_select.rarg.as_ref().unwrap();

                // 1. Evaluate anchor term
                let mut anchor_cols =
                    infer_select_projected_columns(anchor_stmt, catalog, scope, param_map)?;

                // 2. Handle positional aliasing on anchor columns if aliascolnames provided
                if !cte.aliascolnames.is_empty() {
                    if cte.aliascolnames.len() != anchor_cols.len() {
                        return Err(format!(
                            "table \"{}\" has {} columns available but {} columns specified",
                            cte.ctename,
                            anchor_cols.len(),
                            cte.aliascolnames.len()
                        ));
                    }
                    for (i, alias_node) in cte.aliascolnames.iter().enumerate() {
                        if let Some(alias_name) = extract_string(alias_node) {
                            anchor_cols[i].name = alias_name;
                        }
                    }
                }

                // 3. Synthesize stub TableBinding for recursive term
                let mut stub_cols = HashMap::new();
                let mut stub_order = Vec::new();
                for col in &anchor_cols {
                    stub_cols.insert(col.name.to_ascii_lowercase(), col.clone());
                    stub_order.push(col.name.clone());
                }
                let stub_binding = TableBinding {
                    base_table: cte_name.clone(),
                    exposed_name: cte.ctename.clone(),
                    has_explicit_alias: false,
                    is_null_padded: false,
                    columns: stub_cols,
                };

                let mut rec_scope = scope.clone();
                rec_scope.register_cte(stub_binding)?;
                rec_scope
                    .cte_column_orders
                    .insert(cte_name.clone(), stub_order.clone());

                // 4. Evaluate recursive term against stubbed scope
                let rec_cols =
                    infer_select_projected_columns(rec_stmt, catalog, &rec_scope, param_map)?;

                // 5. Validate column counts match
                if anchor_cols.len() != rec_cols.len() {
                    return Err(format!(
                        "Recursive query \"{}\" column count mismatch: anchor term has {}, recursive term has {}",
                        cte.ctename,
                        anchor_cols.len(),
                        rec_cols.len()
                    ));
                }

                // 6. Unify types and combine nullability
                let mut final_columns = HashMap::new();
                let mut final_order = Vec::new();

                for (i, anchor_col) in anchor_cols.iter().enumerate() {
                    let rec_col = &rec_cols[i];
                    let anchor_pg = PgType::from_pg_str(&anchor_col.pg_type);
                    let rec_pg = PgType::from_pg_str(&rec_col.pg_type);
                    let unified_pg = unify_cte_types(&anchor_pg, &rec_pg)?;
                    let is_nullable = anchor_col.is_nullable || rec_col.is_nullable;
                    let ts_type = unified_pg.to_ts(catalog);
                    let col_meta = ColumnMetadata {
                        name: anchor_col.name.clone(),
                        pg_type: unified_pg.to_pg_str(),
                        ts_type,
                        is_nullable,
                        has_default: false,
                    };
                    final_columns.insert(anchor_col.name.to_ascii_lowercase(), col_meta);
                    final_order.push(anchor_col.name.clone());
                }

                let resolved_binding = TableBinding {
                    base_table: cte_name.clone(),
                    exposed_name: cte.ctename.clone(),
                    has_explicit_alias: false,
                    is_null_padded: false,
                    columns: final_columns,
                };
                scope.register_cte(resolved_binding)?;
                scope.cte_column_orders.insert(cte_name, final_order);
            } else {
                // Standard CTE
                let mut proj_cols =
                    infer_select_projected_columns(cte_select, catalog, scope, param_map)?;

                if !cte.aliascolnames.is_empty() {
                    if cte.aliascolnames.len() != proj_cols.len() {
                        return Err(format!(
                            "table \"{}\" has {} columns available but {} columns specified",
                            cte.ctename,
                            proj_cols.len(),
                            cte.aliascolnames.len()
                        ));
                    }
                    for (i, alias_node) in cte.aliascolnames.iter().enumerate() {
                        if let Some(alias_name) = extract_string(alias_node) {
                            proj_cols[i].name = alias_name;
                        }
                    }
                }

                let mut final_columns = HashMap::new();
                let mut final_order = Vec::new();
                for col in proj_cols {
                    final_columns.insert(col.name.to_ascii_lowercase(), col.clone());
                    final_order.push(col.name);
                }

                let resolved_binding = TableBinding {
                    base_table: cte_name.clone(),
                    exposed_name: cte.ctename.clone(),
                    has_explicit_alias: false,
                    is_null_padded: false,
                    columns: final_columns,
                };
                scope.register_cte(resolved_binding)?;
                scope.cte_column_orders.insert(cte_name, final_order);
            }
        }
    }

    Ok(())
}

fn validate_setop_sort_clause(
    sort_clause: &[pg_query::protobuf::Node],
    projected_columns: &[ColumnMetadata],
) -> Result<(), String> {
    for item in sort_clause {
        let sb = match &item.node {
            Some(NodeEnum::SortBy(sb)) => sb,
            _ => {
                return Err(
                    "Only output column names or 1-based ordinals are allowed in set operation ORDER BY".to_string(),
                );
            }
        };

        let node = match &sb.node {
            Some(n) => n,
            None => {
                return Err(
                    "Only output column names or 1-based ordinals are allowed in set operation ORDER BY".to_string(),
                );
            }
        };

        match &node.node {
            Some(NodeEnum::ColumnRef(cr)) => {
                if cr.fields.len() > 1 {
                    let full_name: Vec<String> =
                        cr.fields.iter().filter_map(extract_string).collect();
                    return Err(format!(
                        "Qualified column name \"{}\" is not allowed in set operation ORDER BY",
                        full_name.join(".")
                    ));
                }
                if let Some(first) = cr.fields.first()
                    && let Some(col_name) = extract_string(first)
                    && !projected_columns
                        .iter()
                        .any(|c| c.name.eq_ignore_ascii_case(&col_name))
                {
                    return Err(format!(
                        "Column \"{}\" in ORDER BY does not exist in set operation result",
                        col_name
                    ));
                }
            }
            Some(NodeEnum::AConst(ac)) => {
                if let Some(pg_query::protobuf::a_const::Val::Ival(int_val)) = &ac.val {
                    let pos = int_val.ival;
                    if pos < 1 || pos as usize > projected_columns.len() {
                        return Err(format!(
                            "ORDER BY position {} is out of range: must be between 1 and {}",
                            pos,
                            projected_columns.len()
                        ));
                    }
                } else {
                    return Err(
                        "Only output column names or 1-based ordinals are allowed in set operation ORDER BY".to_string(),
                    );
                }
            }
            _ => {
                return Err(
                    "Only output column names or 1-based ordinals are allowed in set operation ORDER BY".to_string(),
                );
            }
        }
    }
    Ok(())
}

fn infer_select_projected_columns(
    select: &pg_query::protobuf::SelectStmt,
    catalog: &Catalog,
    parent_scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<Vec<ColumnMetadata>, String> {
    let op_kind = SetOpKind::from(select.op());
    if op_kind != SetOpKind::None {
        let mut query_scope = QueryScope::with_ctes_from(parent_scope);
        if let Some(wc) = &select.with_clause {
            process_with_clause_with_params(wc, &mut query_scope, catalog, param_map)?;
        }

        let left_stmt = select
            .larg
            .as_ref()
            .ok_or_else(|| format!("Set operation {:?} missing left query", op_kind))?;
        let right_stmt = select
            .rarg
            .as_ref()
            .ok_or_else(|| format!("Set operation {:?} missing right query", op_kind))?;

        let left_cols =
            infer_select_projected_columns(left_stmt, catalog, &query_scope, param_map)?;
        let right_cols =
            infer_select_projected_columns(right_stmt, catalog, &query_scope, param_map)?;

        if left_cols.len() != right_cols.len() {
            return Err(format!(
                "Each {:?} query must have the same number of columns: expected {}, found {}",
                op_kind,
                left_cols.len(),
                right_cols.len()
            ));
        }

        let mut unified_cols = Vec::with_capacity(left_cols.len());
        for (i, left_col) in left_cols.iter().enumerate() {
            let right_col = &right_cols[i];
            let left_pg = PgType::from_pg_str(&left_col.pg_type);
            let right_pg = PgType::from_pg_str(&right_col.pg_type);
            let unified_pg = unify_types(&left_pg, &right_pg)?;

            let is_nullable = match op_kind {
                SetOpKind::Union => left_col.is_nullable || right_col.is_nullable,
                SetOpKind::Intersect | SetOpKind::Except => left_col.is_nullable,
                SetOpKind::None => unreachable!(),
            };

            let ts_type = unified_pg.to_ts(catalog);
            unified_cols.push(ColumnMetadata {
                name: left_col.name.clone(),
                pg_type: unified_pg.to_pg_str(),
                ts_type,
                is_nullable,
                has_default: false,
            });
        }

        if !select.sort_clause.is_empty() {
            validate_setop_sort_clause(&select.sort_clause, &unified_cols)?;
        }

        if let Some(limit_node) = &select.limit_count {
            let param_num = if let Some(NodeEnum::ParamRef(p)) = &limit_node.node {
                Some(p.number)
            } else {
                extract_param_info(limit_node).map(|(n, _)| n)
            };
            if let Some(num) = param_num {
                let entry = param_map.entry(num).or_default();
                if entry.suggested_name.is_none() {
                    entry.suggested_name = Some("limit".to_string());
                }
                if entry.inferred_type.is_none() {
                    entry.inferred_type = Some("number".to_string());
                }
            }
        }

        if let Some(offset_node) = &select.limit_offset {
            let param_num = if let Some(NodeEnum::ParamRef(p)) = &offset_node.node {
                Some(p.number)
            } else {
                extract_param_info(offset_node).map(|(n, _)| n)
            };
            if let Some(num) = param_num {
                let entry = param_map.entry(num).or_default();
                if entry.suggested_name.is_none() {
                    entry.suggested_name = Some("offset".to_string());
                }
                if entry.inferred_type.is_none() {
                    entry.inferred_type = Some("number".to_string());
                }
            }
        }

        return Ok(unified_cols);
    }

    // 0. Register CTEs from with_clause
    let mut query_scope = QueryScope::with_ctes_from(parent_scope);

    if let Some(wc) = &select.with_clause {
        process_with_clause_with_params(wc, &mut query_scope, catalog, param_map)?;
    }

    // 1. Build QueryScope from from_clause
    let mut catalog_mut = catalog.clone();
    for from_item in &select.from_clause {
        let item_scope =
            resolve_from_clause_node(from_item, &mut catalog_mut, &query_scope, param_map)?;
        query_scope.merge(item_scope)?;
    }

    // 2. Resolve projections (target_list)
    let mut columns: Vec<ColumnMetadata> = Vec::new();
    for target in &select.target_list {
        if let Some(NodeEnum::ResTarget(rt)) = &target.node {
            resolve_target_columns(rt, catalog, &query_scope, &mut columns)?;
            if let Some(val) = &rt.val {
                resolve_params_in_expr(val, catalog, &query_scope, param_map)?;
            }
        }
    }

    // 3. Resolve parameters from WHERE clause (and limit / offset if present)
    if let Some(where_node) = &select.where_clause {
        resolve_params_in_expr(where_node, catalog, &query_scope, param_map)?;
    }

    if let Some(limit_node) = &select.limit_count {
        let param_num = if let Some(NodeEnum::ParamRef(p)) = &limit_node.node {
            Some(p.number)
        } else {
            extract_param_info(limit_node).map(|(n, _)| n)
        };
        if let Some(num) = param_num {
            let entry = param_map.entry(num).or_default();
            if entry.suggested_name.is_none() {
                entry.suggested_name = Some("limit".to_string());
            }
            if entry.inferred_type.is_none() {
                entry.inferred_type = Some("number".to_string());
            }
        }
    }

    if let Some(offset_node) = &select.limit_offset {
        let param_num = if let Some(NodeEnum::ParamRef(p)) = &offset_node.node {
            Some(p.number)
        } else {
            extract_param_info(offset_node).map(|(n, _)| n)
        };
        if let Some(num) = param_num {
            let entry = param_map.entry(num).or_default();
            if entry.suggested_name.is_none() {
                entry.suggested_name = Some("offset".to_string());
            }
            if entry.inferred_type.is_none() {
                entry.inferred_type = Some("number".to_string());
            }
        }
    }

    Ok(columns)
}

fn analyze_select_stmt(
    select: &pg_query::protobuf::SelectStmt,
    catalog: &Catalog,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<Vec<QueryField>, String> {
    let empty_scope = QueryScope::default();
    let cols = infer_select_projected_columns(select, catalog, &empty_scope, param_map)?;
    let fields = cols
        .into_iter()
        .map(|col| {
            let ts_type = if col.is_nullable {
                format_nullable(&col.ts_type)
            } else {
                col.ts_type
            };
            QueryField {
                name: col.name,
                ts_type,
            }
        })
        .collect();
    Ok(fields)
}

fn collect_param_refs(node: &pg_query::protobuf::Node, out: &mut Vec<i32>) {
    match &node.node {
        Some(NodeEnum::ParamRef(p)) => {
            out.push(p.number);
        }
        Some(NodeEnum::AExpr(ae)) => {
            if let Some(l) = &ae.lexpr {
                collect_param_refs(l, out);
            }
            if let Some(r) = &ae.rexpr {
                collect_param_refs(r, out);
            }
        }
        Some(NodeEnum::TypeCast(tc)) => {
            if let Some(arg) = &tc.arg {
                collect_param_refs(arg, out);
            }
        }
        Some(NodeEnum::FuncCall(fc)) => {
            for arg in &fc.args {
                collect_param_refs(arg, out);
            }
        }
        Some(NodeEnum::CoalesceExpr(ce)) => {
            for arg in &ce.args {
                collect_param_refs(arg, out);
            }
        }
        Some(NodeEnum::CaseExpr(ce)) => {
            for arg in &ce.args {
                collect_param_refs(arg, out);
            }
            if let Some(def) = &ce.defresult {
                collect_param_refs(def, out);
            }
        }
        Some(NodeEnum::CaseWhen(cw)) => {
            if let Some(expr) = &cw.expr {
                collect_param_refs(expr, out);
            }
            if let Some(res) = &cw.result {
                collect_param_refs(res, out);
            }
        }
        Some(NodeEnum::BoolExpr(be)) => {
            for arg in &be.args {
                collect_param_refs(arg, out);
            }
        }
        Some(NodeEnum::NullTest(nt)) => {
            if let Some(arg) = &nt.arg {
                collect_param_refs(arg, out);
            }
        }
        Some(NodeEnum::BooleanTest(bt)) => {
            if let Some(arg) = &bt.arg {
                collect_param_refs(arg, out);
            }
        }
        _ => {}
    }
}

fn bind_untyped_params_in_node(
    node: &pg_query::protobuf::Node,
    target_col: &ColumnMeta,
    param_map: &mut HashMap<i32, ParamInfo>,
) {
    let mut param_nums = Vec::new();
    collect_param_refs(node, &mut param_nums);
    for num in param_nums {
        let entry = param_map.entry(num).or_default();
        if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
            entry.inferred_type = Some(target_col.ts_type.clone());
        }
        if entry.suggested_name.is_none() {
            entry.suggested_name = Some(target_col.name.clone());
        }
        if target_col.is_nullable {
            entry.is_optional = true;
        }
    }
}

fn analyze_insert_stmt(
    insert: &pg_query::protobuf::InsertStmt,
    catalog: &Catalog,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<Vec<QueryField>, String> {
    let mut scope = QueryScope::default();
    if let Some(wc) = &insert.with_clause {
        process_with_clause_with_params(wc, &mut scope, catalog, param_map)?;
    }

    let rel = insert
        .relation
        .as_ref()
        .ok_or_else(|| "INSERT statement missing target relation".to_string())?;
    let table_name = rel.relname.to_ascii_lowercase();
    let table_meta = catalog
        .get_table(&table_name)
        .ok_or_else(|| format!("Table \"{}\" does not exist in schema catalog", rel.relname))?
        .clone();

    let (has_explicit_alias, exposed_name) = if let Some(a) = &rel.alias {
        (true, a.aliasname.clone())
    } else {
        (false, table_name.clone())
    };

    let mut columns = HashMap::new();
    for col in &table_meta.columns {
        columns.insert(col.name.to_ascii_lowercase(), col.clone());
    }

    let binding = TableBinding {
        base_table: table_name.clone(),
        exposed_name,
        has_explicit_alias,
        is_null_padded: false,
        columns,
    };

    scope.add_binding(binding)?;

    // Determine target column names
    let target_col_names: Vec<String> = if !insert.cols.is_empty() {
        let mut cols = Vec::new();
        for col_node in &insert.cols {
            if let Some(NodeEnum::ResTarget(rt)) = &col_node.node {
                if table_meta.get_column(&rt.name).is_none() {
                    return Err(format!(
                        "Column \"{}\" does not exist on table \"{}\"",
                        rt.name, table_name
                    ));
                }
                cols.push(rt.name.clone());
            }
        }
        cols
    } else {
        table_meta.columns.iter().map(|c| c.name.clone()).collect()
    };

    // Process values / parameters from select_stmt
    if let Some(select_node) = &insert.select_stmt
        && let Some(NodeEnum::SelectStmt(select)) = &select_node.node
    {
        if !select.values_lists.is_empty() {
            for row_node in &select.values_lists {
                if let Some(NodeEnum::List(row_list)) = &row_node.node {
                    for (i, val_node) in row_list.items.iter().enumerate() {
                        if let Some(col_name) = target_col_names.get(i) {
                            let col_meta = table_meta.get_column(col_name);
                            let is_optional = col_meta
                                .map(|c| c.is_nullable || c.has_default)
                                .unwrap_or(false);

                            if let Some((param_num, cast_opt)) = extract_param_info(val_node) {
                                record_param(
                                    param_num,
                                    cast_opt,
                                    None,
                                    col_name,
                                    catalog,
                                    &scope,
                                    param_map,
                                    is_optional,
                                )?;
                            } else {
                                resolve_params_in_expr(val_node, catalog, &scope, param_map)?;
                                if let Some(target_col) = col_meta {
                                    bind_untyped_params_in_node(val_node, target_col, param_map);
                                }
                            }
                        }
                    }
                }
            }
        } else {
            infer_select_projected_columns(select, catalog, &scope, param_map)?;
        }
    }

    // Process on_conflict_clause if present
    if let Some(occ) = &insert.on_conflict_clause {
        let is_update = occ.action == OnConflictAction::OnconflictUpdate as i32;
        let conflict_scope = if is_update {
            let mut cs = scope.clone();
            let mut excluded_cols = HashMap::new();
            for col in &table_meta.columns {
                excluded_cols.insert(col.name.to_ascii_lowercase(), col.clone());
            }
            let excluded_binding = TableBinding {
                base_table: "excluded".to_string(),
                exposed_name: "excluded".to_string(),
                has_explicit_alias: false,
                is_null_padded: false,
                columns: excluded_cols,
            };
            cs.add_binding(excluded_binding)?;
            cs
        } else {
            scope.clone()
        };

        if let Some(infer) = &occ.infer
            && let Some(where_node) = &infer.where_clause
        {
            resolve_params_in_expr(where_node, catalog, &scope, param_map)?;
        }

        for target in &occ.target_list {
            if let Some(NodeEnum::ResTarget(rt)) = &target.node {
                let col_name = &rt.name;
                let target_col = table_meta.get_column(col_name).ok_or_else(|| {
                    format!(
                        "Column \"{}\" does not exist on table \"{}\"",
                        col_name, table_name
                    )
                })?;
                let is_optional = target_col.is_nullable;
                if let Some(val_node) = &rt.val {
                    if let Some((param_num, cast_opt)) = extract_param_info(val_node) {
                        record_param(
                            param_num,
                            cast_opt,
                            None,
                            col_name,
                            catalog,
                            &conflict_scope,
                            param_map,
                            is_optional,
                        )?;
                    } else {
                        resolve_params_in_expr(val_node, catalog, &conflict_scope, param_map)?;
                        bind_untyped_params_in_node(val_node, target_col, param_map);
                    }
                }
            }
        }
        if let Some(where_node) = &occ.where_clause {
            resolve_params_in_expr(where_node, catalog, &conflict_scope, param_map)?;
        }
    }

    // Resolve RETURNING list if present
    let mut fields = Vec::new();
    for rt_node in &insert.returning_list {
        if let Some(NodeEnum::ResTarget(rt)) = &rt_node.node {
            resolve_target(rt, catalog, &scope, &mut fields)?;
            if let Some(val) = &rt.val {
                resolve_params_in_expr(val, catalog, &scope, param_map)?;
            }
        }
    }

    Ok(fields)
}

fn analyze_update_stmt(
    update: &pg_query::protobuf::UpdateStmt,
    catalog: &Catalog,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<Vec<QueryField>, String> {
    let mut scope = QueryScope::default();
    if let Some(wc) = &update.with_clause {
        process_with_clause_with_params(wc, &mut scope, catalog, param_map)?;
    }

    let rel = update
        .relation
        .as_ref()
        .ok_or_else(|| "UPDATE statement missing target relation".to_string())?;
    let table_name = rel.relname.to_ascii_lowercase();
    let table_meta = catalog
        .get_table(&table_name)
        .ok_or_else(|| format!("Table \"{}\" does not exist in schema catalog", rel.relname))?
        .clone();

    let (has_explicit_alias, exposed_name) = if let Some(a) = &rel.alias {
        (true, a.aliasname.clone())
    } else {
        (false, table_name.clone())
    };

    let mut columns = HashMap::new();
    for col in &table_meta.columns {
        columns.insert(col.name.to_ascii_lowercase(), col.clone());
    }

    let binding = TableBinding {
        base_table: table_name.clone(),
        exposed_name,
        has_explicit_alias,
        is_null_padded: false,
        columns,
    };

    scope.add_binding(binding)?;

    let mut catalog_mut = catalog.clone();
    for from_item in &update.from_clause {
        let item_scope = resolve_from_clause_node(from_item, &mut catalog_mut, &scope, param_map)?;
        scope.merge(item_scope)?;
    }

    // 1. Column updates in target_list
    for target in &update.target_list {
        if let Some(NodeEnum::ResTarget(rt)) = &target.node {
            let col_name = &rt.name;
            let target_col = table_meta.get_column(col_name).ok_or_else(|| {
                format!(
                    "Column \"{}\" does not exist on table \"{}\"",
                    col_name, table_name
                )
            })?;
            let is_optional = target_col.is_nullable;
            if let Some(val_node) = &rt.val {
                if let Some((param_num, cast_opt)) = extract_param_info(val_node) {
                    record_param(
                        param_num,
                        cast_opt,
                        None,
                        col_name,
                        catalog,
                        &scope,
                        param_map,
                        is_optional,
                    )?;
                } else {
                    resolve_params_in_expr(val_node, catalog, &scope, param_map)?;
                    bind_untyped_params_in_node(val_node, target_col, param_map);
                }
            }
        }
    }

    // 2. WHERE clause
    if let Some(where_node) = &update.where_clause {
        resolve_params_in_expr(where_node, catalog, &scope, param_map)?;
    }

    // 3. RETURNING list
    let mut fields = Vec::new();
    for rt_node in &update.returning_list {
        if let Some(NodeEnum::ResTarget(rt)) = &rt_node.node {
            resolve_target(rt, catalog, &scope, &mut fields)?;
            if let Some(val) = &rt.val {
                resolve_params_in_expr(val, catalog, &scope, param_map)?;
            }
        }
    }

    Ok(fields)
}

fn analyze_delete_stmt(
    delete: &pg_query::protobuf::DeleteStmt,
    catalog: &Catalog,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<Vec<QueryField>, String> {
    let mut scope = QueryScope::default();
    if let Some(wc) = &delete.with_clause {
        process_with_clause_with_params(wc, &mut scope, catalog, param_map)?;
    }

    let rel = delete
        .relation
        .as_ref()
        .ok_or_else(|| "DELETE statement missing target relation".to_string())?;
    let table_name = rel.relname.to_ascii_lowercase();
    let table_meta = catalog
        .get_table(&table_name)
        .ok_or_else(|| format!("Table \"{}\" does not exist in schema catalog", rel.relname))?
        .clone();

    let (has_explicit_alias, exposed_name) = if let Some(a) = &rel.alias {
        (true, a.aliasname.clone())
    } else {
        (false, table_name.clone())
    };

    let mut columns = HashMap::new();
    for col in &table_meta.columns {
        columns.insert(col.name.to_ascii_lowercase(), col.clone());
    }

    let binding = TableBinding {
        base_table: table_name,
        exposed_name,
        has_explicit_alias,
        is_null_padded: false,
        columns,
    };

    scope.add_binding(binding)?;

    let mut catalog_mut = catalog.clone();
    for using_item in &delete.using_clause {
        let item_scope = resolve_from_clause_node(using_item, &mut catalog_mut, &scope, param_map)?;
        scope.merge(item_scope)?;
    }

    // 1. WHERE clause
    if let Some(where_node) = &delete.where_clause {
        resolve_params_in_expr(where_node, catalog, &scope, param_map)?;
    }

    // 2. RETURNING list
    let mut fields = Vec::new();
    for rt_node in &delete.returning_list {
        if let Some(NodeEnum::ResTarget(rt)) = &rt_node.node {
            resolve_target(rt, catalog, &scope, &mut fields)?;
            if let Some(val) = &rt.val {
                resolve_params_in_expr(val, catalog, &scope, param_map)?;
            }
        }
    }

    Ok(fields)
}

fn get_ordered_columns<'a>(
    binding: &'a TableBinding,
    catalog: &'a Catalog,
    scope: Option<&'a QueryScope>,
) -> Vec<&'a ColumnMeta> {
    if let Some(scope) = scope
        && let Some(order) = scope
            .cte_column_orders
            .get(&binding.base_table.to_ascii_lowercase())
            .or_else(|| {
                scope
                    .cte_column_orders
                    .get(&binding.exposed_name.to_ascii_lowercase())
            })
    {
        let mut cols = Vec::new();
        for col_name in order {
            if let Some(col) = binding.get_column(col_name) {
                cols.push(col);
            }
        }
        if !cols.is_empty() {
            return cols;
        }
    }
    if let Some(table_meta) = catalog.get_table(&binding.exposed_name) {
        let mut cols = Vec::new();
        for c in &table_meta.columns {
            if let Some(col) = binding.get_column(&c.name) {
                cols.push(col);
            }
        }
        if !cols.is_empty() {
            return cols;
        }
    }
    if let Some(table_meta) = catalog.get_table(&binding.base_table) {
        let mut cols = Vec::new();
        for c in &table_meta.columns {
            if let Some(col) = binding.get_column(&c.name) {
                cols.push(col);
            }
        }
        if !cols.is_empty() {
            return cols;
        }
    }
    let mut cols: Vec<&ColumnMeta> = binding.columns.values().collect();
    cols.sort_by(|a, b| a.name.cmp(&b.name));
    cols
}

fn resolve_from_clause_node(
    node: &pg_query::protobuf::Node,
    catalog: &mut Catalog,
    active_scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<QueryScope, String> {
    match &node.node {
        Some(NodeEnum::RangeVar(rv)) => resolve_range_var(rv, catalog, active_scope),
        Some(NodeEnum::JoinExpr(je)) => resolve_join_expr(je, catalog, active_scope, param_map),
        Some(NodeEnum::RangeSubselect(rss)) => {
            resolve_range_subselect(rss, catalog, active_scope, param_map)
        }
        Some(NodeEnum::RangeFunction(rf)) => {
            resolve_range_function(rf, catalog, active_scope, param_map)
        }
        _ => Ok(QueryScope::default()),
    }
}

fn resolve_range_var(
    rv: &pg_query::protobuf::RangeVar,
    catalog: &mut Catalog,
    active_scope: &QueryScope,
) -> Result<QueryScope, String> {
    let is_unqualified = rv.schemaname.is_empty();
    let table_name = rv.relname.to_ascii_lowercase();

    // 1. Catalog Shadowing: If unqualified, check active_scope.ctes first
    if is_unqualified && let Some(cte_binding) = active_scope.ctes.get(&table_name) {
        let (has_explicit_alias, exposed_name, colnames) = if let Some(a) = &rv.alias {
            (true, a.aliasname.clone(), &a.colnames)
        } else {
            (false, table_name.clone(), &Vec::new())
        };

        let orig_order: Vec<String> =
            if let Some(order) = active_scope.cte_column_orders.get(&table_name) {
                order.clone()
            } else {
                let mut cols: Vec<String> = cte_binding.columns.keys().cloned().collect();
                cols.sort();
                cols
            };

        let mut columns = HashMap::new();
        let mut final_order = Vec::new();

        if !colnames.is_empty() {
            for (i, orig_col_name) in orig_order.iter().enumerate() {
                let col_name = if let Some(alias_node) = colnames.get(i) {
                    extract_string(alias_node).unwrap_or_else(|| orig_col_name.clone())
                } else {
                    orig_col_name.clone()
                };
                if let Some(col) = cte_binding.get_column(orig_col_name) {
                    let mut aliased_col = col.clone();
                    aliased_col.name = col_name.clone();
                    columns.insert(col_name.to_ascii_lowercase(), aliased_col);
                    final_order.push(col_name);
                }
            }
        } else {
            for orig_col_name in &orig_order {
                if let Some(col) = cte_binding.get_column(orig_col_name) {
                    columns.insert(col.name.to_ascii_lowercase(), col.clone());
                    final_order.push(col.name.clone());
                }
            }
        }

        let binding = TableBinding {
            base_table: table_name.clone(),
            exposed_name: exposed_name.clone(),
            has_explicit_alias,
            is_null_padded: false,
            columns,
        };

        let mut scope = QueryScope::with_ctes_from(active_scope);
        scope
            .cte_column_orders
            .insert(exposed_name.to_ascii_lowercase(), final_order);
        scope.add_binding(binding)?;
        return Ok(scope);
    }

    // 2. Database Catalog Resolution
    let table_meta = catalog
        .get_table(&table_name)
        .ok_or_else(|| format!("Table \"{}\" does not exist in schema catalog", rv.relname))?
        .clone();

    if !rv.schemaname.is_empty() {
        if let Some(tbl_schema) = &table_meta.schema {
            if !tbl_schema.eq_ignore_ascii_case(&rv.schemaname) {
                return Err(format!(
                    "Schema mismatch for table \"{}\": expected \"{}\", got \"{}\"",
                    table_name, tbl_schema, rv.schemaname
                ));
            }
        } else if !rv.schemaname.eq_ignore_ascii_case("public") {
            return Err(format!(
                "Schema mismatch for table \"{}\": expected public, got \"{}\"",
                table_name, rv.schemaname
            ));
        }
    }

    let (has_explicit_alias, exposed_name, colnames) = if let Some(a) = &rv.alias {
        (true, a.aliasname.clone(), &a.colnames)
    } else {
        (false, table_name.clone(), &Vec::new())
    };

    let mut columns = HashMap::new();
    let mut ordered_columns = Vec::new();

    if !colnames.is_empty() {
        for (i, col) in table_meta.columns.iter().enumerate() {
            let col_name = if let Some(alias_node) = colnames.get(i) {
                extract_string(alias_node).unwrap_or_else(|| col.name.clone())
            } else {
                col.name.clone()
            };
            let mut aliased_col = col.clone();
            aliased_col.name = col_name.clone();
            columns.insert(col_name.to_ascii_lowercase(), aliased_col.clone());
            ordered_columns.push(aliased_col);
        }
        catalog.tables.insert(
            exposed_name.to_ascii_lowercase(),
            TableMetadata {
                name: exposed_name.clone(),
                schema: None,
                columns: ordered_columns,
            },
        );
    } else {
        for col in &table_meta.columns {
            columns.insert(col.name.to_ascii_lowercase(), col.clone());
        }
    }

    let binding = TableBinding {
        base_table: table_name,
        exposed_name: exposed_name.clone(),
        has_explicit_alias,
        is_null_padded: false,
        columns,
    };

    let mut scope = QueryScope::with_ctes_from(active_scope);
    scope.add_binding(binding)?;
    Ok(scope)
}

fn resolve_join_expr(
    je: &pg_query::protobuf::JoinExpr,
    catalog: &mut Catalog,
    active_scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<QueryScope, String> {
    let larg = je
        .larg
        .as_ref()
        .ok_or_else(|| "JoinExpr missing left argument".to_string())?;
    let rarg = je
        .rarg
        .as_ref()
        .ok_or_else(|| "JoinExpr missing right argument".to_string())?;

    let mut left_scope = resolve_from_clause_node(larg, catalog, active_scope, param_map)?;
    let mut right_scope = resolve_from_clause_node(rarg, catalog, active_scope, param_map)?;

    let join_kind = JoinKind::from_i32(je.jointype);

    // Combined scope for evaluating join quals
    let mut combined_for_quals = left_scope.clone();
    for name in &right_scope.binding_order {
        if let Some(b) = right_scope.bindings.get(&name.to_ascii_lowercase()) {
            let _ = combined_for_quals.add_binding(b.clone());
        }
    }

    // Process join quals (ON condition) for parameter resolution
    if let Some(quals) = &je.quals {
        resolve_params_in_expr(quals, catalog, &combined_for_quals, param_map)?;
    }

    // Apply join nullability rules
    match join_kind {
        JoinKind::Inner => {}
        JoinKind::Left => {
            right_scope.force_all_nullable();
        }
        JoinKind::Right => {
            left_scope.force_all_nullable();
        }
        JoinKind::Full => {
            left_scope.force_all_nullable();
            right_scope.force_all_nullable();
        }
        JoinKind::Semi | JoinKind::Anti => {
            return Ok(left_scope);
        }
    }

    // Merge left and right scopes
    let mut result_scope = left_scope;
    for name in &right_scope.binding_order {
        if let Some(binding) = right_scope.bindings.get(&name.to_ascii_lowercase()) {
            result_scope.add_binding(binding.clone())?;
        }
    }

    // If join has an explicit alias (e.g. (a JOIN b) AS j):
    if let Some(alias) = &je.alias {
        let alias_name = alias.aliasname.clone();
        let mut unified_columns = HashMap::new();
        let mut ordered_columns = Vec::new();

        for name in &result_scope.binding_order {
            if let Some(b) = result_scope.bindings.get(&name.to_ascii_lowercase()) {
                let ordered = get_ordered_columns(b, catalog, Some(&result_scope));
                for col in ordered {
                    unified_columns.insert(col.name.to_ascii_lowercase(), col.clone());
                    ordered_columns.push(col.clone());
                }
            }
        }

        catalog.tables.insert(
            alias_name.to_ascii_lowercase(),
            TableMetadata {
                name: alias_name.clone(),
                schema: None,
                columns: ordered_columns,
            },
        );

        let unified_binding = TableBinding {
            base_table: alias_name.clone(),
            exposed_name: alias_name.clone(),
            has_explicit_alias: true,
            is_null_padded: false,
            columns: unified_columns,
        };

        let mut unified_scope = QueryScope::with_ctes_from(active_scope);
        unified_scope.add_binding(unified_binding)?;
        return Ok(unified_scope);
    }

    Ok(result_scope)
}

fn resolve_range_subselect(
    rss: &pg_query::protobuf::RangeSubselect,
    catalog: &mut Catalog,
    active_scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<QueryScope, String> {
    let subquery_node = rss
        .subquery
        .as_ref()
        .ok_or_else(|| "RangeSubselect missing subquery".to_string())?;
    let sub_select = match &subquery_node.node {
        Some(NodeEnum::SelectStmt(s)) => s,
        _ => return Err("RangeSubselect subquery is not a SelectStmt".to_string()),
    };

    let cols = infer_select_projected_columns(sub_select, catalog, active_scope, param_map)?;

    let alias = rss
        .alias
        .as_ref()
        .ok_or_else(|| "Subquery in FROM must have an alias".to_string())?;
    let alias_name = alias.aliasname.clone();

    let mut columns = HashMap::new();
    let mut ordered_columns = Vec::new();
    let mut col_names = Vec::new();

    for (i, col) in cols.iter().enumerate() {
        let col_name = if let Some(alias_node) = alias.colnames.get(i) {
            extract_string(alias_node).unwrap_or_else(|| col.name.clone())
        } else {
            col.name.clone()
        };

        let mut col_meta = col.clone();
        col_meta.name = col_name.clone();

        columns.insert(col_name.to_ascii_lowercase(), col_meta.clone());
        ordered_columns.push(col_meta);
        col_names.push(col_name);
    }

    catalog.tables.insert(
        alias_name.to_ascii_lowercase(),
        TableMetadata {
            name: alias_name.clone(),
            schema: None,
            columns: ordered_columns,
        },
    );

    let binding = TableBinding {
        base_table: alias_name.clone(),
        exposed_name: alias_name.clone(),
        has_explicit_alias: true,
        is_null_padded: false,
        columns,
    };

    let mut scope = QueryScope::with_ctes_from(active_scope);
    scope
        .cte_column_orders
        .insert(alias_name.to_ascii_lowercase(), col_names);
    scope.add_binding(binding)?;
    Ok(scope)
}

fn extract_func_call_from_range_function(
    rf: &pg_query::protobuf::RangeFunction,
) -> Option<&pg_query::protobuf::FuncCall> {
    for func_node in &rf.functions {
        if let Some(NodeEnum::List(list)) = &func_node.node
            && let Some(first) = list.items.first()
            && let Some(NodeEnum::FuncCall(fc)) = &first.node
        {
            return Some(fc);
        } else if let Some(NodeEnum::FuncCall(fc)) = &func_node.node {
            return Some(fc);
        }
    }
    None
}

fn resolve_range_function(
    rf: &pg_query::protobuf::RangeFunction,
    catalog: &mut Catalog,
    active_scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<QueryScope, String> {
    let fc = match extract_func_call_from_range_function(rf) {
        Some(fc) => fc,
        None => return Ok(QueryScope::default()),
    };

    let func_name = extract_func_name(&fc.funcname);
    if func_name.eq_ignore_ascii_case("unnest") {
        let arg = fc
            .args
            .first()
            .ok_or_else(|| "unnest() requires at least one argument".to_string())?;

        resolve_params_in_expr(arg, catalog, active_scope, param_map)?;
        let arg_inferred = infer_expr(arg, catalog, active_scope)?;

        let elem_pg = arg_inferred
            .pg_type
            .element_type()
            .cloned()
            .unwrap_or(PgType::Unknown);

        let elem_ts = elem_pg.to_ts(catalog);

        let (exposed_table_name, col_name, has_explicit_alias) = if let Some(alias) = &rf.alias {
            let tbl = if !alias.aliasname.is_empty() {
                alias.aliasname.clone()
            } else {
                "unnest".to_string()
            };
            let col = if let Some(first_col_node) = alias.colnames.first() {
                extract_string(first_col_node).unwrap_or_else(|| tbl.clone())
            } else if !alias.aliasname.is_empty() {
                alias.aliasname.clone()
            } else {
                "unnest".to_string()
            };
            (tbl, col, true)
        } else {
            ("unnest".to_string(), "unnest".to_string(), false)
        };

        let col_meta = ColumnMetadata {
            name: col_name.clone(),
            pg_type: elem_pg.to_pg_str(),
            ts_type: elem_ts,
            is_nullable: arg_inferred.is_nullable,
            has_default: false,
        };

        let mut columns = HashMap::new();
        columns.insert(col_name.to_ascii_lowercase(), col_meta);

        let binding = TableBinding {
            base_table: exposed_table_name.clone(),
            exposed_name: exposed_table_name.clone(),
            has_explicit_alias,
            is_null_padded: false,
            columns,
        };

        let mut scope = QueryScope::with_ctes_from(active_scope);
        scope
            .cte_column_orders
            .insert(exposed_table_name.to_ascii_lowercase(), vec![col_name]);
        scope.add_binding(binding)?;
        Ok(scope)
    } else {
        Ok(QueryScope::default())
    }
}

fn infer_expr(
    node: &pg_query::protobuf::Node,
    catalog: &Catalog,
    scope: &QueryScope,
) -> Result<InferredExpr, String> {
    match &node.node {
        Some(NodeEnum::ColumnRef(cr)) => {
            let (col, is_nullable) = scope.resolve_column_ref(cr, catalog)?;
            let pg_type = if col.pg_type == "unknown" && !col.ts_type.is_empty() {
                match col.ts_type.as_str() {
                    "string" => PgType::Text,
                    "number" => PgType::Int4,
                    "boolean" => PgType::Bool,
                    "Date" => PgType::Timestamp,
                    "bigint" => PgType::Int8,
                    "Buffer" | "Uint8Array" => PgType::Bytea,
                    other => PgType::Custom(other.to_string()),
                }
            } else {
                PgType::from_pg_str(&col.pg_type)
            };
            Ok(InferredExpr {
                pg_type,
                is_nullable,
            })
        }
        Some(NodeEnum::AConst(ac)) => {
            if ac.isnull {
                Ok(InferredExpr {
                    pg_type: PgType::Unknown,
                    is_nullable: true,
                })
            } else if let Some(val) = &ac.val {
                match val {
                    pg_query::protobuf::a_const::Val::Ival(_) => Ok(InferredExpr {
                        pg_type: PgType::Int4,
                        is_nullable: false,
                    }),
                    pg_query::protobuf::a_const::Val::Fval(_) => Ok(InferredExpr {
                        pg_type: PgType::Numeric,
                        is_nullable: false,
                    }),
                    pg_query::protobuf::a_const::Val::Sval(_) => Ok(InferredExpr {
                        pg_type: PgType::Text,
                        is_nullable: false,
                    }),
                    pg_query::protobuf::a_const::Val::Boolval(_) => Ok(InferredExpr {
                        pg_type: PgType::Bool,
                        is_nullable: false,
                    }),
                    _ => Ok(InferredExpr {
                        pg_type: PgType::Unknown,
                        is_nullable: false,
                    }),
                }
            } else {
                Ok(InferredExpr {
                    pg_type: PgType::Unknown,
                    is_nullable: false,
                })
            }
        }
        Some(NodeEnum::TypeCast(tc)) => {
            let pg_type_str = tc
                .type_name
                .as_ref()
                .map(extract_type_name)
                .unwrap_or_else(|| "unknown".to_string());
            let pg_type = PgType::from_pg_str(&pg_type_str);
            let is_nullable = if let Some(arg) = &tc.arg {
                if let Some(NodeEnum::AConst(ac)) = &arg.node {
                    ac.isnull
                } else {
                    infer_expr(arg, catalog, scope)
                        .map(|inf| inf.is_nullable)
                        .unwrap_or(false)
                }
            } else {
                false
            };
            Ok(InferredExpr {
                pg_type,
                is_nullable,
            })
        }
        Some(NodeEnum::CaseExpr(ce)) => {
            let mut unified_type = PgType::Unknown;
            let mut is_nullable = false;

            for arg in &ce.args {
                if let Some(NodeEnum::CaseWhen(cw)) = arg.node.as_ref()
                    && let Some(res_node) = &cw.result
                {
                    let inferred = infer_expr(res_node, catalog, scope)?;
                    unified_type = unify_types(&unified_type, &inferred.pg_type)?;
                    if inferred.is_nullable {
                        is_nullable = true;
                    }
                }
            }

            if let Some(def_node) = &ce.defresult {
                let inferred = infer_expr(def_node, catalog, scope)?;
                unified_type = unify_types(&unified_type, &inferred.pg_type)?;
                if inferred.is_nullable {
                    is_nullable = true;
                }
            } else {
                // If defresult (ELSE) is omitted, nullable = true
                is_nullable = true;
            }

            Ok(InferredExpr {
                pg_type: unified_type,
                is_nullable,
            })
        }
        Some(NodeEnum::CoalesceExpr(ce)) => {
            let mut unified_type = PgType::Unknown;
            let mut has_non_nullable = false;

            for arg in &ce.args {
                let inferred = infer_expr(arg, catalog, scope)?;
                unified_type = unify_types(&unified_type, &inferred.pg_type)?;
                if !inferred.is_nullable {
                    has_non_nullable = true;
                }
            }

            Ok(InferredExpr {
                pg_type: unified_type,
                is_nullable: !has_non_nullable,
            })
        }
        Some(NodeEnum::NullTest(_)) => Ok(InferredExpr {
            pg_type: PgType::Bool,
            is_nullable: false,
        }),
        Some(NodeEnum::BooleanTest(_)) => Ok(InferredExpr {
            pg_type: PgType::Bool,
            is_nullable: false,
        }),
        Some(NodeEnum::FuncCall(fc)) => {
            let func_name = extract_func_name(&fc.funcname);

            if fc.over.is_some() {
                // Window functions
                match func_name.as_str() {
                    "row_number" | "rank" | "dense_rank" | "count" => Ok(InferredExpr {
                        pg_type: PgType::Int8,
                        is_nullable: false,
                    }),
                    "percent_rank" | "cume_dist" => Ok(InferredExpr {
                        pg_type: PgType::Float8,
                        is_nullable: false,
                    }),
                    "ntile" => Ok(InferredExpr {
                        pg_type: PgType::Int4,
                        is_nullable: false,
                    }),
                    "sum" | "avg" => {
                        let arg_type = if let Some(arg) = fc.args.first() {
                            infer_expr(arg, catalog, scope)?.pg_type
                        } else {
                            PgType::Numeric
                        };
                        let pg_type = if is_numeric_type(&arg_type) {
                            arg_type
                        } else {
                            PgType::Numeric
                        };
                        Ok(InferredExpr {
                            pg_type,
                            is_nullable: true,
                        })
                    }
                    "min" | "max" => {
                        let arg_type = if let Some(arg) = fc.args.first() {
                            infer_expr(arg, catalog, scope)?.pg_type
                        } else {
                            PgType::Unknown
                        };
                        Ok(InferredExpr {
                            pg_type: arg_type,
                            is_nullable: true,
                        })
                    }
                    "json_agg" | "jsonb_agg" => {
                        if fc.args.len() != 1 {
                            return Err(format!(
                                "Function {} requires 1 argument; found {}",
                                func_name,
                                fc.args.len()
                            ));
                        }
                        let inner_type = infer_expr(&fc.args[0], catalog, scope)?;
                        Ok(InferredType {
                            pg_type: PgType::JsonArray(Box::new(inner_type)),
                            is_nullable: true,
                        })
                    }
                    _ => {
                        let arg_type = if let Some(arg) = fc.args.first() {
                            infer_expr(arg, catalog, scope)?.pg_type
                        } else {
                            PgType::Unknown
                        };
                        Ok(InferredExpr {
                            pg_type: arg_type,
                            is_nullable: true,
                        })
                    }
                }
            } else {
                // Regular function call
                match func_name.as_str() {
                    "json_build_object" | "jsonb_build_object" => {
                        if fc.args.len() % 2 != 0 {
                            return Err(format!(
                                "Argument list for {} must have an even number of elements; found {}",
                                func_name,
                                fc.args.len()
                            ));
                        }

                        let mut fields: Vec<(String, InferredType)> = Vec::new();
                        let mut is_dynamic = false;

                        for chunk in fc.args.chunks(2) {
                            let key_node = &chunk[0];
                            let val_node = &chunk[1];

                            let val_inferred = infer_expr(val_node, catalog, scope)?;

                            if let Some(key_str) = extract_const_string(key_node) {
                                if let Some(existing) =
                                    fields.iter_mut().find(|(k, _)| k == &key_str)
                                {
                                    existing.1 = val_inferred;
                                } else {
                                    fields.push((key_str, val_inferred));
                                }
                            } else {
                                is_dynamic = true;
                            }
                        }

                        if is_dynamic {
                            Ok(InferredType {
                                pg_type: PgType::JsonDynamicObject,
                                is_nullable: false,
                            })
                        } else {
                            Ok(InferredType {
                                pg_type: PgType::JsonObject(fields),
                                is_nullable: false,
                            })
                        }
                    }
                    "json_agg" | "jsonb_agg" => {
                        if fc.args.len() != 1 {
                            return Err(format!(
                                "Function {} requires 1 argument; found {}",
                                func_name,
                                fc.args.len()
                            ));
                        }
                        let inner_type = infer_expr(&fc.args[0], catalog, scope)?;
                        Ok(InferredType {
                            pg_type: PgType::JsonArray(Box::new(inner_type)),
                            is_nullable: true,
                        })
                    }
                    "to_json" | "to_jsonb" => {
                        if fc.args.len() != 1 {
                            return Err(format!(
                                "Function {} requires 1 argument; found {}",
                                func_name,
                                fc.args.len()
                            ));
                        }
                        let inner = infer_expr(&fc.args[0], catalog, scope)?;
                        match inner.pg_type {
                            PgType::JsonObject(_)
                            | PgType::JsonArray(_)
                            | PgType::JsonDynamicObject => Ok(inner),
                            _ => Ok(InferredType {
                                pg_type: PgType::JsonbRaw,
                                is_nullable: inner.is_nullable,
                            }),
                        }
                    }
                    "json_build_array" | "jsonb_build_array" => {
                        let mut elem_type = PgType::Unknown;
                        let mut is_elem_nullable = false;
                        for arg in &fc.args {
                            let inf = infer_expr(arg, catalog, scope)?;
                            elem_type =
                                unify_types(&elem_type, &inf.pg_type).unwrap_or(PgType::Unknown);
                            if inf.is_nullable {
                                is_elem_nullable = true;
                            }
                        }
                        Ok(InferredType {
                            pg_type: PgType::JsonArray(Box::new(InferredType {
                                pg_type: elem_type,
                                is_nullable: is_elem_nullable,
                            })),
                            is_nullable: false,
                        })
                    }
                    "count" => Ok(InferredExpr {
                        pg_type: PgType::Int4,
                        is_nullable: false,
                    }),
                    "sum" | "avg" => {
                        let arg_type = if let Some(arg) = fc.args.first() {
                            infer_expr(arg, catalog, scope)?.pg_type
                        } else {
                            PgType::Numeric
                        };
                        let pg_type = if is_numeric_type(&arg_type) {
                            arg_type
                        } else {
                            PgType::Numeric
                        };
                        Ok(InferredExpr {
                            pg_type,
                            is_nullable: true,
                        })
                    }
                    "min" | "max" => {
                        let arg_type = if let Some(arg) = fc.args.first() {
                            infer_expr(arg, catalog, scope)?.pg_type
                        } else {
                            PgType::Unknown
                        };
                        Ok(InferredExpr {
                            pg_type: arg_type,
                            is_nullable: true,
                        })
                    }
                    "coalesce" => {
                        let mut unified_type = PgType::Unknown;
                        let mut has_non_nullable = false;
                        for arg in &fc.args {
                            let inferred = infer_expr(arg, catalog, scope)?;
                            unified_type = unify_types(&unified_type, &inferred.pg_type)?;
                            if !inferred.is_nullable {
                                has_non_nullable = true;
                            }
                        }
                        Ok(InferredExpr {
                            pg_type: unified_type,
                            is_nullable: !has_non_nullable,
                        })
                    }
                    _ => Ok(InferredExpr {
                        pg_type: PgType::Unknown,
                        is_nullable: true,
                    }),
                }
            }
        }
        Some(NodeEnum::SubLink(sl)) => {
            if sl.sub_link_type == SubLinkType::ExprSublink as i32 {
                let subselect_node = sl
                    .subselect
                    .as_ref()
                    .ok_or_else(|| "SubLink missing subselect".to_string())?;
                let sub_select = match &subselect_node.node {
                    Some(NodeEnum::SelectStmt(s)) => s,
                    _ => return Err("SubLink subselect is not a SelectStmt".to_string()),
                };

                if sub_select.target_list.len() != 1 {
                    return Err("Scalar subquery must have exactly one target column".to_string());
                }

                let mut dummy_params = HashMap::new();
                let mut sub_scope = scope.clone();
                if let Some(wc) = &sub_select.with_clause {
                    process_with_clause_with_params(
                        wc,
                        &mut sub_scope,
                        catalog,
                        &mut dummy_params,
                    )?;
                }

                let mut sub_catalog = catalog.clone();
                for from_item in &sub_select.from_clause {
                    let item_scope = resolve_from_clause_node(
                        from_item,
                        &mut sub_catalog,
                        &sub_scope,
                        &mut dummy_params,
                    )?;
                    let _ = sub_scope.merge(item_scope);
                }

                let target = &sub_select.target_list[0];
                let target_rt = match &target.node {
                    Some(NodeEnum::ResTarget(rt)) => rt,
                    _ => return Err("Subquery target is not a ResTarget".to_string()),
                };

                let val = target_rt
                    .val
                    .as_ref()
                    .ok_or_else(|| "Subquery target missing expression".to_string())?;
                let inner_inferred = infer_expr(val, &sub_catalog, &sub_scope)?;

                // Scalar subqueries strictly evaluate to SQL NULL if zero rows match
                Ok(InferredExpr {
                    pg_type: inner_inferred.pg_type,
                    is_nullable: true,
                })
            } else if sl.sub_link_type == SubLinkType::ExistsSublink as i32 {
                Ok(InferredExpr {
                    pg_type: PgType::Bool,
                    is_nullable: false,
                })
            } else {
                Ok(InferredExpr {
                    pg_type: PgType::Unknown,
                    is_nullable: true,
                })
            }
        }
        Some(NodeEnum::ScalarArrayOpExpr(saoe)) => {
            let mut is_nullable = false;
            for arg in &saoe.args {
                if let Ok(inf) = infer_expr(arg, catalog, scope)
                    && inf.is_nullable
                {
                    is_nullable = true;
                }
            }
            Ok(InferredExpr {
                pg_type: PgType::Bool,
                is_nullable,
            })
        }
        Some(NodeEnum::AExpr(ae)) => {
            let l_inf = if let Some(l) = &ae.lexpr {
                infer_expr(l, catalog, scope)?
            } else {
                InferredExpr {
                    pg_type: PgType::Unknown,
                    is_nullable: false,
                }
            };
            let r_inf = if let Some(r) = &ae.rexpr {
                infer_expr(r, catalog, scope)?
            } else {
                InferredExpr {
                    pg_type: PgType::Unknown,
                    is_nullable: false,
                }
            };

            if ae.kind == AExprKind::AexprOpAny as i32 || ae.kind == AExprKind::AexprOpAll as i32 {
                let is_nullable = l_inf.is_nullable || r_inf.is_nullable;
                return Ok(InferredType {
                    pg_type: PgType::Bool,
                    is_nullable,
                });
            }

            let op = ae.name.first().and_then(extract_string).unwrap_or_default();

            match op.as_str() {
                "->>" | "#>>" => Ok(InferredType {
                    pg_type: PgType::Text,
                    is_nullable: true,
                }),
                "->" => {
                    let right_key = ae.rexpr.as_ref().and_then(|r| extract_const_string(r));
                    if let PgType::JsonObject(fields) = &l_inf.pg_type
                        && let Some(key) = right_key
                    {
                        if let Some((_, field_inferred)) = fields.iter().find(|(k, _)| k == &key) {
                            return Ok(InferredType {
                                pg_type: field_inferred.pg_type.clone(),
                                is_nullable: true,
                            });
                        }
                    } else if let PgType::JsonArray(inner) = &l_inf.pg_type {
                        return Ok(InferredType {
                            pg_type: inner.pg_type.clone(),
                            is_nullable: true,
                        });
                    }
                    Ok(InferredType {
                        pg_type: PgType::JsonbRaw,
                        is_nullable: true,
                    })
                }
                "#>" => {
                    if let Some(path) = ae
                        .rexpr
                        .as_ref()
                        .and_then(|r| extract_static_string_array(r))
                    {
                        let mut curr_type = &l_inf.pg_type;
                        let mut matched = true;
                        for key in &path {
                            if let PgType::JsonObject(fields) = curr_type {
                                if let Some((_, next_inferred)) =
                                    fields.iter().find(|(k, _)| k == key)
                                {
                                    curr_type = &next_inferred.pg_type;
                                } else {
                                    matched = false;
                                    break;
                                }
                            } else {
                                matched = false;
                                break;
                            }
                        }
                        if matched && !path.is_empty() {
                            return Ok(InferredType {
                                pg_type: curr_type.clone(),
                                is_nullable: true,
                            });
                        }
                    }
                    Ok(InferredType {
                        pg_type: PgType::JsonbRaw,
                        is_nullable: true,
                    })
                }
                "+" | "-" | "*" | "/" | "%" => {
                    let is_nullable = l_inf.is_nullable || r_inf.is_nullable;
                    let pg_type =
                        if is_numeric_type(&l_inf.pg_type) && is_numeric_type(&r_inf.pg_type) {
                            unify_types(&l_inf.pg_type, &r_inf.pg_type).unwrap_or(PgType::Numeric)
                        } else {
                            PgType::Numeric
                        };
                    Ok(InferredType {
                        pg_type,
                        is_nullable,
                    })
                }
                "&&" | "@>" | "<@" => {
                    let is_nullable = l_inf.is_nullable || r_inf.is_nullable;
                    Ok(InferredType {
                        pg_type: PgType::Bool,
                        is_nullable,
                    })
                }
                "||" => {
                    let is_nullable = l_inf.is_nullable || r_inf.is_nullable;
                    if let (PgType::Array(l_elem), PgType::Array(r_elem)) =
                        (&l_inf.pg_type, &r_inf.pg_type)
                    {
                        let unified = unify_types(l_elem, r_elem).unwrap_or(PgType::Unknown);
                        Ok(InferredType {
                            pg_type: PgType::Array(Box::new(unified)),
                            is_nullable,
                        })
                    } else if let PgType::Array(l_elem) = &l_inf.pg_type {
                        let unified = unify_types(l_elem, &r_inf.pg_type)
                            .unwrap_or_else(|_| l_elem.as_ref().clone());
                        Ok(InferredType {
                            pg_type: PgType::Array(Box::new(unified)),
                            is_nullable,
                        })
                    } else if let PgType::Array(r_elem) = &r_inf.pg_type {
                        let unified = unify_types(&l_inf.pg_type, r_elem)
                            .unwrap_or_else(|_| r_elem.as_ref().clone());
                        Ok(InferredType {
                            pg_type: PgType::Array(Box::new(unified)),
                            is_nullable,
                        })
                    } else {
                        Ok(InferredType {
                            pg_type: PgType::Text,
                            is_nullable,
                        })
                    }
                }
                "=" | "<>" | "!=" | "<" | "<=" | ">" | ">=" => {
                    let is_nullable = l_inf.is_nullable || r_inf.is_nullable;
                    Ok(InferredType {
                        pg_type: PgType::Bool,
                        is_nullable,
                    })
                }
                _ => {
                    let is_nullable = l_inf.is_nullable || r_inf.is_nullable;
                    Ok(InferredType {
                        pg_type: PgType::Unknown,
                        is_nullable,
                    })
                }
            }
        }
        Some(NodeEnum::AArrayExpr(aae)) => {
            let mut unified_elem = PgType::Unknown;
            for elem in &aae.elements {
                let inf = infer_expr(elem, catalog, scope)?;
                unified_elem = unify_types(&unified_elem, &inf.pg_type).unwrap_or(unified_elem);
            }
            Ok(InferredExpr {
                pg_type: PgType::Array(Box::new(unified_elem)),
                is_nullable: false,
            })
        }
        _ => Ok(InferredExpr {
            pg_type: PgType::Unknown,
            is_nullable: true,
        }),
    }
}

fn resolve_target_columns(
    rt: &pg_query::protobuf::ResTarget,
    catalog: &Catalog,
    scope: &QueryScope,
    columns: &mut Vec<ColumnMetadata>,
) -> Result<(), String> {
    let explicit_alias = if !rt.name.is_empty() {
        Some(rt.name.clone())
    } else {
        None
    };

    let val_node = match &rt.val {
        Some(v) => v,
        None => return Ok(()),
    };

    match &val_node.node {
        Some(NodeEnum::ColumnRef(cr)) => {
            // Check for wildcard '*'
            if let Some(first) = cr.fields.first()
                && let Some(NodeEnum::AStar(_)) = &first.node
            {
                // SELECT * FROM ...
                for name in &scope.binding_order {
                    if let Some(binding) = scope.bindings.get(&name.to_ascii_lowercase()) {
                        let ordered_cols = get_ordered_columns(binding, catalog, Some(scope));
                        for col in ordered_cols {
                            let is_null = col.is_nullable || binding.is_null_padded;
                            let ts_type = col.ts_type.replace(" | null", "").trim().to_string();
                            columns.push(ColumnMetadata {
                                name: col.name.clone(),
                                pg_type: col.pg_type.clone(),
                                ts_type,
                                is_nullable: is_null,
                                has_default: col.has_default,
                            });
                        }
                    }
                }
                return Ok(());
            }

            // Check for 'alias.*'
            if cr.fields.len() == 2
                && let (Some(first), Some(second)) = (cr.fields.first(), cr.fields.get(1))
                && let Some(NodeEnum::AStar(_)) = &second.node
                && let Some(target) = extract_string(first)
            {
                // Check alias hiding
                for binding in scope.bindings.values() {
                    if binding.has_explicit_alias
                        && binding.base_table.eq_ignore_ascii_case(&target)
                        && !binding.exposed_name.eq_ignore_ascii_case(&target)
                    {
                        return Err(format!(
                            "Cannot reference base table \"{}\" because it is aliased as \"{}\"",
                            target, binding.exposed_name
                        ));
                    }
                }

                let binding = scope
                    .bindings
                    .get(&target.to_ascii_lowercase())
                    .ok_or_else(|| format!("Unknown table alias \"{}\"", target))?;

                let ordered_cols = get_ordered_columns(binding, catalog, Some(scope));
                for col in ordered_cols {
                    let is_null = col.is_nullable || binding.is_null_padded;
                    let ts_type = col.ts_type.replace(" | null", "").trim().to_string();
                    columns.push(ColumnMetadata {
                        name: col.name.clone(),
                        pg_type: col.pg_type.clone(),
                        ts_type,
                        is_nullable: is_null,
                        has_default: col.has_default,
                    });
                }
                return Ok(());
            }

            // Single column reference
            let inferred = infer_expr(val_node, catalog, scope)?;
            let name = explicit_alias.unwrap_or_else(|| {
                cr.fields
                    .last()
                    .and_then(extract_string)
                    .unwrap_or_else(|| "column".to_string())
            });
            let ts_type = inferred.pg_type.to_ts(catalog);
            columns.push(ColumnMetadata {
                name,
                pg_type: inferred.pg_type.to_pg_str(),
                ts_type,
                is_nullable: inferred.is_nullable,
                has_default: false,
            });
            Ok(())
        }
        _ => {
            let inferred = infer_expr(val_node, catalog, scope)?;
            let ts_type = inferred.pg_type.to_ts(catalog);
            let default_name = match &val_node.node {
                Some(NodeEnum::FuncCall(fc)) => extract_func_name(&fc.funcname),
                Some(NodeEnum::CaseExpr(_)) => "case".to_string(),
                Some(NodeEnum::CoalesceExpr(_)) => "coalesce".to_string(),
                Some(NodeEnum::NullTest(_)) => "null_test".to_string(),
                Some(NodeEnum::BooleanTest(_)) => "bool_test".to_string(),
                Some(NodeEnum::SubLink(_)) => "subquery".to_string(),
                Some(NodeEnum::AConst(_)) => "constant".to_string(),
                Some(NodeEnum::AExpr(_)) => "expr".to_string(),
                Some(NodeEnum::TypeCast(_)) => "cast".to_string(),
                Some(NodeEnum::AArrayExpr(_)) => "arr".to_string(),
                _ => "column".to_string(),
            };
            columns.push(ColumnMetadata {
                name: explicit_alias.unwrap_or(default_name),
                pg_type: inferred.pg_type.to_pg_str(),
                ts_type,
                is_nullable: inferred.is_nullable,
                has_default: false,
            });
            Ok(())
        }
    }
}

fn resolve_target(
    rt: &pg_query::protobuf::ResTarget,
    catalog: &Catalog,
    scope: &QueryScope,
    fields: &mut Vec<QueryField>,
) -> Result<(), String> {
    let mut cols = Vec::new();
    resolve_target_columns(rt, catalog, scope, &mut cols)?;
    for col in cols {
        let ts_type = if col.is_nullable {
            format_nullable(&col.ts_type)
        } else {
            col.ts_type
        };
        fields.push(QueryField {
            name: col.name,
            ts_type,
        });
    }
    Ok(())
}

/// Helper to inspect an expression for ParamRef and ColumnRef combinations.
fn extract_param_info(node: &pg_query::protobuf::Node) -> Option<(i32, Option<String>)> {
    match &node.node {
        Some(NodeEnum::ParamRef(p)) => Some((p.number, None)),
        Some(NodeEnum::TypeCast(tc)) => {
            if let Some(arg) = &tc.arg
                && let Some(NodeEnum::ParamRef(p)) = &arg.node
            {
                let cast_type = tc.type_name.as_ref().map(extract_type_name);
                return Some((p.number, cast_type));
            }
            None
        }
        _ => None,
    }
}

fn extract_column_info(node: &pg_query::protobuf::Node) -> Option<(Option<String>, String)> {
    match &node.node {
        Some(NodeEnum::ColumnRef(cr)) => {
            if cr.fields.len() == 2 {
                let alias = extract_string(&cr.fields[0]);
                let col = extract_string(&cr.fields[1]);
                if let Some(c) = col {
                    return Some((alias, c));
                }
            } else if cr.fields.len() == 1
                && let Some(c) = extract_string(&cr.fields[0])
            {
                return Some((None, c));
            }
            None
        }
        Some(NodeEnum::TypeCast(tc)) => {
            if let Some(arg) = &tc.arg {
                extract_column_info(arg)
            } else {
                None
            }
        }
        _ => None,
    }
}

struct OptionalFilterMatch {
    param_num: i32,
    cast_opt: Option<String>,
    alias_opt: Option<String>,
    col_name: String,
}

fn match_null_test_param(node: &pg_query::protobuf::Node) -> Option<(i32, Option<String>)> {
    if let Some(NodeEnum::NullTest(nt)) = &node.node
        && nt.nulltesttype == NullTestType::IsNull as i32
        && let Some(arg) = &nt.arg
    {
        return extract_param_info(arg);
    }
    None
}

fn match_equality_col_param(
    node: &pg_query::protobuf::Node,
) -> Option<(i32, Option<String>, Option<String>, String)> {
    if let Some(NodeEnum::AExpr(ae)) = &node.node
        && ae.kind == AExprKind::AexprOp as i32
        && ae.name.first().and_then(extract_string).as_deref() == Some("=")
    {
        let param_opt_l = ae.lexpr.as_ref().and_then(|n| extract_param_info(n));
        let col_opt_l = ae.lexpr.as_ref().and_then(|n| extract_column_info(n));

        let param_opt_r = ae.rexpr.as_ref().and_then(|n| extract_param_info(n));
        let col_opt_r = ae.rexpr.as_ref().and_then(|n| extract_column_info(n));

        if let (Some((param_num, cast_opt)), Some((alias_opt, col_name))) = (param_opt_r, col_opt_l)
        {
            return Some((param_num, cast_opt, alias_opt, col_name));
        } else if let (Some((param_num, cast_opt)), Some((alias_opt, col_name))) =
            (param_opt_l, col_opt_r)
        {
            return Some((param_num, cast_opt, alias_opt, col_name));
        }
    }
    None
}

fn match_optional_filter(be: &pg_query::protobuf::BoolExpr) -> Option<OptionalFilterMatch> {
    if be.boolop != BoolExprType::OrExpr as i32 || be.args.len() != 2 {
        return None;
    }

    let arm0 = &be.args[0];
    let arm1 = &be.args[1];

    // Case 1: arm0 is NullTest($N), arm1 is col = $N
    if let Some((p_null, cast_null)) = match_null_test_param(arm0)
        && let Some((p_eq, cast_eq, alias_opt, col_name)) = match_equality_col_param(arm1)
        && p_null == p_eq
    {
        let cast_opt = cast_null.or(cast_eq);
        return Some(OptionalFilterMatch {
            param_num: p_null,
            cast_opt,
            alias_opt,
            col_name,
        });
    }

    // Case 2: arm0 is col = $N, arm1 is NullTest($N)
    if let Some((p_eq, cast_eq, alias_opt, col_name)) = match_equality_col_param(arm0)
        && let Some((p_null, cast_null)) = match_null_test_param(arm1)
        && p_null == p_eq
    {
        let cast_opt = cast_null.or(cast_eq);
        return Some(OptionalFilterMatch {
            param_num: p_null,
            cast_opt,
            alias_opt,
            col_name,
        });
    }

    None
}

fn resolve_scalar_array_comparison(
    lhs: &pg_query::protobuf::Node,
    rhs: &pg_query::protobuf::Node,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    let param_opt_l = extract_param_info(lhs);
    let param_opt_r = extract_param_info(rhs);

    if let Some((param_num, cast_opt)) = param_opt_r {
        let mut resolved_type = cast_opt.map(|c| catalog.resolve_type(&c));
        let mut col_name_opt = None;

        if resolved_type.is_none() {
            if let Ok(lhs_inf) = infer_expr(lhs, catalog, scope)
                && lhs_inf.pg_type != PgType::Unknown
            {
                resolved_type = Some(lhs_inf.pg_type.to_array().to_ts(catalog));
            }
            if let Some((_, col_name)) = extract_column_info(lhs) {
                col_name_opt = Some(col_name);
            }
        }

        let entry = param_map.entry(param_num).or_default();
        if let Some(col_name) = col_name_opt
            && entry.suggested_name.is_none()
        {
            entry.suggested_name = Some(col_name);
        }
        if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
            entry.inferred_type = resolved_type;
        }

        resolve_params_in_expr(lhs, catalog, scope, param_map)?;
    } else if let Some((param_num, cast_opt)) = param_opt_l {
        let mut resolved_type = cast_opt.map(|c| catalog.resolve_type(&c));
        let mut col_name_opt = None;

        if resolved_type.is_none() {
            if let Ok(rhs_inf) = infer_expr(rhs, catalog, scope)
                && let Some(elem) = rhs_inf.pg_type.element_type()
            {
                resolved_type = Some(elem.to_ts(catalog));
            }
            if let Some((_, col_name)) = extract_column_info(rhs) {
                col_name_opt = Some(col_name);
            }
        }

        let entry = param_map.entry(param_num).or_default();
        if let Some(col_name) = col_name_opt
            && entry.suggested_name.is_none()
        {
            entry.suggested_name = Some(col_name);
        }
        if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
            entry.inferred_type = resolved_type;
        }

        resolve_params_in_expr(rhs, catalog, scope, param_map)?;
    } else {
        resolve_params_in_expr(lhs, catalog, scope, param_map)?;
        resolve_params_in_expr(rhs, catalog, scope, param_map)?;
    }

    Ok(())
}

fn resolve_array_op_params(
    lexpr: &pg_query::protobuf::Node,
    rexpr: &pg_query::protobuf::Node,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    let param_opt_l = extract_param_info(lexpr);
    let param_opt_r = extract_param_info(rexpr);

    if let Some((param_num, cast_opt)) = param_opt_r {
        let mut resolved_type = cast_opt.map(|c| catalog.resolve_type(&c));
        let mut col_name_opt = None;

        if resolved_type.is_none() {
            if let Ok(lhs_inf) = infer_expr(lexpr, catalog, scope)
                && lhs_inf.pg_type != PgType::Unknown
            {
                resolved_type = Some(lhs_inf.pg_type.to_ts(catalog));
            }
            if let Some((_, col_name)) = extract_column_info(lexpr) {
                col_name_opt = Some(col_name);
            }
        }

        let entry = param_map.entry(param_num).or_default();
        if let Some(col_name) = col_name_opt
            && entry.suggested_name.is_none()
        {
            entry.suggested_name = Some(col_name);
        }
        if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
            entry.inferred_type = resolved_type;
        }

        resolve_params_in_expr(lexpr, catalog, scope, param_map)?;
    } else if let Some((param_num, cast_opt)) = param_opt_l {
        let mut resolved_type = cast_opt.map(|c| catalog.resolve_type(&c));
        let mut col_name_opt = None;

        if resolved_type.is_none() {
            if let Ok(rhs_inf) = infer_expr(rexpr, catalog, scope)
                && rhs_inf.pg_type != PgType::Unknown
            {
                resolved_type = Some(rhs_inf.pg_type.to_ts(catalog));
            }
            if let Some((_, col_name)) = extract_column_info(rexpr) {
                col_name_opt = Some(col_name);
            }
        }

        let entry = param_map.entry(param_num).or_default();
        if let Some(col_name) = col_name_opt
            && entry.suggested_name.is_none()
        {
            entry.suggested_name = Some(col_name);
        }
        if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
            entry.inferred_type = resolved_type;
        }

        resolve_params_in_expr(rexpr, catalog, scope, param_map)?;
    } else {
        resolve_params_in_expr(lexpr, catalog, scope, param_map)?;
        resolve_params_in_expr(rexpr, catalog, scope, param_map)?;
    }

    Ok(())
}

fn resolve_params_in_expr(
    expr: &pg_query::protobuf::Node,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    match &expr.node {
        Some(NodeEnum::BoolExpr(be)) => {
            if let Some(m) = match_optional_filter(be) {
                record_param(
                    m.param_num,
                    m.cast_opt,
                    m.alias_opt,
                    &m.col_name,
                    catalog,
                    scope,
                    param_map,
                    true,
                )?;
                return Ok(());
            }

            for arg in &be.args {
                resolve_params_in_expr(arg, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::ScalarArrayOpExpr(saoe)) => {
            if saoe.args.len() == 2 {
                resolve_scalar_array_comparison(
                    &saoe.args[0],
                    &saoe.args[1],
                    catalog,
                    scope,
                    param_map,
                )?;
            } else {
                for arg in &saoe.args {
                    resolve_params_in_expr(arg, catalog, scope, param_map)?;
                }
            }
        }
        Some(NodeEnum::AExpr(ae)) => {
            if ae.kind == AExprKind::AexprOpAny as i32 || ae.kind == AExprKind::AexprOpAll as i32 {
                if let (Some(lexpr), Some(rexpr)) = (&ae.lexpr, &ae.rexpr) {
                    resolve_scalar_array_comparison(lexpr, rexpr, catalog, scope, param_map)?;
                }
                return Ok(());
            }

            if ae.kind == AExprKind::AexprOp as i32 {
                let op = ae.name.first().and_then(extract_string).unwrap_or_default();
                if matches!(op.as_str(), "&&" | "@>" | "<@") {
                    if let (Some(lexpr), Some(rexpr)) = (&ae.lexpr, &ae.rexpr) {
                        resolve_array_op_params(lexpr, rexpr, catalog, scope, param_map)?;
                    }
                    return Ok(());
                }

                let param_opt_l = ae.lexpr.as_ref().and_then(|n| extract_param_info(n));
                let col_opt_l = ae.lexpr.as_ref().and_then(|n| extract_column_info(n));

                let param_opt_r = ae.rexpr.as_ref().and_then(|n| extract_param_info(n));
                let col_opt_r = ae.rexpr.as_ref().and_then(|n| extract_column_info(n));

                if let (Some((param_num, cast_opt)), Some((alias_opt, col_name))) =
                    (param_opt_r, col_opt_l)
                {
                    record_param(
                        param_num, cast_opt, alias_opt, &col_name, catalog, scope, param_map, false,
                    )?;
                } else if let (Some((param_num, cast_opt)), Some((alias_opt, col_name))) =
                    (param_opt_l, col_opt_r)
                {
                    record_param(
                        param_num, cast_opt, alias_opt, &col_name, catalog, scope, param_map, false,
                    )?;
                } else {
                    // Recurse both branches
                    if let Some(lexpr) = &ae.lexpr {
                        resolve_params_in_expr(lexpr, catalog, scope, param_map)?;
                    }
                    if let Some(rexpr) = &ae.rexpr {
                        resolve_params_in_expr(rexpr, catalog, scope, param_map)?;
                    }
                }
            } else {
                if let Some(lexpr) = &ae.lexpr {
                    resolve_params_in_expr(lexpr, catalog, scope, param_map)?;
                }
                if let Some(rexpr) = &ae.rexpr {
                    resolve_params_in_expr(rexpr, catalog, scope, param_map)?;
                }
            }
        }
        Some(NodeEnum::TypeCast(tc)) => {
            if let Some(arg) = &tc.arg {
                if let Some(NodeEnum::ParamRef(p)) = &arg.node {
                    let ts_type = tc
                        .type_name
                        .as_ref()
                        .map(extract_type_name)
                        .map(|t| catalog.resolve_type(&t));
                    let entry = param_map.entry(p.number).or_default();
                    if entry.inferred_type.is_none()
                        || entry.inferred_type.as_deref() == Some("unknown")
                    {
                        entry.inferred_type = ts_type;
                    }
                } else {
                    resolve_params_in_expr(arg, catalog, scope, param_map)?;
                }
            }
        }
        Some(NodeEnum::ParamRef(p)) => {
            param_map.entry(p.number).or_default();
        }
        Some(NodeEnum::NullTest(nt)) => {
            if let Some(arg) = &nt.arg {
                resolve_params_in_expr(arg, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::BooleanTest(bt)) => {
            if let Some(arg) = &bt.arg {
                resolve_params_in_expr(arg, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::CaseExpr(ce)) => {
            for arg in &ce.args {
                resolve_params_in_expr(arg, catalog, scope, param_map)?;
            }
            if let Some(def) = &ce.defresult {
                resolve_params_in_expr(def, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::CaseWhen(cw)) => {
            if let Some(expr) = &cw.expr {
                resolve_params_in_expr(expr, catalog, scope, param_map)?;
            }
            if let Some(res) = &cw.result {
                resolve_params_in_expr(res, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::CoalesceExpr(ce)) => {
            for arg in &ce.args {
                resolve_params_in_expr(arg, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::FuncCall(fc)) => {
            for arg in &fc.args {
                resolve_params_in_expr(arg, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::SubLink(sl)) => {
            if let Some(testexpr) = &sl.testexpr {
                resolve_params_in_expr(testexpr, catalog, scope, param_map)?;
            }
            if let Some(sub_node) = &sl.subselect
                && let Some(NodeEnum::SelectStmt(sub_select)) = &sub_node.node
            {
                let mut sub_scope = scope.clone();
                if let Some(wc) = &sub_select.with_clause {
                    let _ = process_with_clause_with_params(wc, &mut sub_scope, catalog, param_map);
                }
                let mut sub_catalog = catalog.clone();
                for from_item in &sub_select.from_clause {
                    if let Ok(item_scope) =
                        resolve_from_clause_node(from_item, &mut sub_catalog, &sub_scope, param_map)
                    {
                        let _ = sub_scope.merge(item_scope);
                    }
                }
                for target in &sub_select.target_list {
                    if let Some(NodeEnum::ResTarget(rt)) = &target.node
                        && let Some(val) = &rt.val
                    {
                        resolve_params_in_expr(val, &sub_catalog, &sub_scope, param_map)?;
                    }
                }
                if let Some(where_node) = &sub_select.where_clause {
                    resolve_params_in_expr(where_node, &sub_catalog, &sub_scope, param_map)?;
                }
            }
        }
        Some(NodeEnum::ResTarget(rt)) => {
            if let Some(val) = &rt.val {
                resolve_params_in_expr(val, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::AArrayExpr(aae)) => {
            let mut unified_elem = PgType::Unknown;
            for elem in &aae.elements {
                if extract_param_info(elem).is_none()
                    && let Ok(inf) = infer_expr(elem, catalog, scope)
                {
                    unified_elem = unify_types(&unified_elem, &inf.pg_type).unwrap_or(unified_elem);
                }
            }
            for elem in &aae.elements {
                if let Some((param_num, cast_opt)) = extract_param_info(elem) {
                    let resolved_type = cast_opt.map(|c| catalog.resolve_type(&c)).or_else(|| {
                        if unified_elem != PgType::Unknown {
                            Some(unified_elem.to_ts(catalog))
                        } else {
                            None
                        }
                    });
                    let entry = param_map.entry(param_num).or_default();
                    if entry.inferred_type.is_none()
                        || entry.inferred_type.as_deref() == Some("unknown")
                    {
                        entry.inferred_type = resolved_type;
                    }
                } else {
                    resolve_params_in_expr(elem, catalog, scope, param_map)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_param(
    param_num: i32,
    cast_opt: Option<String>,
    alias_opt: Option<String>,
    col_name: &str,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
    is_optional: bool,
) -> Result<(), String> {
    let mut resolved_type: Option<String> = cast_opt.map(|c| catalog.resolve_type(&c));

    // Lookup column to verify and get type if not explicitly cast
    let col_meta = if let Some(alias) = alias_opt {
        for binding in scope.bindings.values() {
            if binding.has_explicit_alias
                && binding.base_table.eq_ignore_ascii_case(&alias)
                && !binding.exposed_name.eq_ignore_ascii_case(&alias)
            {
                return Err(format!(
                    "Cannot reference base table \"{}\" because it is aliased as \"{}\"",
                    alias, binding.exposed_name
                ));
            }
        }
        let binding = scope
            .bindings
            .get(&alias.to_ascii_lowercase())
            .ok_or_else(|| format!("Unknown table alias \"{}\" in parameter comparison", alias))?;
        binding.get_column(col_name)
    } else {
        let mut found = None;
        for name in &scope.binding_order {
            if let Some(binding) = scope.bindings.get(&name.to_ascii_lowercase())
                && let Some(c) = binding.get_column(col_name)
            {
                found = Some(c);
                break;
            }
        }
        found
    };

    if let Some(col) = col_meta
        && resolved_type.is_none()
    {
        resolved_type = Some(col.ts_type.clone());
    }

    let entry = param_map.entry(param_num).or_default();
    if is_optional {
        entry.is_optional = true;
    }
    if entry.suggested_name.is_none() {
        entry.suggested_name = Some(col_name.to_string());
    }
    if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
        entry.inferred_type = resolved_type;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_catalog() -> Catalog {
        let sql = "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                email TEXT NOT NULL
            );
            CREATE TABLE posts (
                id UUID PRIMARY KEY,
                user_id UUID NOT NULL,
                title VARCHAR(255) NOT NULL,
                views INT NOT NULL
            );
            CREATE TABLE comments (
                id UUID PRIMARY KEY,
                post_id UUID NOT NULL,
                body TEXT NOT NULL
            );
        ";
        let mut catalog = Catalog::default();
        catalog.apply_sql(sql).unwrap();
        catalog
    }

    #[test]
    fn test_analyze_query_from_prompt() {
        let catalog = setup_test_catalog();
        let query_sql = "
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
        ";

        let analyzed = analyze_query(query_sql, &catalog, None).unwrap();
        assert_eq!(analyzed.name, "GetUserWithPosts");

        // Parameters
        assert_eq!(analyzed.params.len(), 1);
        assert_eq!(analyzed.params[0].name, "id");
        assert_eq!(analyzed.params[0].ts_type, "string");
        assert_eq!(analyzed.params[0].index, 1);

        // Projected Fields
        assert_eq!(analyzed.fields.len(), 4);

        // u.id -> non-nullable string
        assert_eq!(analyzed.fields[0].name, "id");
        assert_eq!(analyzed.fields[0].ts_type, "string");

        // u.email -> non-nullable string
        assert_eq!(analyzed.fields[1].name, "email");
        assert_eq!(analyzed.fields[1].ts_type, "string");

        // p.title AS post_title -> nullable string because posts is LEFT JOINed
        assert_eq!(analyzed.fields[2].name, "post_title");
        assert_eq!(analyzed.fields[2].ts_type, "string | null");

        // COUNT(c.id) AS comment_count -> strictly non-nullable number
        assert_eq!(analyzed.fields[3].name, "comment_count");
        assert_eq!(analyzed.fields[3].ts_type, "number");
    }

    #[test]
    fn test_sum_and_avg_aggregates() {
        let catalog = setup_test_catalog();
        let query_sql = "
-- name: GetPostStats
SELECT 
  SUM(p.views) AS total_views,
  AVG(p.views) AS avg_views
FROM posts p;
        ";
        let analyzed = analyze_query(query_sql, &catalog, None).unwrap();
        assert_eq!(analyzed.name, "GetPostStats");
        assert_eq!(analyzed.fields.len(), 2);
        assert_eq!(analyzed.fields[0].name, "total_views");
        assert_eq!(analyzed.fields[0].ts_type, "number | null");
        assert_eq!(analyzed.fields[1].name, "avg_views");
        assert_eq!(analyzed.fields[1].ts_type, "number | null");
    }

    #[test]
    fn test_fallback_filename_and_typecast_param() {
        let catalog = setup_test_catalog();
        let query_sql = "
SELECT u.id FROM users u WHERE u.email = $1::text;
        ";
        let analyzed = analyze_query(query_sql, &catalog, Some("get_user_by_email.sql")).unwrap();
        assert_eq!(analyzed.name, "GetUserByEmail");
        assert_eq!(analyzed.params.len(), 1);
        assert_eq!(analyzed.params[0].name, "email");
        assert_eq!(analyzed.params[0].ts_type, "string");
    }

    #[test]
    fn test_full_join_and_right_join_nullability() {
        let catalog = setup_test_catalog();

        // FULL JOIN: both sides nullable
        let full_sql = "
-- name: GetFull
SELECT u.id AS user_id, p.id AS post_id
FROM users u
FULL JOIN posts p ON p.user_id = u.id;
        ";
        let analyzed_full = analyze_query(full_sql, &catalog, None).unwrap();
        assert_eq!(analyzed_full.fields[0].ts_type, "string | null");
        assert_eq!(analyzed_full.fields[1].ts_type, "string | null");

        // RIGHT JOIN: left side nullable
        let right_sql = "
-- name: GetRight
SELECT u.id AS user_id, p.id AS post_id
FROM users u
RIGHT JOIN posts p ON p.user_id = u.id;
        ";
        let analyzed_right = analyze_query(right_sql, &catalog, None).unwrap();
        assert_eq!(analyzed_right.fields[0].ts_type, "string | null");
        assert_eq!(analyzed_right.fields[1].ts_type, "string");
    }

    #[test]
    fn test_wildcard_projections() {
        let catalog = setup_test_catalog();
        let star_sql = "SELECT * FROM users;";
        let analyzed_star = analyze_query(star_sql, &catalog, Some("all_users.sql")).unwrap();
        assert_eq!(analyzed_star.fields.len(), 2);
        assert_eq!(analyzed_star.fields[0].name, "id");
        assert_eq!(analyzed_star.fields[0].ts_type, "string");
        assert_eq!(analyzed_star.fields[1].name, "email");
        assert_eq!(analyzed_star.fields[1].ts_type, "string");

        let alias_star_sql = "SELECT u.* FROM users u LEFT JOIN posts p ON p.user_id = u.id;";
        let analyzed_alias_star =
            analyze_query(alias_star_sql, &catalog, Some("users_alias.sql")).unwrap();
        assert_eq!(analyzed_alias_star.fields.len(), 2);
        assert_eq!(analyzed_alias_star.fields[0].name, "id");
        assert_eq!(analyzed_alias_star.fields[0].ts_type, "string");
    }

    #[test]
    fn test_validation_errors() {
        let catalog = setup_test_catalog();

        // Unknown table
        let bad_table_sql = "SELECT id FROM nonexistent;";
        assert!(analyze_query(bad_table_sql, &catalog, None).is_err());

        // Unknown column
        let bad_col_sql = "SELECT nonexistent_col FROM users;";
        assert!(analyze_query(bad_col_sql, &catalog, None).is_err());
    }

    #[test]
    fn test_cte_selection() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                email TEXT NOT NULL,
                active BOOLEAN NOT NULL
            );
            CREATE TABLE posts (
                id UUID PRIMARY KEY,
                user_id UUID NOT NULL,
                title VARCHAR(255) NOT NULL
            );
        ",
            )
            .unwrap();

        let query_sql = "
            WITH active_users AS (
                SELECT id, email FROM users WHERE active = true
            )
            SELECT au.email, p.title 
            FROM active_users au 
            JOIN posts p ON p.user_id = au.id;
        ";
        let analyzed = analyze_query(query_sql, &catalog, Some("cte_query.sql")).unwrap();
        assert_eq!(analyzed.fields.len(), 2);
        assert_eq!(analyzed.fields[0].name, "email");
        assert_eq!(analyzed.fields[0].ts_type, "string");
        assert_eq!(analyzed.fields[1].name, "title");
        assert_eq!(analyzed.fields[1].ts_type, "string");
    }

    #[test]
    fn test_arithmetic_and_coalesce() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                age INT NOT NULL,
                nickname TEXT
            );
        ",
            )
            .unwrap();

        let query_sql = "
            SELECT (u.age + 1) AS next_age, COALESCE(u.nickname, 'anon') AS display_name FROM users u;
        ";
        let analyzed = analyze_query(query_sql, &catalog, Some("expr_query.sql")).unwrap();
        assert_eq!(analyzed.fields.len(), 2);
        assert_eq!(analyzed.fields[0].name, "next_age");
        assert_eq!(analyzed.fields[0].ts_type, "number");
        assert_eq!(analyzed.fields[1].name, "display_name");
        assert_eq!(analyzed.fields[1].ts_type, "string");
    }

    #[test]
    fn test_array_type_mappings() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE articles (
                id UUID PRIMARY KEY,
                tags TEXT[] NOT NULL,
                scores INT4[] NOT NULL,
                optional_tags TEXT[]
            );
        ",
            )
            .unwrap();

        let query_sql = "
            SELECT a.tags, a.scores, a.optional_tags, ARRAY['rust', 'typescript'] AS defaults
            FROM articles a;
        ";
        let analyzed = analyze_query(query_sql, &catalog, Some("array_query.sql")).unwrap();
        assert_eq!(analyzed.fields.len(), 4);
        assert_eq!(analyzed.fields[0].name, "tags");
        assert_eq!(analyzed.fields[0].ts_type, "Array<string>");
        assert_eq!(analyzed.fields[1].name, "scores");
        assert_eq!(analyzed.fields[1].ts_type, "Array<number>");
        assert_eq!(analyzed.fields[2].name, "optional_tags");
        assert_eq!(analyzed.fields[2].ts_type, "Array<string> | null");
        assert_eq!(analyzed.fields[3].name, "defaults");
        assert_eq!(analyzed.fields[3].ts_type, "Array<string>");
    }

    #[test]
    fn test_optional_dynamic_filter() {
        let mut catalog = Catalog::default();
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
        let analyzed = analyze_query(query_sql, &catalog, Some("optional_filter.sql")).unwrap();
        assert_eq!(analyzed.params.len(), 2);

        let p1 = &analyzed.params[0];
        assert_eq!(p1.index, 1);
        assert_eq!(p1.name, "name");
        assert_eq!(p1.ts_type, "string");
        assert!(p1.is_optional);

        let p2 = &analyzed.params[1];
        assert_eq!(p2.index, 2);
        assert_eq!(p2.name, "id");
        assert_eq!(p2.ts_type, "string");
        assert!(!p2.is_optional);
    }

    #[test]
    fn test_insert_with_returning() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
                CREATE TABLE users (
                    id UUID PRIMARY KEY,
                    name TEXT NOT NULL,
                    email VARCHAR(255) NOT NULL,
                    created_at TIMESTAMPTZ NOT NULL
                );
            ",
            )
            .unwrap();

        let query_sql = "
            -- name: CreateUser
            INSERT INTO users (name, email) VALUES ($1, $2) RETURNING id, created_at;
        ";
        let analyzed = analyze_query(query_sql, &catalog, None).unwrap();
        assert_eq!(analyzed.name, "CreateUser");
        assert_eq!(analyzed.params.len(), 2);

        // $1 -> users.name
        assert_eq!(analyzed.params[0].index, 1);
        assert_eq!(analyzed.params[0].name, "name");
        assert_eq!(analyzed.params[0].ts_type, "string");

        // $2 -> users.email
        assert_eq!(analyzed.params[1].index, 2);
        assert_eq!(analyzed.params[1].name, "email");
        assert_eq!(analyzed.params[1].ts_type, "string");

        // RETURNING id, created_at
        assert_eq!(analyzed.fields.len(), 2);
        assert_eq!(analyzed.fields[0].name, "id");
        assert_eq!(analyzed.fields[0].ts_type, "string");
        assert_eq!(analyzed.fields[1].name, "created_at");
        assert_eq!(analyzed.fields[1].ts_type, "Date");
    }

    #[test]
    fn test_update_with_returning() {
        let mut catalog = Catalog::default();
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
        let analyzed = analyze_query(query_sql, &catalog, None).unwrap();
        assert_eq!(analyzed.name, "UpdateUser");
        assert_eq!(analyzed.params.len(), 2);

        // $1 -> users.name
        assert_eq!(analyzed.params[0].index, 1);
        assert_eq!(analyzed.params[0].name, "name");
        assert_eq!(analyzed.params[0].ts_type, "string");

        // $2 -> users.id
        assert_eq!(analyzed.params[1].index, 2);
        assert_eq!(analyzed.params[1].name, "id");
        assert_eq!(analyzed.params[1].ts_type, "string");

        // RETURNING id, name
        assert_eq!(analyzed.fields.len(), 2);
        assert_eq!(analyzed.fields[0].name, "id");
        assert_eq!(analyzed.fields[0].ts_type, "string");
        assert_eq!(analyzed.fields[1].name, "name");
        assert_eq!(analyzed.fields[1].ts_type, "string");
    }

    #[test]
    fn test_delete_without_returning() {
        let mut catalog = Catalog::default();
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
        let analyzed = analyze_query(query_sql, &catalog, None).unwrap();
        assert_eq!(analyzed.name, "DeleteUser");
        assert_eq!(analyzed.params.len(), 1);

        // $1 -> users.id
        assert_eq!(analyzed.params[0].index, 1);
        assert_eq!(analyzed.params[0].name, "id");
        assert_eq!(analyzed.params[0].ts_type, "string");

        // No row return projection
        assert!(analyzed.fields.is_empty());
    }

    #[test]
    fn test_unify_types() {
        assert_eq!(
            unify_types(&PgType::Int2, &PgType::Int4).unwrap(),
            PgType::Int4
        );
        assert_eq!(
            unify_types(&PgType::Int4, &PgType::Int8).unwrap(),
            PgType::Int8
        );
        assert_eq!(
            unify_types(&PgType::Int8, &PgType::Numeric).unwrap(),
            PgType::Numeric
        );
        assert_eq!(
            unify_types(&PgType::Float4, &PgType::Float8).unwrap(),
            PgType::Float8
        );
        assert_eq!(
            unify_types(&PgType::Float4, &PgType::Numeric).unwrap(),
            PgType::Numeric
        );
        assert_eq!(
            unify_types(&PgType::Int4, &PgType::Float4).unwrap(),
            PgType::Float8
        );

        // String hierarchy: Varchar / Uuid widen to Text
        assert_eq!(
            unify_types(&PgType::Varchar, &PgType::Text).unwrap(),
            PgType::Text
        );
        assert_eq!(
            unify_types(&PgType::Uuid, &PgType::Text).unwrap(),
            PgType::Text
        );
        assert_eq!(
            unify_types(&PgType::Varchar, &PgType::Uuid).unwrap(),
            PgType::Text
        );

        // Datetime hierarchy: Date -> Timestamp -> Timestamptz
        assert_eq!(
            unify_types(&PgType::Date, &PgType::Timestamp).unwrap(),
            PgType::Timestamp
        );
        assert_eq!(
            unify_types(&PgType::Timestamp, &PgType::Timestamptz).unwrap(),
            PgType::Timestamptz
        );
        assert_eq!(
            unify_types(&PgType::Date, &PgType::Timestamptz).unwrap(),
            PgType::Timestamptz
        );

        // Unknown unifies to concrete companion
        assert_eq!(
            unify_types(&PgType::Unknown, &PgType::Text).unwrap(),
            PgType::Text
        );
        assert_eq!(
            unify_types(&PgType::Int8, &PgType::Unknown).unwrap(),
            PgType::Int8
        );

        // Incompatible types
        assert!(unify_types(&PgType::Bool, &PgType::Int4).is_err());
    }

    #[test]
    fn test_advanced_coalesce_inference() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                nickname TEXT,
                alt_name VARCHAR(100)
            );
        ",
            )
            .unwrap();

        // Statically non-nullable because literal 'anonymous' is non-null
        let sql = "SELECT COALESCE(users.nickname, 'anonymous') AS display_name FROM users;";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "display_name");
        assert_eq!(analyzed.fields[0].ts_type, "string");

        // Nullable because both operands are nullable columns
        let sql2 = "SELECT COALESCE(users.nickname, users.alt_name) AS display_name FROM users;";
        let analyzed2 = analyze_query(sql2, &catalog, None).unwrap();
        assert_eq!(analyzed2.fields.len(), 1);
        assert_eq!(analyzed2.fields[0].name, "display_name");
        assert_eq!(analyzed2.fields[0].ts_type, "string | null");
    }

    #[test]
    fn test_advanced_case_inference() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                status INT NOT NULL
            );
        ",
            )
            .unwrap();

        // Non-nullable string because both branches are non-null and ELSE is present
        let sql = "SELECT CASE WHEN status = 1 THEN 'active' ELSE 'inactive' END AS status_text FROM users;";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "status_text");
        assert_eq!(analyzed.fields[0].ts_type, "string");

        // Nullable string because ELSE is omitted
        let sql2 = "SELECT CASE WHEN status = 1 THEN 'active' END AS status_text FROM users;";
        let analyzed2 = analyze_query(sql2, &catalog, None).unwrap();
        assert_eq!(analyzed2.fields.len(), 1);
        assert_eq!(analyzed2.fields[0].name, "status_text");
        assert_eq!(analyzed2.fields[0].ts_type, "string | null");
    }

    #[test]
    fn test_advanced_window_functions_inference() {
        let mut catalog_pg = Catalog::default();
        catalog_pg
            .apply_sql(
                "
            CREATE TABLE users (
                id UUID PRIMARY KEY
            );
        ",
            )
            .unwrap();

        let sql = "SELECT ROW_NUMBER() OVER (ORDER BY id) AS row_num FROM users;";
        let analyzed_pg = analyze_query(sql, &catalog_pg, None).unwrap();
        assert_eq!(analyzed_pg.fields.len(), 1);
        assert_eq!(analyzed_pg.fields[0].name, "row_num");
        // Under postgres/pg driver, Int8 maps to string
        assert_eq!(analyzed_pg.fields[0].ts_type, "string");

        // Under bun driver, Int8 maps to bigint
        let catalog_bun = catalog_pg.clone().with_driver(DriverTarget::Bun);
        let analyzed_bun = analyze_query(sql, &catalog_bun, None).unwrap();
        assert_eq!(analyzed_bun.fields.len(), 1);
        assert_eq!(analyzed_bun.fields[0].name, "row_num");
        assert_eq!(analyzed_bun.fields[0].ts_type, "bigint");
    }

    #[test]
    fn test_advanced_scalar_subquery_inference() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                email TEXT NOT NULL
            );
        ",
            )
            .unwrap();

        let sql = "SELECT (SELECT id FROM users WHERE email = $1) AS user_id;";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "user_id");
        // Scalar subquery is strictly nullable because 0 matching rows yields SQL NULL
        assert_eq!(analyzed.fields[0].ts_type, "string | null");

        // Parameter $1 inferred from subquery's WHERE email = $1
        assert_eq!(analyzed.params.len(), 1);
        assert_eq!(analyzed.params[0].index, 1);
        assert_eq!(analyzed.params[0].name, "email");
        assert_eq!(analyzed.params[0].ts_type, "string");
        assert!(!analyzed.params[0].is_optional);
    }

    #[test]
    fn test_null_test_and_boolean_test() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                nickname TEXT,
                active BOOLEAN NOT NULL
            );
        ",
            )
            .unwrap();

        let sql =
            "SELECT (nickname IS NULL) AS is_missing, (active IS TRUE) AS is_active FROM users;";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 2);
        assert_eq!(analyzed.fields[0].name, "is_missing");
        assert_eq!(analyzed.fields[0].ts_type, "boolean");
        assert_eq!(analyzed.fields[1].name, "is_active");
        assert_eq!(analyzed.fields[1].ts_type, "boolean");
    }

    #[test]
    fn test_left_join_nullability() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                email TEXT NOT NULL
            );
            CREATE TABLE profiles (
                id UUID PRIMARY KEY,
                user_id UUID NOT NULL,
                bio TEXT NOT NULL
            );
        ",
            )
            .unwrap();

        let sql = "
            SELECT u.id, u.email, p.bio
            FROM users u
            LEFT JOIN profiles p ON u.id = p.user_id;
        ";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 3);
        assert_eq!(analyzed.fields[0].name, "id");
        assert_eq!(analyzed.fields[0].ts_type, "string");
        assert_eq!(analyzed.fields[1].name, "email");
        assert_eq!(analyzed.fields[1].ts_type, "string");
        assert_eq!(analyzed.fields[2].name, "bio");
        assert_eq!(analyzed.fields[2].ts_type, "string | null");
    }

    #[test]
    fn test_nested_join_traversal() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                name TEXT NOT NULL
            );
            CREATE TABLE posts (
                id UUID PRIMARY KEY,
                user_id UUID NOT NULL,
                title TEXT NOT NULL
            );
            CREATE TABLE tags (
                id UUID PRIMARY KEY,
                post_id UUID NOT NULL,
                tag TEXT NOT NULL
            );
        ",
            )
            .unwrap();

        // Nested join on right side: users LEFT JOIN (posts JOIN tags)
        let sql = "
            SELECT u.name, p.title, t.tag
            FROM users u
            LEFT JOIN (posts p JOIN tags t ON p.id = t.post_id) ON u.id = p.user_id;
        ";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 3);
        assert_eq!(analyzed.fields[0].name, "name");
        assert_eq!(analyzed.fields[0].ts_type, "string");
        assert_eq!(analyzed.fields[1].name, "title");
        assert_eq!(analyzed.fields[1].ts_type, "string | null");
        assert_eq!(analyzed.fields[2].name, "tag");
        assert_eq!(analyzed.fields[2].ts_type, "string | null");
    }

    #[test]
    fn test_column_ambiguity_error() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                name TEXT NOT NULL
            );
            CREATE TABLE profiles (
                id UUID PRIMARY KEY,
                user_id UUID NOT NULL
            );
        ",
            )
            .unwrap();

        let sql = "SELECT id FROM users JOIN profiles ON users.id = profiles.user_id;";
        let err = analyze_query(sql, &catalog, None).unwrap_err();
        assert!(
            err.contains(
                "Column reference \"id\" is ambiguous. Present in tables: users, profiles"
            )
        );
    }

    #[test]
    fn test_alias_hiding_error() {
        let mut catalog = Catalog::default();
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

        let sql = "SELECT users.name FROM users u;";
        let err = analyze_query(sql, &catalog, None).unwrap_err();
        assert!(
            err.contains("Cannot reference base table \"users\" because it is aliased as \"u\"")
        );
    }

    #[test]
    fn test_column_aliasing() {
        let mut catalog = Catalog::default();
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

        let sql = "SELECT u.user_id, u.user_name FROM users u(user_id, user_name);";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 2);
        assert_eq!(analyzed.fields[0].name, "user_id");
        assert_eq!(analyzed.fields[0].ts_type, "string");
        assert_eq!(analyzed.fields[1].name, "user_name");
        assert_eq!(analyzed.fields[1].ts_type, "string");
    }

    #[test]
    fn test_schema_qualification_and_alias_hiding() {
        let mut catalog = Catalog::default();
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

        let sql_err = "SELECT public.users.name FROM users u;";
        let err = analyze_query(sql_err, &catalog, None).unwrap_err();
        assert!(
            err.contains("Cannot reference base table \"users\" because it is aliased as \"u\"")
        );

        let sql_ok = "SELECT public.users.name FROM users;";
        let analyzed = analyze_query(sql_ok, &catalog, None).unwrap();
        assert_eq!(analyzed.fields[0].name, "name");
        assert_eq!(analyzed.fields[0].ts_type, "string");
    }

    #[test]
    fn test_query_scope_force_all_nullable() {
        let mut columns = HashMap::new();
        columns.insert(
            "id".to_string(),
            ColumnMetadata {
                name: "id".to_string(),
                pg_type: "uuid".to_string(),
                ts_type: "string".to_string(),
                is_nullable: false,
                has_default: false,
            },
        );

        let binding = TableBinding {
            base_table: "users".to_string(),
            exposed_name: "u".to_string(),
            has_explicit_alias: true,
            is_null_padded: false,
            columns,
        };

        let mut scope = QueryScope::default();
        scope.add_binding(binding).unwrap();

        assert!(!scope.bindings["u"].is_null_padded);
        assert!(!scope.bindings["u"].columns["id"].is_nullable);

        scope.force_all_nullable();

        assert!(scope.bindings["u"].is_null_padded);
        assert!(scope.bindings["u"].columns["id"].is_nullable);
    }

    #[test]
    fn test_cte_standard_inference() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                email TEXT NOT NULL,
                age INT NOT NULL
            );
        ",
            )
            .unwrap();

        let sql = "
            WITH user_summary AS (
                SELECT id, email, (age + 1) AS next_age FROM users
            )
            SELECT email, next_age FROM user_summary;
        ";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 2);
        assert_eq!(analyzed.fields[0].name, "email");
        assert_eq!(analyzed.fields[0].ts_type, "string");
        assert_eq!(analyzed.fields[1].name, "next_age");
        assert_eq!(analyzed.fields[1].ts_type, "number");
    }

    #[test]
    fn test_cte_positional_aliasing() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                email TEXT NOT NULL
            );
        ",
            )
            .unwrap();

        let sql = "
            WITH user_emails(uid, contact_email) AS (
                SELECT id, email FROM users
            )
            SELECT ue.uid, ue.contact_email FROM user_emails ue;
        ";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 2);
        assert_eq!(analyzed.fields[0].name, "uid");
        assert_eq!(analyzed.fields[0].ts_type, "string");
        assert_eq!(analyzed.fields[1].name, "contact_email");
        assert_eq!(analyzed.fields[1].ts_type, "string");

        // Length mismatch error
        let bad_sql = "
            WITH user_emails(uid) AS (
                SELECT id, email FROM users
            )
            SELECT uid FROM user_emails;
        ";
        let err = analyze_query(bad_sql, &catalog, None).unwrap_err();
        assert!(err.contains("columns available but"));
    }

    #[test]
    fn test_cte_recursive_counter() {
        let catalog = Catalog::default();
        let sql = "
            WITH RECURSIVE counter AS (
                SELECT 1 AS n
                UNION ALL
                SELECT n + 1 FROM counter WHERE n < 10
            )
            SELECT n FROM counter;
        ";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "n");
        assert_eq!(analyzed.fields[0].ts_type, "number");
    }

    #[test]
    fn test_cte_catalog_shadowing() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE items (
                id UUID PRIMARY KEY,
                name TEXT NOT NULL
            );
        ",
            )
            .unwrap();

        // Unqualified 'items' shadows physical table
        let sql_shadow = "
            WITH items AS (
                SELECT 1 AS virtual_id
            )
            SELECT virtual_id FROM items;
        ";
        let analyzed_shadow = analyze_query(sql_shadow, &catalog, None).unwrap();
        assert_eq!(analyzed_shadow.fields.len(), 1);
        assert_eq!(analyzed_shadow.fields[0].name, "virtual_id");
        assert_eq!(analyzed_shadow.fields[0].ts_type, "number");

        // Schema-qualified 'public.items' accesses physical table
        let sql_physical = "
            WITH items AS (
                SELECT 1 AS virtual_id
            )
            SELECT id, name FROM public.items;
        ";
        let analyzed_physical = analyze_query(sql_physical, &catalog, None).unwrap();
        assert_eq!(analyzed_physical.fields.len(), 2);
        assert_eq!(analyzed_physical.fields[0].name, "id");
        assert_eq!(analyzed_physical.fields[0].ts_type, "string");
        assert_eq!(analyzed_physical.fields[1].name, "name");
        assert_eq!(analyzed_physical.fields[1].ts_type, "string");
    }

    #[test]
    fn test_cte_duplicate_rejection() {
        let catalog = Catalog::default();
        let sql = "
            WITH a AS (SELECT 1 AS x), a AS (SELECT 2 AS x)
            SELECT x FROM a;
        ";
        let err = analyze_query(sql, &catalog, None).unwrap_err();
        assert!(err.contains("WITH query name \"a\" specified more than once"));
    }

    #[test]
    fn test_setop_union_all_widening_and_nullability() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE tbl_a (
                id INT NOT NULL,
                val VARCHAR(50) NOT NULL
            );
            CREATE TABLE tbl_b (
                id BIGINT,
                val TEXT
            );
        ",
            )
            .unwrap();

        let query = "
-- name: GetCombined
SELECT id, val FROM tbl_a
UNION ALL
SELECT id, val FROM tbl_b;
        ";
        let analyzed = analyze_query(query, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 2);

        // id: int4 + int8 -> int8. Is nullable because tbl_b.id is nullable
        assert_eq!(analyzed.fields[0].name, "id");
        assert_eq!(analyzed.fields[0].ts_type, "string | null");

        // val: varchar + text -> text. Is nullable because tbl_b.val is nullable
        assert_eq!(analyzed.fields[1].name, "val");
        assert_eq!(analyzed.fields[1].ts_type, "string | null");
    }

    #[test]
    fn test_setop_except_nullability_retainment() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
            CREATE TABLE active_users (
                id UUID NOT NULL,
                name TEXT NOT NULL
            );
            CREATE TABLE suspended_users (
                id UUID,
                name TEXT
            );
        ",
            )
            .unwrap();

        let except_query = "
-- name: GetActiveNotSuspended
SELECT id, name FROM active_users
EXCEPT
SELECT id, name FROM suspended_users;
        ";
        let analyzed_except = analyze_query(except_query, &catalog, None).unwrap();
        assert_eq!(analyzed_except.fields.len(), 2);
        // Candidate rows strictly from left branch (active_users), which is NOT NULL
        assert_eq!(analyzed_except.fields[0].name, "id");
        assert_eq!(analyzed_except.fields[0].ts_type, "string");
        assert_eq!(analyzed_except.fields[1].name, "name");
        assert_eq!(analyzed_except.fields[1].ts_type, "string");

        let intersect_query = "
-- name: GetActiveAndSuspended
SELECT id, name FROM active_users
INTERSECT
SELECT id, name FROM suspended_users;
        ";
        let analyzed_intersect = analyze_query(intersect_query, &catalog, None).unwrap();
        assert_eq!(analyzed_intersect.fields.len(), 2);
        assert_eq!(analyzed_intersect.fields[0].name, "id");
        assert_eq!(analyzed_intersect.fields[0].ts_type, "string");
        assert_eq!(analyzed_intersect.fields[1].name, "name");
        assert_eq!(analyzed_intersect.fields[1].ts_type, "string");
    }

    #[test]
    fn test_setop_column_count_mismatch() {
        let catalog = Catalog::default();
        let query = "
SELECT 1 AS a, 2 AS b
UNION
SELECT 3 AS a;
        ";
        let err = analyze_query(query, &catalog, None).unwrap_err();
        assert!(err.contains(
            "Each Union query must have the same number of columns: expected 2, found 1"
        ));
    }

    #[test]
    fn test_setop_nested_tree() {
        let catalog = setup_test_catalog();

        let query = "
-- name: GetNestedSet
(
    SELECT u.id, u.email FROM users u
    UNION ALL
    SELECT u.id, u.email FROM users u
)
EXCEPT
SELECT u.id, u.email FROM users u
ORDER BY email DESC, 1 ASC
LIMIT $1 OFFSET $2;
        ";
        let analyzed = analyze_query(query, &catalog, None).unwrap();
        assert_eq!(analyzed.name, "GetNestedSet");
        assert_eq!(analyzed.fields.len(), 2);
        assert_eq!(analyzed.fields[0].name, "id");
        assert_eq!(analyzed.fields[0].ts_type, "string");
        assert_eq!(analyzed.fields[1].name, "email");
        assert_eq!(analyzed.fields[1].ts_type, "string");

        // Validate parameter inference for LIMIT and OFFSET
        assert_eq!(analyzed.params.len(), 2);
        assert_eq!(analyzed.params[0].index, 1);
        assert_eq!(analyzed.params[0].name, "limit");
        assert_eq!(analyzed.params[0].ts_type, "number");
        assert_eq!(analyzed.params[1].index, 2);
        assert_eq!(analyzed.params[1].name, "offset");
        assert_eq!(analyzed.params[1].ts_type, "number");
    }

    #[test]
    fn test_setop_order_by_validation() {
        let catalog = setup_test_catalog();

        // 1. Qualified column name error
        let err_qualified = analyze_query(
            "SELECT id FROM users UNION SELECT id FROM users ORDER BY users.id;",
            &catalog,
            None,
        )
        .unwrap_err();
        assert!(err_qualified.contains(
            "Qualified column name \"users.id\" is not allowed in set operation ORDER BY"
        ));

        // 2. Nonexistent column name
        let err_nonexistent = analyze_query(
            "SELECT id FROM users UNION SELECT id FROM users ORDER BY non_existent_col;",
            &catalog,
            None,
        )
        .unwrap_err();
        assert!(err_nonexistent.contains(
            "Column \"non_existent_col\" in ORDER BY does not exist in set operation result"
        ));

        // 3. Positional ordinal out of range
        let err_out_of_range = analyze_query(
            "SELECT id FROM users UNION SELECT id FROM users ORDER BY 5;",
            &catalog,
            None,
        )
        .unwrap_err();
        assert!(
            err_out_of_range
                .contains("ORDER BY position 5 is out of range: must be between 1 and 1")
        );

        // 4. Positional ordinal 0 (out of range)
        let err_zero = analyze_query(
            "SELECT id FROM users UNION SELECT id FROM users ORDER BY 0;",
            &catalog,
            None,
        )
        .unwrap_err();
        assert!(err_zero.contains("ORDER BY position 0 is out of range: must be between 1 and 1"));
    }

    fn setup_json_test_catalog() -> Catalog {
        let sql = "
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                name TEXT NOT NULL,
                metadata JSONB
            );
        ";
        let mut catalog = Catalog::default();
        catalog.apply_sql(sql).unwrap();
        catalog
    }

    #[test]
    fn test_jsonb_build_object_inference() {
        let catalog = setup_json_test_catalog();
        let query = "
-- name: GetUserJson
SELECT jsonb_build_object('id', u.id, 'name', u.name) AS user_obj
FROM users u;
        ";
        let analyzed = analyze_query(query, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "user_obj");
        assert_eq!(analyzed.fields[0].ts_type, "{ id: string; name: string }");
    }

    #[test]
    fn test_json_build_object_odd_args_error() {
        let catalog = setup_json_test_catalog();
        let query = "
SELECT json_build_object('id') FROM users;
        ";
        let err = analyze_query(query, &catalog, None).unwrap_err();
        assert!(err.contains(
            "Argument list for json_build_object must have an even number of elements; found 1"
        ));
    }

    #[test]
    fn test_json_agg_inference() {
        let catalog = setup_json_test_catalog();
        let query = "
-- name: GetUsersAgg
SELECT json_agg(jsonb_build_object('id', u.id)) AS users_agg
FROM users u;
        ";
        let analyzed = analyze_query(query, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "users_agg");
        assert_eq!(analyzed.fields[0].ts_type, "Array<{ id: string }> | null");
    }

    #[test]
    fn test_coalesce_json_agg_shielding() {
        let catalog = setup_json_test_catalog();
        let query = "
-- name: GetUsersAggCoalesced
SELECT COALESCE(jsonb_agg(jsonb_build_object('id', u.id)), '[]'::jsonb) AS users_agg
FROM users u;
        ";
        let analyzed = analyze_query(query, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "users_agg");
        assert_eq!(analyzed.fields[0].ts_type, "Array<{ id: string }>");
    }

    #[test]
    fn test_json_operators_text_extraction() {
        let catalog = setup_json_test_catalog();
        let query = "
-- name: GetUserRole
SELECT u.metadata->>'role' AS role
FROM users u;
        ";
        let analyzed = analyze_query(query, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "role");
        assert_eq!(analyzed.fields[0].ts_type, "string | null");
    }

    #[test]
    fn test_json_operators_deep_and_nested() {
        let catalog = setup_json_test_catalog();

        // -> on JsonObject returns nested type (nullable = true)
        let query_field = "
-- name: GetNestedField
SELECT jsonb_build_object('nested', jsonb_build_object('id', u.id))->'nested' AS nested_obj
FROM users u;
        ";
        let analyzed_field = analyze_query(query_field, &catalog, None).unwrap();
        assert_eq!(analyzed_field.fields[0].ts_type, "{ id: string } | null");

        // #> with static array path matches nested JsonObject
        let query_path = "
-- name: GetNestedPath
SELECT jsonb_build_object('a', jsonb_build_object('b', u.name)) #> '{a, b}' AS val
FROM users u;
        ";
        let analyzed_path = analyze_query(query_path, &catalog, None).unwrap();
        assert_eq!(analyzed_path.fields[0].ts_type, "string | null");

        // Empty json_build_object -> Record<string, never>
        let query_empty = "
-- name: GetEmpty
SELECT jsonb_build_object() AS empty_obj
FROM users u;
        ";
        let analyzed_empty = analyze_query(query_empty, &catalog, None).unwrap();
        assert_eq!(analyzed_empty.fields[0].ts_type, "Record<string, never>");

        // Dynamic key json_build_object -> Record<string, unknown>
        let query_dyn = "
-- name: GetDynamic
SELECT jsonb_build_object(u.name, u.id) AS dyn_obj
FROM users u;
        ";
        let analyzed_dyn = analyze_query(query_dyn, &catalog, None).unwrap();
        assert_eq!(analyzed_dyn.fields[0].ts_type, "Record<string, unknown>");
    }

    #[test]
    fn test_dml_insert_values_and_returning_wildcard() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
                CREATE TABLE users (
                    id UUID PRIMARY KEY,
                    name TEXT NOT NULL,
                    email TEXT NOT NULL
                );
            ",
            )
            .unwrap();

        let sql = "
            -- name: InsertUser
            INSERT INTO users (id, name, email) VALUES ($1, $2, $3) RETURNING *;
        ";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.name, "InsertUser");
        assert_eq!(analyzed.params.len(), 3);
        assert_eq!(analyzed.params[0].name, "id");
        assert_eq!(analyzed.params[0].ts_type, "string");
        assert_eq!(analyzed.params[1].name, "name");
        assert_eq!(analyzed.params[1].ts_type, "string");
        assert_eq!(analyzed.params[2].name, "email");
        assert_eq!(analyzed.params[2].ts_type, "string");

        assert_eq!(analyzed.fields.len(), 3);
        assert_eq!(analyzed.fields[0].name, "id");
        assert_eq!(analyzed.fields[0].ts_type, "string");
        assert_eq!(analyzed.fields[1].name, "name");
        assert_eq!(analyzed.fields[1].ts_type, "string");
        assert_eq!(analyzed.fields[2].name, "email");
        assert_eq!(analyzed.fields[2].ts_type, "string");
    }

    #[test]
    fn test_dml_upsert_with_excluded() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
                CREATE TABLE counters (
                    key TEXT PRIMARY KEY,
                    val INT NOT NULL
                );
            ",
            )
            .unwrap();

        let sql = "
            -- name: UpsertCounter
            INSERT INTO counters (key, val) VALUES ($1, $2)
            ON CONFLICT (key) DO UPDATE SET val = EXCLUDED.val + 1
            RETURNING val;
        ";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.name, "UpsertCounter");
        assert_eq!(analyzed.params.len(), 2);
        assert_eq!(analyzed.params[0].name, "key");
        assert_eq!(analyzed.params[0].ts_type, "string");
        assert_eq!(analyzed.params[1].name, "val");
        assert_eq!(analyzed.params[1].ts_type, "number");

        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "val");
        assert_eq!(analyzed.fields[0].ts_type, "number");
    }

    #[test]
    fn test_dml_update_with_set_parameters() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
                CREATE TABLE accounts (
                    id UUID PRIMARY KEY,
                    balance NUMERIC NOT NULL
                );
            ",
            )
            .unwrap();

        let sql = "
            -- name: DebitAccount
            UPDATE accounts SET balance = balance - $1 WHERE id = $2 RETURNING balance;
        ";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.name, "DebitAccount");
        assert_eq!(analyzed.params.len(), 2);
        assert_eq!(analyzed.params[0].name, "balance");
        assert_eq!(analyzed.params[0].ts_type, "number");
        assert_eq!(analyzed.params[1].name, "id");
        assert_eq!(analyzed.params[1].ts_type, "string");

        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "balance");
        assert_eq!(analyzed.fields[0].ts_type, "number");
    }

    #[test]
    fn test_dml_delete_with_where() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "
                CREATE TABLE sessions (
                    id UUID PRIMARY KEY,
                    expires_at TIMESTAMPTZ NOT NULL
                );
            ",
            )
            .unwrap();

        let sql = "
            -- name: CleanSessions
            DELETE FROM sessions WHERE expires_at < $1;
        ";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.name, "CleanSessions");
        assert_eq!(analyzed.params.len(), 1);
        assert_eq!(analyzed.params[0].name, "expires_at");
        assert_eq!(analyzed.params[0].ts_type, "Date");
        assert_eq!(analyzed.fields.len(), 0);
    }

    #[test]
    fn test_scalar_array_any_comparison() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "CREATE TABLE users (
                    id UUID PRIMARY KEY,
                    name TEXT NOT NULL
                );",
            )
            .unwrap();

        let sql = "SELECT id FROM users WHERE id = ANY($1);";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.params.len(), 1);
        assert_eq!(analyzed.params[0].name, "id");
        assert_eq!(analyzed.params[0].ts_type, "Array<string>");
        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "id");
        assert_eq!(analyzed.fields[0].ts_type, "string");
    }

    #[test]
    fn test_array_overlap_operator() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "CREATE TABLE posts (
                    id UUID PRIMARY KEY,
                    tags TEXT[] NOT NULL
                );",
            )
            .unwrap();

        let sql = "SELECT id FROM posts WHERE tags && $1;";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.params.len(), 1);
        assert_eq!(analyzed.params[0].name, "tags");
        assert_eq!(analyzed.params[0].ts_type, "Array<string>");
        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "id");
        assert_eq!(analyzed.fields[0].ts_type, "string");
    }

    #[test]
    fn test_array_constructor() {
        let catalog = Catalog::default();
        let sql = "SELECT ARRAY[1, 2, 3] AS nums;";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "nums");
        assert_eq!(analyzed.fields[0].ts_type, "Array<number>");
    }

    #[test]
    fn test_range_function_unnest() {
        let catalog = Catalog::default();
        let sql = "SELECT item FROM unnest(ARRAY['a', 'b']) AS t(item);";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.fields.len(), 1);
        assert_eq!(analyzed.fields[0].name, "item");
        assert_eq!(analyzed.fields[0].ts_type, "string");
    }

    #[test]
    fn test_array_operators_and_empty_constructor() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "CREATE TABLE items (
                    id UUID PRIMARY KEY,
                    tags TEXT[] NOT NULL
                );",
            )
            .unwrap();

        // Containment operator @>
        let sql = "SELECT id FROM items WHERE tags @> $1;";
        let analyzed = analyze_query(sql, &catalog, None).unwrap();
        assert_eq!(analyzed.params.len(), 1);
        assert_eq!(analyzed.params[0].name, "tags");
        assert_eq!(analyzed.params[0].ts_type, "Array<string>");

        // Array concatenation
        let sql2 =
            "SELECT ARRAY['a', 'b'] || ARRAY['c'] AS concatenated, ARRAY[]::int4[] AS empty_ints;";
        let analyzed2 = analyze_query(sql2, &catalog, None).unwrap();
        assert_eq!(analyzed2.fields.len(), 2);
        assert_eq!(analyzed2.fields[0].name, "concatenated");
        assert_eq!(analyzed2.fields[0].ts_type, "Array<string>");
        assert_eq!(analyzed2.fields[1].name, "empty_ints");
        assert_eq!(analyzed2.fields[1].ts_type, "Array<number>");
    }
}
