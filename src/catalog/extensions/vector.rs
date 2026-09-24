use super::{ExtensionDefinition, ExtensionFunction, ExtensionOperator, ExtensionType};
use crate::analyzer::PgType;

pub struct PgVectorExtension;

impl ExtensionDefinition for PgVectorExtension {
    fn name(&self) -> &'static str {
        "vector"
    }

    fn types(&self) -> Vec<ExtensionType> {
        vec![
            ExtensionType {
                name: "vector",
                pg_type: PgType::Vector(None),
                aliases: &[],
            },
            ExtensionType {
                name: "halfvec",
                pg_type: PgType::HalfVec(None),
                aliases: &[],
            },
        ]
    }

    fn operators(&self) -> Vec<ExtensionOperator> {
        vec![
            // Distance operators returning PgType::Float8
            ExtensionOperator {
                name: "<=>",
                left: PgType::Vector(None),
                right: PgType::Vector(None),
                returns: PgType::Float8,
                description: "cosine distance",
            },
            ExtensionOperator {
                name: "<=>",
                left: PgType::HalfVec(None),
                right: PgType::HalfVec(None),
                returns: PgType::Float8,
                description: "halfvec cosine distance",
            },
            ExtensionOperator {
                name: "<->",
                left: PgType::Vector(None),
                right: PgType::Vector(None),
                returns: PgType::Float8,
                description: "L2 distance",
            },
            ExtensionOperator {
                name: "<->",
                left: PgType::HalfVec(None),
                right: PgType::HalfVec(None),
                returns: PgType::Float8,
                description: "halfvec L2 distance",
            },
            ExtensionOperator {
                name: "<#>",
                left: PgType::Vector(None),
                right: PgType::Vector(None),
                returns: PgType::Float8,
                description: "negative inner product",
            },
            ExtensionOperator {
                name: "<#>",
                left: PgType::HalfVec(None),
                right: PgType::HalfVec(None),
                returns: PgType::Float8,
                description: "halfvec negative inner product",
            },
            ExtensionOperator {
                name: "<+>",
                left: PgType::Vector(None),
                right: PgType::Vector(None),
                returns: PgType::Float8,
                description: "L1 distance",
            },
            ExtensionOperator {
                name: "<+>",
                left: PgType::HalfVec(None),
                right: PgType::HalfVec(None),
                returns: PgType::Float8,
                description: "halfvec L1 distance",
            },
            // Vector arithmetic returning PgType::Vector / HalfVec
            ExtensionOperator {
                name: "+",
                left: PgType::Vector(None),
                right: PgType::Vector(None),
                returns: PgType::Vector(None),
                description: "vector addition",
            },
            ExtensionOperator {
                name: "+",
                left: PgType::HalfVec(None),
                right: PgType::HalfVec(None),
                returns: PgType::HalfVec(None),
                description: "halfvec addition",
            },
            ExtensionOperator {
                name: "-",
                left: PgType::Vector(None),
                right: PgType::Vector(None),
                returns: PgType::Vector(None),
                description: "vector subtraction",
            },
            ExtensionOperator {
                name: "-",
                left: PgType::HalfVec(None),
                right: PgType::HalfVec(None),
                returns: PgType::HalfVec(None),
                description: "halfvec subtraction",
            },
            ExtensionOperator {
                name: "*",
                left: PgType::Vector(None),
                right: PgType::Vector(None),
                returns: PgType::Vector(None),
                description: "element-wise multiplication",
            },
            ExtensionOperator {
                name: "*",
                left: PgType::HalfVec(None),
                right: PgType::HalfVec(None),
                returns: PgType::HalfVec(None),
                description: "halfvec multiplication",
            },
        ]
    }

    fn functions(&self) -> Vec<ExtensionFunction> {
        vec![
            ExtensionFunction {
                name: "cosine_distance",
                params: vec![PgType::Vector(None), PgType::Vector(None)],
                variadic: false,
                returns: PgType::Float8,
            },
            ExtensionFunction {
                name: "l2_distance",
                params: vec![PgType::Vector(None), PgType::Vector(None)],
                variadic: false,
                returns: PgType::Float8,
            },
            ExtensionFunction {
                name: "inner_product",
                params: vec![PgType::Vector(None), PgType::Vector(None)],
                variadic: false,
                returns: PgType::Float8,
            },
            ExtensionFunction {
                name: "l1_distance",
                params: vec![PgType::Vector(None), PgType::Vector(None)],
                variadic: false,
                returns: PgType::Float8,
            },
            ExtensionFunction {
                name: "vector_dims",
                params: vec![PgType::Vector(None)],
                variadic: false,
                returns: PgType::Int4,
            },
            ExtensionFunction {
                name: "vector_norm",
                params: vec![PgType::Vector(None)],
                variadic: false,
                returns: PgType::Float8,
            },
        ]
    }
}
