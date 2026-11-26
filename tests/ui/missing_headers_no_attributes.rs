//@ check-pass
#![deny(unfulfilled_lint_expectations)]

#[expect(clippy::missing_safety_doc)]
pub mod empty_attributes {
    pub unsafe fn no_docs() {}
}

fn main() {}
