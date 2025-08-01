use std::vec;

use cairo_lang_defs::patcher::PatchBuilder;
use cairo_lang_defs::plugin::{
    MacroPlugin, MacroPluginMetadata, PluginDiagnostic, PluginGeneratedFile, PluginResult,
};
use cairo_lang_filesystem::cfg::{Cfg, CfgSet};
use cairo_lang_syntax::attribute::structured::{
    Attribute, AttributeArg, AttributeArgVariant, AttributeStructurize,
};
use cairo_lang_syntax::node::db::SyntaxGroup;
use cairo_lang_syntax::node::helpers::{BodyItems, GetIdentifier, QueryAttrs};
use cairo_lang_syntax::node::{TypedStablePtr, TypedSyntaxNode, ast};
use cairo_lang_utils::try_extract_matches;
use itertools::Itertools;

/// Represents a predicate tree used to evaluate configuration attributes to handle nested
/// predicates, such as logical `not` operations, and evaluate them based on a given set of
/// configuration flags (`CfgSet`).
#[derive(Debug, Clone)]
enum PredicateTree {
    Cfg(Cfg),
    Not(Box<PredicateTree>),
    And(Vec<PredicateTree>),
    Or(Vec<PredicateTree>),
}

impl PredicateTree {
    /// Evaluates the predicate tree against the provided configuration set (`CfgSet`) by traversing
    /// the `PredicateTree` and determines whether the predicate is satisfied by the given
    /// `cfg_set`.
    fn evaluate(&self, cfg_set: &CfgSet) -> bool {
        match self {
            PredicateTree::Cfg(cfg) => cfg_set.contains(cfg),
            PredicateTree::Not(inner) => !inner.evaluate(cfg_set),
            PredicateTree::And(predicates) => predicates.iter().all(|p| p.evaluate(cfg_set)),
            PredicateTree::Or(predicates) => predicates.iter().any(|p| p.evaluate(cfg_set)),
        }
    }
}

/// Represents a part of a configuration predicate.
pub enum ConfigPredicatePart<'db> {
    /// A configuration item, either a key-value pair or a simple name.
    Cfg(Cfg),
    /// A function call in the predicate (`not`, `and`, `or`).
    Call(ast::ExprFunctionCall<'db>),
}

/// Plugin that enables ignoring modules not involved in the current config.
///
/// Mostly useful for marking test modules to prevent usage of their functionality out of tests,
/// and reduce compilation time when the tests data isn't required.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ConfigPlugin;

const CFG_ATTR: &str = "cfg";

impl MacroPlugin for ConfigPlugin {
    fn generate_code<'db>(
        &self,
        db: &'db dyn SyntaxGroup,
        item_ast: ast::ModuleItem<'db>,
        metadata: &MacroPluginMetadata<'_>,
    ) -> PluginResult<'db> {
        let mut diagnostics = vec![];

        if should_drop(db, metadata.cfg_set, &item_ast, &mut diagnostics) {
            PluginResult { code: None, diagnostics, remove_original_item: true }
        } else if let Some(builder) =
            handle_undropped_item(db, metadata.cfg_set, item_ast, &mut diagnostics)
        {
            let (content, code_mappings) = builder.build();
            PluginResult {
                code: Some(PluginGeneratedFile {
                    name: "config".into(),
                    content,
                    code_mappings,
                    aux_data: None,
                    diagnostics_note: Default::default(),
                    is_unhygienic: false,
                }),
                diagnostics,
                remove_original_item: true,
            }
        } else {
            PluginResult { code: None, diagnostics, remove_original_item: false }
        }
    }

    fn declared_attributes(&self) -> Vec<String> {
        vec![CFG_ATTR.to_string()]
    }
}

/// Extension trait for `BodyItems` filtering out items that are not included in the cfg.
pub trait HasItemsInCfgEx<'a, Item: QueryAttrs<'a>>: BodyItems<'a, Item = Item> {
    fn iter_items_in_cfg(
        &self,
        db: &'a dyn SyntaxGroup,
        cfg_set: &CfgSet,
    ) -> impl Iterator<Item = Item>;
}

impl<'a, Item: QueryAttrs<'a>, Body: BodyItems<'a, Item = Item>> HasItemsInCfgEx<'a, Item>
    for Body
{
    fn iter_items_in_cfg(
        &self,
        db: &'a dyn SyntaxGroup,
        cfg_set: &CfgSet,
    ) -> impl Iterator<Item = Item> {
        self.iter_items(db).filter(move |item| !should_drop(db, cfg_set, item, &mut vec![]))
    }
}

/// Handles an item that is not dropped from the AST completely due to not matching the config.
/// In case it includes dropped elements and needs to be rewritten, it returns the appropriate
/// PatchBuilder. Otherwise returns `None`, and it won't be rewritten or dropped.
fn handle_undropped_item<'a>(
    db: &'a dyn SyntaxGroup,
    cfg_set: &CfgSet,
    item_ast: ast::ModuleItem<'a>,
    diagnostics: &mut Vec<PluginDiagnostic<'a>>,
) -> Option<PatchBuilder<'a>> {
    match item_ast {
        ast::ModuleItem::Trait(trait_item) => {
            let body = try_extract_matches!(trait_item.body(db), ast::MaybeTraitBody::Some)?;
            let items = get_kept_items_nodes(db, cfg_set, body.iter_items(db), diagnostics)?;
            let mut builder = PatchBuilder::new(db, &trait_item);
            builder.add_node(trait_item.attributes(db).as_syntax_node());
            builder.add_node(trait_item.trait_kw(db).as_syntax_node());
            builder.add_node(trait_item.name(db).as_syntax_node());
            builder.add_node(trait_item.generic_params(db).as_syntax_node());
            builder.add_node(body.lbrace(db).as_syntax_node());
            for item in items {
                builder.add_node(item);
            }
            builder.add_node(body.rbrace(db).as_syntax_node());
            Some(builder)
        }
        ast::ModuleItem::Impl(impl_item) => {
            let body = try_extract_matches!(impl_item.body(db), ast::MaybeImplBody::Some)?;
            let items = get_kept_items_nodes(db, cfg_set, body.iter_items(db), diagnostics)?;
            let mut builder = PatchBuilder::new(db, &impl_item);
            builder.add_node(impl_item.attributes(db).as_syntax_node());
            builder.add_node(impl_item.impl_kw(db).as_syntax_node());
            builder.add_node(impl_item.name(db).as_syntax_node());
            builder.add_node(impl_item.generic_params(db).as_syntax_node());
            builder.add_node(impl_item.of_kw(db).as_syntax_node());
            builder.add_node(impl_item.trait_path(db).as_syntax_node());
            builder.add_node(body.lbrace(db).as_syntax_node());
            for item in items {
                builder.add_node(item);
            }
            builder.add_node(body.rbrace(db).as_syntax_node());
            Some(builder)
        }
        _ => None,
    }
}

/// Gets the list of items that should be kept in the AST.
/// Returns `None` if all items should be kept.
fn get_kept_items_nodes<'a, Item: QueryAttrs<'a> + TypedSyntaxNode<'a>>(
    db: &'a dyn SyntaxGroup,
    cfg_set: &CfgSet,
    all_items: impl Iterator<Item = Item>,
    diagnostics: &mut Vec<PluginDiagnostic<'a>>,
) -> Option<Vec<cairo_lang_syntax::node::SyntaxNode<'a>>> {
    let mut any_dropped = false;
    let mut kept_items_nodes = vec![];
    for item in all_items {
        if should_drop(db, cfg_set, &item, diagnostics) {
            any_dropped = true;
        } else {
            kept_items_nodes.push(item.as_syntax_node());
        }
    }
    if any_dropped { Some(kept_items_nodes) } else { None }
}

/// Check if the given item should be dropped from the AST.
fn should_drop<'a, Item: QueryAttrs<'a>>(
    db: &'a dyn SyntaxGroup,
    cfg_set: &CfgSet,
    item: &Item,
    diagnostics: &mut Vec<PluginDiagnostic<'a>>,
) -> bool {
    item.query_attr(db, CFG_ATTR).any(|attr| {
        match parse_predicate(db, attr.structurize(db), diagnostics) {
            Some(predicate_tree) => !predicate_tree.evaluate(cfg_set),
            None => false,
        }
    })
}

/// Parse `#[cfg(not(ghf)...)]` attribute arguments as a predicate matching [`Cfg`] items.
fn parse_predicate<'a>(
    db: &'a dyn SyntaxGroup,
    attr: Attribute<'a>,
    diagnostics: &mut Vec<PluginDiagnostic<'a>>,
) -> Option<PredicateTree> {
    Some(PredicateTree::And(
        attr.args
            .into_iter()
            .filter_map(|arg| parse_predicate_item(db, arg, diagnostics))
            .collect(),
    ))
}

/// Parse single `#[cfg(...)]` attribute argument as a [`Cfg`] item.
fn parse_predicate_item<'a>(
    db: &'a dyn SyntaxGroup,
    item: AttributeArg<'a>,
    diagnostics: &mut Vec<PluginDiagnostic<'a>>,
) -> Option<PredicateTree> {
    match extract_config_predicate_part(db, &item) {
        Some(ConfigPredicatePart::Cfg(cfg)) => Some(PredicateTree::Cfg(cfg)),
        Some(ConfigPredicatePart::Call(call)) => {
            let operator = call.path(db).as_syntax_node().get_text(db);
            let args = call
                .arguments(db)
                .arguments(db)
                .elements(db)
                .map(|arg| AttributeArg::from_ast(arg, db))
                .collect_vec();

            match operator {
                "not" => {
                    if args.len() != 1 {
                        diagnostics.push(PluginDiagnostic::error(
                            call.stable_ptr(db),
                            "`not` operator expects exactly one argument.".into(),
                        ));
                        None
                    } else {
                        Some(PredicateTree::Not(Box::new(parse_predicate_item(
                            db,
                            args[0].clone(),
                            diagnostics,
                        )?)))
                    }
                }
                "and" => {
                    if args.len() < 2 {
                        diagnostics.push(PluginDiagnostic::error(
                            call.stable_ptr(db),
                            "`and` operator expects at least two arguments.".into(),
                        ));
                        None
                    } else {
                        Some(PredicateTree::And(
                            args.into_iter()
                                .filter_map(|arg| parse_predicate_item(db, arg, diagnostics))
                                .collect(),
                        ))
                    }
                }
                "or" => {
                    if args.len() < 2 {
                        diagnostics.push(PluginDiagnostic::error(
                            call.stable_ptr(db),
                            "`or` operator expects at least two arguments.".into(),
                        ));
                        None
                    } else {
                        Some(PredicateTree::Or(
                            args.into_iter()
                                .filter_map(|arg| parse_predicate_item(db, arg, diagnostics))
                                .collect(),
                        ))
                    }
                }
                _ => {
                    diagnostics.push(PluginDiagnostic::error(
                        call.stable_ptr(db),
                        format!("Unsupported operator: `{operator}`."),
                    ));
                    None
                }
            }
        }
        None => {
            diagnostics.push(PluginDiagnostic::error(
                item.arg.stable_ptr(db).untyped(),
                "Invalid configuration argument.".into(),
            ));
            None
        }
    }
}

/// Extracts a configuration predicate part from an attribute argument.
fn extract_config_predicate_part<'a>(
    db: &dyn SyntaxGroup,
    arg: &AttributeArg<'a>,
) -> Option<ConfigPredicatePart<'a>> {
    match &arg.variant {
        AttributeArgVariant::Unnamed(ast::Expr::Path(path)) => {
            if let Some([ast::PathSegment::Simple(segment)]) =
                path.segments(db).elements(db).collect_array()
            {
                Some(ConfigPredicatePart::Cfg(Cfg::name(segment.identifier(db).to_string())))
            } else {
                None
            }
        }
        AttributeArgVariant::Unnamed(ast::Expr::FunctionCall(call)) => {
            Some(ConfigPredicatePart::Call(call.clone()))
        }
        AttributeArgVariant::Named { name, value } => {
            let value_text = match value {
                ast::Expr::String(terminal) => terminal.string_value(db).unwrap_or_default(),
                ast::Expr::ShortString(terminal) => terminal.string_value(db).unwrap_or_default(),
                _ => return None,
            };

            Some(ConfigPredicatePart::Cfg(Cfg::kv(name.text.clone(), value_text)))
        }
        _ => None,
    }
}
