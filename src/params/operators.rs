//! AST Operator and Expression Traversal Engine
//!
//! Handles parameter deduction across PostgreSQL binary operators (`A_Expr`),
//! scalar array comparisons (`ANY`, `ALL`), set membership (`IN`), range bounds (`BETWEEN`),
//! literal counterparts (`A_Const`), type casts, and function calls.

use crate::analyzer::{PgType, QueryScope, extract_func_name, extract_string, infer_expr};
use crate::catalog::Catalog;
use crate::params::{
    ParamInfo, extract_column_info, extract_param_info, record_param, resolve_params_in_expr,
    resolve_type_from_cast,
};
use pg_query::NodeEnum;
use pg_query::protobuf::{AExpr, AExprKind, FuncCall, Node};
use std::collections::HashMap;

/// Deduces prepared statement parameters inside an `A_Expr` node.
pub fn deduce_aexpr(
    ae: &AExpr,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    // 1. Array comparison: ANY / ALL
    if ae.kind == AExprKind::AexprOpAny as i32 || ae.kind == AExprKind::AexprOpAll as i32 {
        if let (Some(lexpr), Some(rexpr)) = (&ae.lexpr, &ae.rexpr) {
            resolve_scalar_array_comparison(lexpr, rexpr, catalog, scope, param_map)?;
        }
        return Ok(());
    }

    // 2. Pattern matching: LIKE, ILIKE, SIMILAR TO
    if ae.kind == AExprKind::AexprLike as i32
        || ae.kind == AExprKind::AexprIlike as i32
        || ae.kind == AExprKind::AexprSimilar as i32
    {
        handle_pattern_matching(ae, catalog, scope, param_map)?;
        return Ok(());
    }

    // 3. Set membership: IN / NOT IN
    if ae.kind == AExprKind::AexprIn as i32 {
        handle_in_expr(ae, catalog, scope, param_map)?;
        return Ok(());
    }

    // 4. Range operations: BETWEEN, NOT BETWEEN, BETWEEN SYMMETRIC, NOT BETWEEN SYMMETRIC
    if ae.kind == AExprKind::AexprBetween as i32
        || ae.kind == AExprKind::AexprNotBetween as i32
        || ae.kind == AExprKind::AexprBetweenSym as i32
        || ae.kind == AExprKind::AexprNotBetweenSym as i32
    {
        handle_between_expr(ae, catalog, scope, param_map)?;
        return Ok(());
    }

    // 5. Standard binary operator (kind: AexprOp)
    if ae.kind == AExprKind::AexprOp as i32 {
        let op = ae.name.first().and_then(extract_string).unwrap_or_default();

        // Check if operator is raw pattern matching symbol (~~, ~~*, !~~, !~~*, ~, ~*, !~, !~*)
        if matches!(
            op.as_str(),
            "~~" | "~~*" | "!~~" | "!~~*" | "~" | "~*" | "!~" | "!~*"
        ) {
            handle_pattern_matching(ae, catalog, scope, param_map)?;
            return Ok(());
        }

        // Check array overlap / containment (&&, @>, <@)
        if matches!(op.as_str(), "&&" | "@>" | "<@") {
            if let (Some(lexpr), Some(rexpr)) = (&ae.lexpr, &ae.rexpr) {
                resolve_array_op_params(lexpr, rexpr, catalog, scope, param_map)?;
            }
            return Ok(());
        }

        // Check string concatenation (||)
        if op == "||" {
            handle_concat_params(ae, catalog, scope, param_map)?;
            return Ok(());
        }

        // Check symmetrical function unwrapping: lower(col) = lower($1)
        if let (Some(lexpr), Some(rexpr)) = (&ae.lexpr, &ae.rexpr)
            && let (Some(NodeEnum::FuncCall(fc_l)), Some(NodeEnum::FuncCall(fc_r))) =
                (&lexpr.node, &rexpr.node)
        {
            let fn_l = extract_func_name(&fc_l.funcname);
            let fn_r = extract_func_name(&fc_r.funcname);
            if fn_l.eq_ignore_ascii_case(&fn_r) && fc_l.args.len() == 1 && fc_r.args.len() == 1 {
                let arg_l = &fc_l.args[0];
                let arg_r = &fc_r.args[0];
                let param_opt_r = extract_param_info(arg_r);
                let col_opt_l = extract_column_info(arg_l);
                let param_opt_l = extract_param_info(arg_l);
                let col_opt_r = extract_column_info(arg_r);

                if let (Some((param_num, cast_opt)), Some((alias_opt, col_name))) =
                    (param_opt_r, col_opt_l)
                {
                    record_param(
                        param_num, cast_opt, alias_opt, &col_name, catalog, scope, param_map, false,
                    )?;
                    return Ok(());
                } else if let (Some((param_num, cast_opt)), Some((alias_opt, col_name))) =
                    (param_opt_l, col_opt_r)
                {
                    record_param(
                        param_num, cast_opt, alias_opt, &col_name, catalog, scope, param_map, false,
                    )?;
                    return Ok(());
                }
            }
        }

        let param_opt_l = ae.lexpr.as_ref().and_then(|n| extract_param_info(n));
        let col_opt_l = ae.lexpr.as_ref().and_then(|n| extract_column_info(n));

        let param_opt_r = ae.rexpr.as_ref().and_then(|n| extract_param_info(n));
        let col_opt_r = ae.rexpr.as_ref().and_then(|n| extract_column_info(n));

        // Comparison against column on counterpart side
        if let (Some((param_num, cast_opt)), Some((alias_opt, col_name))) =
            (&param_opt_r, &col_opt_l)
        {
            record_param(
                *param_num,
                cast_opt.clone(),
                alias_opt.clone(),
                col_name,
                catalog,
                scope,
                param_map,
                false,
            )?;
            return Ok(());
        } else if let (Some((param_num, cast_opt)), Some((alias_opt, col_name))) =
            (&param_opt_l, &col_opt_r)
        {
            record_param(
                *param_num,
                cast_opt.clone(),
                alias_opt.clone(),
                col_name,
                catalog,
                scope,
                param_map,
                false,
            )?;
            return Ok(());
        }

        // If one side is a column, propagate suggested column name to parameters in counterpart expression
        if let Some((_, ref col_name)) = col_opt_l {
            if let Some(rexpr) = &ae.rexpr {
                let mut params = Vec::new();
                crate::params::collect_param_refs(rexpr, &mut params);
                for p in params {
                    let entry = param_map.entry(p).or_default();
                    if entry.suggested_name.is_none() {
                        entry.suggested_name = Some(col_name.clone());
                    }
                }
            }
        } else if let Some((_, ref col_name)) = col_opt_r
            && let Some(lexpr) = &ae.lexpr
        {
            let mut params = Vec::new();
            crate::params::collect_param_refs(lexpr, &mut params);
            for p in params {
                let entry = param_map.entry(p).or_default();
                if entry.suggested_name.is_none() {
                    entry.suggested_name = Some(col_name.clone());
                }
            }
        }

        // Comparison against literal (AConst) or expression on counterpart side
        if let Some((param_num, cast_opt)) = &param_opt_r
            && let Some(lexpr) = &ae.lexpr
            && let Ok(l_inf) = infer_expr(lexpr, catalog, scope)
            && l_inf.pg_type != PgType::Unknown
        {
            let ts_type = cast_opt
                .as_deref()
                .map(|c| resolve_type_from_cast(c, catalog))
                .unwrap_or_else(|| l_inf.pg_type.to_ts(catalog));
            let entry = param_map.entry(*param_num).or_default();
            if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
                entry.inferred_type = Some(ts_type);
            }
            resolve_params_in_expr(lexpr, catalog, scope, param_map)?;
            return Ok(());
        } else if let Some((param_num, cast_opt)) = &param_opt_l
            && let Some(rexpr) = &ae.rexpr
            && let Ok(r_inf) = infer_expr(rexpr, catalog, scope)
            && r_inf.pg_type != PgType::Unknown
        {
            let ts_type = cast_opt
                .as_deref()
                .map(|c| resolve_type_from_cast(c, catalog))
                .unwrap_or_else(|| r_inf.pg_type.to_ts(catalog));
            let entry = param_map.entry(*param_num).or_default();
            if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
                entry.inferred_type = Some(ts_type);
            }
            resolve_params_in_expr(rexpr, catalog, scope, param_map)?;
            return Ok(());
        }

        // Custom extension operators
        if let Some((param_num, cast_opt)) = param_opt_r
            && let Some(lexpr) = &ae.lexpr
            && let Ok(l_inf) = infer_expr(lexpr, catalog, scope)
            && l_inf.pg_type != PgType::Unknown
            && let Some(ext_op) = catalog.resolve_operator(&op, &l_inf.pg_type, &PgType::Unknown)
        {
            let ts_type = cast_opt
                .map(|c| resolve_type_from_cast(&c, catalog))
                .unwrap_or_else(|| {
                    if matches!(ext_op.right, PgType::Vector(_) | PgType::HalfVec(_)) {
                        l_inf.pg_type.to_ts(catalog)
                    } else {
                        ext_op.right.to_ts(catalog)
                    }
                });
            let entry = param_map.entry(param_num).or_default();
            if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
                entry.inferred_type = Some(ts_type);
            }
            return Ok(());
        } else if let Some((param_num, cast_opt)) = param_opt_l
            && let Some(rexpr) = &ae.rexpr
            && let Ok(r_inf) = infer_expr(rexpr, catalog, scope)
            && r_inf.pg_type != PgType::Unknown
            && let Some(ext_op) = catalog.resolve_operator(&op, &PgType::Unknown, &r_inf.pg_type)
        {
            let ts_type = cast_opt
                .map(|c| resolve_type_from_cast(&c, catalog))
                .unwrap_or_else(|| {
                    if matches!(ext_op.left, PgType::Vector(_) | PgType::HalfVec(_)) {
                        r_inf.pg_type.to_ts(catalog)
                    } else {
                        ext_op.left.to_ts(catalog)
                    }
                });
            let entry = param_map.entry(param_num).or_default();
            if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
                entry.inferred_type = Some(ts_type);
            }
            return Ok(());
        }

        // Recurse both branches
        if let Some(lexpr) = &ae.lexpr {
            resolve_params_in_expr(lexpr, catalog, scope, param_map)?;
        }
        if let Some(rexpr) = &ae.rexpr {
            resolve_params_in_expr(rexpr, catalog, scope, param_map)?;
        }
        return Ok(());
    }

    // 6. Distinct / Nullif
    if matches!(
        ae.kind,
        4..=6 // AexprDistinct, AexprNotDistinct, AexprNullif
    ) {
        if let (Some(lexpr), Some(rexpr)) = (&ae.lexpr, &ae.rexpr) {
            let param_l = extract_param_info(lexpr);
            let col_l = extract_column_info(lexpr);
            let param_r = extract_param_info(rexpr);
            let col_r = extract_column_info(rexpr);

            if let (Some((p, cast_r)), Some((alias_l, col_name_l))) = (param_r, col_l) {
                record_param(
                    p,
                    cast_r,
                    alias_l,
                    &col_name_l,
                    catalog,
                    scope,
                    param_map,
                    false,
                )?;
                return Ok(());
            } else if let (Some((p, cast_l)), Some((alias_r, col_name_r))) = (param_l, col_r) {
                record_param(
                    p,
                    cast_l,
                    alias_r,
                    &col_name_r,
                    catalog,
                    scope,
                    param_map,
                    false,
                )?;
                return Ok(());
            }
            resolve_params_in_expr(lexpr, catalog, scope, param_map)?;
            resolve_params_in_expr(rexpr, catalog, scope, param_map)?;
        }
        return Ok(());
    }

    // Fallthrough for any other A_Expr variants
    if let Some(lexpr) = &ae.lexpr {
        resolve_params_in_expr(lexpr, catalog, scope, param_map)?;
    }
    if let Some(rexpr) = &ae.rexpr {
        resolve_params_in_expr(rexpr, catalog, scope, param_map)?;
    }
    Ok(())
}

/// Handles pattern matching expressions (`LIKE`, `ILIKE`, `SIMILAR TO`, `~~`, `~~*`).
pub fn handle_pattern_matching(
    ae: &AExpr,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    let param_opt_l = ae.lexpr.as_ref().and_then(|n| extract_param_info(n));
    let col_opt_l = ae.lexpr.as_ref().and_then(|n| extract_column_info(n));

    let param_opt_r = ae.rexpr.as_ref().and_then(|n| extract_param_info(n));
    let col_opt_r = ae.rexpr.as_ref().and_then(|n| extract_column_info(n));

    if let Some((param_num, cast_opt)) = param_opt_r {
        let col_name = col_opt_l.map(|(_, name)| name);
        let entry = param_map.entry(param_num).or_default();
        if entry.suggested_name.is_none() && col_name.is_some() {
            entry.suggested_name = col_name;
        }
        if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
            entry.inferred_type = Some(
                cast_opt
                    .map(|c| resolve_type_from_cast(&c, catalog))
                    .unwrap_or_else(|| "string".to_string()),
            );
        }
    } else if let Some((param_num, cast_opt)) = param_opt_l {
        let col_name = col_opt_r.map(|(_, name)| name);
        let entry = param_map.entry(param_num).or_default();
        if entry.suggested_name.is_none() && col_name.is_some() {
            entry.suggested_name = col_name;
        }
        if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
            entry.inferred_type = Some(
                cast_opt
                    .map(|c| resolve_type_from_cast(&c, catalog))
                    .unwrap_or_else(|| "string".to_string()),
            );
        }
    } else if let (Some((p_l, cast_l)), Some((p_r, cast_r))) = (param_opt_l, param_opt_r) {
        let entry_l = param_map.entry(p_l).or_default();
        if entry_l.inferred_type.is_none() || entry_l.inferred_type.as_deref() == Some("unknown") {
            entry_l.inferred_type = Some(
                cast_l
                    .map(|c| resolve_type_from_cast(&c, catalog))
                    .unwrap_or_else(|| "string".to_string()),
            );
        }
        let entry_r = param_map.entry(p_r).or_default();
        if entry_r.inferred_type.is_none() || entry_r.inferred_type.as_deref() == Some("unknown") {
            entry_r.inferred_type = Some(
                cast_r
                    .map(|c| resolve_type_from_cast(&c, catalog))
                    .unwrap_or_else(|| "string".to_string()),
            );
        }
    } else if let Some((_, ref col_name)) = col_opt_l {
        if let Some(rexpr) = &ae.rexpr {
            let mut params = Vec::new();
            crate::params::collect_param_refs(rexpr, &mut params);
            for p in params {
                let entry = param_map.entry(p).or_default();
                if entry.suggested_name.is_none() {
                    entry.suggested_name = Some(col_name.clone());
                }
                if entry.inferred_type.is_none()
                    || entry.inferred_type.as_deref() == Some("unknown")
                {
                    entry.inferred_type = Some("string".to_string());
                }
            }
        }
    } else if let Some((_, ref col_name)) = col_opt_r
        && let Some(lexpr) = &ae.lexpr
    {
        let mut params = Vec::new();
        crate::params::collect_param_refs(lexpr, &mut params);
        for p in params {
            let entry = param_map.entry(p).or_default();
            if entry.suggested_name.is_none() {
                entry.suggested_name = Some(col_name.clone());
            }
            if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
                entry.inferred_type = Some("string".to_string());
            }
        }
    }

    if let Some(lexpr) = &ae.lexpr {
        resolve_params_in_expr(lexpr, catalog, scope, param_map)?;
    }
    if let Some(rexpr) = &ae.rexpr {
        resolve_params_in_expr(rexpr, catalog, scope, param_map)?;
    }

    Ok(())
}

/// Handles string concatenation expressions (`||`).
pub fn handle_concat_params(
    ae: &AExpr,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    if let Some(lexpr) = &ae.lexpr {
        if let Some((param_num, cast_opt)) = extract_param_info(lexpr) {
            let entry = param_map.entry(param_num).or_default();
            if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
                entry.inferred_type = Some(
                    cast_opt
                        .map(|c| resolve_type_from_cast(&c, catalog))
                        .unwrap_or_else(|| "string".to_string()),
                );
            }
        }
        resolve_params_in_expr(lexpr, catalog, scope, param_map)?;
    }
    if let Some(rexpr) = &ae.rexpr {
        if let Some((param_num, cast_opt)) = extract_param_info(rexpr) {
            let entry = param_map.entry(param_num).or_default();
            if entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown") {
                entry.inferred_type = Some(
                    cast_opt
                        .map(|c| resolve_type_from_cast(&c, catalog))
                        .unwrap_or_else(|| "string".to_string()),
                );
            }
        }
        resolve_params_in_expr(rexpr, catalog, scope, param_map)?;
    }
    Ok(())
}

/// Resolves scalar-array comparisons (`ANY` and `ALL`).
pub fn resolve_scalar_array_comparison(
    lhs: &Node,
    rhs: &Node,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    let param_opt_l = extract_param_info(lhs);
    let param_opt_r = extract_param_info(rhs);

    if let Some((param_num, cast_opt)) = param_opt_r {
        // RHS is parameter: col = ANY($1) -> $1 is an array of LHS type
        let mut resolved_type = cast_opt.map(|c| {
            let lower = c.to_ascii_lowercase();
            if let Some(inner) = lower.strip_suffix("[]") {
                format!("Array<{}>", catalog.resolve_type(inner))
            } else {
                format!("Array<{}>", catalog.resolve_type(&c))
            }
        });
        let col_name_opt = extract_column_info(lhs).map(|(_, col_name)| col_name);

        if resolved_type.is_none()
            && let Ok(lhs_inf) = infer_expr(lhs, catalog, scope)
            && lhs_inf.pg_type != PgType::Unknown
        {
            let elem_ts = lhs_inf.pg_type.to_ts(catalog);
            resolved_type = Some(format!("Array<{}>", elem_ts));
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
        // LHS is parameter: $1 = ANY(arr) -> $1 is a scalar element of RHS array
        let mut resolved_type = cast_opt.map(|c| resolve_type_from_cast(&c, catalog));
        let col_name_opt = extract_column_info(rhs).map(|(_, col_name)| col_name);

        if resolved_type.is_none()
            && let Ok(rhs_inf) = infer_expr(rhs, catalog, scope)
            && let Some(elem) = rhs_inf.pg_type.element_type()
        {
            resolved_type = Some(elem.to_ts(catalog));
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

/// Resolves array operators (`&&`, `@>`, `<@`).
pub fn resolve_array_op_params(
    lexpr: &Node,
    rexpr: &Node,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    let param_opt_l = extract_param_info(lexpr);
    let param_opt_r = extract_param_info(rexpr);

    if let Some((param_num, cast_opt)) = param_opt_r {
        let mut resolved_type = cast_opt.map(|c| {
            let lower = c.to_ascii_lowercase();
            if let Some(inner) = lower.strip_suffix("[]") {
                format!("Array<{}>", catalog.resolve_type(inner))
            } else {
                format!("Array<{}>", catalog.resolve_type(&c))
            }
        });
        let col_name_opt = extract_column_info(lexpr).map(|(_, col_name)| col_name);

        if resolved_type.is_none()
            && let Ok(lhs_inf) = infer_expr(lexpr, catalog, scope)
            && lhs_inf.pg_type != PgType::Unknown
        {
            resolved_type = Some(lhs_inf.pg_type.to_ts(catalog));
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
        let mut resolved_type = cast_opt.map(|c| {
            let lower = c.to_ascii_lowercase();
            if let Some(inner) = lower.strip_suffix("[]") {
                format!("Array<{}>", catalog.resolve_type(inner))
            } else {
                format!("Array<{}>", catalog.resolve_type(&c))
            }
        });
        let col_name_opt = extract_column_info(rexpr).map(|(_, col_name)| col_name);

        if resolved_type.is_none()
            && let Ok(rhs_inf) = infer_expr(rexpr, catalog, scope)
            && rhs_inf.pg_type != PgType::Unknown
        {
            resolved_type = Some(rhs_inf.pg_type.to_ts(catalog));
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

/// Handles `IN` and `NOT IN` expressions (`AexprIn`).
pub fn handle_in_expr(
    ae: &AExpr,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    if let (Some(lexpr), Some(rexpr)) = (&ae.lexpr, &ae.rexpr) {
        let col_opt = extract_column_info(lexpr);
        let col_name_opt = col_opt.map(|(_, name)| name);

        let target_type = if let Ok(inf) = infer_expr(lexpr, catalog, scope)
            && inf.pg_type != PgType::Unknown
        {
            Some(inf.pg_type.to_ts(catalog))
        } else {
            None
        };

        if let Some(NodeEnum::List(list)) = &rexpr.node {
            for item in &list.items {
                if let Some((param_num, cast_opt)) = extract_param_info(item) {
                    let resolved = cast_opt
                        .map(|c| resolve_type_from_cast(&c, catalog))
                        .or_else(|| target_type.clone());

                    let entry = param_map.entry(param_num).or_default();
                    if entry.suggested_name.is_none()
                        && let Some(ref c) = col_name_opt
                    {
                        entry.suggested_name = Some(c.clone());
                    }
                    if entry.inferred_type.is_none()
                        || entry.inferred_type.as_deref() == Some("unknown")
                    {
                        entry.inferred_type = resolved;
                    }
                } else {
                    resolve_params_in_expr(item, catalog, scope, param_map)?;
                }
            }
        } else {
            resolve_params_in_expr(rexpr, catalog, scope, param_map)?;
        }

        resolve_params_in_expr(lexpr, catalog, scope, param_map)?;
    }
    Ok(())
}

/// Handles `BETWEEN` and range expressions.
pub fn handle_between_expr(
    ae: &AExpr,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    if let (Some(lexpr), Some(rexpr)) = (&ae.lexpr, &ae.rexpr) {
        let col_opt = extract_column_info(lexpr);
        let col_name_opt = col_opt.map(|(_, name)| name);

        let target_type = if let Ok(inf) = infer_expr(lexpr, catalog, scope)
            && inf.pg_type != PgType::Unknown
        {
            Some(inf.pg_type.to_ts(catalog))
        } else {
            None
        };

        if let Some(NodeEnum::List(list)) = &rexpr.node {
            for item in &list.items {
                if let Some((param_num, cast_opt)) = extract_param_info(item) {
                    let resolved = cast_opt
                        .map(|c| resolve_type_from_cast(&c, catalog))
                        .or_else(|| target_type.clone());

                    let entry = param_map.entry(param_num).or_default();
                    if entry.suggested_name.is_none()
                        && let Some(ref c) = col_name_opt
                    {
                        entry.suggested_name = Some(c.clone());
                    }
                    if entry.inferred_type.is_none()
                        || entry.inferred_type.as_deref() == Some("unknown")
                    {
                        entry.inferred_type = resolved;
                    }
                } else {
                    resolve_params_in_expr(item, catalog, scope, param_map)?;
                }
            }

            // If lexpr itself is a parameter ($1 BETWEEN min AND max)
            if let Some((p, cast_opt)) = extract_param_info(lexpr) {
                let inferred_bound_type = list.items.first().and_then(|item| {
                    infer_expr(item, catalog, scope)
                        .ok()
                        .filter(|inf| inf.pg_type != PgType::Unknown)
                        .map(|inf| inf.pg_type.to_ts(catalog))
                });
                let resolved = cast_opt
                    .map(|c| resolve_type_from_cast(&c, catalog))
                    .or(inferred_bound_type);
                let entry = param_map.entry(p).or_default();
                if entry.inferred_type.is_none()
                    || entry.inferred_type.as_deref() == Some("unknown")
                {
                    entry.inferred_type = resolved;
                }
            }
        } else {
            resolve_params_in_expr(rexpr, catalog, scope, param_map)?;
        }

        resolve_params_in_expr(lexpr, catalog, scope, param_map)?;
    }
    Ok(())
}

/// Deduces parameters in function calls (`FuncCall`).
pub fn deduce_func_call(
    fc: &FuncCall,
    catalog: &Catalog,
    scope: &QueryScope,
    param_map: &mut HashMap<i32, ParamInfo>,
) -> Result<(), String> {
    let func_name = extract_func_name(&fc.funcname);
    let lower_fn = func_name.to_ascii_lowercase();

    // 1. Built-in string functions
    if matches!(
        lower_fn.as_str(),
        "lower"
            | "upper"
            | "trim"
            | "ltrim"
            | "rtrim"
            | "btrim"
            | "initcap"
            | "substr"
            | "substring"
            | "concat"
            | "length"
            | "char_length"
            | "octet_length"
    ) {
        for arg in &fc.args {
            if let Some((param_num, cast_opt)) = extract_param_info(arg) {
                let resolved = cast_opt
                    .map(|c| resolve_type_from_cast(&c, catalog))
                    .unwrap_or_else(|| "string".to_string());
                let entry = param_map.entry(param_num).or_default();
                if entry.inferred_type.is_none()
                    || entry.inferred_type.as_deref() == Some("unknown")
                {
                    entry.inferred_type = Some(resolved);
                }
            } else {
                resolve_params_in_expr(arg, catalog, scope, param_map)?;
            }
        }
        return Ok(());
    }

    // 2. Built-in date functions (date_trunc, date_part)
    if matches!(lower_fn.as_str(), "date_trunc" | "date_part") {
        if let Some(arg2) = fc.args.get(1) {
            if let Some((param_num, cast_opt)) = extract_param_info(arg2) {
                let resolved = cast_opt
                    .map(|c| resolve_type_from_cast(&c, catalog))
                    .unwrap_or_else(|| "Date".to_string());
                let entry = param_map.entry(param_num).or_default();
                if entry.inferred_type.is_none()
                    || entry.inferred_type.as_deref() == Some("unknown")
                {
                    entry.inferred_type = Some(resolved);
                }
            } else {
                resolve_params_in_expr(arg2, catalog, scope, param_map)?;
            }
        }
        if let Some(arg1) = fc.args.first() {
            resolve_params_in_expr(arg1, catalog, scope, param_map)?;
        }
        return Ok(());
    }

    // 3. Coalesce
    if lower_fn == "coalesce" {
        let mut unified_ts: Option<String> = None;
        for arg in &fc.args {
            if extract_param_info(arg).is_none()
                && let Ok(inf) = infer_expr(arg, catalog, scope)
                && inf.pg_type != PgType::Unknown
            {
                unified_ts = Some(inf.pg_type.to_ts(catalog));
                break;
            }
        }
        for arg in &fc.args {
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
        return Ok(());
    }

    // 4. Function definition in catalog
    let arg_types: Vec<PgType> = fc
        .args
        .iter()
        .map(|arg| {
            infer_expr(arg, catalog, scope)
                .map(|inf| inf.pg_type)
                .unwrap_or(PgType::Unknown)
        })
        .collect();

    let fn_def = catalog.resolve_function_with_args(&func_name, &arg_types);

    for (i, arg) in fc.args.iter().enumerate() {
        if let Some((param_num, cast_opt)) = extract_param_info(arg) {
            let expected_type = fn_def.and_then(|f| {
                if i < f.params.len() {
                    Some(&f.params[i])
                } else if f.variadic && !f.params.is_empty() {
                    f.params.last()
                } else {
                    None
                }
            });

            let resolved = cast_opt
                .map(|c| resolve_type_from_cast(&c, catalog))
                .or_else(|| expected_type.map(|t| t.to_ts(catalog)));

            let entry = param_map.entry(param_num).or_default();
            if (entry.inferred_type.is_none() || entry.inferred_type.as_deref() == Some("unknown"))
                && resolved.is_some()
            {
                entry.inferred_type = resolved;
            }
        } else {
            resolve_params_in_expr(arg, catalog, scope, param_map)?;
        }
    }

    Ok(())
}
