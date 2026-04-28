use agave_feature_set::FeatureSet;
use solana_compute_budget::compute_budget::ComputeBudget;
use solana_fee_structure::FeeStructure;
use solana_hash::Hash;

/// Block-scoped runtime state exposed by the VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEnv {
    pub latest_blockhash: Hash,
    pub slot: u64,
}

/// Internal VM configuration that influences validation and fees.
#[derive(Debug, Clone)]
pub struct SvmCfg {
    pub feature_set: FeatureSet,
    pub sigverify: bool,
    pub blockhash_check: bool,
    pub fee_structure: FeeStructure,
}

/// Runtime knobs that shape execution independently from block state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeEnv {
    pub compute_budget: Option<ComputeBudget>,
    pub log_bytes_limit: Option<usize>,
}
