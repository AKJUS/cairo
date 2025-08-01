use std::sync::Arc;

use cairo_lang_debug::DebugWithDb;
use cairo_lang_defs::ids::{
    NamedLanguageElementId, TopLevelLanguageElementId, TraitFunctionId, UnstableSalsaId,
};
use cairo_lang_diagnostics::{DiagnosticAdded, DiagnosticNote, Maybe};
use cairo_lang_proc_macros::{DebugWithDb, SemanticObject};
use cairo_lang_semantic::items::functions::ImplGenericFunctionId;
use cairo_lang_semantic::items::imp::ImplLongId;
use cairo_lang_semantic::{GenericArgumentId, TypeLongId};
use cairo_lang_syntax::node::ast::ExprPtr;
use cairo_lang_syntax::node::kind::SyntaxKind;
use cairo_lang_syntax::node::{TypedStablePtr, ast};
use cairo_lang_utils::{Intern, define_short_id, try_extract_matches};
use defs::diagnostic_utils::StableLocation;
use defs::ids::{ExternFunctionId, FreeFunctionId};
use itertools::zip_eq;
use semantic::items::functions::GenericFunctionId;
use semantic::substitution::{GenericSubstitution, SubstitutionRewriter};
use semantic::{ExprVar, Mutability};
use {cairo_lang_defs as defs, cairo_lang_semantic as semantic};

use crate::Location;
use crate::db::LoweringGroup;
use crate::ids::semantic::substitution::SemanticRewriter;
use crate::specialization::SpecializationArg;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum FunctionWithBodyLongId<'db> {
    Semantic(defs::ids::FunctionWithBodyId<'db>),
    Generated { parent: defs::ids::FunctionWithBodyId<'db>, key: GeneratedFunctionKey<'db> },
}
define_short_id!(
    FunctionWithBodyId,
    FunctionWithBodyLongId<'db>,
    LoweringGroup,
    lookup_intern_lowering_function_with_body,
    intern_lowering_function_with_body
);
impl<'db> FunctionWithBodyLongId<'db> {
    pub fn base_semantic_function(
        &self,
        _db: &'db dyn LoweringGroup,
    ) -> cairo_lang_defs::ids::FunctionWithBodyId<'db> {
        match self {
            FunctionWithBodyLongId::Semantic(id) => *id,
            FunctionWithBodyLongId::Generated { parent, .. } => *parent,
        }
    }
    pub fn to_concrete(
        &self,
        db: &'db dyn LoweringGroup,
    ) -> Maybe<ConcreteFunctionWithBodyLongId<'db>> {
        Ok(match self {
            FunctionWithBodyLongId::Semantic(semantic) => ConcreteFunctionWithBodyLongId::Semantic(
                semantic::ConcreteFunctionWithBodyId::from_generic(db, *semantic)?,
            ),
            FunctionWithBodyLongId::Generated { parent, key } => {
                ConcreteFunctionWithBodyLongId::Generated(GeneratedFunction {
                    parent: semantic::ConcreteFunctionWithBodyId::from_generic(db, *parent)?,
                    key: *key,
                })
            }
        })
    }
}
impl<'db> FunctionWithBodyId<'db> {
    pub fn base_semantic_function(
        &self,
        db: &'db dyn LoweringGroup,
    ) -> cairo_lang_defs::ids::FunctionWithBodyId<'db> {
        self.long(db).base_semantic_function(db)
    }
    pub fn signature(&self, db: &'db dyn LoweringGroup) -> Maybe<Signature<'db>> {
        Ok(db.priv_function_with_body_lowering(*self)?.signature.clone())
    }
    pub fn to_concrete(
        &self,
        db: &'db dyn LoweringGroup,
    ) -> Maybe<ConcreteFunctionWithBodyId<'db>> {
        Ok(self.long(db).to_concrete(db)?.intern(db))
    }
}
pub trait SemanticFunctionWithBodyIdEx<'db> {
    fn lowered(&self, db: &'db dyn LoweringGroup) -> FunctionWithBodyId<'db>;
}
impl<'db> SemanticFunctionWithBodyIdEx<'db> for cairo_lang_defs::ids::FunctionWithBodyId<'db> {
    fn lowered(&self, db: &'db dyn LoweringGroup) -> FunctionWithBodyId<'db> {
        FunctionWithBodyLongId::Semantic(*self).intern(db)
    }
}

/// Concrete function with body.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum ConcreteFunctionWithBodyLongId<'db> {
    Semantic(semantic::ConcreteFunctionWithBodyId<'db>),
    Generated(GeneratedFunction<'db>),
    Specialized(SpecializedFunction<'db>),
}
define_short_id!(
    ConcreteFunctionWithBodyId,
    ConcreteFunctionWithBodyLongId<'db>,
    LoweringGroup,
    lookup_intern_lowering_concrete_function_with_body,
    intern_lowering_concrete_function_with_body
);

// The result of `generic_or_specialized`.
pub enum GenericOrSpecialized<'db> {
    Generic(FunctionWithBodyId<'db>),
    Specialized(SpecializedFunction<'db>),
}

impl<'db> ConcreteFunctionWithBodyId<'db> {
    pub fn is_panic_destruct_fn(&self, db: &'db dyn LoweringGroup) -> Maybe<bool> {
        match db.lookup_intern_lowering_concrete_function_with_body(*self) {
            ConcreteFunctionWithBodyLongId::Semantic(semantic_func) => {
                semantic_func.is_panic_destruct_fn(db)
            }
            ConcreteFunctionWithBodyLongId::Generated(GeneratedFunction {
                parent: _,
                key: GeneratedFunctionKey::TraitFunc(function, _),
            }) => Ok(function == db.core_info().panic_destruct_fn),
            _ => Ok(false),
        }
    }

    /// Returns the generic version of the function if it exists, otherwise the function is a
    /// specialized function and the `SpecializedFunction` struct is returned.
    pub fn generic_or_specialized(&self, db: &'db dyn LoweringGroup) -> GenericOrSpecialized<'db> {
        self.long(db).clone().generic_or_specialized(db)
    }
}

impl<'db> UnstableSalsaId for ConcreteFunctionWithBodyId<'db> {
    fn get_internal_id(&self) -> salsa::Id {
        self.as_intern_id()
    }
}
impl<'db> ConcreteFunctionWithBodyLongId<'db> {
    /// Returns the generic `FunctionWithLongId` if one exists, otherwise returns the specialized
    /// function.
    pub fn generic_or_specialized(self, db: &'db dyn LoweringGroup) -> GenericOrSpecialized<'db> {
        let long_id = match self {
            ConcreteFunctionWithBodyLongId::Semantic(id) => {
                FunctionWithBodyLongId::Semantic(id.function_with_body_id(db))
            }
            ConcreteFunctionWithBodyLongId::Generated(GeneratedFunction { parent, key }) => {
                FunctionWithBodyLongId::Generated { parent: parent.function_with_body_id(db), key }
            }
            ConcreteFunctionWithBodyLongId::Specialized(specialized) => {
                return GenericOrSpecialized::Specialized(specialized);
            }
        };
        GenericOrSpecialized::Generic(long_id.intern(db))
    }
    pub fn substitution(&self, db: &'db dyn LoweringGroup) -> Maybe<GenericSubstitution<'db>> {
        match self {
            ConcreteFunctionWithBodyLongId::Semantic(id) => id.substitution(db),
            ConcreteFunctionWithBodyLongId::Generated(GeneratedFunction { parent, .. }) => {
                parent.substitution(db)
            }
            ConcreteFunctionWithBodyLongId::Specialized(specialized) => {
                specialized.base.substitution(db)
            }
        }
    }
    pub fn function_id(&self, db: &'db dyn LoweringGroup) -> Maybe<FunctionId<'db>> {
        let long_id = match self {
            ConcreteFunctionWithBodyLongId::Semantic(id) => {
                FunctionLongId::Semantic(id.function_id(db)?)
            }
            ConcreteFunctionWithBodyLongId::Generated(generated) => {
                FunctionLongId::Generated(*generated)
            }
            ConcreteFunctionWithBodyLongId::Specialized(specialized) => {
                FunctionLongId::Specialized(specialized.clone())
            }
        };
        Ok(long_id.intern(db))
    }
    pub fn base_semantic_function(
        &self,
        db: &'db dyn LoweringGroup,
    ) -> semantic::ConcreteFunctionWithBodyId<'db> {
        match self {
            ConcreteFunctionWithBodyLongId::Semantic(id) => *id,
            ConcreteFunctionWithBodyLongId::Generated(generated) => generated.parent,
            ConcreteFunctionWithBodyLongId::Specialized(specialized) => {
                specialized.base.base_semantic_function(db)
            }
        }
    }
    pub fn full_path(&self, db: &dyn LoweringGroup) -> String {
        match self {
            ConcreteFunctionWithBodyLongId::Semantic(semantic) => semantic.full_path(db),
            ConcreteFunctionWithBodyLongId::Generated(generated) => generated.full_path(db),
            ConcreteFunctionWithBodyLongId::Specialized(specialized) => specialized.full_path(db),
        }
    }
}
impl<'db> ConcreteFunctionWithBodyId<'db> {
    pub fn from_semantic(
        db: &'db dyn LoweringGroup,
        semantic: semantic::ConcreteFunctionWithBodyId<'db>,
    ) -> Self {
        ConcreteFunctionWithBodyLongId::Semantic(semantic).intern(db)
    }
    pub fn substitution(&self, db: &'db dyn LoweringGroup) -> Maybe<GenericSubstitution<'db>> {
        self.long(db).substitution(db)
    }
    pub fn function_id(&self, db: &'db dyn LoweringGroup) -> Maybe<FunctionId<'db>> {
        self.long(db).function_id(db)
    }
    pub fn full_path(&self, db: &dyn LoweringGroup) -> String {
        self.long(db).full_path(db)
    }
    pub fn signature(&self, db: &'db dyn LoweringGroup) -> Maybe<Signature<'db>> {
        match self.generic_or_specialized(db) {
            GenericOrSpecialized::Generic(id) => {
                let generic_signature = id.signature(db)?;
                self.substitution(db)?.substitute(db, generic_signature)
            }
            GenericOrSpecialized::Specialized(specialized) => specialized.signature(db),
        }
    }
    pub fn from_no_generics_free(
        db: &'db dyn LoweringGroup,
        free_function_id: FreeFunctionId<'db>,
    ) -> Option<Self> {
        let semantic =
            semantic::ConcreteFunctionWithBodyId::from_no_generics_free(db, free_function_id)?;
        Some(ConcreteFunctionWithBodyLongId::Semantic(semantic).intern(db))
    }
    pub fn base_semantic_function(
        &self,
        db: &'db dyn LoweringGroup,
    ) -> semantic::ConcreteFunctionWithBodyId<'db> {
        self.long(db).base_semantic_function(db)
    }
    pub fn stable_location(&self, db: &'db dyn LoweringGroup) -> Maybe<StableLocation<'db>> {
        Ok(match self.long(db) {
            ConcreteFunctionWithBodyLongId::Semantic(id) => id.stable_location(db),
            ConcreteFunctionWithBodyLongId::Generated(generated) => match generated.key {
                GeneratedFunctionKey::Loop(stable_ptr) => StableLocation::new(stable_ptr.untyped()),
                GeneratedFunctionKey::TraitFunc(_, stable_location) => stable_location,
            },
            ConcreteFunctionWithBodyLongId::Specialized(specialized_function) => {
                specialized_function.base.stable_location(db)?
            }
        })
    }
}

/// Function.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum FunctionLongId<'db> {
    /// An original function from the user code.
    Semantic(semantic::FunctionId<'db>),
    /// A function generated by the compiler.
    Generated(GeneratedFunction<'db>),
    /// A specialized function.
    Specialized(SpecializedFunction<'db>),
}
define_short_id!(
    FunctionId,
    FunctionLongId<'db>,
    LoweringGroup,
    lookup_intern_lowering_function,
    intern_lowering_function
);
impl<'db> FunctionLongId<'db> {
    pub fn body(
        &self,
        db: &'db dyn LoweringGroup,
    ) -> Maybe<Option<ConcreteFunctionWithBodyId<'db>>> {
        Ok(Some(match self {
            FunctionLongId::Semantic(id) => {
                let concrete_function = id.get_concrete(db);
                if let GenericFunctionId::Impl(ImplGenericFunctionId { impl_id, function }) =
                    concrete_function.generic_function
                {
                    if let ImplLongId::GeneratedImpl(imp) = db.lookup_intern_impl(impl_id) {
                        let concrete_trait = imp.concrete_trait(db);
                        let info = db.core_info();
                        assert!(
                            [
                                info.destruct_fn,
                                info.panic_destruct_fn,
                                info.call_fn,
                                info.call_once_fn
                            ]
                            .contains(&function)
                        );

                        let generic_args = concrete_trait.generic_args(db);
                        let Some(GenericArgumentId::Type(ty)) = generic_args.first() else {
                            unreachable!("Expected Generated Impl to have a type argument");
                        };
                        let TypeLongId::Closure(ty) = ty.long(db) else {
                            unreachable!("Expected Generated Impl to have a closure type argument");
                        };

                        let Some(parent) = ty.parent_function?.get_concrete(db).body(db)? else {
                            return Ok(None);
                        };
                        return Ok(Some(
                            GeneratedFunction {
                                parent,
                                key: GeneratedFunctionKey::TraitFunc(function, ty.wrapper_location),
                            }
                            .body(db),
                        ));
                    }
                }

                let Some(body) = concrete_function.body(db)? else {
                    return Ok(None);
                };
                ConcreteFunctionWithBodyLongId::Semantic(body).intern(db)
            }
            FunctionLongId::Generated(generated) => generated.body(db),
            FunctionLongId::Specialized(specialized) => {
                ConcreteFunctionWithBodyLongId::Specialized(specialized.clone()).intern(db)
            }
        }))
    }
    pub fn signature(&self, db: &'db dyn LoweringGroup) -> Maybe<Signature<'db>> {
        match self {
            FunctionLongId::Semantic(semantic) => {
                Ok(Signature::from_semantic(db, db.concrete_function_signature(*semantic)?))
            }
            FunctionLongId::Generated(generated) => generated.body(db).signature(db),
            FunctionLongId::Specialized(specialized) => specialized.signature(db),
        }
    }
    pub fn full_path(&self, db: &dyn LoweringGroup) -> String {
        format!("{:?}", self.debug(db))
    }
    /// Returns the full path of the relevant semantic function:
    /// - If the function itself is semantic (non generated), its own full path.
    /// - If the function is generated, then its (semantic) parent's full path.
    pub fn semantic_full_path(&self, db: &dyn LoweringGroup) -> String {
        match self {
            FunctionLongId::Semantic(id) => id.full_path(db),
            FunctionLongId::Generated(generated) => generated.parent.full_path(db),
            FunctionLongId::Specialized(specialized) => specialized.full_path(db),
        }
    }
}
impl<'db> FunctionId<'db> {
    pub fn body(
        &self,
        db: &'db dyn LoweringGroup,
    ) -> Maybe<Option<ConcreteFunctionWithBodyId<'db>>> {
        self.long(db).body(db)
    }
    pub fn signature(&self, db: &'db dyn LoweringGroup) -> Maybe<Signature<'db>> {
        self.long(db).signature(db)
    }
    pub fn full_path(&self, db: &dyn LoweringGroup) -> String {
        self.long(db).full_path(db)
    }
    pub fn semantic_full_path(&self, db: &dyn LoweringGroup) -> String {
        self.long(db).semantic_full_path(db)
    }
    /// Returns the function as an `ExternFunctionId` and its generic arguments, if it is an
    /// `extern` functions.
    pub fn get_extern(
        &self,
        db: &'db dyn LoweringGroup,
    ) -> Option<(ExternFunctionId<'db>, Vec<GenericArgumentId<'db>>)> {
        let semantic = try_extract_matches!(self.long(db), FunctionLongId::Semantic)?;
        let concrete = semantic.get_concrete(db);
        Some((
            try_extract_matches!(concrete.generic_function, GenericFunctionId::Extern)?,
            concrete.generic_args,
        ))
    }
}
pub trait SemanticFunctionIdEx<'db> {
    fn lowered(&self, db: &'db dyn LoweringGroup) -> FunctionId<'db>;
}
impl<'db> SemanticFunctionIdEx<'db> for semantic::FunctionId<'db> {
    fn lowered(&self, db: &'db dyn LoweringGroup) -> FunctionId<'db> {
        let ret = FunctionLongId::Semantic(*self).intern(db);
        // If the function is generated, we need to check if it has a body, so we can return its
        // generated function id.
        // TODO(orizi): This is a hack, we should have a better way to do this.
        if let Ok(Some(body)) = ret.body(db) {
            if let Ok(id) = body.function_id(db) {
                return id;
            }
        }
        ret
    }
}
impl<'a> DebugWithDb<'a> for FunctionLongId<'a> {
    type Db = dyn LoweringGroup;
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &'a dyn LoweringGroup) -> std::fmt::Result {
        match self {
            FunctionLongId::Semantic(semantic) => write!(f, "{:?}", semantic.debug(db)),
            FunctionLongId::Generated(generated) => write!(f, "{:?}", generated.debug(db)),
            FunctionLongId::Specialized(specialized) => write!(f, "{:?}", specialized.debug(db)),
        }
    }
}

/// A key for a generated functions.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, salsa::Update)]
pub enum GeneratedFunctionKey<'db> {
    /// Generated loop functions are identified by the loop expr_id.
    Loop(ExprPtr<'db>),
    TraitFunc(TraitFunctionId<'db>, StableLocation<'db>),
}

/// Generated function.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct GeneratedFunction<'db> {
    pub parent: semantic::ConcreteFunctionWithBodyId<'db>,
    pub key: GeneratedFunctionKey<'db>,
}
impl<'db> GeneratedFunction<'db> {
    pub fn body(&self, db: &'db dyn LoweringGroup) -> ConcreteFunctionWithBodyId<'db> {
        let long_id = ConcreteFunctionWithBodyLongId::Generated(*self);
        long_id.intern(db)
    }
    pub fn full_path(&self, db: &dyn LoweringGroup) -> String {
        format!("{:?}", self.debug(db))
    }
}
impl<'a> DebugWithDb<'a> for GeneratedFunction<'a> {
    type Db = dyn LoweringGroup;
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &'a dyn LoweringGroup) -> std::fmt::Result {
        match self.key {
            GeneratedFunctionKey::Loop(expr_ptr) => {
                let mut func_ptr = expr_ptr.untyped();
                while !matches!(
                    func_ptr.kind(db),
                    SyntaxKind::FunctionWithBody | SyntaxKind::TraitItemFunction
                ) {
                    func_ptr = func_ptr.parent(db)
                }

                let span = expr_ptr.0.lookup(db).span(db);
                let function_start = func_ptr.lookup(db).span(db).start.as_u32();
                write!(
                    f,
                    "{:?}[{}-{}]",
                    self.parent.debug(db),
                    span.start.as_u32() - function_start,
                    span.end.as_u32() - function_start
                )
            }
            GeneratedFunctionKey::TraitFunc(trait_func, loc) => {
                let trait_id = trait_func.trait_id(db);
                write!(
                    f,
                    "Generated `{}::{}` for {{closure@{:?}}}",
                    trait_id.full_path(db),
                    trait_func.name(db),
                    loc.debug(db.upcast()),
                )
            }
        }
    }
}

/// Specialized function
///
/// Specialized functions are generated by the compiler some of the function
/// arguments are known at compile time and the resulting specialized function is smaller
/// than the original one.
///
/// Specialized functions are identified by the base function and the arguments.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct SpecializedFunction<'db> {
    /// The base function.
    pub base: crate::ids::ConcreteFunctionWithBodyId<'db>,
    /// Optional const assignments for the arguments.
    pub args: Arc<[Option<SpecializationArg<'db>>]>,
}

impl<'db> SpecializedFunction<'db> {
    pub fn body(&self, db: &'db dyn LoweringGroup) -> ConcreteFunctionWithBodyId<'db> {
        let long_id = ConcreteFunctionWithBodyLongId::Specialized(self.clone());
        long_id.intern(db)
    }
    pub fn full_path(&self, db: &dyn LoweringGroup) -> String {
        format!("{:?}", self.debug(db))
    }

    pub fn signature(&self, db: &'db dyn LoweringGroup) -> Maybe<Signature<'db>> {
        let mut base_sign = self.base.signature(db)?;

        base_sign.params = zip_eq(base_sign.params, self.args.iter())
            .filter_map(|(param, arg)| arg.is_none().then_some(param))
            .collect::<Vec<_>>();

        Ok(base_sign)
    }
}
impl<'a> DebugWithDb<'a> for SpecializedFunction<'a> {
    type Db = dyn LoweringGroup;
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>, db: &'a dyn LoweringGroup) -> std::fmt::Result {
        write!(f, "{}{{", self.base.full_path(db))?;
        for arg in self.args.iter() {
            match arg {
                Some(value) => write!(f, "{:?}, ", value.debug(db))?,
                None => write!(f, "None, ")?,
            }
        }
        write!(f, "}}")
    }
}

/// Lowered signature of a function.
#[derive(Clone, Debug, PartialEq, Eq, DebugWithDb, SemanticObject, Hash, salsa::Update)]
#[debug_db(dyn LoweringGroup)]
pub struct Signature<'db> {
    /// Input params.
    pub params: Vec<semantic::ExprVarMemberPath<'db>>,
    /// Extra returns - e.g. ref params
    pub extra_rets: Vec<semantic::ExprVarMemberPath<'db>>,
    /// Return type.
    pub return_type: semantic::TypeId<'db>,
    /// Explicit implicit requirements.
    pub implicits: Vec<semantic::TypeId<'db>>,
    /// Panicable.
    #[dont_rewrite]
    pub panicable: bool,
    /// Location.
    #[dont_rewrite]
    #[hide_field_debug_with_db]
    pub location: LocationId<'db>,
}
impl<'db> Signature<'db> {
    pub fn from_semantic(db: &'db dyn LoweringGroup, value: semantic::Signature<'db>) -> Self {
        let semantic::Signature {
            params,
            return_type,
            implicits,
            panicable,
            stable_ptr,
            is_const: _,
        } = value;
        let ref_params = params
            .iter()
            .filter(|param| param.mutability == Mutability::Reference)
            .map(|param| parameter_as_member_path(param.clone()))
            .collect();
        let params: Vec<semantic::ExprVarMemberPath<'_>> =
            params.into_iter().map(parameter_as_member_path).collect();
        Self {
            params,
            extra_rets: ref_params,
            return_type,
            implicits,
            panicable,
            location: LocationId::from_stable_location(
                db,
                StableLocation::new(stable_ptr.untyped()),
            ),
        }
    }
    pub fn is_fully_concrete(&self, db: &dyn LoweringGroup) -> bool {
        self.params.iter().all(|param| param.ty().is_fully_concrete(db))
            && self.extra_rets.iter().all(|param| param.ty().is_fully_concrete(db))
            && self.return_type.is_fully_concrete(db)
            && self.implicits.iter().all(|ty| ty.is_fully_concrete(db))
    }
}
semantic::add_rewrite!(<'a, 'b>, SubstitutionRewriter<'a, 'b>, DiagnosticAdded, Signature<'a>);

/// Converts a [semantic::Parameter] to a [semantic::ExprVarMemberPath].
pub(crate) fn parameter_as_member_path<'db>(
    param: semantic::Parameter<'db>,
) -> semantic::ExprVarMemberPath<'db> {
    let semantic::Parameter { id, ty, stable_ptr, .. } = param;
    semantic::ExprVarMemberPath::Var(ExprVar {
        var: semantic::VarId::Param(id),
        ty,
        stable_ptr: ast::ExprPtr(stable_ptr.0),
    })
}

define_short_id!(LocationId, Location<'db>, LoweringGroup, lookup_intern_location, intern_location);
impl<'db> LocationId<'db> {
    pub fn from_stable_location(
        db: &'db dyn LoweringGroup,
        stable_location: StableLocation<'db>,
    ) -> LocationId<'db> {
        Location::new(stable_location).intern(db)
    }

    /// Adds a note to the location.
    pub fn with_note(
        &self,
        db: &'db dyn LoweringGroup,
        note: DiagnosticNote<'db>,
    ) -> LocationId<'db> {
        self.long(db).clone().with_note(note).intern(db)
    }

    /// Adds a note that this location was generated while compiling an auto-generated function.
    pub fn with_auto_generation_note(
        &self,
        db: &'db dyn LoweringGroup,
        logic_name: &str,
    ) -> LocationId<'db> {
        self.with_note(
            db,
            DiagnosticNote::text_only(format!(
                "this error originates in auto-generated {logic_name} logic."
            )),
        )
    }
}
