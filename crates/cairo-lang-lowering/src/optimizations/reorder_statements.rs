#[cfg(test)]
#[path = "reorder_statements_test.rs"]
mod test;

use std::cmp::Reverse;

use cairo_lang_defs::ids::ExternFunctionId;
use cairo_lang_utils::ordered_hash_map::OrderedHashMap;
use cairo_lang_utils::unordered_hash_map::{Entry, UnorderedHashMap};
use cairo_lang_utils::unordered_hash_set::UnorderedHashSet;
use itertools::{Itertools, zip_eq};

use crate::borrow_check::analysis::{Analyzer, BackAnalysis, StatementLocation};
use crate::db::LoweringGroup;
use crate::{
    BlockId, Lowered, MatchInfo, Statement, StatementCall, VarRemapping, VarUsage, VariableId,
};

/// Reorder the statements in the lowering in order to move variable definitions closer to their
/// usage. Statement with no side effects and unused outputs are removed.
///
/// The list of call statements that can be moved is currently hardcoded.
///
/// Removing unnecessary remapping before this optimization will result in better code.
pub fn reorder_statements(db: &dyn LoweringGroup, lowered: &mut Lowered<'_>) {
    if lowered.blocks.is_empty() {
        return;
    }
    let ctx = ReorderStatementsContext {
        db,
        lowered: &*lowered,
        moveable_functions: &db.priv_movable_function_ids(),
        statement_to_move: vec![],
    };
    let mut analysis = BackAnalysis::new(lowered, ctx);
    analysis.get_root_info();
    let ctx = analysis.analyzer;

    let mut changes_by_block =
        OrderedHashMap::<BlockId, Vec<(usize, Option<Statement<'_>>)>>::default();

    for (src, opt_dst) in ctx.statement_to_move {
        changes_by_block.entry(src.0).or_insert_with(Vec::new).push((src.1, None));

        if let Some(dst) = opt_dst {
            let statement = lowered.blocks[src.0].statements[src.1].clone();
            changes_by_block.entry(dst.0).or_insert_with(Vec::new).push((dst.1, Some(statement)));
        }
    }

    for (block_id, block_changes) in changes_by_block {
        let statements = &mut lowered.blocks[block_id].statements;

        // Apply block changes in reverse order to prevent a change from invalidating the
        // indices of the other changes.
        for (index, opt_statement) in
            block_changes.into_iter().sorted_by_key(|(index, _)| Reverse(*index))
        {
            match opt_statement {
                Some(stmt) => statements.insert(index, stmt),
                None => {
                    statements.remove(index);
                }
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct ReorderStatementsInfo {
    // A mapping from var_id to a candidate location that it can be moved to.
    // If the variable is used in multiple match arms we define the next use to be
    // the match.
    next_use: UnorderedHashMap<VariableId, StatementLocation>,
}

pub struct ReorderStatementsContext<'db> {
    db: &'db dyn LoweringGroup,
    lowered: &'db Lowered<'db>,
    // A list of function that can be moved.
    moveable_functions: &'db UnorderedHashSet<ExternFunctionId<'db>>,
    statement_to_move: Vec<(StatementLocation, Option<StatementLocation>)>,
}
impl<'db> ReorderStatementsContext<'db> {
    fn call_can_be_moved(&mut self, stmt: &StatementCall<'db>) -> bool {
        if let Some((extern_id, _)) = stmt.function.get_extern(self.db) {
            self.moveable_functions.contains(&extern_id)
        } else {
            false
        }
    }
}
impl<'db> Analyzer<'db, '_> for ReorderStatementsContext<'db> {
    type Info = ReorderStatementsInfo;

    fn visit_stmt(
        &mut self,
        info: &mut Self::Info,
        statement_location: StatementLocation,
        stmt: &Statement<'db>,
    ) {
        let mut immovable = matches!(stmt, Statement::Call(stmt) if !self.call_can_be_moved(stmt));
        let mut optional_target_location = None;
        for var_to_move in stmt.outputs() {
            let Some((block_id, index)) = info.next_use.remove(var_to_move) else { continue };
            if let Some((target_block_id, target_index)) = &mut optional_target_location {
                *target_index = std::cmp::min(*target_index, index);
                // If the output is used in multiple places we can't move their creation point.
                immovable |= target_block_id != &block_id;
            } else {
                optional_target_location = Some((block_id, index));
            }
        }
        if immovable {
            for var_usage in stmt.inputs() {
                info.next_use.insert(var_usage.var_id, statement_location);
            }
            return;
        }

        if let Some(target_location) = optional_target_location {
            // If the statement is not removed add demand for its inputs.
            for var_usage in stmt.inputs() {
                match info.next_use.entry(var_usage.var_id) {
                    Entry::Occupied(mut e) => {
                        // Since we don't know where `e.get()` and `target_location` converge
                        // we use `statement_location` as a conservative estimate.
                        &e.insert(statement_location)
                    }
                    Entry::Vacant(e) => e.insert(target_location),
                };
            }

            self.statement_to_move.push((statement_location, Some(target_location)))
        } else if stmt
            .inputs()
            .iter()
            .all(|v| self.lowered.variables[v.var_id].info.droppable.is_ok())
        {
            // If a movable statement is unused, and all its inputs are droppable removing it is
            // valid.
            self.statement_to_move.push((statement_location, None))
        } else {
            // Statement is unused but can't be removed.
            for var_usage in stmt.inputs() {
                info.next_use.insert(var_usage.var_id, statement_location);
            }
        }
    }

    fn visit_goto(
        &mut self,
        info: &mut Self::Info,
        statement_location: StatementLocation,
        _target_block_id: BlockId,
        remapping: &VarRemapping<'db>,
    ) {
        for VarUsage { var_id, .. } in remapping.values() {
            info.next_use.insert(*var_id, statement_location);
        }
    }

    fn merge_match(
        &mut self,
        statement_location: StatementLocation,
        match_info: &MatchInfo<'db>,
        infos: impl Iterator<Item = Self::Info>,
    ) -> Self::Info {
        let mut infos = zip_eq(infos, match_info.arms()).map(|(mut info, arm)| {
            for var_id in &arm.var_ids {
                info.next_use.remove(var_id);
            }
            info
        });
        let mut info = infos.next().unwrap_or_default();
        for arm_info in infos {
            info.next_use.merge(&arm_info.next_use, |e, _| {
                *e.into_mut() = statement_location;
            });
        }

        for var_usage in match_info.inputs() {
            info.next_use.insert(var_usage.var_id, statement_location);
        }

        info
    }

    fn info_from_return(
        &mut self,
        statement_location: StatementLocation,
        vars: &[VarUsage<'db>],
    ) -> Self::Info {
        let mut info = Self::Info::default();
        for var_usage in vars {
            info.next_use.insert(var_usage.var_id, statement_location);
        }
        info
    }
}
