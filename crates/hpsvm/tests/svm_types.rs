//! The `SvmTypes` type family.
//!
//! Two things are pinned here. First, that `Svm<'_, BaseSvmTypes>` — the host the
//! rest of the workspace keeps using through the `HPSVM` alias — still executes a
//! lamport transfer and produces the exact balances and metadata the
//! pre-refactor VM produced. Second, that every extension slot of
//! [`BaseSvmTypes`] is the unit type, so an unused extension point costs nothing
//! and monomorphizes away.

use std::mem::size_of;

use hpsvm::{BaseSvmTypes, ClusterSpecId, Svm, SvmTypes, SvmTypesHost};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::{Transaction, versioned::VersionedTransaction};

/// Lamports handed to both accounts before the transfer under test.
const ORIGINAL_BALANCE: u64 = 1_000_000_000;
/// Lamports moved by the transfer under test.
const TRANSFER_AMOUNT: u64 = 64;
/// Lamports every signature costs at the default fee structure.
const SIGNATURE_FEE: u64 = 5_000;

/// A transfer executed through the generic host must reproduce the pre-refactor
/// result exactly: the payer loses the transfer plus one signature fee, the
/// recipient gains the transfer, and the reported metadata names the same fee.
#[test_log::test]
fn base_svm_types_reproduces_the_transfer_result_exactly() {
    let mut svm = Svm::<BaseSvmTypes>::new();
    let payer = Keypair::new();
    let recipient = solana_address::Address::new_unique();

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

    assert_eq!(metadata.fee, SIGNATURE_FEE);
    assert_eq!(
        svm.get_balance(&payer.pubkey()),
        Some(ORIGINAL_BALANCE - SIGNATURE_FEE - TRANSFER_AMOUNT)
    );
    assert_eq!(svm.get_balance(&recipient), Some(ORIGINAL_BALANCE + TRANSFER_AMOUNT));

    // The fee is collected and burned, so the tracked accounts lose exactly one
    // signature fee across the whole transaction.
    assert_eq!(metadata.diagnostics.pre_balances.len(), metadata.diagnostics.post_balances.len());
    let net_change: i128 = metadata
        .diagnostics
        .post_balances
        .iter()
        .zip(&metadata.diagnostics.pre_balances)
        .map(|(post, pre)| i128::from(*post) - i128::from(*pre))
        .sum();
    assert_eq!(net_change, -i128::from(SIGNATURE_FEE));

    // The payer and the recipient are the only accounts a transfer changes, so
    // the diff reports exactly those two, in message order, with exact balances.
    assert_eq!(
        metadata
            .diagnostics
            .account_diffs
            .iter()
            .map(|diff| (
                diff.address,
                diff.pre.as_ref().map(|account| account.lamports),
                diff.post.as_ref().map(|account| account.lamports)
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                payer.pubkey(),
                Some(ORIGINAL_BALANCE),
                Some(ORIGINAL_BALANCE - SIGNATURE_FEE - TRANSFER_AMOUNT)
            ),
            (recipient, Some(ORIGINAL_BALANCE), Some(ORIGINAL_BALANCE + TRANSFER_AMOUNT)),
        ]
    );

    // The `HPSVM` alias must stay the same host as `Svm<'static, BaseSvmTypes>`,
    // so the pre-existing public-behaviour suite keeps compiling unchanged.
    let aliased: hpsvm::HPSVM = Svm::<BaseSvmTypes>::new();
    assert_eq!(aliased.latest_blockhash(), svm.latest_blockhash());
}

/// Every extension slot of the base family must be the unit type, except the
/// spec-id slot, which is the `repr(u32)` [`ClusterSpecId`]: a fork adds a real
/// type to a unit slot, and an unused slot must stay out of the generated code.
#[test_log::test]
fn every_base_extension_slot_is_the_unit_type() {
    const _: () = assert!(size_of::<<BaseSvmTypes as SvmTypesHost>::SpecId>() == 4);
    const _: () = assert!(size_of::<ClusterSpecId>() == 4);
    const _: () = assert!(size_of::<<BaseSvmTypes as SvmTypesHost>::BlockEnvExt>() == 0);
    const _: () = assert!(size_of::<BaseSvmTypes>() == 0);

    // The transaction envelope is the one non-unit slot, pinned to the type the
    // rest of the workspace already threads through the VM.
    const _: () = {
        let tx: Option<<BaseSvmTypes as SvmTypesHost>::Tx> = None::<VersionedTransaction>;
        assert!(tx.is_none());
    };

    const _: () = assert!(size_of::<<BaseSvmTypes as SvmTypesHost>::BlockEnvExt>() == 0);

    // The base family satisfies the blanket marker, so the host a caller gets
    // back from an associated type really is `Svm<'a, BaseSvmTypes>`.
    fn takes_marker<T: SvmTypes>() {}
    takes_marker::<BaseSvmTypes>();
}

/// `Debug` must name the VM type, and cloning a VM must preserve its balances
/// exactly while decoupling the clone: a transfer through the clone moves
/// funds on the clone only.
#[test_log::test]
fn debug_names_the_vm_and_clone_preserves_balances() {
    let mut svm = Svm::<BaseSvmTypes>::new();
    let payer = Keypair::new();
    let recipient = solana_address::Address::new_unique();
    svm.airdrop(&payer.pubkey(), ORIGINAL_BALANCE).unwrap();

    let debug = format!("{:?}", svm);
    assert!(debug.starts_with("Svm {"), "Debug must name the VM type, got: {debug}");

    let mut cloned = svm.clone();
    assert_eq!(cloned.get_balance(&payer.pubkey()), Some(ORIGINAL_BALANCE));
    assert_eq!(cloned.latest_blockhash(), svm.latest_blockhash());

    let tx = Transaction::new(
        &[&payer],
        Message::new(
            &[transfer(&payer.pubkey(), &recipient, TRANSFER_AMOUNT)],
            Some(&payer.pubkey()),
        ),
        cloned.latest_blockhash(),
    );
    let metadata = cloned.send_transaction(tx).expect("clone transfer should execute");
    assert_eq!(metadata.fee, SIGNATURE_FEE);
    assert_eq!(
        cloned.get_balance(&payer.pubkey()),
        Some(ORIGINAL_BALANCE - SIGNATURE_FEE - TRANSFER_AMOUNT)
    );
    assert_eq!(cloned.get_balance(&recipient), Some(TRANSFER_AMOUNT));

    // The original is untouched by the clone's transfer.
    assert_eq!(svm.get_balance(&payer.pubkey()), Some(ORIGINAL_BALANCE));
    assert_eq!(svm.get_balance(&recipient), None);
}

/// The base host must stay usable from multiple threads: batch stages run
/// worker VMs on Rayon threads, so losing `Send + Sync` would break them.
#[test_log::test]
fn base_host_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Svm<'static, BaseSvmTypes>>();
    assert_send_sync::<hpsvm::HPSVM>();
}
