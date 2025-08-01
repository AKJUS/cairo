use cairo_lang_debug::DebugWithDb;
use cairo_lang_defs::ids::NamedLanguageElementId;

use super::constant::ConstValue;
use crate::db::SemanticGroup;
use crate::{ConcreteVariant, MatchArmSelector};

impl<'db> DebugWithDb<'db> for ConstValue<'db> {
    type Db = dyn SemanticGroup;

    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        db: &'db (dyn SemanticGroup + 'static),
    ) -> std::fmt::Result {
        let semantic_db = db.upcast();
        match self {
            ConstValue::Int(value, _ty) => write!(f, "{value}"),
            ConstValue::Struct(inner, _) => {
                write!(f, "{{")?;
                let mut inner = inner.iter().peekable();
                while let Some(value) = inner.next() {
                    write!(f, " ")?;
                    value.fmt(f, semantic_db)?;
                    write!(f, ": ")?;
                    value.ty(semantic_db).unwrap().fmt(f, semantic_db.elongate())?;
                    if inner.peek().is_some() {
                        write!(f, ",")?;
                    } else {
                        write!(f, " ")?;
                    }
                }
                write!(f, "}}")
            }
            ConstValue::Enum(variant, inner) => {
                variant.fmt(f, semantic_db)?;
                write!(f, "(")?;
                inner.fmt(f, semantic_db)?;
                write!(f, ")")
            }
            ConstValue::NonZero(value) => {
                write!(f, "NonZero(")?;
                value.fmt(f, semantic_db)?;
                write!(f, ")")
            }
            ConstValue::Boxed(value) => {
                value.fmt(f, semantic_db)?;
                write!(f, ".into_box()")
            }
            ConstValue::Generic(param) => write!(f, "{}", param.debug_name(semantic_db)),
            ConstValue::Var(var, _) => write!(f, "?{}", var.id.0),
            ConstValue::Missing(_) => write!(f, "missing"),
            ConstValue::ImplConstant(id) => id.fmt(f, semantic_db),
        }
    }
}

impl<'db> DebugWithDb<'db> for ConcreteVariant<'db> {
    type Db = dyn SemanticGroup;

    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        semantic_db: &'db (dyn SemanticGroup + 'static),
    ) -> std::fmt::Result {
        let semantic_db = semantic_db.upcast();
        let enum_name = self.concrete_enum_id.enum_id(semantic_db).name(semantic_db);
        let variant_name = self.id.name(semantic_db);
        write!(f, "{enum_name}::{variant_name}")
    }
}

impl<'db> DebugWithDb<'db> for MatchArmSelector<'db> {
    type Db = dyn SemanticGroup;

    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
        semantic_db: &'db (dyn SemanticGroup + 'static),
    ) -> std::fmt::Result {
        let semantic_db = semantic_db.upcast();
        match self {
            MatchArmSelector::VariantId(variant_id) => {
                write!(f, "{:?}", variant_id.debug(semantic_db))
            }
            MatchArmSelector::Value(s) => {
                write!(f, "{:?}", s.value)
            }
        }
    }
}
