//! Prepared Statement Parameter Type Deduction Engine
//!
//! Provides AST-driven deduction of PostgreSQL prepared statement parameters ($1, $2, ..., $n),
//! resolving types against table columns, type casts, literals, pattern matching operators,
//! array operations, dynamic optional filter idioms, and collision-free parameter naming.

pub mod operators;
pub mod patterns;

use crate::analyzer::{PgType, QueryScope, extract_string, infer_expr};
use crate::catalog::{Catalog, ColumnMetadata};
use crate::nullability::{ColumnBinding, TableBinding};
use pg_query::NodeEnum;
use pg_query::protobuf::{Node, SelectStmt};
use std::collections::{HashMap, HashSet};

/// Represents a prepared statement parameter deduced from query expressions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryParam {
    /// 1-based parameter index ($1 -> 1, $2 -> 2, etc.)
    pub index: usize,
    /// Inferred or suggested parameter name (e.g. column name or "param1")
    pub name: String,
    /// Inferred TypeScript type (e.g. "string", "number", "Date", "Array<string>")
    pub ts_type: String,
    /// Whether the parameter is optional in runtime execution (e.g. optional filters or nullable inserts)
    pub is_optional: bool,
}

/// Internal bookkeeping for parameter type deduction prior to final disambiguation.
#[derive(Debug, Clone, Default)]
pub struct ParamInfo {
    pub suggested_name: Option<String>,
    pub inferred_type: Option<String>,
    pub is_optional: bool,
}

/// Primary public entrypoint: deduces prepared statement parameters directly from an AST statement node.
pub fn deduce_query_params(
    stmt: &Node,
    catalog: &Catalog,
    scope: &QueryScope,
) -> Result<Vec<QueryParam>, String> {
    let mut param_map: HashMap<i32, ParamInfo> = HashMap::new();

    let actual_stmt = match &stmt.node {
        Some(NodeEnum::RawStmt(raw)) => raw.stmt.as_deref().unwrap_or(stmt),
        _ => stmt,
    };

    match &actual_stmt.node {
        Some(NodeEnum::SelectStmt(select)) => {
            walk_select_stmt(select, catalog, scope, &mut param_map)?;
        }
        Some(NodeEnum::InsertStmt(insert)) => {
            walk_insert_stmt(insert, catalog, scope, &mut param_map)?;
        }
        Some(NodeEnum::UpdateStmt(update)) => {
            walk_update_stmt(update, catalog, scope, &mut param_map)?;
        }
        Some(NodeEnum::DeleteStmt(delete)) => {
            walk_delete_stmt(delete, catalog, scope, &mut param_map)?;
        }
        _ => {
            return Err("Unsupported statement for parameter deduction".to_string());
        }
    }

    Ok(finalize_params(&param_map))
}

/// Lossy helper returning an empty vector on parse/deduction failure.
pub fn deduce_query_params_lossy(
    stmt: &Node,
    catalog: &Catalog,
    scope: &QueryScope,
) -> Vec<QueryParam> {
    deduce_query_params(stmt, catalog, scope).unwrap_or_default()
}

/// Resolves the TypeScript type from a type cast string (e.g. "uuid", "timestamptz[]").
pub fn resolve_type_from_cast(cast_str: &str, catalog: &Catalog) -> String {
    let lower = cast_str.to_ascii_lowercase();
    let trimmed = lower.trim();
    if let Some(inner) = trimmed.strip_suffix("[]") {
        let inner_ts = catalog.resolve_type(inner);
        format!("Array<{}>", inner_ts)
    } else {
        catalog.resolve_type(trimmed)
    }
}

/// Finalizes raw `param_map` into deterministically sorted, globally collision-free `QueryParam` list.
pub fn finalize_params(param_map: &HashMap<i32, ParamInfo>) -> Vec<QueryParam> {
    let mut param_indices: Vec<i32> = param_map.keys().copied().collect();
    param_indices.sort();

    let mut params = Vec::with_capacity(param_indices.len());
    let mut used_base_counts: HashMap<String, usize> = HashMap::new();
    let mut allocated_names: HashSet<String> = HashSet::new();

    for idx in param_indices {
        let info = param_map.get(&idx).unwrap();
        let base_name = info
            .suggested_name
            .clone()
            .unwrap_or_else(|| format!("param{}", idx));

        let mut final_name = base_name.clone();
        let mut count = used_base_counts.get(&base_name).copied().unwrap_or(0);

        if allocated_names.contains(&final_name) || count > 0 {
            if count == 0 {
                count = 1;
            }
            loop {
                count += 1;
                final_name = format!("{}{}", base_name, count);
                if !allocated_names.contains(&final_name) {
                    break;
                }
            }
        }

        used_base_counts.insert(base_name, count.max(1));
        allocated_names.insert(final_name.clone());

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

    params
}

/// Helper to inspect an expression for `ParamRef` and outer `TypeCast` combinations.
pub fn extract_param_info(node: &Node) -> Option<(i32, Option<String>)> {
    let mut current = node;
    let mut outer_cast: Option<String> = None;

    loop {
        match &current.node {
            Some(NodeEnum::ParamRef(p)) => {
                return Some((p.number, outer_cast));
            }
            Some(NodeEnum::TypeCast(tc)) => {
                if outer_cast.is_none() {
                    outer_cast = tc.type_name.as_ref().map(crate::catalog::extract_type_name);
                }
                if let Some(arg) = &tc.arg {
                    current = arg;
                } else {
                    return None;
                }
            }
            _ => return None,
        }
    }
}

/// Helper to extract table alias and column name from a `ColumnRef` or wrapped expression.
pub fn extract_column_info(node: &Node) -> Option<(Option<String>, String)> {
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

/// Collects all parameter indices referenced in an expression tree.
pub fn collect_param_refs(node: &Node, out: &mut Vec<i32>) {
    match &node.node {
        Some(NodeEnum::ParamRef(p)) => {
            out.push(p.number);
        }
        Some(NodeEnum::TypeCast(tc)) => {
            if let Some(arg) = &tc.arg {
                collect_param_refs(arg, out);
            }
        }
        Some(NodeEnum::AExpr(ae)) => {
            if let Some(l) = &ae.lexpr {
                collect_param_refs(l, out);
            }
            if let Some(r) = &ae.rexpr {
                collect_param_refs(r, out);
            }
        }
        Some(NodeEnum::FuncCall(fc)) => {
            for arg in &fc.args {
                collect_param_refs(arg, out);
            }
        }
        Some(NodeEnum::List(list)) => {
            for item in &list.items {
                collect_param_refs(item, out);
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
        Some(NodeEnum::ScalarArrayOpExpr(saoe)) => {
            for arg in &saoe.args {
                collect_param_refs(arg, out);
            }
        }
        Some(NodeEnum::ResTarget(rt)) => {
            if let Some(val) = &rt.val {
                collect_param_refs(val, out);
            }
        }
        Some(NodeEnum::AArrayExpr(aae)) => {
            for elem in &aae.elements {
                collect_param_refs(elem, out);
            }
        }
        Some(NodeEnum::SubLink(sl)) => {
            if let Some(te) = &sl.testexpr {
                collect_param_refs(te, out);
            }
        }
        _ => {}
    }
}

/// Binds untyped parameters found in a node against a known target column metadata.
pub fn bind_untyped_params_in_node(
    node: &Node,
    target_col: &ColumnMetadata,
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

/// Records parameter metadata into the working map with column metadata and alias verification.
#[allow(clippy::too_many_arguments)]
pub fn record_param(
    param_num: i32,
    cast_opt: Option<String>,
    alias_opt: Option<String>,
    col_name: &str,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
    is_optional: bool,
) -> Result<(), String> {
    let mut resolved_type: Option<String> = cast_opt.map(|c| resolve_type_from_cast(&c, catalog));

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
        resolved_type = Some(catalog.resolve_type(&col.pg_type));
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

/// Recursively resolves parameters inside an arbitrary expression AST node.
pub fn resolve_params_in_expr(
    expr: &Node,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    match &expr.node {
        Some(NodeEnum::BoolExpr(be)) => {
            if let Some(m) = patterns::match_optional_filter(be, catalog, scope) {
                let entry = param_map.entry(m.param_num).or_default();
                entry.is_optional = true;
                if entry.suggested_name.is_none() && m.suggested_name.is_some() {
                    entry.suggested_name = m.suggested_name;
                }
                if (entry.inferred_type.is_none()
                    || entry.inferred_type.as_deref() == Some("unknown"))
                    && m.inferred_type.is_some()
                {
                    entry.inferred_type = m.inferred_type;
                }
                return Ok(());
            }

            for arg in &be.args {
                resolve_params_in_expr(arg, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::ScalarArrayOpExpr(saoe)) => {
            if saoe.args.len() == 2 {
                operators::resolve_scalar_array_comparison(
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
            operators::deduce_aexpr(ae, catalog, scope, param_map)?;
        }
        Some(NodeEnum::TypeCast(tc)) => {
            if let Some(arg) = &tc.arg {
                if let Some(NodeEnum::ParamRef(p)) = &arg.node {
                    let ts_type = tc
                        .type_name
                        .as_ref()
                        .map(crate::catalog::extract_type_name)
                        .map(|t| resolve_type_from_cast(&t, catalog));
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
            let mut unified_ts: Option<String> = None;
            for arg in &ce.args {
                if extract_param_info(arg).is_none()
                    && let Ok(inf) = infer_expr(arg, catalog, scope)
                    && inf.pg_type != PgType::Unknown
                {
                    unified_ts = Some(inf.pg_type.to_ts(catalog));
                    break;
                }
            }
            for arg in &ce.args {
                if let Some((param_num, cast_opt)) = extract_param_info(arg) {
                    let resolved = cast_opt
                        .map(|c| resolve_type_from_cast(&c, catalog))
                        .or_else(|| unified_ts.clone());
                    let entry = param_map.entry(param_num).or_default();
                    if (entry.inferred_type.is_none()
                        || entry.inferred_type.as_deref() == Some("unknown"))
                        && resolved.is_some()
                    {
                        entry.inferred_type = resolved;
                    }
                } else {
                    resolve_params_in_expr(arg, catalog, scope, param_map)?;
                }
            }
        }
        Some(NodeEnum::FuncCall(fc)) => {
            operators::deduce_func_call(fc, catalog, scope, param_map)?;
        }
        Some(NodeEnum::List(list)) => {
            for item in &list.items {
                resolve_params_in_expr(item, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::SubLink(sl)) => {
            if let Some(testexpr) = &sl.testexpr {
                resolve_params_in_expr(testexpr, catalog, scope, param_map)?;
            }
            if let Some(sub_node) = &sl.subselect
                && let Some(NodeEnum::SelectStmt(sub_select)) = &sub_node.node
            {
                walk_select_stmt(sub_select, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::ResTarget(rt)) => {
            if let Some(val) = &rt.val {
                if let Some((p, cast_opt)) = extract_param_info(val) {
                    let entry = param_map.entry(p).or_default();
                    if !rt.name.is_empty() && entry.suggested_name.is_none() {
                        entry.suggested_name = Some(rt.name.clone());
                    }
                    if let Some(c) = cast_opt
                        && (entry.inferred_type.is_none()
                            || entry.inferred_type.as_deref() == Some("unknown"))
                    {
                        entry.inferred_type = Some(resolve_type_from_cast(&c, catalog));
                    }
                }
                resolve_params_in_expr(val, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::AArrayExpr(aae)) => {
            let mut unified_elem = PgType::Unknown;
            for elem in &aae.elements {
                if extract_param_info(elem).is_none()
                    && let Ok(inf) = infer_expr(elem, catalog, scope)
                {
                    unified_elem = crate::analyzer::unify_types(&unified_elem, &inf.pg_type)
                        .unwrap_or(unified_elem);
                }
            }
            for elem in &aae.elements {
                if let Some((param_num, cast_opt)) = extract_param_info(elem) {
                    let resolved_type = cast_opt
                        .map(|c| resolve_type_from_cast(&c, catalog))
                        .or_else(|| {
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

fn walk_select_stmt(
    select: &SelectStmt,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    let mut query_scope = QueryScope::with_ctes_from(scope);

    // 1. Process CTEs in with_clause
    if let Some(wc) = &select.with_clause {
        crate::analyzer::process_with_clause_with_params(wc, &mut query_scope, catalog, param_map)?;
    }

    // 2. Build scope from from_clause if not already populated
    if !select.from_clause.is_empty() {
        let from_scope = crate::nullability::propagate_join_nullability_result(
            &select.from_clause,
            catalog,
            &query_scope,
        )?;
        query_scope.merge(from_scope)?;
    }

    // 3. Walk FROM clause for join quals, subqueries, and table functions
    for from_item in &select.from_clause {
        walk_from_clause_node(from_item, catalog, &query_scope, param_map)?;
    }

    // 4. Walk projections (target_list)
    for target in &select.target_list {
        if let Some(NodeEnum::ResTarget(rt)) = &target.node
            && let Some(val) = &rt.val
        {
            if let Some((p, cast_opt)) = extract_param_info(val) {
                let entry = param_map.entry(p).or_default();
                if !rt.name.is_empty() && entry.suggested_name.is_none() {
                    entry.suggested_name = Some(rt.name.clone());
                }
                if let Some(c) = cast_opt
                    && (entry.inferred_type.is_none()
                        || entry.inferred_type.as_deref() == Some("unknown"))
                {
                    entry.inferred_type = Some(resolve_type_from_cast(&c, catalog));
                }
            }
            resolve_params_in_expr(val, catalog, &query_scope, param_map)?;
        }
    }

    // 5. Walk WHERE clause
    if let Some(where_node) = &select.where_clause {
        resolve_params_in_expr(where_node, catalog, &query_scope, param_map)?;
    }

    // 6. Walk HAVING clause
    if let Some(having_node) = &select.having_clause {
        resolve_params_in_expr(having_node, catalog, &query_scope, param_map)?;
    }

    // 7. Walk limit_count
    if let Some(limit_node) = &select.limit_count {
        if let Some((num, _)) = extract_param_info(limit_node) {
            let entry = param_map.entry(num).or_default();
            if entry.suggested_name.is_none() {
                entry.suggested_name = Some("limit".to_string());
            }
            if entry.inferred_type.is_none() {
                entry.inferred_type = Some("number".to_string());
            }
        } else {
            resolve_params_in_expr(limit_node, catalog, &query_scope, param_map)?;
        }
    }

    // 8. Walk limit_offset
    if let Some(offset_node) = &select.limit_offset {
        if let Some((num, _)) = extract_param_info(offset_node) {
            let entry = param_map.entry(num).or_default();
            if entry.suggested_name.is_none() {
                entry.suggested_name = Some("offset".to_string());
            }
            if entry.inferred_type.is_none() {
                entry.inferred_type = Some("number".to_string());
            }
        } else {
            resolve_params_in_expr(offset_node, catalog, &query_scope, param_map)?;
        }
    }

    // 9. Walk values_lists
    for row_node in &select.values_lists {
        if let Some(NodeEnum::List(row_list)) = &row_node.node {
            for item in &row_list.items {
                resolve_params_in_expr(item, catalog, &query_scope, param_map)?;
            }
        }
    }

    // 10. Walk set operations (UNION / INTERSECT / EXCEPT)
    if let Some(larg) = &select.larg {
        walk_select_stmt(larg, catalog, scope, param_map)?;
    }
    if let Some(rarg) = &select.rarg {
        walk_select_stmt(rarg, catalog, scope, param_map)?;
    }

    Ok(())
}

fn walk_from_clause_node(
    node: &Node,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    match &node.node {
        Some(NodeEnum::JoinExpr(je)) => {
            if let Some(larg) = &je.larg {
                walk_from_clause_node(larg, catalog, scope, param_map)?;
            }
            if let Some(rarg) = &je.rarg {
                walk_from_clause_node(rarg, catalog, scope, param_map)?;
            }
            if let Some(quals) = &je.quals {
                resolve_params_in_expr(quals, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::RangeSubselect(rss)) => {
            if let Some(sub_node) = &rss.subquery
                && let Some(NodeEnum::SelectStmt(sub_select)) = &sub_node.node
            {
                walk_select_stmt(sub_select, catalog, scope, param_map)?;
            }
        }
        Some(NodeEnum::RangeFunction(rf)) => {
            for func_node in &rf.functions {
                if let Some(NodeEnum::List(list)) = &func_node.node {
                    for item in &list.items {
                        resolve_params_in_expr(item, catalog, scope, param_map)?;
                    }
                } else {
                    resolve_params_in_expr(func_node, catalog, scope, param_map)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn walk_insert_stmt(
    insert: &pg_query::protobuf::InsertStmt,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    let mut insert_scope = QueryScope::with_ctes_from(scope);
    if let Some(wc) = &insert.with_clause {
        crate::analyzer::process_with_clause_with_params(
            wc,
            &mut insert_scope,
            catalog,
            param_map,
        )?;
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
    let mut col_order = Vec::new();
    for col in &table_meta.columns {
        columns.insert(
            col.name.to_ascii_lowercase(),
            ColumnBinding::from_column_metadata(col),
        );
        col_order.push(col.name.clone());
    }

    let binding = TableBinding {
        base_table: table_name.clone(),
        exposed_name,
        has_explicit_alias,
        is_null_producing: false,
        is_null_padded: false,
        columns,
        column_order: col_order,
        ambiguous_columns: HashSet::new(),
    };
    insert_scope.add_binding(binding)?;

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
                                    &insert_scope,
                                    param_map,
                                    is_optional,
                                )?;
                            } else {
                                resolve_params_in_expr(
                                    val_node,
                                    catalog,
                                    &insert_scope,
                                    param_map,
                                )?;
                                if let Some(target_col) = col_meta {
                                    bind_untyped_params_in_node(val_node, target_col, param_map);
                                }
                            }
                        }
                    }
                }
            }
        } else {
            walk_select_stmt(select, catalog, &insert_scope, param_map)?;
        }
    }

    if let Some(occ) = &insert.on_conflict_clause {
        let is_update = occ.action == pg_query::protobuf::OnConflictAction::OnconflictUpdate as i32;
        let conflict_scope = if is_update {
            let mut cs = insert_scope.clone();
            let mut excluded_cols = HashMap::new();
            let mut excluded_order = Vec::new();
            for col in &table_meta.columns {
                excluded_cols.insert(
                    col.name.to_ascii_lowercase(),
                    ColumnBinding::from_column_metadata(col),
                );
                excluded_order.push(col.name.clone());
            }
            let excluded_binding = TableBinding {
                base_table: "excluded".to_string(),
                exposed_name: "excluded".to_string(),
                has_explicit_alias: false,
                is_null_producing: false,
                is_null_padded: false,
                columns: excluded_cols,
                column_order: excluded_order,
                ambiguous_columns: HashSet::new(),
            };
            cs.add_binding(excluded_binding)?;
            cs
        } else {
            insert_scope.clone()
        };

        if let Some(infer) = &occ.infer
            && let Some(where_node) = &infer.where_clause
        {
            resolve_params_in_expr(where_node, catalog, &insert_scope, param_map)?;
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

    for rt_node in &insert.returning_list {
        if let Some(NodeEnum::ResTarget(rt)) = &rt_node.node
            && let Some(val) = &rt.val
        {
            resolve_params_in_expr(val, catalog, &insert_scope, param_map)?;
        }
    }

    Ok(())
}

fn walk_update_stmt(
    update: &pg_query::protobuf::UpdateStmt,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    let mut update_scope = QueryScope::with_ctes_from(scope);
    if let Some(wc) = &update.with_clause {
        crate::analyzer::process_with_clause_with_params(
            wc,
            &mut update_scope,
            catalog,
            param_map,
        )?;
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
    let mut col_order = Vec::new();
    for col in &table_meta.columns {
        columns.insert(
            col.name.to_ascii_lowercase(),
            ColumnBinding::from_column_metadata(col),
        );
        col_order.push(col.name.clone());
    }

    let binding = TableBinding {
        base_table: table_name.clone(),
        exposed_name,
        has_explicit_alias,
        is_null_producing: false,
        is_null_padded: false,
        columns,
        column_order: col_order,
        ambiguous_columns: HashSet::new(),
    };
    update_scope.add_binding(binding)?;

    if !update.from_clause.is_empty() {
        let from_scope = crate::nullability::propagate_join_nullability_result(
            &update.from_clause,
            catalog,
            &update_scope,
        )?;
        update_scope.merge(from_scope)?;
    }

    for from_item in &update.from_clause {
        walk_from_clause_node(from_item, catalog, &update_scope, param_map)?;
    }

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
                        &update_scope,
                        param_map,
                        is_optional,
                    )?;
                } else {
                    resolve_params_in_expr(val_node, catalog, &update_scope, param_map)?;
                    bind_untyped_params_in_node(val_node, target_col, param_map);
                }
            }
        }
    }

    if let Some(where_node) = &update.where_clause {
        resolve_params_in_expr(where_node, catalog, &update_scope, param_map)?;
    }

    for rt_node in &update.returning_list {
        if let Some(NodeEnum::ResTarget(rt)) = &rt_node.node
            && let Some(val) = &rt.val
        {
            resolve_params_in_expr(val, catalog, &update_scope, param_map)?;
        }
    }

    Ok(())
}

fn walk_delete_stmt(
    delete: &pg_query::protobuf::DeleteStmt,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    let mut delete_scope = QueryScope::with_ctes_from(scope);
    if let Some(wc) = &delete.with_clause {
        crate::analyzer::process_with_clause_with_params(
            wc,
            &mut delete_scope,
            catalog,
            param_map,
        )?;
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
    let mut col_order = Vec::new();
    for col in &table_meta.columns {
        columns.insert(
            col.name.to_ascii_lowercase(),
            ColumnBinding::from_column_metadata(col),
        );
        col_order.push(col.name.clone());
    }

    let binding = TableBinding {
        base_table: table_name,
        exposed_name,
        has_explicit_alias,
        is_null_producing: false,
        is_null_padded: false,
        columns,
        column_order: col_order,
        ambiguous_columns: HashSet::new(),
    };
    delete_scope.add_binding(binding)?;

    if !delete.using_clause.is_empty() {
        let using_scope = crate::nullability::propagate_join_nullability_result(
            &delete.using_clause,
            catalog,
            &delete_scope,
        )?;
        delete_scope.merge(using_scope)?;
    }

    for using_item in &delete.using_clause {
        walk_from_clause_node(using_item, catalog, &delete_scope, param_map)?;
    }

    if let Some(where_node) = &delete.where_clause {
        resolve_params_in_expr(where_node, catalog, &delete_scope, param_map)?;
    }

    for rt_node in &delete.returning_list {
        if let Some(NodeEnum::ResTarget(rt)) = &rt_node.node
            && let Some(val) = &rt.val
        {
            resolve_params_in_expr(val, catalog, &delete_scope, param_map)?;
        }
    }

    Ok(())
}
