use crate::analyzer::PgType;
use std::collections::HashMap;

pub mod postgis;
pub mod vector;

pub use postgis::PostGisExtension;
pub use vector::PgVectorExtension;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionType {
    pub name: &'static str,
    pub pg_type: PgType,
    pub aliases: &'static [&'static str],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionOperator {
    pub name: &'static str,
    pub left: PgType,
    pub right: PgType,
    pub returns: PgType,
    pub description: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionFunction {
    pub name: &'static str,
    pub params: Vec<PgType>,
    pub variadic: bool,
    pub returns: PgType,
}

pub trait ExtensionDefinition: Send + Sync {
    fn name(&self) -> &'static str;
    fn types(&self) -> Vec<ExtensionType>;
    fn operators(&self) -> Vec<ExtensionOperator>;
    fn functions(&self) -> Vec<ExtensionFunction>;
}

pub struct ExtensionRegistry {
    extensions: HashMap<String, Box<dyn ExtensionDefinition>>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self {
            extensions: HashMap::new(),
        }
    }

    pub fn register(&mut self, ext: Box<dyn ExtensionDefinition>) {
        self.extensions.insert(ext.name().to_ascii_lowercase(), ext);
    }

    pub fn get(&self, name: &str) -> Option<&dyn ExtensionDefinition> {
        self.extensions
            .get(&name.to_ascii_lowercase())
            .map(|b| b.as_ref())
    }
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(PgVectorExtension));
        registry.register(Box::new(PostGisExtension));
        registry
    }
}
