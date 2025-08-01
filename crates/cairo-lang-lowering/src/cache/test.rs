use cairo_lang_filesystem::ids::BlobLongId;
use cairo_lang_semantic::test_utils::{setup_test_function, setup_test_function_ex};
use cairo_lang_test_utils::parse_test_file::TestRunnerResult;
use cairo_lang_test_utils::verify_diagnostics_expectation;
use cairo_lang_utils::Intern;
use cairo_lang_utils::ordered_hash_map::OrderedHashMap;

use super::generate_crate_cache;
use crate::LoweringStage;
use crate::db::LoweringGroup;
use crate::ids::ConcreteFunctionWithBodyId;
use crate::test_utils::{LoweringDatabaseForTesting, formatted_lowered};

cairo_lang_test_utils::test_file_test!(
    cache,
    "src/cache/test_data",
    {
        cache: "cache",
    },
    test_cache_check
);

fn test_cache_check(
    inputs: &OrderedHashMap<String, String>,
    args: &OrderedHashMap<String, String>,
) -> TestRunnerResult {
    let db = &mut LoweringDatabaseForTesting::default();

    let (test_function, _semantic_diagnostics) = setup_test_function(
        db,
        inputs["function"].as_str(),
        inputs["function_name"].as_str(),
        inputs["module_code"].as_str(),
    )
    .split();

    let artifact = generate_crate_cache(db, test_function.module_id.owning_crate(db)).unwrap();
    let new_db = LoweringDatabaseForTesting::new();
    let cached_file = BlobLongId::Virtual(artifact).intern(&new_db);
    let (test_function, semantic_diagnostics) = setup_test_function_ex(
        &new_db,
        inputs["function"].as_str(),
        inputs["function_name"].as_str(),
        inputs["module_code"].as_str(),
        None,
        Some(cached_file),
    )
    .split();

    let function_id: ConcreteFunctionWithBodyId<'_> =
        ConcreteFunctionWithBodyId::from_semantic(&new_db, test_function.concrete_function_id);

    let lowered = new_db.lowered_body(function_id, LoweringStage::Final);
    if let Ok(lowered) = &lowered {
        assert!(
            lowered.blocks.iter().all(|(_, b)| b.is_set()),
            "There should not be any unset flat blocks"
        );
    }
    let diagnostics =
        new_db.module_lowering_diagnostics(test_function.module_id).unwrap_or_default();
    let formatted_lowering_diagnostics = diagnostics.format(&new_db);
    let combined_diagnostics = format!("{semantic_diagnostics}\n{formatted_lowering_diagnostics}");
    let error = verify_diagnostics_expectation(args, &combined_diagnostics);
    TestRunnerResult {
        outputs: OrderedHashMap::from([
            ("semantic_diagnostics".into(), semantic_diagnostics),
            ("lowering_diagnostics".into(), formatted_lowering_diagnostics),
            ("lowering_flat".into(), formatted_lowered(&new_db, lowered.ok().as_deref())),
        ]),
        error,
    }
}
