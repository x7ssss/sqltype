#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMetadata {
    pub name: String,
    pub pg_type: String,
    pub ts_type: String,
    pub is_nullable: bool,
    pub has_default: bool,
    pub is_primary_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableMetadata {
    pub name: String,
    pub schema: Option<String>,
    pub columns: Vec<ColumnMetadata>,
    pub primary_keys: Vec<String>,
}

impl TableMetadata {
    pub fn get_column(&self, name: &str) -> Option<&ColumnMetadata> {
        self.columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    pub fn get_column_mut(&mut self, name: &str) -> Option<&mut ColumnMetadata> {
        self.columns
            .iter_mut()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum DriverTarget {
    #[default]
    Postgres,
    Pg,
    Bun,
}

impl std::fmt::Display for DriverTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriverTarget::Postgres => write!(f, "postgres"),
            DriverTarget::Pg => write!(f, "pg"),
            DriverTarget::Bun => write!(f, "bun"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositeTypeAttribute {
    pub name: String,
    pub pg_type: String,
    pub ts_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositeTypeMetadata {
    pub name: String,
    pub schema: Option<String>,
    pub attributes: Vec<CompositeTypeAttribute>,
}

impl CompositeTypeMetadata {
    pub fn to_ts(&self) -> String {
        if self.attributes.is_empty() {
            "{}".to_string()
        } else {
            let inner = self
                .attributes
                .iter()
                .map(|a| {
                    format!(
                        "{}: {}",
                        crate::codegen::format_property_key(&a.name),
                        a.ts_type
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            format!("{{ {} }}", inner)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainMetadata {
    pub name: String,
    pub schema: Option<String>,
    pub base_type: String,
    pub ts_type: String,
    pub is_not_null: bool,
    pub has_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QualifiedTypeName {
    pub schema: Option<String>,
    pub name: String,
    pub is_array: bool,
    pub typmod: Option<String>,
}

impl QualifiedTypeName {
    pub fn to_type_string(&self) -> String {
        let typmod_str = self.typmod.as_deref().unwrap_or("");
        let array_str = if self.is_array { "[]" } else { "" };
        if let Some(ref s) = self.schema {
            if s == "pg_catalog" && self.name == "varchar" {
                format!("varchar{}{}", typmod_str, array_str)
            } else if s == "pg_catalog" {
                format!("pg_catalog.{}{}{}", self.name, typmod_str, array_str)
            } else {
                format!("{}.{}{}{}", s, self.name, typmod_str, array_str)
            }
        } else {
            format!("{}{}{}", self.name, typmod_str, array_str)
        }
    }
}
