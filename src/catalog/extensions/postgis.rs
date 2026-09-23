use super::{ExtensionDefinition, ExtensionFunction, ExtensionOperator, ExtensionType};
use crate::analyzer::PgType;

pub struct PostGisExtension;

impl ExtensionDefinition for PostGisExtension {
    fn name(&self) -> &'static str {
        "postgis"
    }

    fn types(&self) -> Vec<ExtensionType> {
        vec![
            ExtensionType {
                name: "geometry",
                pg_type: PgType::Geometry,
                aliases: &[],
            },
            ExtensionType {
                name: "geography",
                pg_type: PgType::Geography,
                aliases: &[],
            },
            ExtensionType {
                name: "box2d",
                pg_type: PgType::Box2D,
                aliases: &[],
            },
            ExtensionType {
                name: "box3d",
                pg_type: PgType::Box3D,
                aliases: &[],
            },
        ]
    }

    fn operators(&self) -> Vec<ExtensionOperator> {
        vec![
            // 2D bounding box intersection
            ExtensionOperator {
                name: "&&",
                left: PgType::Geometry,
                right: PgType::Geometry,
                returns: PgType::Bool,
                description: "2D bounding box intersection",
            },
            ExtensionOperator {
                name: "&&",
                left: PgType::Geography,
                right: PgType::Geography,
                returns: PgType::Bool,
                description: "2D bounding box intersection",
            },
            // Centroid distance
            ExtensionOperator {
                name: "<->",
                left: PgType::Geometry,
                right: PgType::Geometry,
                returns: PgType::Float8,
                description: "centroid distance",
            },
            ExtensionOperator {
                name: "<->",
                left: PgType::Geography,
                right: PgType::Geography,
                returns: PgType::Float8,
                description: "centroid distance",
            },
        ]
    }

    fn functions(&self) -> Vec<ExtensionFunction> {
        vec![
            ExtensionFunction {
                name: "st_dwithin",
                params: vec![PgType::Geometry, PgType::Geometry, PgType::Float8],
                variadic: false,
                returns: PgType::Bool,
            },
            ExtensionFunction {
                name: "st_dwithin",
                params: vec![PgType::Geography, PgType::Geography, PgType::Float8],
                variadic: false,
                returns: PgType::Bool,
            },
            ExtensionFunction {
                name: "st_distance",
                params: vec![PgType::Geometry, PgType::Geometry],
                variadic: false,
                returns: PgType::Float8,
            },
            ExtensionFunction {
                name: "st_distance",
                params: vec![PgType::Geography, PgType::Geography],
                variadic: false,
                returns: PgType::Float8,
            },
            ExtensionFunction {
                name: "st_asgeojson",
                params: vec![PgType::Geometry],
                variadic: false,
                returns: PgType::Text,
            },
            ExtensionFunction {
                name: "st_asgeojson",
                params: vec![PgType::Geography],
                variadic: false,
                returns: PgType::Text,
            },
            ExtensionFunction {
                name: "st_makepoint",
                params: vec![PgType::Float8, PgType::Float8],
                variadic: false,
                returns: PgType::Geometry,
            },
            ExtensionFunction {
                name: "st_setsrid",
                params: vec![PgType::Geometry, PgType::Int4],
                variadic: false,
                returns: PgType::Geometry,
            },
            ExtensionFunction {
                name: "st_point",
                params: vec![PgType::Float8, PgType::Float8],
                variadic: false,
                returns: PgType::Geometry,
            },
            ExtensionFunction {
                name: "st_contains",
                params: vec![PgType::Geometry, PgType::Geometry],
                variadic: false,
                returns: PgType::Bool,
            },
            ExtensionFunction {
                name: "st_intersects",
                params: vec![PgType::Geometry, PgType::Geometry],
                variadic: false,
                returns: PgType::Bool,
            },
        ]
    }
}
