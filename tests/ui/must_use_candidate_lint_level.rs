//@ check-pass
#![allow(clippy::must_use_candidate)]
#![deny(unfulfilled_lint_expectations)]

pub fn allowed(n: u32) -> u32 {
    n
}

#[expect(clippy::must_use_candidate)]
pub fn expected(n: u32) -> u32 {
    n
}

pub struct Example;

#[expect(clippy::must_use_candidate)]
impl Example {
    pub fn expected(&self) -> bool {
        true
    }
}

fn main() {}
