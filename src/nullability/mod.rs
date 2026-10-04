//! Join Tree Nullability Inference Engine
//!
//! Provides top-down nullability propagation across PostgreSQL join trees (LEFT, RIGHT, FULL,
//! INNER, and nested joins), DDL nullability preservation, scope isolation, and robust
//! TypeScript type projection for nullable types (including composite and object types).

use crate::catalog::{Catalog, ColumnMetadata};
use pg_query::NodeEnum;
use pg_query::protobuf::{JoinType, Node};
use std::collections::{HashMap, HashSet};

/// Kinds of SQL joins supported during query analysis and nullability propagation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    pub fn is_outer(&self) -> bool {
        matches!(self, JoinKind::Left | JoinKind::Right | JoinKind::Full)
    }

    pub fn nullifies_left(&self) -> bool {
        matches!(self, JoinKind::Right | JoinKind::Full)
    }

    pub fn nullifies_right(&self) -> bool {
        matches!(self, JoinKind::Left | JoinKind::Full)
    }
}

/// A column binding within a query scope or table binding.
/// Preserves original DDL nullability without destructive in-place mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnBinding {
    pub name: String,
    pub pg_type: String,
    pub ts_type: String,
    pub ddl_nullable: bool,
    /// Mirrors `ddl_nullable` for backwards compatibility.
    pub is_nullable: bool,
    pub has_default: bool,
    pub is_primary_key: bool,
}

impl ColumnBinding {
    pub fn new(
        name: String,
        pg_type: String,
        ts_type: String,
        ddl_nullable: bool,
        has_default: bool,
        is_primary_key: bool,
    ) -> Self {
        Self {
            name,
            pg_type,
            ts_type,
            ddl_nullable,
            is_nullable: ddl_nullable,
            has_default,
            is_primary_key,
        }
    }

    pub fn from_column_metadata(col: &ColumnMetadata) -> Self {
        Self {
            name: col.name.clone(),
            pg_type: col.pg_type.clone(),
            ts_type: col.ts_type.clone(),
            ddl_nullable: col.is_nullable,
            is_nullable: col.is_nullable,
            has_default: col.has_default,
            is_primary_key: col.is_primary_key,
        }
    }

    /// Computes the effective nullability given whether the containing table binding
    /// is null-producing in the current join context: `ddl_nullable || table_is_null_producing`.
    pub fn effective_nullable(&self, table_is_null_producing: bool) -> bool {
        self.ddl_nullable || table_is_null_producing
    }

    pub fn to_column_metadata(&self, table_is_null_producing: bool) -> ColumnMetadata {
        ColumnMetadata {
            name: self.name.clone(),
            pg_type: self.pg_type.clone(),
            ts_type: self.ts_type.clone(),
            is_nullable: self.effective_nullable(table_is_null_producing),
            has_default: self.has_default,
            is_primary_key: self.is_primary_key,
        }
    }
}

impl From<ColumnMetadata> for ColumnBinding {
    fn from(col: ColumnMetadata) -> Self {
        ColumnBinding::from_column_metadata(&col)
    }
}

impl From<&ColumnMetadata> for ColumnBinding {
    fn from(col: &ColumnMetadata) -> Self {
        ColumnBinding::from_column_metadata(col)
    }
}

impl From<ColumnBinding> for ColumnMetadata {
    fn from(col: ColumnBinding) -> Self {
        col.to_column_metadata(false)
    }
}

impl From<&ColumnBinding> for ColumnMetadata {
    fn from(col: &ColumnBinding) -> Self {
        col.to_column_metadata(false)
    }
}

/// Represents a table binding in a query's active scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableBinding {
    pub base_table: String,
    pub exposed_name: String,
    pub has_explicit_alias: bool,
    /// Indicates if this table binding is null-producing due to being on the nullable side of an outer join.
    pub is_null_producing: bool,
    /// Kept in sync with `is_null_producing` for compatibility.
    pub is_null_padded: bool,
    pub columns: HashMap<String, ColumnBinding>,
    pub column_order: Vec<String>,
    pub ambiguous_columns: HashSet<String>,
}

impl TableBinding {
    pub fn new(
        base_table: String,
        exposed_name: String,
        has_explicit_alias: bool,
        is_null_producing: bool,
        columns: HashMap<String, ColumnBinding>,
    ) -> Self {
        Self {
            base_table,
            exposed_name,
            has_explicit_alias,
            is_null_producing,
            is_null_padded: is_null_producing,
            columns,
            column_order: Vec::new(),
            ambiguous_columns: HashSet::new(),
        }
    }

    pub fn table_name(&self) -> &str {
        &self.base_table
    }

    pub fn alias(&self) -> Option<&str> {
        if self.has_explicit_alias {
            Some(&self.exposed_name)
        } else {
            None
        }
    }

    pub fn is_null_producing(&self) -> bool {
        self.is_null_producing || self.is_null_padded
    }

    pub fn mark_null_producing(&mut self) {
        self.is_null_producing = true;
        self.is_null_padded = true;
    }

    pub fn get_column(&self, name: &str) -> Option<&ColumnBinding> {
        self.columns.get(name).or_else(|| {
            self.columns
                .values()
                .find(|c| c.name.eq_ignore_ascii_case(name))
        })
    }
}

/// AST Join Tree representation used for top-down nullability propagation.
#[derive(Debug, Clone, PartialEq)]
pub enum JoinTreeNode {
    Table {
        rel_name: String,
        schema: Option<String>,
        alias: Option<String>,
        colnames: Vec<String>,
        is_null_producing: bool,
    },
    Join {
        kind: JoinKind,
        left: Box<JoinTreeNode>,
        right: Box<JoinTreeNode>,
        alias: Option<String>,
        is_null_producing: bool,
    },
    Subquery {
        alias: Option<String>,
        is_null_producing: bool,
    },
    Function {
        alias: Option<String>,
        is_null_producing: bool,
    },
    Empty,
}

impl JoinTreeNode {
    /// Builds a `JoinTreeNode` recursively from a `pg_query` Node.
    pub fn from_node(node: &Node) -> Self {
        match &node.node {
            Some(NodeEnum::RangeVar(rv)) => {
                let schema = if rv.schemaname.is_empty() {
                    None
                } else {
                    Some(rv.schemaname.clone())
                };
                let (alias, colnames) = if let Some(a) = &rv.alias {
                    let cnames = a
                        .colnames
                        .iter()
                        .filter_map(|n| {
                            if let Some(NodeEnum::String(s)) = &n.node {
                                Some(s.sval.clone())
                            } else {
                                None
                            }
                        })
                        .collect();
                    (Some(a.aliasname.clone()), cnames)
                } else {
                    (None, Vec::new())
                };
                JoinTreeNode::Table {
                    rel_name: rv.relname.clone(),
                    schema,
                    alias,
                    colnames,
                    is_null_producing: false,
                }
            }
            Some(NodeEnum::JoinExpr(je)) => {
                let kind = JoinKind::from_i32(je.jointype);
                let left = je
                    .larg
                    .as_ref()
                    .map(|n| Box::new(Self::from_node(n)))
                    .unwrap_or_else(|| Box::new(JoinTreeNode::Empty));
                let right = je
                    .rarg
                    .as_ref()
                    .map(|n| Box::new(Self::from_node(n)))
                    .unwrap_or_else(|| Box::new(JoinTreeNode::Empty));
                let alias = je.alias.as_ref().map(|a| a.aliasname.clone());
                JoinTreeNode::Join {
                    kind,
                    left,
                    right,
                    alias,
                    is_null_producing: false,
                }
            }
            Some(NodeEnum::RangeSubselect(rss)) => {
                let alias = rss.alias.as_ref().map(|a| a.aliasname.clone());
                JoinTreeNode::Subquery {
                    alias,
                    is_null_producing: false,
                }
            }
            Some(NodeEnum::RangeFunction(rf)) => {
                let alias = rf.alias.as_ref().map(|a| a.aliasname.clone());
                JoinTreeNode::Function {
                    alias,
                    is_null_producing: false,
                }
            }
            _ => JoinTreeNode::Empty,
        }
    }

    /// Propagates nullability top-down across join branches.
    ///
    /// - If `parent_null_producing` is true, the current node and all descendants become null-producing.
    /// - For `JOIN_LEFT`: the right subtree becomes null-producing; the left inherits `parent_null_producing`.
    /// - For `JOIN_RIGHT`: the left subtree becomes null-producing; the right inherits `parent_null_producing`.
    /// - For `JOIN_FULL`: both left and right subtrees become null-producing.
    /// - For `JOIN_INNER` / other joins: both left and right inherit `parent_null_producing`.
    pub fn propagate_nullability(&mut self, parent_null_producing: bool) {
        match self {
            JoinTreeNode::Table {
                is_null_producing, ..
            } => {
                *is_null_producing = parent_null_producing;
            }
            JoinTreeNode::Join {
                kind,
                left,
                right,
                is_null_producing,
                ..
            } => {
                *is_null_producing = parent_null_producing;
                let left_null = parent_null_producing || kind.nullifies_left();
                let right_null = parent_null_producing || kind.nullifies_right();
                left.propagate_nullability(left_null);
                right.propagate_nullability(right_null);
            }
            JoinTreeNode::Subquery {
                is_null_producing, ..
            } => {
                *is_null_producing = parent_null_producing;
            }
            JoinTreeNode::Function {
                is_null_producing, ..
            } => {
                *is_null_producing = parent_null_producing;
            }
            JoinTreeNode::Empty => {}
        }
    }

    pub fn is_null_producing(&self) -> bool {
        match self {
            JoinTreeNode::Table {
                is_null_producing, ..
            } => *is_null_producing,
            JoinTreeNode::Join {
                is_null_producing, ..
            } => *is_null_producing,
            JoinTreeNode::Subquery {
                is_null_producing, ..
            } => *is_null_producing,
            JoinTreeNode::Function {
                is_null_producing, ..
            } => *is_null_producing,
            JoinTreeNode::Empty => false,
        }
    }

    pub fn collect_table_nullabilities(&self) -> Vec<(String, Option<String>, bool)> {
        let mut result = Vec::new();
        self.collect_internal(&mut result);
        result
    }

    fn collect_internal(&self, acc: &mut Vec<(String, Option<String>, bool)>) {
        match self {
            JoinTreeNode::Table {
                rel_name,
                alias,
                is_null_producing,
                ..
            } => {
                acc.push((rel_name.clone(), alias.clone(), *is_null_producing));
            }
            JoinTreeNode::Join { left, right, .. } => {
                left.collect_internal(acc);
                right.collect_internal(acc);
            }
            _ => {}
        }
    }
}

/// Checks if a TypeScript type string contains `null` at the root union level (depth 0).
/// Correctly distinguishes inner nullables (e.g. inside composite `{ a: string | null }`
/// or generic `Array<string | null>`) from root-level nullability (`T | null`).
pub fn has_root_null(ts_type: &str) -> bool {
    let mut depth: i32 = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut current_part = String::new();
    let mut parts = Vec::new();

    for ch in ts_type.chars() {
        match ch {
            '\'' if !in_double_quote => in_single_quote = !in_single_quote,
            '"' if !in_single_quote => in_double_quote = !in_double_quote,
            '{' | '(' | '<' | '[' if !in_single_quote && !in_double_quote => {
                depth += 1;
                current_part.push(ch);
            }
            '}' | ')' | '>' | ']' if !in_single_quote && !in_double_quote => {
                depth -= 1;
                current_part.push(ch);
            }
            '|' if depth == 0 && !in_single_quote && !in_double_quote => {
                parts.push(std::mem::take(&mut current_part));
            }
            _ => {
                current_part.push(ch);
            }
        }
    }
    parts.push(current_part);

    parts.iter().any(|p| p.trim() == "null")
}

/// Formats a TypeScript type string as nullable (`T | null`).
/// If the root type already contains `| null` at depth 0 or is `"unknown"`, it returns as is.
/// Handles nested composite/object types with inner nullable properties cleanly.
pub fn format_nullable(ts_type: &str) -> String {
    let trimmed = ts_type.trim();
    if trimmed == "unknown" {
        return "unknown".to_string();
    }
    if has_root_null(trimmed) {
        trimmed.to_string()
    } else {
        format!("{} | null", trimmed)
    }
}

/// Strips root-level `| null` without stripping inner nullables within composite/object types.
pub fn strip_root_null(ts_type: &str) -> String {
    let mut depth: i32 = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut current_part = String::new();
    let mut parts = Vec::new();

    for ch in ts_type.chars() {
        match ch {
            '\'' if !in_double_quote => in_single_quote = !in_single_quote,
            '"' if !in_single_quote => in_double_quote = !in_double_quote,
            '{' | '(' | '<' | '[' if !in_single_quote && !in_double_quote => {
                depth += 1;
                current_part.push(ch);
            }
            '}' | ')' | '>' | ']' if !in_single_quote && !in_double_quote => {
                depth -= 1;
                current_part.push(ch);
            }
            '|' if depth == 0 && !in_single_quote && !in_double_quote => {
                parts.push(std::mem::take(&mut current_part));
            }
            _ => {
                current_part.push(ch);
            }
        }
    }
    parts.push(current_part);

    let non_null: Vec<String> = parts
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty() && p != "null")
        .collect();

    if non_null.is_empty() {
        "null".to_string()
    } else {
        non_null.join(" | ")
    }
}

/// Projects a TypeScript type according to an effective nullability flag.
pub fn project_ts_type(ts_type: &str, is_nullable: bool) -> String {
    if is_nullable {
        format_nullable(ts_type)
    } else {
        strip_root_null(ts_type)
    }
}

/// Propagates join tree nullability across the `from_clause` of a SELECT statement,
/// producing a `QueryScope` with accurate table and column bindings without mutating the catalog.
pub fn propagate_join_nullability(
    from_clause: &[Node],
    catalog: &Catalog,
) -> crate::analyzer::QueryScope {
    propagate_join_nullability_result(from_clause, catalog, &crate::analyzer::QueryScope::default())
        .unwrap_or_default()
}

/// Propagates join tree nullability within an existing `active_scope` context.
pub fn propagate_join_nullability_result(
    from_clause: &[Node],
    catalog: &Catalog,
    active_scope: &crate::analyzer::QueryScope,
) -> Result<crate::analyzer::QueryScope, String> {
    let mut scope = crate::analyzer::QueryScope::with_ctes_from(active_scope);
    for node in from_clause {
        let mut tree = JoinTreeNode::from_node(node);
        tree.propagate_nullability(false);
        build_scope_from_tree(&tree, catalog, &mut scope)?;
    }
    Ok(scope)
}

fn build_scope_from_tree(
    tree: &JoinTreeNode,
    catalog: &Catalog,
    scope: &mut crate::analyzer::QueryScope,
) -> Result<(), String> {
    match tree {
        JoinTreeNode::Table {
            rel_name,
            schema,
            alias,
            colnames,
            is_null_producing,
        } => {
            let lower_name = rel_name.to_ascii_lowercase();

            // 1. CTE shadowing
            if schema.is_none() && let Some(cte_binding) = scope.ctes.get(&lower_name) {
                let exposed_name = alias.clone().unwrap_or_else(|| rel_name.clone());
                let has_explicit_alias = alias.is_some();

                let orig_order: Vec<String> = if let Some(order) =
                    scope.cte_column_orders.get(&lower_name)
                {
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
                        let col_name = colnames.get(i).cloned().unwrap_or_else(|| orig_col_name.clone());
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
                    base_table: rel_name.clone(),
                    exposed_name: exposed_name.clone(),
                    has_explicit_alias,
                    is_null_producing: *is_null_producing,
                    is_null_padded: *is_null_producing,
                    columns,
                    column_order: final_order.clone(),
                    ambiguous_columns: HashSet::new(),
                };

                scope
                    .cte_column_orders
                    .insert(exposed_name.to_ascii_lowercase(), final_order);
                scope.add_binding(binding)?;
                return Ok(());
            }

            // 2. Catalog table resolution
            let table_meta = catalog
                .get_table_qualified(schema.as_deref(), rel_name)
                .or_else(|| catalog.get_table(rel_name))
                .ok_or_else(|| {
                    format!("Table \"{}\" does not exist in schema catalog", rel_name)
                })?;

            let exposed_name = alias.clone().unwrap_or_else(|| rel_name.clone());
            let has_explicit_alias = alias.is_some();

            let mut columns = HashMap::new();
            let mut ordered_names = Vec::new();

            if !colnames.is_empty() {
                for (i, col) in table_meta.columns.iter().enumerate() {
                    let col_name = colnames.get(i).cloned().unwrap_or_else(|| col.name.clone());
                    let mut aliased_col = ColumnBinding::from_column_metadata(col);
                    aliased_col.name = col_name.clone();
                    columns.insert(col_name.to_ascii_lowercase(), aliased_col);
                    ordered_names.push(col_name);
                }
            } else {
                for col in &table_meta.columns {
                    columns.insert(
                        col.name.to_ascii_lowercase(),
                        ColumnBinding::from_column_metadata(col),
                    );
                    ordered_names.push(col.name.clone());
                }
            }

            let binding = TableBinding {
                base_table: rel_name.clone(),
                exposed_name: exposed_name.clone(),
                has_explicit_alias,
                is_null_producing: *is_null_producing,
                is_null_padded: *is_null_producing,
                columns,
                column_order: ordered_names.clone(),
                ambiguous_columns: HashSet::new(),
            };

            scope
                .cte_column_orders
                .insert(exposed_name.to_ascii_lowercase(), ordered_names);
            scope.add_binding(binding)?;
            Ok(())
        }
        JoinTreeNode::Join {
            left,
            right,
            alias,
            is_null_producing,
            ..
        } => {
            build_scope_from_tree(left, catalog, scope)?;
            build_scope_from_tree(right, catalog, scope)?;

            if let Some(alias_name) = alias {
                let mut unified_columns = HashMap::new();
                let mut ambiguous_columns = HashSet::new();
                let mut ordered_names = Vec::new();

                for name in &scope.binding_order {
                    if let Some(b) = scope.bindings.get(&name.to_ascii_lowercase()) {
                        for col_name in &b.column_order {
                            if let Some(col) = b.get_column(col_name) {
                                let key = col.name.to_ascii_lowercase();
                                if unified_columns.contains_key(&key) {
                                    ambiguous_columns.insert(key.clone());
                                } else {
                                    let eff_null =
                                        col.effective_nullable(b.is_null_producing);
                                    let mut u_col = col.clone();
                                    u_col.ddl_nullable = eff_null;
                                    u_col.is_nullable = eff_null;
                                    unified_columns.insert(key.clone(), u_col);
                                }
                                ordered_names.push(col.name.clone());
                            }
                        }
                    }
                }

                let unified_binding = TableBinding {
                    base_table: alias_name.clone(),
                    exposed_name: alias_name.clone(),
                    has_explicit_alias: true,
                    is_null_producing: *is_null_producing,
                    is_null_padded: *is_null_producing,
                    columns: unified_columns,
                    column_order: ordered_names.clone(),
                    ambiguous_columns,
                };

                let mut unified_scope = crate::analyzer::QueryScope::with_ctes_from(scope);
                unified_scope
                    .cte_column_orders
                    .insert(alias_name.to_ascii_lowercase(), ordered_names);
                unified_scope.add_binding(unified_binding)?;
                *scope = unified_scope;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_nullable_primitives() {
        assert_eq!(format_nullable("string"), "string | null");
        assert_eq!(format_nullable("number"), "number | null");
        assert_eq!(format_nullable("string | null"), "string | null");
        assert_eq!(format_nullable("unknown"), "unknown");
    }

    #[test]
    fn test_format_nullable_composite_and_objects() {
        // Object without null -> append | null
        assert_eq!(
            format_nullable("{ street: string; zip: string }"),
            "{ street: string; zip: string } | null"
        );

        // Object with INNER nullable property -> must still append | null to outer object!
        assert_eq!(
            format_nullable("{ street: string | null; zip: string }"),
            "{ street: string | null; zip: string } | null"
        );

        // Object that already has outer | null -> keep as is without double | null
        assert_eq!(
            format_nullable("{ street: string | null; zip: string } | null"),
            "{ street: string | null; zip: string } | null"
        );

        // Generic Array with inner nullable element
        assert_eq!(
            format_nullable("Array<{ item: string | null }>"),
            "Array<{ item: string | null }> | null"
        );
    }

    #[test]
    fn test_strip_root_null() {
        assert_eq!(strip_root_null("string | null"), "string");
        assert_eq!(strip_root_null("string"), "string");
        assert_eq!(
            strip_root_null("{ street: string | null; zip: string } | null"),
            "{ street: string | null; zip: string }"
        );
        assert_eq!(
            strip_root_null("{ street: string | null; zip: string }"),
            "{ street: string | null; zip: string }"
        );
    }

    #[test]
    fn test_join_tree_top_down_nullability_propagation() {
        // (A LEFT JOIN B) FULL JOIN (C RIGHT JOIN D)
        // Root: FULL JOIN
        // Left: A LEFT JOIN B
        // Right: C RIGHT JOIN D
        let left_sub = JoinTreeNode::Join {
            kind: JoinKind::Left,
            left: Box::new(JoinTreeNode::Table {
                rel_name: "a".to_string(),
                schema: None,
                alias: None,
                colnames: Vec::new(),
                is_null_producing: false,
            }),
            right: Box::new(JoinTreeNode::Table {
                rel_name: "b".to_string(),
                schema: None,
                alias: None,
                colnames: Vec::new(),
                is_null_producing: false,
            }),
            alias: None,
            is_null_producing: false,
        };

        let right_sub = JoinTreeNode::Join {
            kind: JoinKind::Right,
            left: Box::new(JoinTreeNode::Table {
                rel_name: "c".to_string(),
                schema: None,
                alias: None,
                colnames: Vec::new(),
                is_null_producing: false,
            }),
            right: Box::new(JoinTreeNode::Table {
                rel_name: "d".to_string(),
                schema: None,
                alias: None,
                colnames: Vec::new(),
                is_null_producing: false,
            }),
            alias: None,
            is_null_producing: false,
        };

        let mut root = JoinTreeNode::Join {
            kind: JoinKind::Full,
            left: Box::new(left_sub),
            right: Box::new(right_sub),
            alias: None,
            is_null_producing: false,
        };

        root.propagate_nullability(false);

        let table_nulls = root.collect_table_nullabilities();
        assert_eq!(table_nulls.len(), 4);
        for (tbl, _alias, is_null) in table_nulls {
            assert!(
                is_null,
                "Table {} in FULL JOIN subtree must be null-producing",
                tbl
            );
        }
    }

    #[test]
    fn test_self_join_nullability() {
        // employees e LEFT JOIN employees m
        let mut root = JoinTreeNode::Join {
            kind: JoinKind::Left,
            left: Box::new(JoinTreeNode::Table {
                rel_name: "employees".to_string(),
                schema: None,
                alias: Some("e".to_string()),
                colnames: Vec::new(),
                is_null_producing: false,
            }),
            right: Box::new(JoinTreeNode::Table {
                rel_name: "employees".to_string(),
                schema: None,
                alias: Some("m".to_string()),
                colnames: Vec::new(),
                is_null_producing: false,
            }),
            alias: None,
            is_null_producing: false,
        };

        root.propagate_nullability(false);

        let table_nulls = root.collect_table_nullabilities();
        let e = table_nulls.iter().find(|t| t.1.as_deref() == Some("e")).unwrap();
        let m = table_nulls.iter().find(|t| t.1.as_deref() == Some("m")).unwrap();
        assert!(!e.2, "Left side employee 'e' must not be null-producing");
        assert!(m.2, "Right side manager 'm' must be null-producing");
    }
}
