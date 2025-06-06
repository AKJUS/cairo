use cairo_lang_defs::plugin::GeneratedFileAuxData;
use serde::{Deserialize, Serialize};

use super::events::EventData;

/// Contract related auxiliary data of the Starknet plugin.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarknetContractAuxData {
    /// A list of contracts that were processed by the plugin.
    pub contract_name: smol_str::SmolStr,
}

#[typetag::serde]
impl GeneratedFileAuxData for StarknetContractAuxData {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn eq(&self, other: &dyn GeneratedFileAuxData) -> bool {
        if let Some(other) = other.as_any().downcast_ref::<Self>() { self == other } else { false }
    }
}
/// Contract related auxiliary data of the Starknet plugin.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarknetEventAuxData {
    pub event_data: EventData,
}
#[typetag::serde]
impl GeneratedFileAuxData for StarknetEventAuxData {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn eq(&self, other: &dyn GeneratedFileAuxData) -> bool {
        if let Some(other) = other.as_any().downcast_ref::<Self>() { self == other } else { false }
    }
}
