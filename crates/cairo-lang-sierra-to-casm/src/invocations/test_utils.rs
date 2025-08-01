use cairo_lang_casm::ap_change::ApChange;
use cairo_lang_casm::instructions::Instruction;
use cairo_lang_sierra::extensions::core::{CoreLibfunc, CoreType};
use cairo_lang_sierra::extensions::lib_func::{
    SignatureSpecializationContext, SpecializationContext,
};
use cairo_lang_sierra::extensions::type_specialization_context::TypeSpecializationContext;
use cairo_lang_sierra::extensions::types::TypeInfo;
use cairo_lang_sierra::extensions::{
    ConcreteLibfunc, ConcreteType, GenericLibfuncEx, GenericTypeEx,
};
use cairo_lang_sierra::ids::{ConcreteTypeId, VarId};
use cairo_lang_sierra::program::{BranchInfo, BranchTarget, Invocation, StatementIdx};
use cairo_lang_sierra_ap_change::ap_change_info::ApChangeInfo;
use cairo_lang_sierra_gas::gas_info::GasInfo;
use cairo_lang_sierra_type_size::TypeSizeMap;
use itertools::{Itertools, zip_eq};

use super::{CompiledInvocation, ProgramInfo, compile_invocation};
use crate::environment::Environment;
use crate::environment::gas_wallet::GasWallet;
use crate::metadata::Metadata;
use crate::references::{IntroductionPoint, ReferenceExpression, ReferenceValue};
use crate::relocations::RelocationEntry;

/// Creates a Felt252BinaryOperator from a token operator.
#[macro_export]
macro_rules! cell_expr_operator {
    (+) => {
        cairo_lang_casm::cell_expression::CellOperator::Add
    };
    (-) => {
        cairo_lang_casm::cell_expression::CellOperator::Sub
    };
    (*) => {
        cairo_lang_casm::cell_expression::CellOperator::Mul
    };
    (/) => {
        cairo_lang_casm::cell_expression::CellOperator::Div
    };
}

/// Adds a cell expression to a ReferenceExpression cells vector.
#[macro_export]
macro_rules! ref_expr_extend {
    ($cells:ident) => {};
    ($cells:ident, [$a:ident $($op:tt $offset:expr)?] $(, $tok:tt)*) => {
        $cells.push(
            cairo_lang_casm::cell_expression::CellExpression::Deref(cairo_lang_casm::deref!([$a $($op $offset)?]))
        );
        $crate::ref_expr_extend!($cells $(, $tok)*)
    };
    ($cells:ident, [$a:ident $($op:tt $offset:expr)?] $operator:tt $b:tt $(, $tok:tt)*) => {
        $cells.push(
            cairo_lang_casm::cell_expression::CellExpression::BinOp {
                op: $crate::cell_expr_operator!($operator),
                a: cairo_lang_casm::deref!([$a $($op $offset)?]),
                b: cairo_lang_casm::deref_or_immediate!($b),
        });
        $crate::ref_expr_extend!($cells $(, $tok)*)
    };
    ($cells:ident, [[$a:ident $($op:tt $offset:expr)?]] $(, $tok:tt)*) => {
        $cells.push(
            cairo_lang_casm::cell_expression::CellExpression::DoubleDeref(cairo_lang_casm::deref!([$a $($op $offset)?]), 0)
        );
        $crate::ref_expr_extend!($cells $(, $tok)*)
    };
    ($cells:ident, [[$a:ident $($op:tt $offset:expr)?] + $offset2:expr] $(, $tok:tt)*) => {
        $cells.push(
            cairo_lang_casm::cell_expression::CellExpression::DoubleDeref(cairo_lang_casm::deref!([$a $($op $offset)?]), $offset2)
        );
        $crate::ref_expr_extend!($cells $(, $tok)*)
    };
    ($cells:ident, $a:expr $(, $tok:tt)*) => {
        cells.push(
            cairo_lang_casm::cell_expression::CellExpression::Immediate(num_bigint::BigInt::from($a))
        );
        $crate::ref_expr_extend!($cells $(, $tok)*)
    };
    ($cells:ident, _ $(, $tok:tt)*) => {
        cells.push(cairo_lang_casm::cell_expression::CellExpression::Padding);
        $crate::ref_expr_extend!($cells $(, $tok)*)
    };
}

// TODO(orizi): Make this flexible enough to be used in outside of testing.
/// Creates a reference expression.
#[macro_export]
macro_rules! ref_expr {
    {$($tok:tt)*} => {
        {
            let mut cells = Vec::new();
            #[allow(clippy::vec_init_then_push)]
            {
                $crate::ref_expr_extend!(cells, $($tok)*);
            }
            $crate::references::ReferenceExpression { cells }
        }
    }
}

/// Specialization context for libfuncs and types, based on string names only, allows building
/// libfuncs without salsa db or program registry.
struct MockSpecializationContext {}
impl TypeSpecializationContext for MockSpecializationContext {
    fn try_get_type_info(&self, id: ConcreteTypeId) -> Option<TypeInfo> {
        let long_id = cairo_lang_sierra::ConcreteTypeLongIdParser::new()
            .parse(id.to_string().as_str())
            .unwrap();
        Some(
            CoreType::specialize_by_id(self, &long_id.generic_id, &long_id.generic_args)
                .ok()?
                .info()
                .clone(),
        )
    }
}
impl SignatureSpecializationContext for MockSpecializationContext {
    fn try_get_concrete_type(
        &self,
        id: cairo_lang_sierra::ids::GenericTypeId,
        generic_args: &[cairo_lang_sierra::program::GenericArg],
    ) -> Option<ConcreteTypeId> {
        Some(if generic_args.is_empty() {
            id.to_string().into()
        } else {
            format!(
                "{id}<{}>",
                generic_args
                    .iter()
                    .map(cairo_lang_sierra::program::GenericArg::to_string)
                    .join(", ")
            )
            .into()
        })
    }

    fn try_get_function_signature(
        &self,
        _function_id: &cairo_lang_sierra::ids::FunctionId,
    ) -> Option<cairo_lang_sierra::program::FunctionSignature> {
        unreachable!("Function related specialization functionalities are not implemented.")
    }
}
impl SpecializationContext for MockSpecializationContext {
    fn try_get_function(
        &self,
        _function_id: &cairo_lang_sierra::ids::FunctionId,
    ) -> Option<cairo_lang_sierra::program::Function> {
        unreachable!("Function related specialization functionalities are not implemented.")
    }
}

/// Information from [BranchChanges] we should test when only testing a libfunc lowering by itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReducedBranchChanges {
    /// New references defined at a given branch.
    /// should correspond to BranchInfo.results.
    pub refs: Vec<ReferenceExpression>,
    /// The change to AP caused by the libfunc in the branch.
    pub ap_change: ApChange,
}

/// Information from [CompiledInvocation] we should test when only testing a libfunc lowering by
/// itself.
#[derive(Eq, PartialEq)]
pub struct ReducedCompiledInvocation {
    /// A vector of instructions that implement the invocation.
    pub instructions: Vec<Instruction>,
    /// A vector of static relocations.
    pub relocations: Vec<RelocationEntry>,
    /// A vector of ReducedBranchChanges, should correspond to the branches of the invocation
    /// statement.
    pub results: Vec<ReducedBranchChanges>,
}
impl ReducedCompiledInvocation {
    fn new(compiled_invocation: CompiledInvocation) -> ReducedCompiledInvocation {
        ReducedCompiledInvocation {
            instructions: compiled_invocation.instructions,
            relocations: compiled_invocation.relocations,
            results: compiled_invocation
                .results
                .into_iter()
                .map(|result| ReducedBranchChanges {
                    refs: result.refs.into_iter().map(|r| r.expression).collect(),
                    ap_change: result.ap_change,
                })
                .collect(),
        }
    }
}
impl std::fmt::Debug for ReducedCompiledInvocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReducedCompiledInvocation")
            .field(
                "instructions",
                &self.instructions.iter().map(|inst| format!("{inst};")).collect_vec(),
            )
            .field("relocations", &self.relocations)
            .field("results", &self.results)
            .finish()
    }
}

/// Compiles a libfunc into a [ReducedCompiledInvocation].
/// the arguments are auto-filled according to the signature
/// I.e. the libfunc is invoked by:
/// libfunc(*refs) -> {
///     0([0], [2],..., [n_0])
///     1([0], [2],..., [n_1])
///     ...
///     Fallthrough([0], [2],..., [n_fallthrough_idx])
///     ...
///     k([0], [2],..., [n_k])
/// }
///
/// Currently, only works if all the libfunc's types (both inputs and output) are of size 1.
pub fn compile_libfunc(libfunc: &str, refs: Vec<ReferenceExpression>) -> ReducedCompiledInvocation {
    let long_id = cairo_lang_sierra::ConcreteLibfuncLongIdParser::new()
        .parse(libfunc.to_string().as_str())
        .unwrap();
    let context = MockSpecializationContext {};
    let libfunc =
        CoreLibfunc::specialize_by_id(&context, &long_id.generic_id, &long_id.generic_args)
            .unwrap();

    let mut type_sizes: TypeSizeMap = Default::default();
    for param in libfunc.param_signatures() {
        type_sizes.insert(param.ty.clone(), 1);
    }
    for branch_signature in libfunc.branch_signatures() {
        for var in &branch_signature.vars {
            type_sizes.insert(var.ty.clone(), 1);
        }
    }
    let program_info = ProgramInfo {
        metadata: &Metadata {
            ap_change_info: ApChangeInfo {
                variable_values: Default::default(),
                function_ap_change: Default::default(),
            },
            gas_info: GasInfo {
                variable_values: Default::default(),
                function_costs: Default::default(),
            },
        },
        circuits_info: &Default::default(),
        type_sizes: &type_sizes,
        const_data_values: &|_| panic!("const_data_values not implemented for tests."),
    };

    let args: Vec<ReferenceValue> = zip_eq(refs, libfunc.param_signatures())
        .map(|(expression, param)| ReferenceValue {
            expression,
            ty: param.ty.clone(),
            stack_idx: None,
            introduction_point: IntroductionPoint {
                source_statement_idx: None,
                destination_statement_idx: StatementIdx(0),
                output_idx: 0,
            },
        })
        .collect();

    let environment: Environment = Environment::new(GasWallet::Disabled);
    ReducedCompiledInvocation::new(
        compile_invocation(
            program_info,
            &Invocation {
                libfunc_id: "".into(),
                args: (0..args.len() as u64).map(VarId::new).collect(),
                branches: libfunc
                    .branch_signatures()
                    .iter()
                    .enumerate()
                    .map(|(i, branch)| BranchInfo {
                        target: if libfunc.fallthrough() == Some(i) {
                            BranchTarget::Fallthrough
                        } else {
                            BranchTarget::Statement(StatementIdx(i))
                        },
                        results: (0..branch.vars.len() as u64).map(VarId::new).collect(),
                    })
                    .collect(),
            },
            &libfunc,
            StatementIdx(0),
            &args,
            environment,
        )
        .expect("Failed to compile invocation."),
    )
}
