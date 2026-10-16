use std::sync::Arc;

use solana_account::{AccountSharedData, WritableAccount};
use solana_sdk_ids::native_loader;

use crate::{Svm, SvmTypesHost};

pub(crate) fn load_precompiles<T: SvmTypesHost>(svm: &mut Svm<'_, T>) {
    let mut account = AccountSharedData::default();
    account.set_owner(native_loader::id());
    account.set_lamports(1);
    account.set_executable(true);

    // Install accounts for the active set's program ids only. Gating stays
    // dynamic: each entry carries its own optional feature, which cannot be
    // enumerated into the fixed `TxFeatureFlags` set. Load-time only, so the
    // direct query stays.
    let feature_set = Arc::clone(&svm.version.feature_set);
    for program_id in svm.precompiles.program_ids() {
        let enabled = svm
            .precompiles
            .get(&program_id)
            .is_some_and(|precompile| precompile.is_enabled(&feature_set));
        if enabled {
            svm.set_account(program_id, account.clone().into()).unwrap();
        }
    }
}
