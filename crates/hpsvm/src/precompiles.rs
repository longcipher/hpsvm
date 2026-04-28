use agave_precompiles::get_precompiles;
use solana_account::{AccountSharedData, WritableAccount};
use solana_sdk_ids::native_loader;

use crate::HPSVM;

pub(crate) fn load_precompiles(svm: &mut HPSVM) {
    let mut account = AccountSharedData::default();
    account.set_owner(native_loader::id());
    account.set_lamports(1);
    account.set_executable(true);

    for precompile in get_precompiles() {
        if precompile.feature.is_none_or(|feature_id| svm.cfg.feature_set.is_active(&feature_id)) {
            svm.set_account(precompile.program_id, account.clone().into()).unwrap();
        }
    }
}
