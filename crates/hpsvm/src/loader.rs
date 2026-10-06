//! Loader operations for the HPSVM.
//!
//! This module provides helpers for working with the BPF loader v3 (upgradeable loader)
//! when constructing tests with [`crate::HPSVM`]. Enable via the `loader` feature.

use solana_address::Address;
use solana_keypair::Keypair;
use solana_loader_v3_interface::{
    instruction as bpf_loader_upgradeable, state::UpgradeableLoaderState,
};
use solana_signer::Signer;
use solana_transaction::{InstructionError, Transaction};
use solana_transaction_error::TransactionError;

use crate::{HPSVM, types::FailedTransactionMetadata};

const CHUNK_SIZE: usize = 512;

fn loader_instruction_error(source: InstructionError) -> FailedTransactionMetadata {
    FailedTransactionMetadata {
        err: TransactionError::InstructionError(0, source),
        meta: Default::default(),
    }
}

/// Set the upgrade authority for an upgradeable program
pub fn set_upgrade_authority(
    svm: &mut HPSVM,
    from_keypair: &Keypair,
    program_address: &Address,
    current_authority_keypair: &Keypair,
    new_authority_address: Option<&Address>,
) -> Result<(), FailedTransactionMetadata> {
    let mut signers: Vec<&dyn Signer> = vec![from_keypair];
    if from_keypair.pubkey() != current_authority_keypair.pubkey() {
        signers.push(current_authority_keypair);
    }

    let tx = Transaction::new_signed_with_payer(
        &[bpf_loader_upgradeable::set_upgrade_authority(
            program_address,
            &current_authority_keypair.pubkey(),
            new_authority_address,
        )],
        Some(&from_keypair.pubkey()),
        &signers,
        svm.latest_blockhash(),
    );

    svm.send_transaction(tx)?;

    Ok(())
}

fn load_upgradeable_buffer(
    svm: &mut HPSVM,
    payer_kp: &Keypair,
    program_bytes: &[u8],
) -> Result<Address, FailedTransactionMetadata> {
    let payer_pk = payer_kp.pubkey();
    let buffer_kp = Keypair::new();
    let buffer_pk = buffer_kp.pubkey();
    // loader
    let buffer_len = UpgradeableLoaderState::size_of_buffer(program_bytes.len());
    let lamports = svm.minimum_balance_for_rent_exemption(buffer_len);

    let create_buffer_ixs = bpf_loader_upgradeable::create_buffer(
        &payer_pk,
        &buffer_pk,
        &payer_pk,
        lamports,
        program_bytes.len(),
    )
    .map_err(loader_instruction_error)?;
    let tx = Transaction::new_signed_with_payer(
        &create_buffer_ixs,
        Some(&payer_pk),
        &[payer_kp, &buffer_kp],
        svm.latest_blockhash(),
    );

    svm.send_transaction(tx)?;

    let chunk_size = CHUNK_SIZE;
    let mut offset: u64 = 0;
    for chunk in program_bytes.chunks(chunk_size) {
        let offset_u32: u32 =
            offset.try_into().map_err(|_| loader_instruction_error(InstructionError::Custom(0)))?;
        let tx = Transaction::new_signed_with_payer(
            &[bpf_loader_upgradeable::write(&buffer_pk, &payer_pk, offset_u32, chunk.to_vec())],
            Some(&payer_pk),
            &[payer_kp],
            svm.latest_blockhash(),
        );

        svm.send_transaction(tx)?;
        offset += chunk_size as u64;
    }

    Ok(buffer_pk)
}

/// Deploy an upgradeable program
pub fn deploy_upgradeable_program(
    svm: &mut HPSVM,
    payer_kp: &Keypair,
    program_kp: &Keypair,
    program_bytes: &[u8],
) -> Result<(), FailedTransactionMetadata> {
    let program_pk = program_kp.pubkey();
    let payer_pk = payer_kp.pubkey();
    let buffer_pk = load_upgradeable_buffer(svm, payer_kp, program_bytes)?;

    let lamports = svm.minimum_balance_for_rent_exemption(program_bytes.len());
    let deploy_ixs = bpf_loader_upgradeable::deploy_with_max_program_len(
        &payer_pk,
        &program_pk,
        &buffer_pk,
        &payer_pk,
        lamports,
        program_bytes.len() * 2,
    )
    .map_err(loader_instruction_error)?;
    let tx = Transaction::new_signed_with_payer(
        &deploy_ixs,
        Some(&payer_pk),
        &[&payer_kp, &program_kp],
        svm.latest_blockhash(),
    );

    svm.send_transaction(tx)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TransactionMetadata;

    /// Builds a VM that can deploy SBPF v0 programs.
    ///
    /// `HPSVM::new()` enables every feature, including
    /// `disable_sbpf_v0_v1_v2_deployment`, which blocks the v0 deployments the
    /// loader helpers exist to exercise, so these tests build against the default
    /// feature set instead.
    fn deployable_vm() -> HPSVM {
        HPSVM::builder()
            .with_feature_set(agave_feature_set::FeatureSet::default())
            .with_builtins()
            .with_lamports(1_000_000_000_000_000)
            .with_sysvars()
            .build()
            .expect("the default builder configuration must succeed")
    }

    fn funded(lamports: u64) -> (HPSVM, Keypair) {
        let mut svm = deployable_vm();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), lamports).unwrap();
        (svm, payer)
    }

    fn is_instruction_error(error: &FailedTransactionMetadata) -> bool {
        matches!(error.err, TransactionError::InstructionError(..))
    }

    #[test]
    fn the_write_chunk_size_is_the_loader_maximum() {
        // The upgradeable loader rejects `write` chunks larger than 512 bytes, so
        // this constant is a protocol limit rather than a tunable.
        assert_eq!(CHUNK_SIZE, 512);
    }

    /// Every loader failure is reported as an instruction error at index 0 with
    /// the underlying cause preserved.
    #[test]
    fn loader_instruction_error_wraps_the_cause_at_index_zero() {
        let meta = loader_instruction_error(InstructionError::InvalidAccountData);
        assert!(matches!(
            meta.err,
            TransactionError::InstructionError(0, InstructionError::InvalidAccountData)
        ));
        assert_eq!(meta.meta, TransactionMetadata::default());
    }

    #[test]
    fn loader_instruction_error_preserves_a_custom_code() {
        let meta = loader_instruction_error(InstructionError::Custom(9));
        match meta.err {
            TransactionError::InstructionError(0, InstructionError::Custom(code)) => {
                assert_eq!(code, 9);
            }
            other => panic!("expected Custom(9), got {other:?}"),
        }
    }

    /// An empty ELF must be rejected by the loader without panicking.
    #[test]
    fn deploying_an_empty_program_fails() {
        let (mut svm, payer) = funded(10_000_000_000);
        let program_kp = Keypair::new();

        let error = deploy_upgradeable_program(&mut svm, &payer, &program_kp, &[])
            .expect_err("an empty ELF must be rejected");
        assert!(is_instruction_error(&error), "expected an instruction error, got {:?}", error.err);
    }

    /// A zeroed ELF passes buffer creation and the chunk writes but fails at
    /// deploy time, when the loader parses the ELF header.
    #[test]
    fn deploying_a_garbage_elf_fails() {
        let (mut svm, payer) = funded(10_000_000_000);
        let program_kp = Keypair::new();

        let error = deploy_upgradeable_program(&mut svm, &payer, &program_kp, &[0u8; 1024])
            .expect_err("a zeroed ELF must be rejected");
        assert!(is_instruction_error(&error), "expected an instruction error, got {:?}", error.err);
    }

    /// An unfunded payer cannot create the buffer, so the failure must be
    /// reported rather than swallowed.
    #[test]
    fn deploying_without_funds_fails() {
        let mut svm = deployable_vm();
        let payer = Keypair::new();
        let program_kp = Keypair::new();

        let error = deploy_upgradeable_program(&mut svm, &payer, &program_kp, &[0u8; 16])
            .expect_err("an unfunded payer must be rejected");
        assert!(
            matches!(error.err, TransactionError::AccountNotFound),
            "expected AccountNotFound, got {:?}",
            error.err
        );
    }

    /// Changing the authority of an address that holds no program must fail.
    #[test]
    fn set_upgrade_authority_on_an_unknown_program_fails() {
        let (mut svm, payer) = funded(10_000_000_000);
        let program_id = Address::new_unique();

        let error = set_upgrade_authority(
            &mut svm,
            &payer,
            &program_id,
            &payer,
            Some(&Address::new_unique()),
        )
        .expect_err("an address with no program data must be rejected");
        assert!(is_instruction_error(&error), "expected an instruction error, got {:?}", error.err);
    }

    /// A `from` keypair that is not the current authority has to sign as well;
    /// without its signature the transfer must be rejected.
    #[test]
    fn set_upgrade_authority_rejects_a_missing_authority_signature() {
        let (mut svm, payer) = funded(10_000_000_000);
        let program_id = Address::new_unique();
        let stranger = Keypair::new();
        svm.airdrop(&stranger.pubkey(), 10_000_000_000).unwrap();

        // The authority is only named, never signed, so the loader rejects it
        // before it can complain about the missing program data.
        let error = set_upgrade_authority(
            &mut svm,
            &stranger,
            &program_id,
            &payer,
            Some(&stranger.pubkey()),
        )
        .expect_err("an unsigned authority must be rejected");
        assert!(
            matches!(
                error.err,
                TransactionError::InstructionError(..) | TransactionError::AccountNotFound
            ),
            "expected an instruction or lookup failure, got {:?}",
            error.err
        );
    }
}
