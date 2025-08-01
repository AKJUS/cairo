#[cfg(test)]
#[path = "resolve_labels_test.rs"]
mod test;

use cairo_lang_defs::diagnostic_utils::StableLocation;
use cairo_lang_sierra::program;

use crate::next_statement_index_fetch::NextStatementIndexFetch;
use crate::pre_sierra;

/// Replaces labels with their corresponding StatementIdx.
pub fn resolve_labels_and_extract_locations<'db>(
    statements: Vec<pre_sierra::StatementWithLocation<'db>>,
    label_replacer: &LabelReplacer<'_>,
) -> (Vec<program::Statement>, Vec<Vec<StableLocation<'db>>>) {
    statements
        .into_iter()
        .filter_map(|statement| match statement.statement {
            pre_sierra::Statement::Sierra(sierra_statement) => {
                Some((label_replacer.handle_statement(sierra_statement), statement.location))
            }
            pre_sierra::Statement::Label(_) => None,
            pre_sierra::Statement::PushValues(_) => {
                panic!("Unexpected pre_sierra::Statement:PushValues in resolve_labels().")
            }
        })
        .unzip()
}

/// Helper struct for resolve_labels.
pub struct LabelReplacer<'db> {
    next_statement_index_fetch: NextStatementIndexFetch<'db>,
}
impl<'db> LabelReplacer<'db> {
    pub fn from_statements(
        statements: &[pre_sierra::StatementWithLocation<'db>],
    ) -> LabelReplacer<'db> {
        Self { next_statement_index_fetch: NextStatementIndexFetch::new(statements, false) }
    }

    /// Replaces the pre-sierra labels in the given statement, and returns [program::Statement].
    fn handle_statement(
        &self,
        statement: program::GenStatement<pre_sierra::LabelId<'db>>,
    ) -> program::Statement {
        match statement {
            program::GenStatement::Invocation(invocation) => {
                program::Statement::Invocation(self.handle_invocation(invocation))
            }
            program::GenStatement::Return(statement) => program::Statement::Return(statement),
        }
    }

    /// Replaces the pre-sierra labels in the given invocation, and returns [program::Invocation].
    fn handle_invocation(
        &self,
        invocation: program::GenInvocation<pre_sierra::LabelId<'db>>,
    ) -> program::Invocation {
        program::Invocation {
            libfunc_id: invocation.libfunc_id,
            args: invocation.args,
            branches: invocation
                .branches
                .into_iter()
                .map(|branch_info| self.handle_branch_info(branch_info))
                .collect(),
        }
    }

    /// Replaces the pre-sierra labels in the given branch info, and returns [program::BranchInfo].
    fn handle_branch_info(
        &self,
        branch_info: program::GenBranchInfo<pre_sierra::LabelId<'db>>,
    ) -> program::BranchInfo {
        program::BranchInfo {
            target: self.handle_branch_target(branch_info.target),
            results: branch_info.results,
        }
    }

    /// Replaces the pre-sierra labels in the given branch target, and returns
    /// [program::BranchTarget].
    fn handle_branch_target(
        &self,
        branch_target: program::GenBranchTarget<pre_sierra::LabelId<'db>>,
    ) -> program::BranchTarget {
        match branch_target {
            program::GenBranchTarget::Fallthrough => program::GenBranchTarget::Fallthrough,
            program::GenBranchTarget::Statement(label_id) => {
                program::BranchTarget::Statement(self.handle_label_id(label_id))
            }
        }
    }

    /// Resolves the given pre-sierra label, and returns [program::StatementIdx].
    pub fn handle_label_id(&self, label_id: pre_sierra::LabelId<'db>) -> program::StatementIdx {
        program::StatementIdx(self.next_statement_index_fetch.resolve_label(&label_id))
    }
}
