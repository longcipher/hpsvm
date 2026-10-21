use solana_svm_callback::InvokeContextCallback;
#[cfg(feature = "precompiles")]
use {solana_address::Address, solana_precompile_error::PrecompileError};

use crate::{Svm, SvmTypesHost};

#[cfg(not(feature = "precompiles"))]
impl<T: SvmTypesHost> InvokeContextCallback for Svm<'_, T> {}

#[cfg(feature = "precompiles")]
impl<T: SvmTypesHost> InvokeContextCallback for Svm<'_, T> {
    fn is_precompile(&self, program_id: &Address) -> bool {
        // Dynamic id supplied per call by the instance's provider; it fires
        // once per top-level instruction and cannot be hoisted into the fixed
        // `TxFeatureFlags` set. The feature gate is enforced at lookup,
        // exactly like agave's `get_precompile` feature check.
        self.extensions
            .precompiles
            .get(program_id)
            .is_some_and(|precompile| precompile.is_enabled(&self.version.feature_set))
    }

    fn process_precompile(
        &self,
        program_id: &Address,
        data: &[u8],
        instruction_data_slices: Vec<&[u8]>,
    ) -> Result<(), solana_precompile_error::PrecompileError> {
        // Same dynamic-id reasoning as `is_precompile`. The `&FeatureSet`
        // passed to `verify` is an agave API requirement and cannot change.
        match self.extensions.precompiles.get(program_id) {
            Some(precompile) if precompile.is_enabled(&self.version.feature_set) => {
                precompile.verify(data, &instruction_data_slices, &self.version.feature_set)
            }
            _ => Err(PrecompileError::InvalidPublicKey),
        }
    }
}
