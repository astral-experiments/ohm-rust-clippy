//! Late passes whose inputs can be recorded by a module query.
//!
//! Keep this list explicit: pass fields must contain only configuration or state scoped to
//! the query, and every helper must preserve dependency reads. Raw source text, shared AST
//! collectors, process-wide query-result caches, and crate-wide traversal state need separate
//! tracking before their consumers can be added. See `book/src/development/incremental_lints.md`.

use std::sync::OnceLock;

use clippy_config::Conf;
use declare_clippy_lint::{LintCategory, LintInfo};
use rustc_data_structures::fx::FxHashSet;
use rustc_lint::{LintId, LintVec, is_lint_pass_required};
use rustc_session::config::Options;
use rustc_session::lint::Level;

// The constructors and the cached-lint set come from the same list, so a pass cannot
// accidentally run in both traversals or disappear from both.
macro_rules! audited_passes {
    (($conf:ident: $conf_ty:ty), [$($field:ident: $ty:ty = $ctor:expr,)*]) => {
        rustc_lint::late_lint_methods!(
            crate::combined_late_lint_pass,
            [IncrementalLateLintPass, ($conf: $conf_ty), [$($field: $ty = $ctor,)*]]
        );

        fn lint_ids() -> &'static FxHashSet<LintId> {
            static IDS: OnceLock<FxHashSet<LintId>> = OnceLock::new();
            IDS.get_or_init(|| {
                let mut ids = FxHashSet::default();
                $(ids.extend(<$ty>::lint_vec().into_iter().map(LintId::of));)*
                ids
            })
        }
    };
}

#[rustfmt::skip]
audited_passes!((conf: &'static Conf), [
        NeedlessPassByValue: crate::needless_pass_by_value::NeedlessPassByValue = crate::needless_pass_by_value::NeedlessPassByValue,
        RedundantClone: crate::redundant_clone::RedundantClone = crate::redundant_clone::RedundantClone,
        UnportableVariant: crate::enum_clike::UnportableVariant = crate::enum_clike::UnportableVariant,
        FloatLiteral: crate::float_literal::FloatLiteral = crate::float_literal::FloatLiteral::new(conf),
        UnitReturnExpectingOrd: crate::unit_return_expecting_ord::UnitReturnExpectingOrd = crate::unit_return_expecting_ord::UnitReturnExpectingOrd,
        ApproxConstant: crate::approx_const::ApproxConstant = crate::approx_const::ApproxConstant::new(conf),
        NeedlessQuestionMark: crate::needless_question_mark::NeedlessQuestionMark = crate::needless_question_mark::NeedlessQuestionMark,
        SizeOfInElementCount: crate::size_of_in_element_count::SizeOfInElementCount = crate::size_of_in_element_count::SizeOfInElementCount,
        NeedlessUpdate: crate::needless_update::NeedlessUpdate = crate::needless_update::NeedlessUpdate,
        NeedlessBorrowedRef: crate::needless_borrowed_ref::NeedlessBorrowedRef = crate::needless_borrowed_ref::NeedlessBorrowedRef,
        TemporaryAssignment: crate::temporary_assignment::TemporaryAssignment = crate::temporary_assignment::TemporaryAssignment,
        BoxedLocal: crate::escape::BoxedLocal = crate::escape::BoxedLocal::new(conf),
        CopyIterator: crate::copy_iterator::CopyIterator = crate::copy_iterator::CopyIterator,
        EvalOrderDependence: crate::mixed_read_write_in_expression::EvalOrderDependence = crate::mixed_read_write_in_expression::EvalOrderDependence,
        PartialEqNeImpl: crate::partialeq_ne_impl::PartialEqNeImpl = crate::partialeq_ne_impl::PartialEqNeImpl,
        InfiniteIter: crate::infinite_iter::InfiniteIter = crate::infinite_iter::InfiniteIter,
        FallibleImplFrom: crate::fallible_impl_from::FallibleImplFrom = crate::fallible_impl_from::FallibleImplFrom,
        QuestionMarkUsed: crate::question_mark_used::QuestionMarkUsed = crate::question_mark_used::QuestionMarkUsed,
        SuspiciousImpl: crate::suspicious_trait_impl::SuspiciousImpl = crate::suspicious_trait_impl::SuspiciousImpl,
        InherentToString: crate::inherent_to_string::InherentToString = crate::inherent_to_string::InherentToString,
        UnusedSelf: crate::unused_self::UnusedSelf = crate::unused_self::UnusedSelf::new(conf),
        DebugAssertWithMutCall: crate::mutable_debug_assertion::DebugAssertWithMutCall = crate::mutable_debug_assertion::DebugAssertWithMutCall,
        LargeConstArrays: crate::large_const_arrays::LargeConstArrays = crate::large_const_arrays::LargeConstArrays::new(conf),
        ExcessiveBools: crate::excessive_bools::ExcessiveBools = crate::excessive_bools::ExcessiveBools::new(conf),
        PanicInResultFn: crate::panic_in_result_fn::PanicInResultFn = crate::panic_in_result_fn::PanicInResultFn,
        EmptyDrop: crate::empty_drop::EmptyDrop = crate::empty_drop::EmptyDrop,
        ZeroSizedMapValues: crate::zero_sized_map_values::ZeroSizedMapValues = crate::zero_sized_map_values::ZeroSizedMapValues,
        SelfNamedConstructors: crate::self_named_constructors::SelfNamedConstructors = crate::self_named_constructors::SelfNamedConstructors,
        IterNotReturningIterator: crate::iter_not_returning_iterator::IterNotReturningIterator = crate::iter_not_returning_iterator::IterNotReturningIterator,
        DefaultUnionRepresentation: crate::default_union_representation::DefaultUnionRepresentation = crate::default_union_representation::DefaultUnionRepresentation,
        UnnecessaryOwnedEmptyStrings: crate::unnecessary_owned_empty_strings::UnnecessaryOwnedEmptyStrings = crate::unnecessary_owned_empty_strings::UnnecessaryOwnedEmptyStrings,
        TypeParamMismatch: crate::mismatching_type_param_order::TypeParamMismatch = crate::mismatching_type_param_order::TypeParamMismatch,
        UnusedPeekable: crate::unused_peekable::UnusedPeekable = crate::unused_peekable::UnusedPeekable,
        FromRawWithVoidPtr: crate::from_raw_with_void_ptr::FromRawWithVoidPtr = crate::from_raw_with_void_ptr::FromRawWithVoidPtr,
        SizeOfRef: crate::size_of_ref::SizeOfRef = crate::size_of_ref::SizeOfRef,
        MultipleUnsafeOpsPerBlock: crate::multiple_unsafe_ops_per_block::MultipleUnsafeOpsPerBlock = crate::multiple_unsafe_ops_per_block::MultipleUnsafeOpsPerBlock,
        CollectionIsNeverRead: crate::collection_is_never_read::CollectionIsNeverRead = crate::collection_is_never_read::CollectionIsNeverRead,
        ManualMainSeparatorStr: crate::manual_main_separator_str::ManualMainSeparatorStr = crate::manual_main_separator_str::ManualMainSeparatorStr::new(conf),
        UnnecessaryBoxReturns: crate::unnecessary_box_returns::UnnecessaryBoxReturns = crate::unnecessary_box_returns::UnnecessaryBoxReturns::new(conf),
        MissingFieldsInDebug: crate::missing_fields_in_debug::MissingFieldsInDebug = crate::missing_fields_in_debug::MissingFieldsInDebug,
        RedundantTypeAnnotations: crate::redundant_type_annotations::RedundantTypeAnnotations = crate::redundant_type_annotations::RedundantTypeAnnotations,
        ErrorImplError: crate::error_impl_error::ErrorImplError = crate::error_impl_error::ErrorImplError,
        IgnoredUnitPatterns: crate::ignored_unit_patterns::IgnoredUnitPatterns = crate::ignored_unit_patterns::IgnoredUnitPatterns,
        IterOverHashType: crate::iter_over_hash_type::IterOverHashType = crate::iter_over_hash_type::IterOverHashType,
        ImplHashWithBorrowStrBytes: crate::impl_hash_with_borrow_str_and_bytes::ImplHashWithBorrowStrBytes = crate::impl_hash_with_borrow_str_and_bytes::ImplHashWithBorrowStrBytes,
        PubUnderscoreFields: crate::pub_underscore_fields::PubUnderscoreFields = crate::pub_underscore_fields::PubUnderscoreFields::new(conf),
        ToStringTraitImpl: crate::to_string_trait_impl::ToStringTraitImpl = crate::to_string_trait_impl::ToStringTraitImpl,
        PointersInNomemAsmBlock: crate::pointers_in_nomem_asm_block::PointersInNomemAsmBlock = crate::pointers_in_nomem_asm_block::PointersInNomemAsmBlock,
        UnnecessaryLiteralBound: crate::unnecessary_literal_bound::UnnecessaryLiteralBound = crate::unnecessary_literal_bound::UnnecessaryLiteralBound,
        SingleOptionMap: crate::single_option_map::SingleOptionMap = crate::single_option_map::SingleOptionMap,
        InfallibleTryFrom: crate::infallible_try_from::InfallibleTryFrom = crate::infallible_try_from::InfallibleTryFrom,
        VolatileComposites: crate::volatile_composites::VolatileComposites = crate::volatile_composites::VolatileComposites,
        ManualNoopWaker: crate::manual_noop_waker::ManualNoopWaker = crate::manual_noop_waker::ManualNoopWaker::new(conf),
        RefPatterns: crate::ref_patterns::RefPatterns = crate::ref_patterns::RefPatterns,
]);

/// Register one combined traversal per module, retaining the usual disabled-pass fast path.
pub(crate) fn register(store: &mut rustc_lint::LintStore, conf: &'static Conf) {
    store.register_late_lint_mod_pass(Box::new(move |tcx| {
        let skippable_lints = tcx.skippable_lints(());
        Box::new(IncrementalLateLintPass::new(conf, &|lints| {
            is_lint_pass_required(skippable_lints, lints) && inputs_available(lints)
        }))
    }));
}

/// Whether a complete late pass has moved to the module traversal.
pub(crate) fn contains_pass(lints: &LintVec) -> bool {
    inputs_available(lints) && lints.iter().any(|lint| lint_ids().contains(&LintId::of(lint)))
}

/// Older compilers retain the whole-crate traversal for source-sensitive passes.
fn inputs_available(lints: &LintVec) -> bool {
    clippy_utils::source::SOURCE_FILE_INPUT.get().is_some() || !lints.iter().any(|lint| requires_source_input(lint))
}

fn requires_source_input(lint: &rustc_lint::Lint) -> bool {
    std::ptr::eq(lint, crate::needless_pass_by_value::NEEDLESS_PASS_BY_VALUE)
        || std::ptr::eq(lint, crate::redundant_clone::REDUNDANT_CLONE)
}

/// Metadata for the audited passes, used for activation and coverage checks.
pub fn lints() -> impl Iterator<Item = &'static LintInfo> {
    crate::declared_lints::LINTS
        .iter()
        .copied()
        .filter(|info| lint_ids().contains(&LintId::of(info.lint)))
}

/// Avoid hashing the driver when none of the audited lints is enabled by default or
/// the command line. Attribute-only activation retains the whole-crate traversal.
pub fn enabled(options: &Options) -> bool {
    lints().any(|info| {
        if requires_source_input(info.lint) && clippy_utils::source::SOURCE_FILE_INPUT.get().is_none() {
            return false;
        }
        let name = info.lint.name_lower();
        let group = format!("clippy::{}", info.category.name());
        let in_all = matches!(
            info.category,
            LintCategory::Correctness
                | LintCategory::Suspicious
                | LintCategory::Style
                | LintCategory::Complexity
                | LintCategory::Perf
        );
        let mut level = info.lint.default_level(options.edition);
        for (argument, requested) in &options.lint_opts {
            if argument == &name || argument == &group || (argument == "clippy::all" && in_all) {
                level = *requested;
            }
        }
        level != Level::Allow
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_activation_respects_categories_and_overrides() {
        let mut options = Options::default();
        assert!(enabled(&options));
        options.lint_opts.push(("clippy::all".into(), Level::Allow));
        assert!(!enabled(&options));
        options.lint_opts.push(("clippy::pedantic".into(), Level::Warn));
        assert!(enabled(&options));
        options.lint_opts.push(("clippy::pedantic".into(), Level::Allow));
        assert!(!enabled(&options));
        options.lint_opts.push(("clippy::unused_self".into(), Level::Warn));
        assert!(enabled(&options));
        options.lint_opts.push(("clippy::unused_self".into(), Level::Allow));
        assert!(!enabled(&options));
    }
}
