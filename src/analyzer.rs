use crate::catalog::{extract_type_name, normalize_pg_type_to_ts, Catalog, ColumnMetadata, TableMetadata};
use pg_query::protobuf::{AExprKind, JoinType};
use pg_query::NodeEnum;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryParam {
    pub index: usize,
    pub name: String,
    pub ts_type: String,
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

#[derive(Debug, Clone)]
struct TableInScope {
    table_name: String,
    alias: String,
    is_nullable: bool,
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

fn format_nullable(ts_type: &str) -> String {
    if ts_type.contains("| null") {
        ts_type.to_string()
    } else {
        format!("{} | null", ts_type)
    }
}

fn extract_string(node: &pg_query::protobuf::Node) -> Option<String> {
    if let Some(NodeEnum::String(s)) = &node.node {
        Some(s.sval.clone())
    } else {
        None
    }
}

fn extract_func_name(funcnames: &[pg_query::protobuf::Node]) -> String {
    let mut names = Vec::new();
    for f in funcnames {
        if let Some(s) = extract_string(f) {
            names.push(s);
        }
    }
    names.last().cloned().unwrap_or_default().to_ascii_lowercase()
}

/// Analyzes an application SQL query file against the catalog.
pub fn analyze_query(
    sql: &str,
    catalog: &Catalog,
    fallback_filename: Option<&str>,
) -> Result<AnalyzedQuery, String> {
    let query_name = extract_query_name(sql, fallback_filename);
    let parsed = pg_query::parse(sql).map_err(|e| format!("Query parse error: {}", e))?;

    // Find the SelectStmt
    let mut select_stmt: Option<&pg_query::protobuf::SelectStmt> = None;
    for stmt in &parsed.protobuf.stmts {
        if let Some(node) = &stmt.stmt
            && let Some(NodeEnum::SelectStmt(sel)) = &node.node {
                select_stmt = Some(sel);
                break;
            }
    }

    let select = select_stmt.ok_or_else(|| "No SELECT statement found in query".to_string())?;
    let mut param_map: HashMap<i32, (Option<String>, Option<String>)> = HashMap::new();

    let fields = analyze_select_stmt(select, catalog, &mut param_map)?;

    // Sort parameters deterministically by index (1..=N)
    let mut param_indices: Vec<i32> = param_map.keys().copied().collect();
    param_indices.sort();

    let mut params = Vec::new();
    let mut used_names: HashMap<String, usize> = HashMap::new();

    for idx in param_indices {
        let (suggested_name, inferred_type) = param_map.get(&idx).unwrap();
        let base_name = suggested_name
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
            ts_type: inferred_type.clone().unwrap_or_else(|| "unknown".to_string()),
        });
    }

    Ok(AnalyzedQuery {
        name: query_name,
        raw_sql: sql.to_string(),
        params,
        fields,
    })
}

fn analyze_select_stmt(
    select: &pg_query::protobuf::SelectStmt,
    catalog: &Catalog,
    param_map: &mut HashMap<i32, (Option<String>, Option<String>)>,
) -> Result<Vec<QueryField>, String> {
    // Check if this SelectStmt is a setop (like UNION) where projections are in larg
    if select.target_list.is_empty()
        && let Some(l_sel) = &select.larg {
            return analyze_select_stmt(l_sel, catalog, param_map);
        }

    // 0. Register CTEs from with_clause into a query-scoped catalog overlay
    let mut scoped_catalog = catalog.clone();
    if let Some(wc) = &select.with_clause {
        for cte_node in &wc.ctes {
            if let Some(NodeEnum::CommonTableExpr(cte)) = &cte_node.node {
                let cte_name = cte.ctename.to_ascii_lowercase();
                if let Some(query_node) = &cte.ctequery
                    && let Some(NodeEnum::SelectStmt(cte_select)) = &query_node.node {
                        let cte_fields = analyze_select_stmt(cte_select, &scoped_catalog, param_map)?;
                        let mut cte_columns = Vec::new();

                        for (i, field) in cte_fields.iter().enumerate() {
                            let col_name = if let Some(alias_node) = cte.aliascolnames.get(i) {
                                extract_string(alias_node).unwrap_or_else(|| field.name.clone())
                            } else {
                                field.name.clone()
                            };

                            let is_nullable = field.ts_type.contains("| null");
                            let base_ts_type = field.ts_type.replace(" | null", "").trim().to_string();

                            cte_columns.push(ColumnMetadata {
                                name: col_name,
                                pg_type: "unknown".to_string(),
                                ts_type: base_ts_type,
                                is_nullable,
                            });
                        }

                        let cte_table = TableMetadata {
                            name: cte_name.clone(),
                            schema: None,
                            columns: cte_columns,
                        };
                        scoped_catalog.tables.insert(cte_name, cte_table);
                    }
            }
        }
    }

    // 1. Build table-to-nullability context map from from_clause
    let mut tables_in_scope: Vec<TableInScope> = Vec::new();
    for from_item in &select.from_clause {
        collect_from_node(from_item, false, &scoped_catalog, &mut tables_in_scope)?;
    }

    // 2. Resolve projections (target_list)
    let mut fields: Vec<QueryField> = Vec::new();
    for target in &select.target_list {
        if let Some(NodeEnum::ResTarget(rt)) = &target.node {
            resolve_target(rt, &scoped_catalog, &tables_in_scope, &mut fields)?;
        }
    }

    // 3. Resolve parameters from WHERE clause (and limit / offset if present)
    if let Some(where_node) = &select.where_clause {
        resolve_params_in_expr(where_node, &scoped_catalog, &tables_in_scope, param_map)?;
    }

    if let Some(limit_node) = &select.limit_count
        && let Some(NodeEnum::ParamRef(p)) = &limit_node.node {
            param_map.entry(p.number).or_insert((Some("limit".to_string()), Some("number".to_string())));
        }

    if let Some(offset_node) = &select.limit_offset
        && let Some(NodeEnum::ParamRef(p)) = &offset_node.node {
            param_map.entry(p.number).or_insert((Some("offset".to_string()), Some("number".to_string())));
        }

    Ok(fields)
}

fn collect_from_node(
    node: &pg_query::protobuf::Node,
    parent_nullable: bool,
    catalog: &Catalog,
    list: &mut Vec<TableInScope>,
) -> Result<(), String> {
    match &node.node {
        Some(NodeEnum::RangeVar(rv)) => {
            let table_name = rv.relname.to_ascii_lowercase();
            // Validate table exists in catalog
            if catalog.get_table(&table_name).is_none() {
                return Err(format!("Table \"{}\" does not exist in schema catalog", rv.relname));
            }

            let alias = if let Some(a) = &rv.alias {
                a.aliasname.clone()
            } else {
                table_name.clone()
            };

            list.push(TableInScope {
                table_name,
                alias,
                is_nullable: parent_nullable,
            });
            Ok(())
        }
        Some(NodeEnum::JoinExpr(je)) => {
            let (left_nullable, right_nullable) = match je.jointype {
                x if x == JoinType::JoinLeft as i32 => (parent_nullable, true),
                x if x == JoinType::JoinRight as i32 => (true, parent_nullable),
                x if x == JoinType::JoinFull as i32 => (true, true),
                _ => (parent_nullable, parent_nullable),
            };

            if let Some(larg) = &je.larg {
                collect_from_node(larg, left_nullable, catalog, list)?;
            }
            if let Some(rarg) = &je.rarg {
                collect_from_node(rarg, right_nullable, catalog, list)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn resolve_target(
    rt: &pg_query::protobuf::ResTarget,
    catalog: &Catalog,
    tables: &[TableInScope],
    fields: &mut Vec<QueryField>,
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
                && let Some(NodeEnum::AStar(_)) = &first.node {
                    // SELECT * FROM ...
                    for table in tables {
                        if let Some(table_meta) = catalog.get_table(&table.table_name) {
                            for col in &table_meta.columns {
                                let is_null = col.is_nullable || table.is_nullable;
                                let ts_type = if is_null {
                                    format_nullable(&col.ts_type)
                                } else {
                                    col.ts_type.clone()
                                };
                                fields.push(QueryField {
                                    name: col.name.clone(),
                                    ts_type,
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
                        && let Some(alias) = extract_string(first) {
                            let table = tables
                                .iter()
                                .find(|t| t.alias.eq_ignore_ascii_case(&alias))
                                .ok_or_else(|| format!("Unknown table alias \"{}\"", alias))?;
                            if let Some(table_meta) = catalog.get_table(&table.table_name) {
                                for col in &table_meta.columns {
                                    let is_null = col.is_nullable || table.is_nullable;
                                    let ts_type = if is_null {
                                        format_nullable(&col.ts_type)
                                    } else {
                                        col.ts_type.clone()
                                    };
                                    fields.push(QueryField {
                                        name: col.name.clone(),
                                        ts_type,
                                    });
                                }
                            }
                            return Ok(());
                        }

            // Qualified or unqualified column ref
            if cr.fields.len() == 2 {
                let alias_str = extract_string(&cr.fields[0])
                    .ok_or_else(|| "Invalid column reference alias".to_string())?;
                let col_str = extract_string(&cr.fields[1])
                    .ok_or_else(|| "Invalid column reference name".to_string())?;

                let table = tables
                    .iter()
                    .find(|t| t.alias.eq_ignore_ascii_case(&alias_str))
                    .ok_or_else(|| format!("Unknown table alias \"{}\" in column reference", alias_str))?;

                let table_meta = catalog
                    .get_table(&table.table_name)
                    .ok_or_else(|| format!("Table \"{}\" not found in catalog", table.table_name))?;

                let col = table_meta
                    .get_column(&col_str)
                    .ok_or_else(|| format!("Column \"{}\" not found on table \"{}\"", col_str, table.table_name))?;

                let is_null = col.is_nullable || table.is_nullable;
                let ts_type = if is_null {
                    format_nullable(&col.ts_type)
                } else {
                    col.ts_type.clone()
                };

                fields.push(QueryField {
                    name: explicit_alias.unwrap_or(col.name.clone()),
                    ts_type,
                });
            } else if cr.fields.len() == 1 {
                let col_str = extract_string(&cr.fields[0])
                    .ok_or_else(|| "Invalid column reference name".to_string())?;

                let mut matched: Vec<(&TableInScope, &crate::catalog::ColumnMetadata)> = Vec::new();
                for table in tables {
                    if let Some(table_meta) = catalog.get_table(&table.table_name)
                        && let Some(col) = table_meta.get_column(&col_str) {
                            matched.push((table, col));
                        }
                }

                if matched.is_empty() {
                    return Err(format!("Column \"{}\" not found in any table in scope", col_str));
                }
                if matched.len() > 1 {
                    return Err(format!("Column \"{}\" is ambiguous across multiple tables in scope", col_str));
                }

                let (table, col) = matched[0];
                let is_null = col.is_nullable || table.is_nullable;
                let ts_type = if is_null {
                    format_nullable(&col.ts_type)
                } else {
                    col.ts_type.clone()
                };

                fields.push(QueryField {
                    name: explicit_alias.unwrap_or(col.name.clone()),
                    ts_type,
                });
            }
            Ok(())
        }
        Some(NodeEnum::FuncCall(fc)) => {
            let func_name = extract_func_name(&fc.funcname);
            let name = explicit_alias.unwrap_or_else(|| func_name.clone());

            let ts_type = match func_name.as_str() {
                // COUNT is strictly non-nullable number
                "count" => "number".to_string(),
                // SUM and AVG are nullable (number | null)
                "sum" | "avg" => "number | null".to_string(),
                "min" | "max" => {
                    // Try to resolve argument column type
                    let mut inner_type = "unknown".to_string();
                    if let Some(arg) = fc.args.first() {
                        let mut sub_fields = Vec::new();
                        let dummy_rt = pg_query::protobuf::ResTarget {
                            name: String::new(),
                            indirection: Vec::new(),
                            val: Some(Box::new(arg.clone())),
                            location: 0,
                        };
                        if resolve_target(&dummy_rt, catalog, tables, &mut sub_fields).is_ok()
                            && let Some(first_field) = sub_fields.first() {
                                inner_type = first_field.ts_type.clone();
                            }
                    }
                    format_nullable(&inner_type)
                }
                "coalesce" => {
                    // If any argument is non-nullable, coalesce is non-nullable
                    let mut is_nullable = true;
                    let mut resolved_type = "unknown".to_string();
                    for arg in &fc.args {
                        let mut sub_fields = Vec::new();
                        let dummy_rt = pg_query::protobuf::ResTarget {
                            name: String::new(),
                            indirection: Vec::new(),
                            val: Some(Box::new(arg.clone())),
                            location: 0,
                        };
                        if resolve_target(&dummy_rt, catalog, tables, &mut sub_fields).is_ok()
                            && let Some(f) = sub_fields.first() {
                                let arg_is_nullable = f.ts_type.contains("| null");
                                let base = f.ts_type.replace(" | null", "").trim().to_string();
                                if base != "unknown" {
                                    resolved_type = base;
                                }
                                if !arg_is_nullable {
                                    is_nullable = false;
                                    break;
                                }
                            }
                    }
                    if is_nullable {
                        format_nullable(&resolved_type)
                    } else {
                        resolved_type
                    }
                }
                _ => "unknown".to_string(),
            };

            fields.push(QueryField { name, ts_type });
            Ok(())
        }
        Some(NodeEnum::CoalesceExpr(ce)) => {
            let name = explicit_alias.unwrap_or_else(|| "coalesce".to_string());
            let mut is_nullable = true;
            let mut resolved_type = "unknown".to_string();

            for arg in &ce.args {
                let mut sub_fields = Vec::new();
                let dummy_rt = pg_query::protobuf::ResTarget {
                    name: String::new(),
                    indirection: Vec::new(),
                    val: Some(Box::new(arg.clone())),
                    location: 0,
                };
                if resolve_target(&dummy_rt, catalog, tables, &mut sub_fields).is_ok()
                    && let Some(f) = sub_fields.first() {
                        let arg_is_nullable = f.ts_type.contains("| null");
                        let base = f.ts_type.replace(" | null", "").trim().to_string();
                        if base != "unknown" {
                            resolved_type = base;
                        }
                        if !arg_is_nullable {
                            is_nullable = false;
                            break;
                        }
                    }
            }

            let ts_type = if is_nullable {
                format_nullable(&resolved_type)
            } else {
                resolved_type
            };

            fields.push(QueryField { name, ts_type });
            Ok(())
        }
        Some(NodeEnum::AConst(ac)) => {
            let name = explicit_alias.unwrap_or_else(|| "constant".to_string());
            let ts_type = if ac.isnull {
                "unknown | null".to_string()
            } else if let Some(val) = &ac.val {
                match val {
                    pg_query::protobuf::a_const::Val::Ival(_) => "number".to_string(),
                    pg_query::protobuf::a_const::Val::Fval(_) => "number".to_string(),
                    pg_query::protobuf::a_const::Val::Sval(_) => "string".to_string(),
                    pg_query::protobuf::a_const::Val::Boolval(_) => "boolean".to_string(),
                    _ => "unknown".to_string(),
                }
            } else {
                "unknown".to_string()
            };
            fields.push(QueryField { name, ts_type });
            Ok(())
        }
        Some(NodeEnum::TypeCast(tc)) => {
            let name = explicit_alias.unwrap_or_else(|| "cast".to_string());
            let pg_type = tc
                .type_name
                .as_ref()
                .map(extract_type_name)
                .unwrap_or_else(|| "unknown".to_string());
            let ts_type = normalize_pg_type_to_ts(&pg_type);
            fields.push(QueryField { name, ts_type });
            Ok(())
        }
        Some(NodeEnum::AExpr(ae)) => {
            let name = explicit_alias.unwrap_or_else(|| "expr".to_string());
            let op = ae.name.first().and_then(extract_string).unwrap_or_default();

            let resolve_operand = |operand_node: &Option<Box<pg_query::protobuf::Node>>| -> (String, bool) {
                if let Some(op_node) = operand_node {
                    let mut sub_fields = Vec::new();
                    let dummy_rt = pg_query::protobuf::ResTarget {
                        name: String::new(),
                        indirection: Vec::new(),
                        val: Some(op_node.clone()),
                        location: 0,
                    };
                    if resolve_target(&dummy_rt, catalog, tables, &mut sub_fields).is_ok()
                        && let Some(f) = sub_fields.first() {
                            let is_null = f.ts_type.contains("| null");
                            let base = f.ts_type.replace(" | null", "").trim().to_string();
                            return (base, is_null);
                        }
                }
                ("unknown".to_string(), true)
            };

            let (_l_type, l_null) = resolve_operand(&ae.lexpr);
            let (_r_type, r_null) = resolve_operand(&ae.rexpr);

            let is_arithmetic = matches!(op.as_str(), "+" | "-" | "*" | "/" | "%");
            let is_concat = op == "||";

            let ts_type = if is_arithmetic {
                if l_null || r_null {
                    "number | null".to_string()
                } else {
                    "number".to_string()
                }
            } else if is_concat {
                if l_null || r_null {
                    "string | null".to_string()
                } else {
                    "string".to_string()
                }
            } else if matches!(op.as_str(), "=" | "<>" | "!=" | "<" | "<=" | ">" | ">=") {
                if l_null || r_null {
                    "boolean | null".to_string()
                } else {
                    "boolean".to_string()
                }
            } else {
                "unknown".to_string()
            };

            fields.push(QueryField { name, ts_type });
            Ok(())
        }
        Some(NodeEnum::AArrayExpr(aae)) => {
            let name = explicit_alias.unwrap_or_else(|| "arr".to_string());
            let mut elem_type = "unknown".to_string();
            if let Some(first) = aae.elements.first() {
                let mut sub_fields = Vec::new();
                let dummy_rt = pg_query::protobuf::ResTarget {
                    name: String::new(),
                    indirection: Vec::new(),
                    val: Some(Box::new(first.clone())),
                    location: 0,
                };
                if resolve_target(&dummy_rt, catalog, tables, &mut sub_fields).is_ok()
                    && let Some(f) = sub_fields.first() {
                        elem_type = f.ts_type.replace(" | null", "").trim().to_string();
                    }
            }
            fields.push(QueryField {
                name,
                ts_type: format!("{}[]", elem_type),
            });
            Ok(())
        }
        _ => {
            let name = explicit_alias.unwrap_or_else(|| "column".to_string());
            fields.push(QueryField {
                name,
                ts_type: "unknown".to_string(),
            });
            Ok(())
        }
    }
}

/// Helper to inspect an expression for ParamRef and ColumnRef combinations.
fn extract_param_info(node: &pg_query::protobuf::Node) -> Option<(i32, Option<String>)> {
    match &node.node {
        Some(NodeEnum::ParamRef(p)) => Some((p.number, None)),
        Some(NodeEnum::TypeCast(tc)) => {
            if let Some(arg) = &tc.arg
                && let Some(NodeEnum::ParamRef(p)) = &arg.node {
                    let cast_type = tc.type_name.as_ref().map(extract_type_name);
                    return Some((p.number, cast_type));
                }
            None
        }
        _ => None,
    }
}

fn extract_column_info(
    node: &pg_query::protobuf::Node,
) -> Option<(Option<String>, String)> {
    match &node.node {
        Some(NodeEnum::ColumnRef(cr)) => {
            if cr.fields.len() == 2 {
                let alias = extract_string(&cr.fields[0]);
                let col = extract_string(&cr.fields[1]);
                if let Some(c) = col {
                    return Some((alias, c));
                }
            } else if cr.fields.len() == 1
                && let Some(c) = extract_string(&cr.fields[0]) {
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

fn resolve_params_in_expr(
    expr: &pg_query::protobuf::Node,
    catalog: &Catalog,
    tables: &[TableInScope],
    param_map: &mut HashMap<i32, (Option<String>, Option<String>)>,
) -> Result<(), String> {
    match &expr.node {
        Some(NodeEnum::BoolExpr(be)) => {
            for arg in &be.args {
                resolve_params_in_expr(arg, catalog, tables, param_map)?;
            }
        }
        Some(NodeEnum::AExpr(ae)) => {
            if ae.kind == AExprKind::AexprOp as i32 {
                let param_opt_l = ae.lexpr.as_ref().and_then(|n| extract_param_info(n));
                let col_opt_l = ae.lexpr.as_ref().and_then(|n| extract_column_info(n));

                let param_opt_r = ae.rexpr.as_ref().and_then(|n| extract_param_info(n));
                let col_opt_r = ae.rexpr.as_ref().and_then(|n| extract_column_info(n));

                if let (Some((param_num, cast_opt)), Some((alias_opt, col_name))) =
                    (param_opt_r, col_opt_l)
                {
                    record_param(
                        param_num,
                        cast_opt,
                        alias_opt,
                        &col_name,
                        catalog,
                        tables,
                        param_map,
                    )?;
                } else if let (Some((param_num, cast_opt)), Some((alias_opt, col_name))) =
                    (param_opt_l, col_opt_r)
                {
                    record_param(
                        param_num,
                        cast_opt,
                        alias_opt,
                        &col_name,
                        catalog,
                        tables,
                        param_map,
                    )?;
                } else {
                    // Recurse both branches
                    if let Some(lexpr) = &ae.lexpr {
                        resolve_params_in_expr(lexpr, catalog, tables, param_map)?;
                    }
                    if let Some(rexpr) = &ae.rexpr {
                        resolve_params_in_expr(rexpr, catalog, tables, param_map)?;
                    }
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
                        .map(|t| normalize_pg_type_to_ts(&t));
                    param_map
                        .entry(p.number)
                        .or_insert((None, ts_type));
                } else {
                    resolve_params_in_expr(arg, catalog, tables, param_map)?;
                }
            }
        }
        Some(NodeEnum::ParamRef(p)) => {
            param_map.entry(p.number).or_insert((None, None));
        }
        Some(NodeEnum::NullTest(nt)) => {
            if let Some(arg) = &nt.arg {
                resolve_params_in_expr(arg, catalog, tables, param_map)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn record_param(
    param_num: i32,
    cast_opt: Option<String>,
    alias_opt: Option<String>,
    col_name: &str,
    catalog: &Catalog,
    tables: &[TableInScope],
    param_map: &mut HashMap<i32, (Option<String>, Option<String>)>,
) -> Result<(), String> {
    let mut resolved_type: Option<String> = cast_opt.map(|c| normalize_pg_type_to_ts(&c));

    // Lookup column to verify and get type if not explicitly cast
    let col_meta = if let Some(alias) = alias_opt {
        let table = tables
            .iter()
            .find(|t| t.alias.eq_ignore_ascii_case(&alias))
            .ok_or_else(|| format!("Unknown table alias \"{}\" in parameter comparison", alias))?;
        let t_meta = catalog
            .get_table(&table.table_name)
            .ok_or_else(|| format!("Table \"{}\" not found in catalog", table.table_name))?;
        t_meta.get_column(col_name)
    } else {
        let mut found = None;
        for t in tables {
            if let Some(t_meta) = catalog.get_table(&t.table_name)
                && let Some(c) = t_meta.get_column(col_name) {
                    found = Some(c);
                    break;
                }
        }
        found
    };

    if let Some(col) = col_meta
        && resolved_type.is_none() {
            resolved_type = Some(col.ts_type.clone());
        }

    let entry = param_map.entry(param_num).or_insert((None, None));
    if entry.0.is_none() {
        entry.0 = Some(col_name.to_string());
    }
    if entry.1.is_none() || entry.1.as_deref() == Some("unknown") {
        entry.1 = resolved_type;
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
        let analyzed_alias_star = analyze_query(alias_star_sql, &catalog, Some("users_alias.sql")).unwrap();
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
        catalog.apply_sql("
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
        ").unwrap();

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
        catalog.apply_sql("
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                age INT NOT NULL,
                nickname TEXT
            );
        ").unwrap();

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
        catalog.apply_sql("
            CREATE TABLE articles (
                id UUID PRIMARY KEY,
                tags TEXT[] NOT NULL,
                scores INT4[] NOT NULL,
                optional_tags TEXT[]
            );
        ").unwrap();

        let query_sql = "
            SELECT a.tags, a.scores, a.optional_tags, ARRAY['rust', 'typescript'] AS defaults
            FROM articles a;
        ";
        let analyzed = analyze_query(query_sql, &catalog, Some("array_query.sql")).unwrap();
        assert_eq!(analyzed.fields.len(), 4);
        assert_eq!(analyzed.fields[0].name, "tags");
        assert_eq!(analyzed.fields[0].ts_type, "string[]");
        assert_eq!(analyzed.fields[1].name, "scores");
        assert_eq!(analyzed.fields[1].ts_type, "number[]");
        assert_eq!(analyzed.fields[2].name, "optional_tags");
        assert_eq!(analyzed.fields[2].ts_type, "string[] | null");
        assert_eq!(analyzed.fields[3].name, "defaults");
        assert_eq!(analyzed.fields[3].ts_type, "string[]");
    }
}
