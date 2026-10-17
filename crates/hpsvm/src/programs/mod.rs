pub(crate) mod entry_store;
pub(crate) mod jit_policy;
pub(crate) mod tier_stats;
pub(crate) mod tiering;

pub(crate) use entry_store::{content_key, default_program_cache, program_entry_store};
pub use jit_policy::JitPolicy;
use solana_address::{Address, address};
use solana_sdk_ids::{
    address_lookup_table, bpf_loader, bpf_loader_deprecated, bpf_loader_upgradeable, config, stake,
};
pub use tier_stats::ProgramTierStats;
pub(crate) use tiering::{TierPromoter, has_compiled_program, load_program_uncompiled};

use crate::{FeatureGate, Svm, SvmTypesHost, TxFeatureFlags};

pub(crate) const DEFAULT_PROGRAM_IDS: [Address; 8] = [
    address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
    address!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"),
    address!("Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo"),
    address!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"),
    address!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"),
    config::ID,
    address_lookup_table::ID,
    stake::ID,
];

pub(crate) const SPL_PROGRAM_IDS: [Address; 3] = [
    address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
    address!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"),
    address!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"),
];

pub(crate) fn load_default_programs<T: SvmTypesHost>(svm: &mut Svm<'_, T>) {
    load_spl_programs(svm);
    load_non_spl_default_programs(svm);
}

pub(crate) fn load_spl_programs<T: SvmTypesHost>(svm: &mut Svm<'_, T>) {
    // One flag load per program-load pass; loading runs at setup or feature
    // reconfiguration time, never per transaction.
    let flags = TxFeatureFlags::load(&svm.version.feature_set);
    // if replace spl-token with p-token feature is enabled, the SPL token contract is loaded from
    // a different .so
    if flags.is_enabled(FeatureGate::ReplaceSplTokenWithPToken) {
        svm.add_program_preverified(
            address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
            include_bytes!("../../elf/pinocchio_token_program.so"),
            &bpf_loader_upgradeable::id(),
        )
        .expect("failed to load pinocchio token program (p-token feature active)");
    } else {
        svm.add_program_preverified(
            address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"),
            include_bytes!("../../elf/spl_token-3.5.0.so"),
            &bpf_loader::id(),
        )
        .expect("failed to load spl-token 3.5.0");
    }

    svm.add_program_preverified(
        address!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"),
        include_bytes!("../../elf/spl_token_2022-10.0.0.so"),
        &bpf_loader_upgradeable::id(),
    )
    .expect("failed to load spl-token-2022 10.0.0");
    svm.add_program_preverified(
        address!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"),
        include_bytes!("../../elf/spl_associated_token_account-1.1.1.so"),
        &bpf_loader::id(),
    )
    .expect("failed to load spl-associated-token-account 1.1.1");
}

fn load_non_spl_default_programs<T: SvmTypesHost>(svm: &mut Svm<'_, T>) {
    svm.add_program_preverified(
        address!("Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo"),
        include_bytes!("../../elf/spl_memo-1.0.0.so"),
        &bpf_loader_deprecated::id(),
    )
    .expect("failed to load spl-memo 1.0.0");
    svm.add_program_preverified(
        address!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"),
        include_bytes!("../../elf/spl_memo-3.0.0.so"),
        &bpf_loader::id(),
    )
    .expect("failed to load spl-memo 3.0.0");
    // The ELFs in `crates/hpsvm/elf` are vendored prebuilt binaries, not built
    // from the workspace. They track a Solana release rather than the crate
    // versions in `Cargo.toml`, so bumping the dependencies does not refresh
    // them and they can drift out of step with the programs loaded here.
    //
    // `core_bpf_stake.so` is the current mainnet build (dumped 2026-10-17
    // via `solana program dump -u mainnet-beta
    // Stake11111111111111111111111111111111111111`), matching the 5.x
    // instruction builders that read sysvars via syscall instead of taking
    // them as accounts. The previous `core_bpf_stake-1.0.1.so` predated that
    // interface and failed `Initialize`/`Authorize`/`DelegateStake` with
    // `NotEnoughAccountKeys`.
    //
    // To refresh any vendored program, rebuild it from its upstream source
    // (or re-dump a released build) and commit the result; nothing in this
    // workspace can regenerate them.
    svm.add_program_preverified(
        config::ID,
        include_bytes!("../../elf/config.so"),
        &bpf_loader_upgradeable::id(),
    )
    .expect("failed to load config program");
    svm.add_program_preverified(
        address_lookup_table::ID,
        include_bytes!("../../elf/address_lookup_table.so"),
        &bpf_loader_upgradeable::id(),
    )
    .expect("failed to load address-lookup-table program");
    svm.add_program_preverified(
        stake::ID,
        include_bytes!("../../elf/core_bpf_stake.so"),
        &bpf_loader_upgradeable::id(),
    )
    .expect("failed to load core-bpf-stake");
}
