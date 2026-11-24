use clippy_utils::diagnostics::span_lint_and_then;
use clippy_utils::res::{MaybeDef as _, MaybeQPath as _};
use clippy_utils::source::SpanExt as _;
use clippy_utils::sugg::has_enclosing_paren;
use clippy_utils::{get_parent_expr, is_none_expr, last_path_segment, sym};
use rustc_ast::util::parser::{ExprPrecedence, needs_par_as_let_scrutinee};
use rustc_errors::Applicability;
use rustc_hir::{self as hir, AmbigArg};
use rustc_lint::LateContext;
use rustc_middle::ty;
use rustc_middle::ty::print::with_forced_trimmed_paths;
use rustc_span::{DesugaringKind, Span, Symbol};

use super::UNNECESSARY_LITERAL_UNWRAP;

fn get_ty_from_args<'a>(args: Option<&'a [hir::GenericArg<'a>]>, index: usize) -> Option<&'a hir::Ty<'a, AmbigArg>> {
    let args = args?;

    if args.len() <= index {
        return None;
    }

    match args[index] {
        hir::GenericArg::Type(ty) => Some(ty),
        _ => None,
    }
}

/// Group exposed struct literals and jumps before a block. Parentheses around the entire
/// expression can trigger `unused_parens` if a literal is followed by `.await` or `?`.
fn group_before_block(cx: &LateContext<'_>, value: &hir::Expr<'_>, edits: &mut Vec<(Span, String)>) {
    use hir::ExprKind as E;

    if value.span.with_source_text(cx, |s| has_enclosing_paren(s)) == Some(true) {
        return;
    }
    match value.kind {
        E::Struct(..) | E::Ret(..) | E::Break(..) | E::Yield(..) if !value.span.from_expansion() => {
            edits.push((value.span.shrink_to_lo(), "(".to_owned()));
            edits.push((value.span.shrink_to_hi(), ")".to_owned()));
        },
        E::Binary(_, lhs, rhs) | E::Assign(lhs, rhs, _) | E::AssignOp(_, lhs, rhs) => {
            group_before_block(cx, lhs, edits);
            group_before_block(cx, rhs, edits);
        },
        E::Match(call, _, hir::MatchSource::AwaitDesugar | hir::MatchSource::TryDesugar(_)) => {
            if let E::Call(_, [operand]) = call.kind {
                group_before_block(cx, operand, edits);
            }
        },
        E::Unary(_, base)
        | E::AddrOf(_, _, base)
        | E::Cast(base, _)
        | E::Field(base, _)
        | E::Index(base, ..)
        | E::Call(base, _)
        | E::MethodCall(_, base, ..) => group_before_block(cx, base, edits),
        _ => {},
    }
}

/// Remove the wrapper, preserving both the grammar before blocks and operator precedence.
fn remove_unwrap(cx: &LateContext<'_>, expr: &hir::Expr<'_>, value: &hir::Expr<'_>) -> Vec<(Span, String)> {
    use hir::ExprKind as E;

    let mut edits = vec![
        (expr.span.with_hi(value.span.lo()), String::new()),
        (expr.span.with_lo(value.span.hi()), String::new()),
    ];
    let Some(parent) = get_parent_expr(cx, expr) else {
        return edits;
    };
    let precedence = cx.precedence(value);
    let group = match parent.kind {
        E::Let(..) if needs_par_as_let_scrutinee(precedence) => true,
        E::Call(..) if parent.span.is_desugaring(DesugaringKind::ForLoop) => {
            group_before_block(cx, value, &mut edits);
            false
        },
        E::Let(..) | E::If(..) | E::DropTemps(..) | E::Match(_, _, hir::MatchSource::Normal) => {
            group_before_block(cx, value, &mut edits);
            false
        },
        _ if precedence >= ExprPrecedence::Unambiguous || precedence > cx.precedence(parent) => false,
        // Arguments and indices have delimiters. A desugared call may instead represent a
        // postfix operator such as `?`, whose operand still needs grouping.
        E::Call(base, _) | E::MethodCall(_, base, ..) | E::Index(base, ..) => {
            base.hir_id == expr.hir_id || parent.span.desugaring_kind().is_some()
        },
        // Like tuple and array elements, jump operands, closure bodies and assignment
        // right-hand sides can take an expression of equal or lower precedence.
        E::Tup(..) | E::Array(..) | E::Repeat(..) | E::Ret(_) | E::Break(..) | E::Yield(..) | E::Closure(_) => false,
        E::Assign(_, rhs, _) | E::AssignOp(_, _, rhs) => rhs.hir_id != expr.hir_id,
        _ => true,
    };
    if group {
        edits[0].1.push('(');
        edits[1].1.push(')');
    }
    edits
}

pub(super) fn check(
    cx: &LateContext<'_>,
    expr: &hir::Expr<'_>,
    recv: &hir::Expr<'_>,
    method: Symbol,
    args: &[hir::Expr<'_>],
) {
    let init = clippy_utils::expr_or_init(cx, recv);
    if init.span.from_expansion() {
        // don't lint if the receiver or binding initializer comes from a macro
        // (e.g. `let x = option_env!(..); x.unwrap()`)
        return;
    }

    let (constructor, call_args, ty) = if let hir::ExprKind::Call(call, call_args) = init.kind {
        if let Some((qpath, hir_id)) = call.opt_qpath()
            && let args = last_path_segment(qpath).args.map(|args| args.args)
            && let Some(did) = cx.qpath_res(qpath, hir_id).ctor_parent(cx).opt_def_id()
        {
            let lang_items = cx.tcx.lang_items();
            if Some(did) == lang_items.option_some_variant() {
                (sym::Some, call_args, get_ty_from_args(args, 0))
            } else if Some(did) == lang_items.result_ok_variant() {
                (sym::Ok, call_args, get_ty_from_args(args, 0))
            } else if Some(did) == lang_items.result_err_variant() {
                (sym::Err, call_args, get_ty_from_args(args, 1))
            } else {
                return;
            }
        } else {
            return;
        }
    } else if is_none_expr(cx, init) {
        let call_args: &[hir::Expr<'_>] = &[];
        (sym::None, call_args, None)
    } else {
        return;
    };

    // A diverging argument never reaches the constructor or the unwrap. Moving
    // a `break` out of the call can also change how a following block parses.
    if let [value] = call_args
        && cx.typeck_results().expr_ty(value).is_never()
    {
        return;
    }

    let help_message = format!("used `{method}()` on `{constructor}` value");
    let suggestion_message = format!("remove the `{constructor}` and `{method}()`");

    span_lint_and_then(cx, UNNECESSARY_LITERAL_UNWRAP, expr.span, help_message, |diag| {
        let suggestions = match (constructor, method, ty) {
            (sym::None, sym::unwrap, _) => Some(vec![(expr.span, "panic!()".to_string())]),
            (sym::None, sym::expect, _) => Some(vec![
                (expr.span.with_hi(args[0].span.lo()), "panic!(".to_string()),
                (expr.span.with_lo(args[0].span.hi()), ")".to_string()),
            ]),
            (sym::Some | sym::Ok, sym::unwrap_unchecked, _) | (sym::Err, sym::unwrap_err_unchecked, _) => {
                let mut suggs = vec![
                    (recv.span.with_hi(call_args[0].span.lo()), String::new()),
                    (expr.span.with_lo(call_args[0].span.hi()), String::new()),
                ];
                // try to also remove the unsafe block if present
                if let hir::Node::Block(block) = cx.tcx.parent_hir_node(expr.hir_id)
                    && let hir::BlockCheckMode::UnsafeBlock(hir::UnsafeSource::UserProvided) = block.rules
                {
                    suggs.extend([
                        (block.span.shrink_to_lo().to(expr.span.shrink_to_lo()), String::new()),
                        (expr.span.shrink_to_hi().to(block.span.shrink_to_hi()), String::new()),
                    ]);
                }
                Some(suggs)
            },
            (sym::None, sym::unwrap_or_default, _) => {
                let ty = cx.typeck_results().expr_ty(expr);
                let default_ty_string = if let ty::Adt(def, ..) = ty.kind() {
                    with_forced_trimmed_paths!(cx.tcx.def_path_str(def.did()))
                } else {
                    "Default".to_string()
                };
                Some(vec![(expr.span, format!("{default_ty_string}::default()"))])
            },
            (sym::None, sym::unwrap_or, _) => Some(vec![
                (expr.span.with_hi(args[0].span.lo()), String::new()),
                (expr.span.with_lo(args[0].span.hi()), String::new()),
            ]),
            (sym::None, sym::unwrap_or_else, _) => match args[0].kind {
                hir::ExprKind::Closure(hir::Closure { body, .. }) => Some(vec![
                    (expr.span.with_hi(cx.tcx.hir_body(*body).value.span.lo()), String::new()),
                    (expr.span.with_lo(args[0].span.hi()), String::new()),
                ]),
                _ => None,
            },
            _ if call_args.is_empty() => None,
            (_, _, Some(_)) => None,
            (sym::Ok, sym::unwrap_err, None) | (sym::Err, sym::unwrap, None) => Some(vec![
                (
                    recv.span.with_hi(call_args[0].span.lo()),
                    "panic!(\"{:?}\", ".to_string(),
                ),
                (expr.span.with_lo(call_args[0].span.hi()), ")".to_string()),
            ]),
            (sym::Ok, sym::expect_err, None) | (sym::Err, sym::expect, None) => Some(vec![
                (
                    recv.span.with_hi(call_args[0].span.lo()),
                    "panic!(\"{1}: {:?}\", ".to_string(),
                ),
                (call_args[0].span.with_lo(args[0].span.lo()), ", ".to_string()),
            ]),
            (_, _, None) => Some(remove_unwrap(cx, expr, &call_args[0])),
        };

        match (init.span == recv.span, suggestions) {
            (true, Some(suggestions)) => {
                diag.multipart_suggestion(suggestion_message, suggestions, Applicability::MachineApplicable);
            },
            _ => {
                diag.span_help(init.span, suggestion_message);
            },
        }
    });
}
