//! Precompile registry tests.
//!
//! The default provider must reproduce today's agave-backed behaviour
//! (equivalence is proven against the secp256k1 goldens from
//! `tests/precompiles.rs`), and a custom precompile registered under an
//! existing program id must shadow the native entry.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use agave_feature_set::FeatureSet;
use agave_precompiles::get_precompiles;
use hpsvm::{
    BaseSvmTypes, Hpsvm, NativePrecompiles, NoPrecompiles, Precompile, PrecompileProvider,
    PrecompileSet,
};
use k256::{SecretKey, elliptic_curve::sec1::ToSec1Point};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_precompile_error::PrecompileError;
use solana_secp256k1_program::{
    eth_address_from_pubkey, new_secp256k1_instruction_with_signature, sign_message,
};
use solana_signer::Signer;
use solana_svm_callback::InvokeContextCallback;
use solana_transaction::{InstructionError, Transaction};
use solana_transaction_error::TransactionError;

/// Builds the same valid secp256k1 instruction as
/// `secp256k1_precompile_ok` in `tests/precompiles.rs`.
fn valid_secp256k1_instruction() -> solana_instruction::Instruction {
    let kp_secp256k1 = SecretKey::from_slice(&[1; 32]).unwrap();
    let msg = b"hello world";
    let secp_pubkey = kp_secp256k1.public_key();
    let pub_bytes = secp_pubkey.to_sec1_point(false);
    let eth_address = eth_address_from_pubkey(&pub_bytes.as_bytes()[1..].try_into().unwrap());
    let secret_bytes: [u8; 32] = kp_secp256k1.to_bytes().into();
    let (signature, recovery_id) = sign_message(&secret_bytes, msg).unwrap();
    new_secp256k1_instruction_with_signature(msg, &signature, recovery_id, &eth_address)
}

/// A test precompile that records dispatch and always fails.
///
/// It fails with `InvalidInstructionDataSize` (discriminant 4), which the
/// runtime maps to `InstructionError::Custom(4)` — the same discriminant
/// mapping the secp256k1 golden relies on (`InvalidSignature`, 2, surfaces
/// as `InstructionError::Custom(2)`; see `tests/precompiles.rs`).
#[derive(Debug)]
struct ShadowPrecompile {
    program_id: Address,
    gate: Option<Address>,
    invoked: Arc<AtomicBool>,
}

impl Precompile<BaseSvmTypes> for ShadowPrecompile {
    fn program_id(&self) -> Address {
        self.program_id
    }

    fn is_enabled(&self, feature_set: &FeatureSet) -> bool {
        self.gate.is_none_or(|gate| feature_set.is_active(&gate))
    }

    fn verify(
        &self,
        _data: &[u8],
        _slices: &[&[u8]],
        _feature_set: &FeatureSet,
    ) -> Result<(), PrecompileError> {
        self.invoked.store(true, Ordering::SeqCst);
        Err(PrecompileError::InvalidInstructionDataSize)
    }
}

fn shadow(program_id: Address, gate: Option<Address>) -> (ShadowPrecompile, Arc<AtomicBool>) {
    let invoked = Arc::new(AtomicBool::new(false));
    let precompile = ShadowPrecompile { program_id, gate, invoked: Arc::clone(&invoked) };
    (precompile, invoked)
}

#[test_log::test]
fn native_set_matches_agave_table_exactly() {
    let mut got = NativePrecompiles::<BaseSvmTypes>::default().program_ids();
    let mut want: Vec<Address> =
        get_precompiles().iter().map(|precompile| precompile.program_id).collect();
    got.sort();
    want.sort();
    assert_eq!(got, want, "default set must contain exactly the agave table ids");
    // Every entry in today's table is always-on (`feature: None`), which is
    // why the feature-gate test below uses a synthetic gated entry.
    for precompile in get_precompiles() {
        assert!(
            precompile.feature.is_none(),
            "expected an always-on table entry for {}",
            precompile.program_id
        );
    }
}

#[test_log::test]
fn default_vm_reproduces_secp256k1_ok() {
    let kp = Keypair::new();
    let mut svm = Hpsvm::new();
    svm.airdrop(&kp.pubkey(), 10u64.pow(9)).unwrap();

    let ix = valid_secp256k1_instruction();
    let tx =
        Transaction::new(&[&kp], Message::new(&[ix], Some(&kp.pubkey())), svm.latest_blockhash());
    assert!(svm.send_transaction(tx).is_ok());
}

#[test_log::test]
fn default_vm_reproduces_secp256k1_err() {
    use solana_secp256k1_program as secp256k1_instruction;

    let kp = Keypair::new();
    let mut svm = Hpsvm::new();
    svm.airdrop(&kp.pubkey(), 10u64.pow(9)).unwrap();

    let mut ix = valid_secp256k1_instruction();
    ix.data[secp256k1_instruction::DATA_START + 32] ^= 1;
    let tx =
        Transaction::new(&[&kp], Message::new(&[ix], Some(&kp.pubkey())), svm.latest_blockhash());
    // Same expectation as `secp256k1_precompile_err` in `tests/precompiles.rs`.
    assert_eq!(
        svm.send_transaction(tx).err().map(|fail| fail.err),
        Some(TransactionError::InstructionError(0, InstructionError::Custom(2)))
    );
}

#[test_log::test]
fn custom_precompile_shadows_native_for_same_program_id() {
    let secp_id = solana_sdk_ids::secp256k1_program::id();
    let (custom, invoked) = shadow(secp_id, None);
    let mut set = NativePrecompiles::<BaseSvmTypes>::default().into_set();
    set.insert(Arc::new(custom));

    let mut svm = Hpsvm::new();
    svm.set_precompile_provider(Arc::new(set));

    // Act - a valid secp256k1 instruction, which the native entry would accept.
    let kp = Keypair::new();
    svm.airdrop(&kp.pubkey(), 10u64.pow(9)).unwrap();
    let ix = valid_secp256k1_instruction();
    let tx =
        Transaction::new(&[&kp], Message::new(&[ix], Some(&kp.pubkey())), svm.latest_blockhash());
    let res = svm.send_transaction(tx);

    // Assert - the custom entry ran instead of the native one.
    assert!(invoked.load(Ordering::SeqCst), "custom precompile was not dispatched");
    assert_eq!(
        res.err().map(|fail| fail.err),
        Some(TransactionError::InstructionError(0, InstructionError::Custom(4)))
    );
}

#[test_log::test]
fn no_precompiles_provider_disables_all_dispatch() {
    let native_ids = NativePrecompiles::<BaseSvmTypes>::default().program_ids();
    assert!(!native_ids.is_empty());

    let mut svm = Hpsvm::new();
    for id in &native_ids {
        assert!(svm.is_precompile(id), "native id {id} should dispatch by default");
    }

    svm.set_precompile_provider(Arc::new(NoPrecompiles));
    for id in &native_ids {
        assert!(!svm.is_precompile(id), "native id {id} should stop dispatching");
    }
    assert!(svm.precompile_provider().program_ids().is_empty());
}

#[test_log::test]
fn feature_gated_entry_tracks_feature_set() {
    let gate = agave_feature_set::raise_cpi_nesting_limit_to_8::ID;
    let program_id = Address::new_unique();
    let (gated, _invoked) = shadow(program_id, Some(gate));

    // Unit level: inactive by default, active once the gate is on.
    assert!(!gated.is_enabled(&FeatureSet::default()));
    assert!(gated.is_enabled(&FeatureSet::all_enabled()));

    // Dispatch level: the VM consults the gate through its feature set.
    // (`Hpsvm::new` enables every feature, so start from an empty set.)
    let mut set = PrecompileSet::<BaseSvmTypes>::new();
    set.insert(Arc::new(gated));
    let mut svm = Hpsvm::new();
    svm.set_precompile_provider(Arc::new(set));
    svm.set_feature_set(FeatureSet::default()).unwrap();
    assert!(!svm.is_precompile(&program_id));
    svm.set_feature_set(FeatureSet::all_enabled()).unwrap();
    assert!(svm.is_precompile(&program_id));
}

#[test_log::test]
fn cloned_vm_shares_provider() {
    let program_id = Address::new_unique();
    let (custom, _invoked) = shadow(program_id, None);
    let mut set = PrecompileSet::<BaseSvmTypes>::new();
    set.insert(Arc::new(custom));

    let mut svm = Hpsvm::new();
    svm.set_precompile_provider(Arc::new(set));
    let cloned = svm.clone();

    assert!(svm.is_precompile(&program_id));
    assert!(cloned.is_precompile(&program_id));
    assert!(
        Arc::ptr_eq(&svm.precompile_provider(), &cloned.precompile_provider()),
        "clone must share the provider allocation"
    );
}
