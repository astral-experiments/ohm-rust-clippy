use clippy_utils::diagnostics::span_lint_hir_and_then;
use clippy_utils::source::snippet;
use rustc_data_structures::fx::FxHashSet;
use rustc_errors::Applicability;
use rustc_hir::def::{DefKind, Res};
use rustc_hir::{self as hir, AmbigArg, find_attr};
use rustc_lint::{LateContext, LateLintPass, LintContext as _};
use rustc_session::impl_lint_pass;
use rustc_span::edition::Edition;
use rustc_span::{Span, SyntaxContext};
use std::collections::BTreeMap;

declare_clippy_lint! {
    /// ### What it does
    /// Checks for `#[macro_use] use...`.
    ///
    /// ### Why is this bad?
    /// Since the Rust 2018 edition you can import
    /// macro's directly, this is considered idiomatic.
    ///
    /// ### Example
    /// ```rust,ignore
    /// #[macro_use]
    /// extern crate some_crate;
    ///
    /// fn main() {
    ///     some_macro!();
    /// }
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// use some_crate::some_macro;
    ///
    /// fn main() {
    ///     some_macro!();
    /// }
    /// ```
    #[clippy::version = "1.44.0"]
    pub MACRO_USE_IMPORTS,
    pedantic,
    "#[macro_use] is no longer needed"
}

impl_lint_pass!(MacroUseImports => [MACRO_USE_IMPORTS]);

#[derive(Default)]
pub struct MacroUseImports {
    /// the actual import path used and the span of the attribute above it. The value is
    /// the location, where the lint should be emitted.
    imports: Vec<(String, Span, hir::HirId)>,
    expansions: FxHashSet<SyntaxContext>,
    /// The first visit selects how the lint spells each macro reference.
    references: Vec<(Span, bool)>,
}

impl MacroUseImports {
    fn record_reference(&mut self, span: Span, strip_path: bool) {
        let context = span.ctxt();
        // For a non-root context the call site comes entirely from the
        // expansion chain. Preserve its first visit and the original spelling.
        if !context.is_root() && self.expansions.insert(context) {
            self.references.push((span, strip_path));
        }
    }

    fn macro_names(&self, cx: &LateContext<'_>) -> Vec<String> {
        // Different inner expansions can still have the same outer call site.
        let mut call_sites = FxHashSet::default();
        let mut names = Vec::new();
        for &(span, strip_path) in &self.references {
            let call_site = span.source_callsite();
            if call_sites.insert(call_site) {
                let name = snippet(cx, cx.sess().source_map().span_until_char(call_site, '!'), "_");
                let name = if strip_path {
                    name.rsplit("::").next().unwrap()
                } else {
                    &name
                };
                names.push(name.to_owned());
            }
        }
        names
    }
}

impl LateLintPass<'_> for MacroUseImports {
    fn check_item(&mut self, cx: &LateContext<'_>, item: &hir::Item<'_>) {
        if cx.sess().opts.edition >= Edition::Edition2018
            && let hir::ItemKind::Use(path, _kind) = &item.kind
            && let hir_id = item.hir_id()
            && let attrs = cx.tcx.hir_attrs(hir_id)
            && let Some(mac_attr_span) = find_attr!(attrs, MacroUse {span, ..} => *span)
            && let Some(Res::Def(DefKind::Mod, id)) = path.res.type_ns
            && !id.is_local()
        {
            for kid in cx.tcx.module_children(id) {
                if let Res::Def(DefKind::Macro(_mac_type), mac_id) = kid.res {
                    let def_path = cx.tcx.def_path_str(mac_id);
                    self.imports.push((def_path, mac_attr_span, hir_id));
                }
            }
        } else {
            self.record_reference(item.span, false);
        }
    }
    fn check_expr(&mut self, _: &LateContext<'_>, expr: &hir::Expr<'_>) {
        self.record_reference(expr.span, true);
    }
    fn check_stmt(&mut self, _: &LateContext<'_>, stmt: &hir::Stmt<'_>) {
        self.record_reference(stmt.span, true);
    }
    fn check_pat(&mut self, _: &LateContext<'_>, pat: &hir::Pat<'_>) {
        self.record_reference(pat.span, false);
    }
    fn check_ty(&mut self, _: &LateContext<'_>, ty: &hir::Ty<'_, AmbigArg>) {
        self.record_reference(ty.span, false);
    }
    fn check_crate_post(&mut self, cx: &LateContext<'_>) {
        if self.imports.is_empty() {
            return;
        }
        let mut mac_refs = self.macro_names(cx);
        let mut used = BTreeMap::new();
        let mut check_dup = vec![];
        for (import, span, hir_id) in &self.imports {
            let found_idx = mac_refs.iter().position(|name| import.ends_with(name.as_str()));

            if let Some(idx) = found_idx {
                mac_refs.remove(idx);
                let seg = import.split("::").collect::<Vec<_>>();

                match seg.as_slice() {
                    // an empty path is impossible
                    // a path should always consist of 2 or more segments
                    [] | [_] => return,
                    [root, item] => {
                        if !check_dup.contains(&(*item).to_string()) {
                            used.entry((
                                (*root).to_string(),
                                span,
                                hir_id.local_id,
                                cx.tcx.def_path_hash(hir_id.owner.def_id.into()),
                            ))
                            .or_insert_with(|| (vec![], hir_id))
                            .0
                            .push((*item).to_string());
                            check_dup.push((*item).to_string());
                        }
                    },
                    [root, rest @ ..] => {
                        if rest.iter().all(|item| !check_dup.contains(&(*item).to_string())) {
                            let filtered = rest
                                .iter()
                                .filter_map(|item| {
                                    if check_dup.contains(&(*item).to_string()) {
                                        None
                                    } else {
                                        Some((*item).to_string())
                                    }
                                })
                                .collect::<Vec<_>>();
                            used.entry((
                                (*root).to_string(),
                                span,
                                hir_id.local_id,
                                cx.tcx.def_path_hash(hir_id.owner.def_id.into()),
                            ))
                            .or_insert_with(|| (vec![], hir_id))
                            .0
                            .push(filtered.join("::"));
                            check_dup.extend(filtered);
                        } else {
                            let rest = rest.to_vec();
                            used.entry((
                                (*root).to_string(),
                                span,
                                hir_id.local_id,
                                cx.tcx.def_path_hash(hir_id.owner.def_id.into()),
                            ))
                            .or_insert_with(|| (vec![], hir_id))
                            .0
                            .push(rest.join("::"));
                            check_dup.extend(rest.iter().map(ToString::to_string));
                        }
                    },
                }
            }
        }

        // If mac_refs is not empty we have encountered an import we could not handle
        // such as `std::prelude::v1::foo` or some other macro that expands to an import.
        if mac_refs.is_empty() {
            for ((root, span, ..), (path, hir_id)) in used {
                let import = if let [single] = &path[..] {
                    format!("{root}::{single}")
                } else {
                    format!("{root}::{{{}}}", path.join(", "))
                };

                span_lint_hir_and_then(
                    cx,
                    MACRO_USE_IMPORTS,
                    *hir_id,
                    *span,
                    "`macro_use` attributes are no longer needed in the Rust 2018 edition",
                    |diag| {
                        diag.span_suggestion(
                            *span,
                            "remove the attribute and import the macro directly, try",
                            format!("use {import};"),
                            Applicability::MaybeIncorrect,
                        );
                    },
                );
            }
        }
    }
}
