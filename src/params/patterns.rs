//! Optional Dynamic Filter Pattern Matcher
//!
//! Recognizes PostgreSQL dynamic filtering idioms such as:
//! - `($1 IS NULL OR col = $1)`
//! - `(col = $1 OR $1 IS NULL)`
//! - `($1 IS NULL OR col LIKE $1)`
//! - `(col LIKE $1 OR $1 IS NULL)`
//! - `($1 IS NULL OR col ILIKE $1)`
//! - `(col ILIKE $1 OR $1 IS NULL)`
//! - `($1 IS NULL OR col = ANY($1))`
//! - `(col = ANY($1) OR $1 IS NULL)`
//! - `($1::type IS NULL OR col = $1)`
//! - `($1 IS NULL OR lower(col) = lower($1))`

use crate::analyzer::{QueryScope, extract_func_name, extract_string};
use crate::catalog::Catalog;
use crate::params::{extract_column_info, extract_param_info};
use pg_query::NodeEnum;
use pg_query::protobuf::{AExpr, AExprKind, BoolExpr, BoolExprType, Node, NullTestType};

/// Describes a successful match of an optional dynamic filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionalFilterMatch {
    pub param_num: i32,
    pub suggested_name: Option<String>,
    pub inferred_type: Option<String>,
}

/// Matches dynamic filter idioms in a boolean OR expression.
pub fn match_optional_filter(
    be: &BoolExpr,
    catalog: &Catalog,
    scope: &QueryScope,
) -> Option<OptionalFilterMatch> {
    if be.boolop != BoolExprType::OrExpr as i32 || be.args.len() != 2 {
        return None;
    }

    let arm0 = &be.args[0];
    let arm1 = &be.args[1];

    // Orientation 1: arm0 is NullTest($N), arm1 is predicate testing $N
    if let Some((p_null, cast_null)) = match_null_test_param(arm0)
        && let Some((col_name, col_type)) =
            inspect_predicate_for_param(arm1, p_null, cast_null.as_deref(), catalog, scope)
    {
        return Some(OptionalFilterMatch {
            param_num: p_null,
            suggested_name: col_name,
            inferred_type: col_type,
        });
    }

    // Orientation 2: arm1 is NullTest($N), arm0 is predicate testing $N
    if let Some((p_null, cast_null)) = match_null_test_param(arm1)
        && let Some((col_name, col_type)) =
            inspect_predicate_for_param(arm0, p_null, cast_null.as_deref(), catalog, scope)
    {
        return Some(OptionalFilterMatch {
            param_num: p_null,
            suggested_name: col_name,
            inferred_type: col_type,
        });
    }

    None
}

fn match_null_test_param(node: &Node) -> Option<(i32, Option<String>)> {
    if let Some(NodeEnum::NullTest(nt)) = &node.node
        && nt.nulltesttype == NullTestType::IsNull as i32
        && let Some(arg) = &nt.arg
    {
        extract_param_info(arg)
    } else {
        None
    }
}

fn inspect_predicate_for_param(
    node: &Node,
    target_param: i32,
    null_cast: Option<&str>,
    catalog: &Catalog,
    scope: &QueryScope,
) -> Option<(Option<String>, Option<String>)> {
    match &node.node {
        Some(NodeEnum::AExpr(ae)) => {
            // 1. Equality: col = $N or $N = col, or symmetrical function lower(col) = lower($N)
            if ae.kind == AExprKind::AexprOp as i32 {
                let op = ae.name.first().and_then(extract_string).unwrap_or_default();
                if op == "=" {
                    return match_binary_equality_or_sym_func(
                        ae,
                        target_param,
                        null_cast,
                        catalog,
                        scope,
                    );
                } else if matches!(op.as_str(), "~~" | "~~*" | "!~~" | "!~~*") {
                    return match_pattern_op(ae, target_param);
                }
            }

            // 2. Pattern matching: col LIKE $N, col ILIKE $N, col SIMILAR TO $N
            if ae.kind == AExprKind::AexprLike as i32
                || ae.kind == AExprKind::AexprIlike as i32
                || ae.kind == AExprKind::AexprSimilar as i32
            {
                return match_pattern_op(ae, target_param);
            }

            // 3. Array membership: col = ANY($N)
            if ae.kind == AExprKind::AexprOpAny as i32 || ae.kind == AExprKind::AexprOpAll as i32 {
                return match_scalar_array_any(ae, target_param, catalog, scope);
            }

            None
        }
        Some(NodeEnum::ScalarArrayOpExpr(saoe)) => {
            if saoe.args.len() == 2 {
                let lhs = &saoe.args[0];
                let rhs = &saoe.args[1];
                if let Some((p, _)) = extract_param_info(rhs)
                    && p == target_param
                {
                    let (alias_opt, col_name) = extract_column_info(lhs).unzip();
                    let col_name_str = col_name?;
                    let elem_ts = resolve_col_type(
                        alias_opt.flatten().as_deref(),
                        &col_name_str,
                        catalog,
                        scope,
                    )?;
                    return Some((Some(col_name_str), Some(format!("Array<{}>", elem_ts))));
                }
            }
            None
        }
        _ => None,
    }
}

fn match_binary_equality_or_sym_func(
    ae: &AExpr,
    target_param: i32,
    null_cast: Option<&str>,
    catalog: &Catalog,
    scope: &QueryScope,
) -> Option<(Option<String>, Option<String>)> {
    let lexpr = ae.lexpr.as_ref()?;
    let rexpr = ae.rexpr.as_ref()?;

    // Check symmetrical function: lower(col) = lower($N) or lower($N) = lower(col)
    if let (Some(NodeEnum::FuncCall(fc_l)), Some(NodeEnum::FuncCall(fc_r))) =
        (&lexpr.node, &rexpr.node)
    {
        let fn_l = extract_func_name(&fc_l.funcname);
        let fn_r = extract_func_name(&fc_r.funcname);
        if fn_l.eq_ignore_ascii_case(&fn_r) && fc_l.args.len() == 1 && fc_r.args.len() == 1 {
            let arg_l = &fc_l.args[0];
            let arg_r = &fc_r.args[0];
            if let Some((p, _)) = extract_param_info(arg_r)
                && p == target_param
                && let Some((alias_opt, col_name)) = extract_column_info(arg_l)
            {
                let ts_type = resolve_col_type(alias_opt.as_deref(), &col_name, catalog, scope);
                return Some((Some(col_name), ts_type));
            } else if let Some((p, _)) = extract_param_info(arg_l)
                && p == target_param
                && let Some((alias_opt, col_name)) = extract_column_info(arg_r)
            {
                let ts_type = resolve_col_type(alias_opt.as_deref(), &col_name, catalog, scope);
                return Some((Some(col_name), ts_type));
            }
        }
    }

    let param_l = extract_param_info(lexpr);
    let param_r = extract_param_info(rexpr);
    let col_l = extract_column_info(lexpr);
    let col_r = extract_column_info(rexpr);

    if let (Some((p, cast_r)), Some((alias_l, col_name_l))) = (param_r, col_l) {
        if p == target_param {
            let cast = cast_r.or_else(|| null_cast.map(|s| s.to_string()));
            let ts_type = cast
                .map(|c| catalog.resolve_type(&c))
                .or_else(|| resolve_col_type(alias_l.as_deref(), &col_name_l, catalog, scope));
            return Some((Some(col_name_l), ts_type));
        }
    } else if let (Some((p, cast_l)), Some((alias_r, col_name_r))) = (param_l, col_r)
        && p == target_param
    {
        let cast = cast_l.or_else(|| null_cast.map(|s| s.to_string()));
        let ts_type = cast
            .map(|c| catalog.resolve_type(&c))
            .or_else(|| resolve_col_type(alias_r.as_deref(), &col_name_r, catalog, scope));
        return Some((Some(col_name_r), ts_type));
    }

    None
}

fn match_pattern_op(ae: &AExpr, target_param: i32) -> Option<(Option<String>, Option<String>)> {
    let lexpr = ae.lexpr.as_ref()?;
    let rexpr = ae.rexpr.as_ref()?;

    let param_l = extract_param_info(lexpr);
    let param_r = extract_param_info(rexpr);
    let col_l = extract_column_info(lexpr);
    let col_r = extract_column_info(rexpr);

    if let Some((p, _)) = param_r
        && p == target_param
    {
        let col_name = col_l.map(|(_, name)| name);
        return Some((col_name, Some("string".to_string())));
    }
    if let Some((p, _)) = param_l
        && p == target_param
    {
        let col_name = col_r.map(|(_, name)| name);
        return Some((col_name, Some("string".to_string())));
    }

    None
}

fn match_scalar_array_any(
    ae: &AExpr,
    target_param: i32,
    catalog: &Catalog,
    scope: &QueryScope,
) -> Option<(Option<String>, Option<String>)> {
    let lexpr = ae.lexpr.as_ref()?;
    let rexpr = ae.rexpr.as_ref()?;

    if let Some((p, _)) = extract_param_info(rexpr)
        && p == target_param
        && let Some((alias_opt, col_name)) = extract_column_info(lexpr)
    {
        let elem_ts = resolve_col_type(alias_opt.as_deref(), &col_name, catalog, scope)?;
        return Some((Some(col_name), Some(format!("Array<{}>", elem_ts))));
    }

    None
}

fn resolve_col_type(
    alias: Option<&str>,
    col_name: &str,
    catalog: &Catalog,
    scope: &QueryScope,
) -> Option<String> {
    if let Some(a) = alias {
        let binding = scope.bindings.get(&a.to_ascii_lowercase())?;
        let col = binding.get_column(col_name)?;
        Some(catalog.resolve_type(&col.pg_type))
    } else {
        for name in &scope.binding_order {
            if let Some(binding) = scope.bindings.get(&name.to_ascii_lowercase())
                && let Some(col) = binding.get_column(col_name)
            {
                return Some(catalog.resolve_type(&col.pg_type));
            }
        }
        None
    }
}
