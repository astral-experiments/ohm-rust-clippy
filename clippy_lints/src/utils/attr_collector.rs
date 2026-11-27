use std::mem;
use std::sync::{Arc, OnceLock};

use clippy_utils::sym;
use rustc_ast::{Attribute, Crate};
use rustc_lint::{EarlyContext, EarlyLintPass, impl_lint_pass};
use rustc_span::Span;

#[derive(Clone, Default)]
pub struct AttrStorage(pub Arc<OnceLock<CollectedAttrs>>);

#[derive(Debug, Default)]
pub struct CollectedAttrs {
    pub spans: Vec<Span>,
    pub has_debug_attrs: bool,
}

pub struct AttrCollector {
    storage: AttrStorage,
    attrs: CollectedAttrs,
}

impl AttrCollector {
    pub fn new(storage: AttrStorage) -> Self {
        Self {
            storage,
            attrs: CollectedAttrs::default(),
        }
    }
}

impl_lint_pass!(AttrCollector => []);

impl EarlyLintPass for AttrCollector {
    fn check_attribute(&mut self, _cx: &EarlyContext<'_>, attr: &Attribute) {
        self.attrs.spans.push(attr.span);
        if attr.path_matches(&[sym::clippy, sym::dump]) || attr.path_matches(&[sym::clippy, sym::author]) {
            self.attrs.has_debug_attrs = true;
        }
    }

    fn check_crate_post(&mut self, _: &EarlyContext<'_>, _: &Crate) {
        self.storage
            .0
            .set(mem::take(&mut self.attrs))
            .expect("should only be called once");
    }
}
