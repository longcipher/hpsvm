use solana_hash::Hash;

/// Block-scoped runtime state exposed by the VM.
///
/// The trailing `ext` slot carries chain-specific data selected by the VM's
/// type family ([`SvmTypesHost::BlockEnvExt`](crate::SvmTypesHost)); it
/// defaults to `()` so the base family pays nothing for it.
/// `latest_blockhash` and `slot` keep their positions first, so `BlockEnv<()>`
/// is layout-identical to the pre-generics shape.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEnv<E = ()> {
    pub latest_blockhash: Hash,
    pub slot: u64,
    pub ext: E,
}

/// Transaction-scoped environment.
///
/// Carries an extension slot (defaulting to `()`); it gains
/// transaction-scoped fields in later tasks if needed.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxEnv<E = ()> {
    pub ext: E,
}

#[cfg(test)]
mod tests {
    use std::mem::{offset_of, size_of};

    use agave_feature_set::FeatureSet;
    use solana_address::Address;
    use solana_hash::Hash;
    use solana_keypair::Keypair;
    use solana_message::Message;
    use solana_native_token::LAMPORTS_PER_SOL;
    use solana_signer::Signer;
    use solana_system_interface::instruction::transfer;
    use solana_transaction::{Transaction, versioned::VersionedTransaction};

    use super::*;
    use crate::{ClusterSpecId, Host, Svm, SvmTypesHost};

    /// Chain-specific block-environment extension used to prove a custom
    /// family can attach data to [`BlockEnv`] without touching the core.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct L1SlotExt {
        pub l1_slot: u64,
    }

    /// Test-only type family that fills only the block-env extension slot.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct L1Family;

    impl SvmTypesHost for L1Family {
        type SpecId = ClusterSpecId;
        type Tx = VersionedTransaction;
        type BlockEnvExt = L1SlotExt;
        type Host<'a> = Svm<'a, Self>;
    }

    /// Lamports handed to both accounts before the transfer under test.
    const ORIGINAL_BALANCE: u64 = 1_000_000_000;
    /// Lamports moved by the transfer under test.
    const TRANSFER_AMOUNT: u64 = 64;
    /// Lamports every signature costs at the default fee structure.
    const SIGNATURE_FEE: u64 = 5_000;

    /// The block-env extension slot must survive a full transaction execution:
    /// set the ext, run a transfer, then read the exact value back through
    /// `Host::block_env` (the task text calls it `Host::block()`; the real
    /// method name is `Host::block_env()`).
    #[test]
    fn l1_slot_ext_survives_a_full_transfer_through_host_block_env() {
        let mut svm = Svm::<L1Family>::new_inner(false);
        svm.set_feature_set(FeatureSet::all_enabled())
            .expect("enabling every feature should succeed on a fresh VM");
        svm.set_builtins();
        svm.set_lamports(1_000_000 * LAMPORTS_PER_SOL);
        svm.set_sysvars();
        svm.set_feature_accounts();
        svm.set_default_programs();
        svm.set_sigverify(true);
        svm.set_blockhash_check(true);

        svm.block_env.ext = L1SlotExt { l1_slot: 42 };
        let before = Host::<L1Family>::block_env(&svm);
        assert_eq!(before.ext, L1SlotExt { l1_slot: 42 });

        let payer = Keypair::new();
        let recipient = Address::new_unique();
        svm.airdrop(&payer.pubkey(), ORIGINAL_BALANCE).unwrap();
        svm.airdrop(&recipient, ORIGINAL_BALANCE).unwrap();

        let tx = Transaction::new(
            &[&payer],
            Message::new(
                &[transfer(&payer.pubkey(), &recipient, TRANSFER_AMOUNT)],
                Some(&payer.pubkey()),
            ),
            svm.latest_blockhash(),
        );
        let metadata = svm.send_transaction(tx).expect("transfer should execute");

        // The extension itself must not move any lamports: the transfer keeps
        // the exact fee math of the base family.
        assert_eq!(metadata.fee, SIGNATURE_FEE);
        assert_eq!(
            svm.get_balance(&payer.pubkey()),
            Some(ORIGINAL_BALANCE - SIGNATURE_FEE - TRANSFER_AMOUNT)
        );
        assert_eq!(svm.get_balance(&recipient), Some(ORIGINAL_BALANCE + TRANSFER_AMOUNT));

        let after = Host::<L1Family>::block_env(&svm);
        assert_eq!(after.ext.l1_slot, 42);
        assert_eq!(after, before);
    }

    /// `BlockEnv<()>` must stay layout-identical to the pre-generics shape:
    /// a 32-byte hash followed by the 8-byte slot, with the unit extension
    /// adding no size, while a non-empty extension appends its own size.
    #[test]
    fn base_block_env_is_layout_identical_to_the_previous_shape() {
        assert_eq!(size_of::<BlockEnv<()>>(), size_of::<Hash>() + size_of::<u64>());
        assert_eq!(size_of::<BlockEnv<()>>(), 40);
        assert_eq!(size_of::<BlockEnv<L1SlotExt>>(), 48);
        assert_eq!(offset_of!(BlockEnv<()>, latest_blockhash), 0);
        assert_eq!(offset_of!(BlockEnv<()>, slot), 32);
        assert_eq!(offset_of!(BlockEnv<()>, ext), 40);
    }

    /// The unit block env defaults to the genesis shape.
    #[test]
    fn base_block_env_default_is_the_genesis_shape() {
        let env = BlockEnv::<()>::default();
        assert_eq!(env.latest_blockhash, Hash::default());
        assert_eq!(env.slot, 0);
        assert_eq!(env.ext, ());
    }

    /// `TxEnv` currently carries only the extension slot; it gains
    /// transaction-scoped fields in later tasks if needed.
    #[test]
    fn tx_env_carries_only_the_extension_slot() {
        let env = TxEnv { ext: L1SlotExt { l1_slot: 7 } };
        assert_eq!(env.ext.l1_slot, 7);
        assert_eq!(size_of::<TxEnv<()>>(), 0);
        assert_eq!(TxEnv::<()>::default().ext, ());
    }
}
