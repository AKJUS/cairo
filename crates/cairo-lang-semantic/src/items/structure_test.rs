use cairo_lang_debug::DebugWithDb;
use cairo_lang_defs::ids::ModuleItemId;
use cairo_lang_utils::{Intern, extract_matches};
use indoc::indoc;
use pretty_assertions::assert_eq;
use smol_str::SmolStr;
use test_log::test;

use crate::db::SemanticGroup;
use crate::test_utils::{SemanticDatabaseForTesting, setup_test_module};

#[test]
fn test_struct() {
    let db_val = SemanticDatabaseForTesting::default();
    let db = &db_val;
    let (test_module, diagnostics) = setup_test_module(
        db,
        indoc::indoc! {"
            #[inline(MyImpl1, MyImpl2)]
            struct A {
                a: felt252,
                pub b: (felt252, felt252),
                pub(crate) c: (),
                a: (),
                a: ()
            }

            fn foo(a: A) {
                5;
            }
        "},
    )
    .split();
    assert_eq!(
        diagnostics,
        indoc! {r#"
        error: Redefinition of member "a" on struct "test::A".
         --> lib.cairo:6:5
            a: (),
            ^^^^^

        error: Redefinition of member "a" on struct "test::A".
         --> lib.cairo:7:5
            a: ()
            ^^^^^

        "#}
    );
    let module_id = test_module.module_id;

    let struct_id = extract_matches!(
        db.module_item_by_name(module_id, SmolStr::from("A").intern(db)).unwrap().unwrap(),
        ModuleItemId::Struct
    );
    let actual = db
        .struct_members(struct_id)
        .unwrap()
        .iter()
        .map(|(name, member)| format!("{}: {:?}", name.long(db), member.debug(db)))
        .collect::<Vec<_>>()
        .join(",\n");
    assert_eq!(
        actual,
        indoc! {"
            a: Member { id: MemberId(test::a), ty: (), visibility: Private },
            b: Member { id: MemberId(test::b), ty: (core::felt252, core::felt252), visibility: Public },
            c: Member { id: MemberId(test::c), ty: (), visibility: PublicInCrate }"}
    );

    assert_eq!(
        db.struct_attributes(struct_id)
            .unwrap()
            .iter()
            .map(|attr| format!("{:?}", attr.debug(db)))
            .collect::<Vec<_>>()
            .join(",\n"),
        r#"Attribute { id: "inline", args: ["MyImpl1", "MyImpl2", ] }"#
    );
}
