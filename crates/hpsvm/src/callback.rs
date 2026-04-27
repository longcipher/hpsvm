use solana_svm_callback::InvokeContextCallback;
#[cfg(feature = "precompiles")]
use {
    agave_precompiles::{get_precompile, is_precompile},
    solana_address::Address,
    solana_precompile_error::PrecompileError,
};

use crate::HPSVM;

#[cfg(not(feature = "precompiles"))]
impl InvokeContextCallback for HPSVM {}

#[cfg(feature = "precompiles")]
impl InvokeContextCallback for HPSVM {
    fn is_precompile(&self, program_id: &Address) -> bool {
        is_precompile(program_id, |feature_id: &Address| self.feature_set.is_active(feature_id))
    }

    fn process_precompile(
        &self,
        program_id: &Address,
        data: &[u8],
        instruction_data_slices: Vec<&[u8]>,
    ) -> Result<(), solana_precompile_error::PrecompileError> {
        if let Some(precompile) = get_precompile(program_id, |feature_id: &Address| {
            self.feature_set.is_active(feature_id)
        }) {
            precompile.verify(data, &instruction_data_slices, &self.feature_set)
        } else {
            Err(PrecompileError::InvalidPublicKey)
        }
    }
}
