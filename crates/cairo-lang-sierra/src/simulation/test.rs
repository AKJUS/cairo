use bimap::BiMap;
use num_bigint::BigInt;
use test_case::test_case;

use super::LibfuncSimulationError::{
    self, FunctionSimulationError, WrongArgType, WrongNumberOfArgs,
};
use super::value::CoreValue::{
    self, Array, GasBuiltin, RangeCheck, Uint32, Uint128, Uninitialized,
};
use super::{SimulationError, core};
use crate::extensions::GenericLibfunc;
use crate::extensions::core::CoreLibfunc;
use crate::extensions::lib_func::{SignatureSpecializationContext, SpecializationContext};
use crate::extensions::type_specialization_context::TypeSpecializationContext;
use crate::extensions::types::TypeInfo;
use crate::ids::{ConcreteTypeId, FunctionId, GenericTypeId};
use crate::program::{ConcreteTypeLongId, Function, FunctionSignature, GenericArg, StatementIdx};
use crate::test_utils::build_bijective_mapping;

fn type_arg(name: &str) -> GenericArg {
    GenericArg::Type(name.into())
}

fn value_arg(v: i64) -> GenericArg {
    GenericArg::Value(BigInt::from(v))
}

fn user_func_arg(name: &str) -> GenericArg {
    GenericArg::UserFunc(name.into())
}

struct MockSpecializationContext {
    mapping: BiMap<ConcreteTypeId, ConcreteTypeLongId>,
}
impl MockSpecializationContext {
    pub fn new() -> Self {
        Self { mapping: build_bijective_mapping() }
    }
}

impl SpecializationContext for MockSpecializationContext {
    fn try_get_function(&self, function_id: &FunctionId) -> Option<Function> {
        ["drop_all_inputs", "identity", "unimplemented"]
            .into_iter()
            .map(|name| -> FunctionId { name.into() })
            .find(|id: &FunctionId| id == function_id)
            .map(|_| Function::new(function_id.clone(), vec![], vec![], StatementIdx(0)))
    }
}
impl TypeSpecializationContext for MockSpecializationContext {
    fn try_get_type_info(&self, id: ConcreteTypeId) -> Option<TypeInfo> {
        if id == "u128".into() || id == "u64".into() || id == "NonZeroInt".into() {
            Some(TypeInfo {
                long_id: self.mapping.get_by_left(&id)?.clone(),
                storable: true,
                droppable: true,
                duplicatable: true,
                zero_sized: false,
            })
        } else if id == "UninitializedInt".into() {
            Some(TypeInfo {
                long_id: self.mapping.get_by_left(&id)?.clone(),
                storable: false,
                droppable: true,
                duplicatable: false,
                zero_sized: true,
            })
        } else if id == "ArrayU128".into() {
            Some(TypeInfo {
                long_id: self.mapping.get_by_left(&id)?.clone(),
                storable: true,
                droppable: true,
                duplicatable: false,
                zero_sized: false,
            })
        } else {
            None
        }
    }
}
impl SignatureSpecializationContext for MockSpecializationContext {
    fn try_get_concrete_type(
        &self,
        id: GenericTypeId,
        generic_args: &[GenericArg],
    ) -> Option<ConcreteTypeId> {
        self.mapping
            .get_by_right(&ConcreteTypeLongId {
                generic_id: id,
                generic_args: generic_args.to_vec(),
            })
            .cloned()
    }

    fn try_get_function_signature(&self, function_id: &FunctionId) -> Option<FunctionSignature> {
        self.try_get_function(function_id).map(|f| f.signature)
    }
}

/// Expects to find a libfunc and simulate it.
fn simulate(
    id: &str,
    generic_args: Vec<GenericArg>,
    inputs: Vec<CoreValue>,
) -> Result<(Vec<CoreValue>, usize), LibfuncSimulationError> {
    core::simulate(
        &CoreLibfunc::by_id(&id.into())
            .unwrap()
            .specialize(&MockSpecializationContext::new(), &generic_args)
            .unwrap(),
        inputs,
        || Some(4),
        |id, inputs| {
            if id == &"drop_all_inputs".into() {
                Ok(vec![])
            } else if id == &"identity".into() {
                Ok(inputs)
            } else {
                Err(FunctionSimulationError(
                    id.clone(),
                    Box::new(SimulationError::StatementOutOfBounds(StatementIdx(0))),
                ))
            }
        },
    )
}

#[test_case("withdraw_gas", vec![], vec![RangeCheck, GasBuiltin(5)]
             => Ok((vec![RangeCheck, GasBuiltin(1)], 0)); "withdraw_gas(5)")]
#[test_case("withdraw_gas", vec![], vec![RangeCheck, GasBuiltin(2)]
             => Ok((vec![RangeCheck, GasBuiltin(2)], 1)); "withdraw_gas(2)")]
#[test_case("u128_is_zero", vec![], vec![Uint128(2)]
             => Ok((vec![Uint128(2)], 1)); "u128_is_zero(2)")]
#[test_case("u128_is_zero", vec![], vec![Uint128(0)] => Ok((vec![], 0)); "u128_is_zero(0)")]
#[test_case("jump", vec![], vec![] => Ok((vec![], 0)); "jump()")]
#[test_case("u128_overflowing_add", vec![], vec![RangeCheck, Uint128(2), Uint128(3)]
             => Ok((vec![RangeCheck, Uint128(5)], 0));
            "u128_overflowing_add(2, 3)")]
#[test_case("u128_overflowing_sub", vec![], vec![RangeCheck, Uint128(5), Uint128(3)]
             => Ok((vec![RangeCheck, Uint128(2)], 0));
            "u128_overflowing_sub(5, 3)")]
#[test_case("u128_overflowing_sub", vec![], vec![RangeCheck, Uint128(3), Uint128(5)]
             => Ok((vec![RangeCheck, Uint128(u128::MAX - 1)], 1));
            "u128_overflowing_sub(3, 5)")]
fn simulate_branch(
    id: &str,
    generic_args: Vec<GenericArg>,
    inputs: Vec<CoreValue>,
) -> Result<(Vec<CoreValue>, usize), LibfuncSimulationError> {
    simulate(id, generic_args, inputs)
}

/// Tests for simulation of a non branch invocations.
#[test_case("redeposit_gas", vec![], vec![GasBuiltin(2)] => Ok(vec![GasBuiltin(6)]); "redeposit_gas(2)")]
#[test_case("array_new", vec![type_arg("u128")], vec![] => Ok(vec![Array(vec![])]); "array_new()")]
#[test_case("array_append", vec![type_arg("u128")], vec![Array(vec![]), Uint128(4)] =>
            Ok(vec![Array(vec![Uint128(4)])]); "array_append([], 4)")]
#[test_case("array_get", vec![type_arg("u128")], vec![RangeCheck, Array(vec![Uint128(5)]), Uint32(0)]
             => Ok(vec![RangeCheck, Uint128(5)]); "array_get([5], 0)")]
#[test_case("array_len", vec![type_arg("u128")], vec![Array(vec![])] =>
            Ok(vec![Uint32(0)]); "array_len([])")]
#[test_case("u128_safe_divmod", vec![], vec![RangeCheck, Uint128(32), Uint128(5)]
             => Ok(vec![RangeCheck, Uint128(6), Uint128(2)]); "u128_safe_divmod(32, 5)")]
#[test_case("u128_const", vec![value_arg(3)], vec![] => Ok(vec![Uint128(3)]);
            "u128_const<3>()")]
#[test_case("dup", vec![type_arg("u128")], vec![Uint128(24)]
             => Ok(vec![Uint128(24), Uint128(24)]); "dup<u128>(24)")]
#[test_case("drop", vec![type_arg("u128")], vec![Uint128(2)] => Ok(vec![]); "drop<u128>(2)")]
#[test_case("unwrap_non_zero", vec![type_arg("u128")], vec![Uint128(6)]
             => Ok(vec![Uint128(6)]); "unwrap_non_zero<u128>(6)")]
#[test_case("store_temp", vec![type_arg("u128")], vec![Uint128(6)] => Ok(vec![Uint128(6)]);
            "store_temp<u128>(6)")]
#[test_case("store_local", vec![type_arg("u128")], vec![Uninitialized, Uint128(6)]
             => Ok(vec![Uint128(6)]); "store_local<u128>(_, 6)")]
#[test_case("finalize_locals", vec![], vec![] => Ok(vec![]); "finalize_locals()")]
#[test_case("rename", vec![type_arg("u128")], vec![Uint128(6)] => Ok(vec![Uint128(6)]);
            "rename<u128>(6)")]
#[test_case("function_call", vec![user_func_arg("drop_all_inputs")], vec![Uint128(3), Uint128(5)]
             => Ok(vec![]); "function_call<drop_all_inputs>()")]
#[test_case("function_call", vec![user_func_arg("identity")], vec![Uint128(3), Uint128(5)]
             => Ok(vec![Uint128(3), Uint128(5)]); "function_call<identity>()")]
fn simulate_none_branch(
    id: &str,
    generic_args: Vec<GenericArg>,
    inputs: Vec<CoreValue>,
) -> Result<Vec<CoreValue>, LibfuncSimulationError> {
    simulate(id, generic_args, inputs).map(|(outputs, chosen_branch)| {
        assert_eq!(chosen_branch, 0);
        outputs
    })
}

#[test_case("withdraw_gas", vec![], vec![RangeCheck, Uninitialized] => WrongArgType;
            "withdraw_gas(empty)")]
#[test_case("withdraw_gas", vec![], vec![] => WrongNumberOfArgs; "withdraw_gas()")]
#[test_case("redeposit_gas", vec![], vec![Uninitialized] => WrongArgType;
            "redeposit_gas(empty)")]
#[test_case("redeposit_gas", vec![], vec![] => WrongNumberOfArgs; "redeposit_gas()")]
#[test_case("u128_overflowing_add", vec![], vec![RangeCheck, Uint128(1)] => WrongNumberOfArgs;
            "u128_overflowing_add(1)")]
#[test_case("u128_overflowing_sub", vec![], vec![RangeCheck, Uint128(1)] => WrongNumberOfArgs;
            "u128_overflowing_sub(1)")]
#[test_case("u128_safe_divmod", vec![], vec![RangeCheck, Uint128(1)] => WrongNumberOfArgs;
            "u128_safe_divmod(1)")]
#[test_case("u128_const", vec![value_arg(3)], vec![Uint128(1)] => WrongNumberOfArgs;
            "u128_const<3>(1)")]
#[test_case("dup", vec![type_arg("u128")], vec![] => WrongNumberOfArgs; "dup<u128>()")]
#[test_case("drop", vec![type_arg("u128")], vec![] => WrongNumberOfArgs; "drop<u128>()")]
#[test_case("u128_is_zero", vec![], vec![] => WrongNumberOfArgs; "u128_is_zero()")]
#[test_case("store_temp", vec![type_arg("u128")], vec![] => WrongNumberOfArgs;
            "store_temp<u128>()")]
#[test_case("store_local", vec![type_arg("u128")], vec![] => WrongNumberOfArgs;
            "store_local<u128>()")]
#[test_case("finalize_locals", vec![], vec![Uint128(4)] => WrongNumberOfArgs; "finalize_locals(4)")]
#[test_case("rename", vec![type_arg("u128")], vec![] => WrongNumberOfArgs; "rename<u128>()")]
#[test_case("jump", vec![], vec![Uint128(4)] => WrongNumberOfArgs; "jump(4)")]
#[test_case("function_call", vec![user_func_arg("unimplemented")], vec![] =>
            FunctionSimulationError(
                "unimplemented".into(),
                Box::new(SimulationError::StatementOutOfBounds(StatementIdx(0))));
            "function_call<unimplemented>()")]
fn simulate_error(
    id: &str,
    generic_args: Vec<GenericArg>,
    inputs: Vec<CoreValue>,
) -> LibfuncSimulationError {
    simulate(id, generic_args, inputs).err().unwrap()
}
