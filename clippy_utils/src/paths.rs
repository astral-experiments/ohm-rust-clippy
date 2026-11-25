//! This module contains paths to types and functions Clippy needs to know
//! about.
//!
//! Whenever possible, please consider diagnostic items over hardcoded paths.
//! See <https://github.com/rust-lang/rust-clippy/issues/5393> for more information.

use crate::res::MaybeQPath;
use crate::sym;
use rustc_ast::Mutability;
use rustc_data_structures::fx::FxHashMap;
use rustc_hir::def::Namespace::{MacroNS, TypeNS, ValueNS};
use rustc_hir::def::{DefKind, Namespace, Res};
use rustc_hir::def_id::{CrateNum, DefId, LOCAL_CRATE, LocalDefId};
use rustc_hir::{ItemKind, Node, UseKind, UseTree};
use rustc_lint::LateContext;
use rustc_metadata::creader::CStore;
use rustc_middle::middle::resolve::ModChild;
use rustc_middle::ty::fast_reject::SimplifiedType;
use rustc_middle::ty::layout::HasTyCtxt;
use rustc_middle::ty::{FloatTy, IntTy, Ty, TyCtxt, UintTy};
use rustc_span::{Ident, STDLIB_STABLE_CRATES, Symbol};
use std::sync::OnceLock;

/// Specifies whether to resolve a path in the [`TypeNS`], [`ValueNS`], [`MacroNS`] or in an
/// arbitrary namespace
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PathNS {
    Type,
    Value,
    Macro,
    Field,

    /// Resolves to the name in the first available namespace, e.g. for `std::vec` this would return
    /// either the macro or the module but **not** both
    ///
    /// Must only be used when the specific resolution is unimportant such as in
    /// `missing_enforced_import_renames`
    Arbitrary,
}

impl PathNS {
    fn matches(self, ns: Option<Namespace>) -> bool {
        let required = match self {
            PathNS::Type => TypeNS,
            PathNS::Value => ValueNS,
            PathNS::Macro => MacroNS,
            PathNS::Field => return false,
            PathNS::Arbitrary => return true,
        };

        ns == Some(required)
    }
}

/// Lazily resolves a path into a list of [`DefId`]s using [`lookup_path`].
///
/// Typically it will contain one [`DefId`] or none, but in some situations there can be multiple:
/// - `memchr::memchr` could return the functions from both memchr 1.0 and memchr 2.0
/// - `alloc::boxed::Box::downcast` would return a function for each of the different inherent impls ([1], [2], [3])
///
/// [1]: https://doc.rust-lang.org/std/boxed/struct.Box.html#method.downcast
/// [2]: https://doc.rust-lang.org/std/boxed/struct.Box.html#method.downcast-1
/// [3]: https://doc.rust-lang.org/std/boxed/struct.Box.html#method.downcast-2
pub struct PathLookup {
    ns: PathNS,
    path: &'static [Symbol],
    once: OnceLock<Vec<DefId>>,
}

impl PathLookup {
    /// Only exported for tests and `clippy_lints_internal`
    #[doc(hidden)]
    pub const fn new(ns: PathNS, path: &'static [Symbol]) -> Self {
        Self {
            ns,
            path,
            once: OnceLock::new(),
        }
    }

    /// Returns the list of [`DefId`]s that the path resolves to
    pub fn get<'tcx>(&self, tcx: &impl HasTyCtxt<'tcx>) -> &[DefId] {
        self.once.get_or_init(|| lookup_path(tcx.tcx(), self.ns, self.path))
    }

    /// Returns the single [`DefId`] that the path resolves to, this can only be used for paths into
    /// stdlib crates to avoid the issue of multiple [`DefId`]s being returned
    ///
    /// May return [`None`] in `no_std`/`no_core` environments
    pub fn only(&self, cx: &LateContext<'_>) -> Option<DefId> {
        let ids = self.get(cx);
        debug_assert!(STDLIB_STABLE_CRATES.contains(&self.path[0]));
        debug_assert!(ids.len() <= 1, "{ids:?}");
        ids.first().copied()
    }

    /// Checks if the path resolves to the given `def_id`
    pub fn matches<'tcx>(&self, tcx: &impl HasTyCtxt<'tcx>, def_id: DefId) -> bool {
        self.get(&tcx.tcx()).contains(&def_id)
    }

    /// Resolves `maybe_path` to a [`DefId`] and checks if the [`PathLookup`] matches it
    pub fn matches_path<'tcx>(&self, cx: &LateContext<'_>, maybe_path: impl MaybeQPath<'tcx>) -> bool {
        maybe_path
            .res(cx)
            .opt_def_id()
            .is_some_and(|def_id| self.matches(cx, def_id))
    }

    /// Checks if the path resolves to `ty`'s definition, must be an `Adt`
    pub fn matches_ty<'tcx>(&self, tcx: &impl HasTyCtxt<'tcx>, ty: Ty<'_>) -> bool {
        ty.ty_adt_def().is_some_and(|adt| self.matches(&tcx.tcx(), adt.did()))
    }
}

macro_rules! path_macros {
    ($($name:ident: $ns:expr,)*) => {
        $(
            /// Only exported for tests and `clippy_lints_internal`
            #[doc(hidden)]
            #[macro_export]
            macro_rules! $name {
                ($$($$seg:ident $$(::)?)*) => {
                    PathLookup::new($ns, &[$$(sym::$$seg,)*])
                };
            }
        )*
    };
}

path_macros! {
    type_path: PathNS::Type,
    value_path: PathNS::Value,
    macro_path: PathNS::Macro,
}

// Paths in the standard library missing a diagnostic item

// Paths in external crates
pub static FUTURES_IO_ASYNCREADEXT: PathLookup = type_path!(futures_util::AsyncReadExt);
pub static FUTURES_IO_ASYNCWRITEEXT: PathLookup = type_path!(futures_util::AsyncWriteExt);
pub static ITERTOOLS_NEXT_TUPLE: PathLookup = value_path!(itertools::Itertools::next_tuple);
// Use the public re-exports because `tcx.module_children` can omit private modules.
pub static PARKING_LOT_GUARDS: [PathLookup; 3] = [
    type_path!(lock_api::MutexGuard),
    type_path!(lock_api::RwLockReadGuard),
    type_path!(lock_api::RwLockWriteGuard),
];
pub static REGEX_BUILDER_NEW: PathLookup = value_path!(regex::RegexBuilder::new);
pub static REGEX_BYTES_BUILDER_NEW: PathLookup = value_path!(regex::bytes::RegexBuilder::new);
pub static REGEX_BYTES_NEW: PathLookup = value_path!(regex::bytes::Regex::new);
pub static REGEX_BYTES_SET_NEW: PathLookup = value_path!(regex::bytes::RegexSet::new);
pub static REGEX_NEW: PathLookup = value_path!(regex::Regex::new);
pub static REGEX_SET_NEW: PathLookup = value_path!(regex::RegexSet::new);
pub static SERDE_DESERIALIZE: PathLookup = type_path!(serde::de::Deserialize);
pub static SERDE_DE_VISITOR: PathLookup = type_path!(serde::de::Visitor);
pub static TOKIO_FILE_OPTIONS: PathLookup = value_path!(tokio::fs::File::options);
pub static TOKIO_IO_ASYNCREADEXT: PathLookup = type_path!(tokio::io::AsyncReadExt);
pub static TOKIO_IO_ASYNCWRITEEXT: PathLookup = type_path!(tokio::io::AsyncWriteExt);
pub static TOKIO_IO_OPEN_OPTIONS: PathLookup = type_path!(tokio::fs::OpenOptions);
pub static TOKIO_IO_OPEN_OPTIONS_NEW: PathLookup = value_path!(tokio::fs::OpenOptions::new);
pub static LAZY_STATIC: PathLookup = macro_path!(lazy_static::lazy_static);
pub static ONCE_CELL_SYNC_LAZY: PathLookup = type_path!(once_cell::sync::Lazy);
pub static ONCE_CELL_SYNC_LAZY_NEW: PathLookup = value_path!(once_cell::sync::Lazy::new);

// Paths for internal lints go in `clippy_lints_internal/src/internal_paths.rs`

/// Resolve a path string, including the original module path of a re-exported item.
///
/// This function is expensive and should be used sparingly.
pub fn lookup_path_str(tcx: TyCtxt<'_>, ns: PathNS, path: &str) -> Vec<DefId> {
    let path: Vec<Symbol> = path.split("::").map(Symbol::intern).collect();
    lookup_path_impl(tcx, ns, &path, Search::Imports, |item| item.id)
}

/// Resolves a def path like `std::vec::Vec`.
///
/// Typically it will return one [`DefId`] or none, but in some situations there can be multiple:
/// - `memchr::memchr` could return the functions from both memchr 1.0 and memchr 2.0
/// - `alloc::boxed::Box::downcast` would return a function for each of the different inherent impls ([1], [2], [3])
///
/// This function is expensive and should be used sparingly.
///
/// [1]: https://doc.rust-lang.org/std/boxed/struct.Box.html#method.downcast
/// [2]: https://doc.rust-lang.org/std/boxed/struct.Box.html#method.downcast-1
/// [3]: https://doc.rust-lang.org/std/boxed/struct.Box.html#method.downcast-2
pub fn lookup_path(tcx: TyCtxt<'_>, ns: PathNS, path: &[Symbol]) -> Vec<DefId> {
    lookup_path_impl(tcx, ns, path, Search::Public, |item| item.id)
}

/// A definition named in a lint setting.
///
/// An omitted declaration and its children can change without changing the public hash.
/// For those lookups, read from the current metadata instead of caching query results
/// against that hash. The metadata file is also an input in Cargo's dep-info.
#[derive(Clone, Copy)]
pub struct ConfiguredItem {
    /// The definition to match at uses of the configured name.
    pub id: DefId,
    private: bool,
}

impl ConfiguredItem {
    /// Read an omitted definition from this session's metadata.
    pub fn kind(self, tcx: TyCtxt<'_>) -> DefKind {
        if self.private {
            CStore::from_tcx(tcx).def_kind_untracked(self.id)
        } else {
            tcx.def_kind(self.id)
        }
    }

    /// Expand a configured type alias with its generic parameters intact. A private alias
    /// can change its target even when incremental queries for the dependency are green.
    pub fn alias_ty(self, tcx: TyCtxt<'_>) -> Option<Ty<'_>> {
        if self.kind(tcx) != DefKind::TyAlias {
            return None;
        }
        let ty = if self.private {
            (tcx.query_system.extern_providers.type_of)(tcx, self.id)
        } else {
            tcx.type_of(self.id)
        };
        Some(ty.instantiate_identity().skip_norm_wip())
    }

    /// Describe a mismatched setting without caching queries about private declarations.
    pub fn article_and_description(self, tcx: TyCtxt<'_>) -> (&'static str, &'static str) {
        if !self.private {
            return tcx.article_and_description(self.id);
        }
        let kind = self.kind(tcx);
        if kind == DefKind::AssocFn && (tcx.query_system.extern_providers.associated_item)(tcx, self.id).is_method() {
            ("a", "method")
        } else {
            (kind.article(), kind.descr(self.id))
        }
    }
}

/// Resolve a lint setting that can name private items and imports.
pub fn lookup_configured_path(tcx: TyCtxt<'_>, ns: PathNS, path: &[Symbol]) -> Vec<ConfiguredItem> {
    lookup_path_impl(tcx, ns, path, Search::Declarations, |item| item)
}

/// How far to look beyond the compiler's regular module children.
#[derive(Clone, Copy, PartialEq)]
enum Search {
    Public,
    Imports,
    Declarations,
}

/// Resolve each crate version separately, including versions with different public exports.
fn lookup_path_impl<T>(
    tcx: TyCtxt<'_>,
    ns: PathNS,
    path: &[Symbol],
    search: Search,
    select: impl Fn(ConfiguredItem) -> T,
) -> Vec<T> {
    let (root, rest) = match *path {
        [] | [_] => return Vec::new(),
        [root, ref rest @ ..] => (root, rest),
    };

    let mut out = Vec::new();
    for &base in find_crates(tcx, root).iter().chain(find_primitive_impls(tcx, root)) {
        lookup_with_base(tcx, base, ns, rest, search, |item| out.push(select(item)));
    }
    out
}

/// Finds the crates called `name`, may be multiple due to multiple major versions.
pub fn find_crates(tcx: TyCtxt<'_>, name: Symbol) -> &'static [DefId] {
    static BY_NAME: OnceLock<FxHashMap<Symbol, Vec<DefId>>> = OnceLock::new();
    let map = BY_NAME.get_or_init(|| {
        let mut map = FxHashMap::default();
        map.insert(tcx.crate_name(LOCAL_CRATE), vec![LOCAL_CRATE.as_def_id()]);
        for &num in tcx.crates(()) {
            map.entry(tcx.crate_name(num)).or_default().push(num.as_def_id());
        }
        map
    });
    match map.get(&name) {
        Some(def_ids) => def_ids,
        None => &[],
    }
}

fn find_primitive_impls(tcx: TyCtxt<'_>, name: Symbol) -> &[DefId] {
    let ty = match name {
        sym::bool => SimplifiedType::Bool,
        sym::char => SimplifiedType::Char,
        sym::str => SimplifiedType::Str,
        sym::array => SimplifiedType::Array,
        sym::slice => SimplifiedType::Slice,
        // FIXME: rustdoc documents these two using just `pointer`.
        //
        // Maybe this is something we should do here too.
        sym::const_ptr => SimplifiedType::Ptr(Mutability::Not),
        sym::mut_ptr => SimplifiedType::Ptr(Mutability::Mut),
        sym::isize => SimplifiedType::Int(IntTy::Isize),
        sym::i8 => SimplifiedType::Int(IntTy::I8),
        sym::i16 => SimplifiedType::Int(IntTy::I16),
        sym::i32 => SimplifiedType::Int(IntTy::I32),
        sym::i64 => SimplifiedType::Int(IntTy::I64),
        sym::i128 => SimplifiedType::Int(IntTy::I128),
        sym::usize => SimplifiedType::Uint(UintTy::Usize),
        sym::u8 => SimplifiedType::Uint(UintTy::U8),
        sym::u16 => SimplifiedType::Uint(UintTy::U16),
        sym::u32 => SimplifiedType::Uint(UintTy::U32),
        sym::u64 => SimplifiedType::Uint(UintTy::U64),
        sym::u128 => SimplifiedType::Uint(UintTy::U128),
        sym::f32 => SimplifiedType::Float(FloatTy::F32),
        sym::f64 => SimplifiedType::Float(FloatTy::F64),
        _ => return &[],
    };

    tcx.incoherent_impls(ty)
}

/// Resolves a def path like `vec::Vec` with the base `std`.
fn lookup_with_base(
    tcx: TyCtxt<'_>,
    base: DefId,
    ns: PathNS,
    mut path: &[Symbol],
    search: Search,
    mut emit: impl FnMut(ConfiguredItem),
) {
    let mut base = ConfiguredItem {
        id: base,
        private: false,
    };
    let child_by_name = |base: ConfiguredItem, ns, name, is_last| match item_child_by_name(tcx, base, ns, name) {
        None if search != Search::Public && !base.private => {
            omitted_binding_by_name(tcx, base.id, ns, name, search, is_last)
        },
        child => child.map(|id| ConfiguredItem { id, ..base }),
    };

    loop {
        match *path {
            [segment] => {
                if let Some(item) = child_by_name(base, ns, segment, true) {
                    emit(item);
                }

                // When the current def_id is e.g. `struct S`, check the impl items in
                // `impl S { ... }`
                let impls = if base.private {
                    (tcx.query_system.extern_providers.inherent_impls)(tcx, base.id)
                } else {
                    tcx.inherent_impls(base.id)
                };
                for &id in impls {
                    if let Some(item) = item_child_by_name(tcx, ConfiguredItem { id, ..base }, ns, segment) {
                        emit(ConfiguredItem { id: item, ..base });
                    }
                }

                return;
            },
            [segment, ref rest @ ..] => {
                path = rest;
                let Some(child) = child_by_name(base, PathNS::Type, segment, false) else {
                    return;
                };
                base = child;
            },
            [] => unreachable!(),
        }
    }
}

/// Find a declaration or import omitted from an external crate's public hash.
fn omitted_binding_by_name(
    tcx: TyCtxt<'_>,
    base: DefId,
    ns: PathNS,
    name: Symbol,
    search: Search,
    is_last: bool,
) -> Option<ConfiguredItem> {
    // Upstream rustc puts these bindings in `module_children`. On Ohm, CStore's
    // inherent method takes precedence and reads the omitted bindings on demand.
    #[allow(dead_code, reason = "Only used when building with upstream rustc")]
    trait UnpartitionedMetadata {
        /// Upstream tracks changes to the entire dependency.
        fn has_public_api_hash(&self, _: CrateNum) -> bool {
            false
        }

        fn private_module_children_untracked(&self, _: TyCtxt<'_>, _: DefId) -> std::iter::Empty<ModChild> {
            std::iter::empty()
        }
    }

    impl UnpartitionedMetadata for CStore {}

    if base.is_local() || tcx.def_kind(base) != DefKind::Mod {
        return None;
    }

    let store = CStore::from_tcx(tcx);
    let child = store
        .private_module_children_untracked(tcx, base)
        .find(|child| child.ident.name == name && ns.matches(child.res.ns()));
    let module = child
        .as_ref()
        .is_some_and(|child| matches!(child.res, Res::Def(DefKind::Mod, _)) && child.reexport_chain.is_empty());

    // A module used as a prefix lets the next lookup track the child. At the end of the
    // path, even a module or a missing name could become a private import of a banned item
    // without changing the public hash.
    if (is_last || !module) && store.has_public_api_hash(base.krate) {
        let source = tcx.used_crate_source(base.krate);
        if let Some(path) = source.rmeta.as_ref().or(source.rlib.as_ref()).or(source.dylib.as_ref())
            && let Some(path) = path.to_str()
        {
            tcx.sess.file_depinfo.borrow_mut().insert(Symbol::intern(path));
        }
    }

    let child = child?;
    if search == Search::Imports && !module && child.reexport_chain.is_empty() {
        return None;
    }
    let id = child.res.opt_def_id()?;
    // A private import may itself name a private definition in the same crate.
    Some(ConfiguredItem {
        id,
        private: id.krate == base.krate && !matches!(child.res, Res::Def(DefKind::Mod, _)),
    })
}

fn item_child_by_name(tcx: TyCtxt<'_>, item: ConfiguredItem, ns: PathNS, name: Symbol) -> Option<DefId> {
    if let Some(local_id) = item.id.as_local() {
        local_item_child_by_name(tcx, local_id, ns, name)
    } else {
        non_local_item_child_by_name(tcx, item, ns, name)
    }
}

fn local_item_child_by_name(tcx: TyCtxt<'_>, local_id: LocalDefId, ns: PathNS, name: Symbol) -> Option<DefId> {
    let root_mod;
    let item_kind = match tcx.hir_node_by_def_id(local_id) {
        Node::Crate(r#mod) => {
            root_mod = ItemKind::Mod(Ident::dummy(), r#mod);
            &root_mod
        },
        Node::Item(item) => &item.kind,
        Node::Variant(variant) if ns == PathNS::Field => {
            return if let rustc_hir::VariantData::Struct { fields, .. } = variant.data
                && let Some(field_def_id) = fields.iter().find_map(|field| {
                    if field.ident.name == name {
                        Some(field.def_id.to_def_id())
                    } else {
                        None
                    }
                }) {
                Some(field_def_id)
            } else {
                None
            };
        },
        _ => return None,
    };

    match item_kind {
        ItemKind::Mod(_, r#mod) => r#mod.item_ids.iter().find_map(|&item_id| {
            let item = tcx.hir_item(item_id);
            if let ItemKind::Use(UseTree {
                prefix,
                kind: UseKind::Single(ident),
            }) = item.kind
            {
                if ident.name == name {
                    let opt_def_id = |ns: Option<Res>| ns.and_then(|res| res.opt_def_id());
                    match ns {
                        PathNS::Type => opt_def_id(prefix.res.type_ns),
                        PathNS::Value => opt_def_id(prefix.res.value_ns),
                        PathNS::Macro => opt_def_id(prefix.res.macro_ns),
                        PathNS::Field => None,
                        PathNS::Arbitrary => unreachable!(),
                    }
                } else {
                    None
                }
            } else if let Some(ident) = item.kind.ident()
                && ident.name == name
                && ns.matches(tcx.def_kind(item.owner_id).ns())
            {
                Some(item.owner_id.to_def_id())
            } else {
                None
            }
        }),
        ItemKind::Impl(..) | ItemKind::Trait { .. } => tcx
            .associated_items(local_id)
            .filter_by_name_unhygienic(name)
            .find(|assoc_item| ns.matches(Some(assoc_item.namespace())))
            .map(|assoc_item| assoc_item.def_id),
        ItemKind::Struct(_, _, rustc_hir::VariantData::Struct { fields, .. }) if ns == PathNS::Field => {
            fields.iter().find_map(|field| {
                if field.ident.name == name {
                    Some(field.def_id.to_def_id())
                } else {
                    None
                }
            })
        },
        ItemKind::Enum(_, _, rustc_hir::EnumDef { variants }) if ns == PathNS::Type => {
            variants.iter().find_map(|variant| {
                if variant.ident.name == name {
                    Some(variant.def_id.to_def_id())
                } else {
                    None
                }
            })
        },
        _ => None,
    }
}

fn non_local_item_child_by_name(tcx: TyCtxt<'_>, item: ConfiguredItem, ns: PathNS, name: Symbol) -> Option<DefId> {
    let providers = &tcx.query_system.extern_providers;
    match item.kind(tcx) {
        DefKind::Mod | DefKind::Enum | DefKind::Trait => {
            let children = if item.private {
                (providers.module_children)(tcx, item.id)
            } else {
                tcx.module_children(item.id)
            };
            children.iter().find_map(|child| {
                if child.ident.name == name && ns.matches(child.res.ns()) {
                    child.res.opt_def_id()
                } else {
                    None
                }
            })
        },
        kind @ (DefKind::Impl { .. } | DefKind::Struct) => {
            let ids = if item.private {
                (providers.associated_item_def_ids)(tcx, item.id)
            } else {
                tcx.associated_item_def_ids(item.id)
            };
            ids.iter().copied().find(|&id| {
                tcx.item_name(id) == name
                    && (kind == DefKind::Struct || ns.matches(ConfiguredItem { id, ..item }.kind(tcx).ns()))
            })
        },
        _ => None,
    }
}
