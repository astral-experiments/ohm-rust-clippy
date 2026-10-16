# Incremental late lints

Ohm can run selected late passes through rustc's `lint_mod` queries. Unchanged
modules replay diagnostics through the query cache, including suggestions and
lint expectations. Other passes retain the combined whole-crate traversal.

The explicit list in `clippy_lints/src/incremental_lints.rs` generates both the
module pass and its lint identities. A pass must move in its entirety. It must
not depend on traversal order across modules, and all of its inputs must be
tracked by compiler queries or the driver's compatibility fingerprint.

Statelessness alone is insufficient. Audit helpers transitively, particularly:

- Source snippets and source-map reads, including suggestion applicability.
- Process-local caches that can omit dependency reads on a hit, such as path
  lookup and test-item caches.
- Early-pass collectors and state accumulated across modules.
- Constructors that emit diagnostics or depend on whole-crate state.
- Inherited lint levels, MSRV attributes, and diagnostic expectations.

Module passes bypass `first_node_in_macro`'s single-entry memo so their queries
record parent-HIR dependencies. `Msrv::current` also queries inherited attributes
in module mode even when the process has not yet seen an MSRV attribute.
Whole-crate passes retain those fast paths.

The driver enables module caching only with incremental compilation, compiler
support for the driver input fingerprint, and a verified executable/configuration
snapshot. The executable, complete configuration, Clippy arguments, Cargo MSRV,
and diagnostic-link settings invalidate the existing query graph when changed.
If no selected lint is enabled by default or on the command line, the driver skips
fingerprinting; attribute-only activation then uses the whole-crate pass.

Before adding a pass, add positive coverage for every lint it declares to
`tests/incremental_lint_cases.json` and focused invalidation cases for its inputs.
The native integration tests compare complete diagnostics against the uncached
whole-crate traversal at one and four compiler threads. The combined fixture
also compares metadata byte-for-byte between module and whole-crate traversals
with matching cold and warm edit histories. This accounts for an existing
released-compiler difference between cold and warm metadata after whitespace-only
edits without exempting any output bytes. The focused fixtures additionally
require cold and warm metadata to match. Merely loading an incremental session is not
evidence that a pass is safe or that it improves an edit workload.
