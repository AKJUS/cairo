use std::sync::Arc;

use cairo_lang_defs::ids::FunctionWithBodyId;
use cairo_lang_diagnostics::{DiagnosticAdded, Diagnostics, Maybe, ToMaybe};
use cairo_lang_proc_macros::DebugWithDb;
use cairo_lang_syntax::attribute::consts::{IMPLICIT_PRECEDENCE_ATTR, INLINE_ATTR};
use cairo_lang_syntax::attribute::structured::{Attribute, AttributeArg, AttributeArgVariant};
use cairo_lang_syntax::node::db::SyntaxGroup;
use cairo_lang_syntax::node::{TypedStablePtr, TypedSyntaxNode, ast};
use cairo_lang_utils::unordered_hash_map::UnorderedHashMap;
use cairo_lang_utils::{Upcast, try_extract_matches};
use itertools::Itertools;

use super::functions::InlineConfiguration;
use crate::db::SemanticGroup;
use crate::diagnostic::{
    NotFoundItemType, SemanticDiagnosticKind, SemanticDiagnostics, SemanticDiagnosticsBuilder,
};
use crate::items::functions::ImplicitPrecedence;
use crate::resolve::{ResolvedConcreteItem, Resolver, ResolverData};
use crate::{Arenas, ExprId, PatternId, SemanticDiagnostic, TypeId, semantic};

// === Declaration ===

// --- Selectors ---

/// Query implementation of [crate::db::SemanticGroup::function_declaration_diagnostics].
pub fn function_declaration_diagnostics<'db>(
    db: &'db dyn SemanticGroup,
    function_id: FunctionWithBodyId<'db>,
) -> Diagnostics<'db, SemanticDiagnostic<'db>> {
    let declaration_data = match function_id {
        FunctionWithBodyId::Free(free_function_id) => {
            db.priv_free_function_declaration_data(free_function_id)
        }
        FunctionWithBodyId::Impl(impl_function_id) => db
            .priv_impl_function_declaration_data(impl_function_id)
            .map(|x| x.function_declaration_data),
        FunctionWithBodyId::Trait(trait_function_id) => {
            db.priv_trait_function_declaration_data(trait_function_id)
        }
    };
    declaration_data.map(|data| data.diagnostics).unwrap_or_default()
}

/// Query implementation of [crate::db::SemanticGroup::function_declaration_inline_config].
pub fn function_declaration_inline_config<'db>(
    db: &'db dyn SemanticGroup,
    function_id: FunctionWithBodyId<'db>,
) -> Maybe<InlineConfiguration<'db>> {
    match function_id {
        FunctionWithBodyId::Free(free_function_id) => {
            db.free_function_declaration_inline_config(free_function_id)
        }
        FunctionWithBodyId::Impl(impl_function_id) => {
            db.impl_function_declaration_inline_config(impl_function_id)
        }
        FunctionWithBodyId::Trait(trait_function_id) => {
            db.trait_function_declaration_inline_config(trait_function_id)
        }
    }
}

/// Query implementation of [SemanticGroup::function_declaration_implicit_precedence].
pub fn function_declaration_implicit_precedence<'db>(
    db: &'db dyn SemanticGroup,
    function_id: FunctionWithBodyId<'db>,
) -> Maybe<ImplicitPrecedence<'db>> {
    match function_id {
        FunctionWithBodyId::Free(free_function_id) => {
            db.free_function_declaration_implicit_precedence(free_function_id)
        }
        FunctionWithBodyId::Impl(impl_function_id) => {
            db.impl_function_declaration_implicit_precedence(impl_function_id)
        }
        FunctionWithBodyId::Trait(trait_function_id) => {
            db.trait_function_declaration_implicit_precedence(trait_function_id)
        }
    }
}

/// Query implementation of [crate::db::SemanticGroup::function_with_body_signature].
pub fn function_with_body_signature<'db>(
    db: &'db dyn SemanticGroup,
    function_id: FunctionWithBodyId<'db>,
) -> Maybe<semantic::Signature<'db>> {
    match function_id {
        FunctionWithBodyId::Free(free_function_id) => db.free_function_signature(free_function_id),
        FunctionWithBodyId::Impl(impl_function_id) => db.impl_function_signature(impl_function_id),
        FunctionWithBodyId::Trait(trait_function_id) => {
            db.trait_function_signature(trait_function_id)
        }
    }
}

/// Query implementation of
/// [crate::db::SemanticGroup::function_with_body_generic_params].
pub fn function_with_body_generic_params<'db>(
    db: &'db dyn SemanticGroup,
    function_id: FunctionWithBodyId<'db>,
) -> Maybe<Vec<semantic::GenericParam<'db>>> {
    match function_id {
        FunctionWithBodyId::Free(free_function_id) => {
            db.free_function_generic_params(free_function_id)
        }
        FunctionWithBodyId::Impl(impl_function_id) => {
            let mut res = db.impl_def_generic_params(impl_function_id.impl_def_id(db))?;
            res.extend(db.impl_function_generic_params(impl_function_id)?);
            Ok(res)
        }
        FunctionWithBodyId::Trait(trait_function_id) => {
            let mut res = db.trait_generic_params(trait_function_id.trait_id(db))?;
            res.extend(db.trait_function_generic_params(trait_function_id)?);
            Ok(res)
        }
    }
}

/// Query implementation of [crate::db::SemanticGroup::function_with_body_attributes].
pub fn function_with_body_attributes<'db>(
    db: &'db dyn SemanticGroup,
    function_id: FunctionWithBodyId<'db>,
) -> Maybe<Vec<Attribute<'db>>> {
    match function_id {
        FunctionWithBodyId::Free(free_function_id) => {
            Ok(db.priv_free_function_declaration_data(free_function_id)?.attributes)
        }
        FunctionWithBodyId::Impl(impl_function_id) => Ok(db
            .priv_impl_function_declaration_data(impl_function_id)?
            .function_declaration_data
            .attributes),
        FunctionWithBodyId::Trait(trait_function_id) => {
            Ok(db.priv_trait_function_declaration_data(trait_function_id)?.attributes)
        }
    }
}

// === Body ===

#[derive(Clone, Debug, PartialEq, Eq, DebugWithDb)]
#[debug_db(dyn SemanticGroup)]
pub struct FunctionBodyData<'db> {
    pub diagnostics: Diagnostics<'db, SemanticDiagnostic<'db>>,
    pub expr_lookup: UnorderedHashMap<ast::ExprPtr<'db>, ExprId>,
    pub pattern_lookup: UnorderedHashMap<ast::PatternPtr<'db>, PatternId>,
    pub resolver_data: Arc<ResolverData<'db>>,
    pub body: Arc<FunctionBody<'db>>,
}

unsafe impl<'db> salsa::Update for FunctionBodyData<'db> {
    // Using existing salsa::Update implementations for the fields.
    // For lookups we assume they are built from the arena,
    // so a change will be detected and they will be copied.
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        let old_value = &mut *old_pointer;
        let res = Diagnostics::maybe_update(&mut old_value.diagnostics, new_value.diagnostics)
            | Arc::maybe_update(&mut old_value.resolver_data, new_value.resolver_data)
            | Arc::maybe_update(&mut old_value.body, new_value.body);
        if res {
            old_value.expr_lookup = new_value.expr_lookup;
            old_value.pattern_lookup = new_value.pattern_lookup;
            return true;
        }
        false
    }
}

#[derive(Clone, Debug, PartialEq, Eq, DebugWithDb)]
#[debug_db(dyn SemanticGroup)]
pub struct FunctionBody<'db> {
    pub arenas: Arenas<'db>,
    pub body_expr: semantic::ExprId,
}

unsafe impl<'db> salsa::Update for FunctionBody<'db> {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        // The function body contains both the arena and the expr id, so a change will be detected.
        // The comparison should still be safe to do as we wont follow expired references.
        let old_value = &*old_pointer;

        if old_value != &new_value {
            *old_pointer = new_value;
            return true;
        }

        false
    }
}

// --- Selectors ---

/// Query implementation of [crate::db::SemanticGroup::function_body_diagnostics].
pub fn function_body_diagnostics<'db>(
    db: &'db dyn SemanticGroup,
    function_id: FunctionWithBodyId<'db>,
) -> Diagnostics<'db, SemanticDiagnostic<'db>> {
    let body_data = match function_id {
        FunctionWithBodyId::Free(id) => db.priv_free_function_body_data(id),
        FunctionWithBodyId::Impl(id) => db.priv_impl_function_body_data(id),
        FunctionWithBodyId::Trait(id) => {
            db.priv_trait_function_body_data(id).and_then(|x| x.ok_or(DiagnosticAdded))
        }
    };
    body_data.map(|data| data.diagnostics).unwrap_or_default()
}

/// Query implementation of SemanticGroup::function_body_expr.
pub fn function_body_expr<'db>(
    db: &'db dyn SemanticGroup,
    function_id: FunctionWithBodyId<'db>,
) -> Maybe<semantic::ExprId> {
    Ok(db.function_body(function_id)?.body_expr)
}

/// Query implementation of [crate::db::SemanticGroup::function_body].
pub fn function_body<'db>(
    db: &'db dyn SemanticGroup,
    function_id: FunctionWithBodyId<'db>,
) -> Maybe<Arc<FunctionBody<'db>>> {
    Ok(match function_id {
        FunctionWithBodyId::Free(id) => db.priv_free_function_body_data(id)?.body,
        FunctionWithBodyId::Impl(id) => db.priv_impl_function_body_data(id)?.body,
        FunctionWithBodyId::Trait(id) => {
            db.priv_trait_function_body_data(id)?.ok_or(DiagnosticAdded)?.body
        }
    })
}

// =========================================================

/// Query implementation of SemanticGroup::expr_semantic.
pub fn expr_semantic<'db>(
    db: &'db dyn SemanticGroup,
    function_id: FunctionWithBodyId<'db>,
    id: semantic::ExprId,
) -> semantic::Expr<'db> {
    db.function_body(function_id).unwrap().arenas.exprs.get(id).unwrap().clone()
}

/// Query implementation of SemanticGroup::pattern_semantic.
pub fn pattern_semantic<'db>(
    db: &'db dyn SemanticGroup,
    function_id: FunctionWithBodyId<'db>,
    id: semantic::PatternId,
) -> semantic::Pattern<'db> {
    db.function_body(function_id).unwrap().arenas.patterns.get(id).unwrap().clone()
}

/// Query implementation of SemanticGroup::statement_semantic.
pub fn statement_semantic<'db>(
    db: &'db dyn SemanticGroup,
    function_id: FunctionWithBodyId<'db>,
    id: semantic::StatementId,
) -> semantic::Statement<'db> {
    db.function_body(function_id).unwrap().arenas.statements.get(id).unwrap().clone()
}

pub trait SemanticExprLookup<'db>: Upcast<'db, dyn SemanticGroup> {
    fn lookup_expr_by_ptr(
        &'db self,
        function_id: FunctionWithBodyId<'db>,
        ptr: ast::ExprPtr<'db>,
    ) -> Maybe<ExprId> {
        let body_data = match function_id {
            FunctionWithBodyId::Free(id) => self.upcast().priv_free_function_body_data(id)?,
            FunctionWithBodyId::Impl(id) => self.upcast().priv_impl_function_body_data(id)?,
            FunctionWithBodyId::Trait(id) => {
                self.upcast().priv_trait_function_body_data(id)?.ok_or(DiagnosticAdded)?
            }
        };
        body_data.expr_lookup.get(&ptr).copied().to_maybe()
    }
    fn lookup_pattern_by_ptr(
        &'db self,
        function_id: FunctionWithBodyId<'db>,
        ptr: ast::PatternPtr<'db>,
    ) -> Maybe<PatternId> {
        let body_data = match function_id {
            FunctionWithBodyId::Free(id) => self.upcast().priv_free_function_body_data(id)?,
            FunctionWithBodyId::Impl(id) => self.upcast().priv_impl_function_body_data(id)?,
            FunctionWithBodyId::Trait(id) => {
                self.upcast().priv_trait_function_body_data(id)?.ok_or(DiagnosticAdded)?
            }
        };
        body_data.pattern_lookup.get(&ptr).copied().to_maybe()
    }
}
impl<'db, T: Upcast<'db, dyn SemanticGroup> + ?Sized> SemanticExprLookup<'db> for T {}

/// Get the inline configuration of the given function by parsing its attributes.
pub fn get_inline_config<'db>(
    db: &'db dyn SemanticGroup,
    diagnostics: &mut SemanticDiagnostics<'db>,
    attributes: &[Attribute<'db>],
) -> Maybe<InlineConfiguration<'db>> {
    let mut config = InlineConfiguration::None;
    let mut seen_inline_attr = false;
    for attr in attributes {
        if attr.id != INLINE_ATTR {
            continue;
        }

        match &attr.args[..] {
            [
                AttributeArg {
                    variant: AttributeArgVariant::Unnamed(ast::Expr::Path(path)), ..
                },
            ] if path.as_syntax_node().get_text(db) == "always" => {
                config = InlineConfiguration::Always(attr.stable_ptr);
            }
            [
                AttributeArg {
                    variant: AttributeArgVariant::Unnamed(ast::Expr::Path(path)), ..
                },
            ] if path.as_syntax_node().get_text(db) == "never" => {
                config = InlineConfiguration::Never(attr.stable_ptr);
            }
            [] => {
                config = InlineConfiguration::Should(attr.stable_ptr);
            }
            _ => {
                diagnostics.report(
                    attr.args_stable_ptr.untyped(),
                    SemanticDiagnosticKind::UnsupportedInlineArguments,
                );
            }
        }

        if seen_inline_attr {
            diagnostics.report(
                attr.id_stable_ptr.untyped(),
                SemanticDiagnosticKind::RedundantInlineAttribute,
            );
            // If we have multiple inline attributes revert to InlineConfiguration::None.
            config = InlineConfiguration::None;
        }

        seen_inline_attr = true;
    }
    Ok(config)
}

/// Get [ImplicitPrecedence] of the given function by looking at its attributes.
///
/// Returns the generated implicit precedence and the attribute used to get it, if one exists.
/// If there is no implicit precedence influencing attribute, then this function returns
/// [ImplicitPrecedence::UNSPECIFIED].
pub fn get_implicit_precedence<'a, 'r>(
    syntax_db: &'a dyn SyntaxGroup,
    diagnostics: &mut SemanticDiagnostics<'a>,
    resolver: &mut Resolver<'a>,
    attributes: &'r [Attribute<'a>],
) -> (ImplicitPrecedence<'a>, Option<&'r Attribute<'a>>) {
    let mut attributes = attributes.iter().rev().filter(|attr| attr.id == IMPLICIT_PRECEDENCE_ATTR);

    // Pick the last attribute if any.
    let Some(attr) = attributes.next() else { return (ImplicitPrecedence::UNSPECIFIED, None) };

    // Report warnings for overridden attributes if any.
    for attr in attributes {
        diagnostics.report(
            attr.id_stable_ptr,
            SemanticDiagnosticKind::RedundantImplicitPrecedenceAttribute,
        );
    }

    let Ok(types) =
        attr.args
            .iter()
            .map(|arg| match &arg.variant {
                AttributeArgVariant::Unnamed(value) => {
                    let ast::Expr::Path(path) = value else {
                        return Err(diagnostics.report(
                            value.stable_ptr(syntax_db),
                            SemanticDiagnosticKind::UnsupportedImplicitPrecedenceArguments,
                        ));
                    };

                    resolver
                        .resolve_concrete_path(diagnostics, path, NotFoundItemType::Type)
                        .and_then(|resolved_item: crate::resolve::ResolvedConcreteItem<'_>| {
                            try_extract_matches!(resolved_item, ResolvedConcreteItem::Type)
                                .ok_or_else(|| {
                                    diagnostics.report(
                                        value.stable_ptr(syntax_db),
                                        SemanticDiagnosticKind::UnknownType,
                                    )
                                })
                        })
                }

                _ => Err(diagnostics.report(
                    arg.arg.stable_ptr(syntax_db),
                    SemanticDiagnosticKind::UnsupportedImplicitPrecedenceArguments,
                )),
            })
            .try_collect::<TypeId<'_>, Vec<_>, _>()
    else {
        return (ImplicitPrecedence::UNSPECIFIED, None);
    };

    let precedence = ImplicitPrecedence::from_iter(types);

    (precedence, Some(attr))
}
