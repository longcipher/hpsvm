//! Custom SVM environment end to end.
//!
//! The DX acceptance test for the type-family refactor: roughly twenty lines
//! of setup stand up a custom environment (custom spec id, non-empty block-env
//! extension, version override, custom transaction handler, macro-declared
//! builtin instruction, custom precompile), and every customisation is then
//! asserted observable through `commit()` + `commit_with(&mut sink)` flows.
//!
//! Run with `cargo run -p hpsvm --example custom_svm --all-features`.

use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use agave_feature_set::FeatureSet;
use hpsvm::{
    AccountChange, BlockEnv, ClusterSpecId, ExecutionError, Host, Precompile, PrecompileSet,
    StateChangeSink, Svm, SvmInstructionCtx, SvmTypes, SvmTypesHost, TxError, TxHandler,
    TxRegistry, TxTag, TxValidationError, Version,
};
use hpsvm_macros::svm_instruction;
use solana_account::{Account, ReadableAccount};
use solana_address::Address;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::{Message, VersionedMessage};
use solana_precompile_error::PrecompileError;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::{InstructionError, Transaction, versioned::VersionedTransaction};
use solana_transaction_error::TransactionError;

// ---------------------------------------------------------------------------
// Custom type family.
// ---------------------------------------------------------------------------

/// Custom spec id: a new type converting into the built-in profile the custom
/// chain tracks (`SvmTypesHost::SpecId: Copy + Into<ClusterSpecId>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CustomSpecId {
    Custom,
}

impl From<CustomSpecId> for ClusterSpecId {
    fn from(_: CustomSpecId) -> Self {
        Self::Devnet
    }
}

/// Non-empty block-environment extension: the L1 slot this SVM mirrors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct L1SlotExt {
    l1_slot: u64,
}

/// Custom family: custom spec id + custom block-env ext, every other slot `()`.
///
/// The transaction envelope stays `VersionedTransaction`: transaction handlers
/// only exist for that envelope, so a custom family keeps it.
#[derive(Clone, Copy, Debug, Default)]
struct CustomSvm;

impl SvmTypesHost for CustomSvm {
    type SpecId = CustomSpecId;
    type Tx = VersionedTransaction;
    type SvmExt = ();
    type MessageExt = ();
    type MessageResultExt = ();
    type TxEnvExt = ();
    type TxResultExt = ();
    type BlockEnvExt = L1SlotExt;
    type Host<'a> = Svm<'a, Self>;
}

impl SvmTypes for CustomSvm {}

// ---------------------------------------------------------------------------
// Custom transaction handler (standalone registry with a `String` output).
// ---------------------------------------------------------------------------

/// Handler proving custom-handler dispatch: it reads the custom block-env ext
/// off the host and returns it, so the output is only producible by a handler
/// running against the custom VM.
#[derive(Debug)]
struct L1GreetingHandler {
    calls: Arc<AtomicUsize>,
}

impl TxHandler<CustomSvm, VersionedTransaction, String> for L1GreetingHandler {
    fn validate(
        &self,
        _tx: &VersionedTransaction,
        _host: &Svm<'_, CustomSvm>,
    ) -> Result<(), TxValidationError> {
        Ok(())
    }

    fn execute(
        &self,
        _tx: VersionedTransaction,
        host: &Svm<'_, CustomSvm>,
    ) -> Result<String, ExecutionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(format!("l1_slot={}", host.block_env().ext.l1_slot))
    }
}

// ---------------------------------------------------------------------------
// Macro-declared builtin instruction.
// ---------------------------------------------------------------------------

#[svm_instruction(program_id = "84LtecYhx229KabnaiMixiuD5Tey4bBo6N4yzZ81Vzpz", compute_units = 150)]
fn custom_move_seven(ctx: &mut SvmInstructionCtx) -> Result<(), InstructionError> {
    if ctx.data.first().is_some_and(|byte| *byte == 0xFF) {
        return Err(InstructionError::Custom(42));
    }
    let from = ctx.accounts.first_mut().ok_or(InstructionError::MissingAccount)?;
    from.lamports = from.lamports.checked_sub(7).ok_or(InstructionError::ArithmeticOverflow)?;
    let to = ctx.accounts.get_mut(1).ok_or(InstructionError::MissingAccount)?;
    to.lamports = to.lamports.checked_add(7).ok_or(InstructionError::ArithmeticOverflow)?;
    if let Some(byte) = to.data.first_mut() {
        *byte = 0xAB;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Custom precompile.
// ---------------------------------------------------------------------------

/// Precompile proving custom precompile dispatch: it records invocation and
/// always fails with `InvalidInstructionDataSize`, which the runtime maps to
/// `InstructionError::Custom(4)` — distinct from any native outcome.
#[derive(Debug)]
struct FailingPrecompile {
    program_id: Address,
    invoked: Arc<AtomicBool>,
}

impl Precompile<CustomSvm> for FailingPrecompile {
    fn program_id(&self) -> Address {
        self.program_id
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

// ---------------------------------------------------------------------------
// Recording state-change sink for `commit_with`.
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct RecordingSink {
    seen: Vec<(Address, Option<u64>, Option<u64>)>,
}

impl StateChangeSink for RecordingSink {
    type Error = Infallible;

    fn on_account_change(&mut self, change: AccountChange<'_>) -> Result<(), Self::Error> {
        self.seen.push((
            change.address,
            change.before.map(ReadableAccount::lamports),
            change.after.map(ReadableAccount::lamports),
        ));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn legacy_tx(
    payer: &Keypair,
    ix: Instruction,
    blockhash: solana_hash::Hash,
) -> VersionedTransaction {
    let message = Message::new_with_blockhash(&[ix], Some(&payer.pubkey()), &blockhash);
    VersionedTransaction::try_new(VersionedMessage::Legacy(message), &[payer])
        .expect("legacy transaction should build")
}

fn program_owned_account(owner: &Address, lamports: u64) -> Account {
    Account { lamports, data: vec![0u8; 8], owner: *owner, executable: false, rent_epoch: 0 }
}

/// A funded system-owned signer so fee collection succeeds.
///
/// The macro handler only reads accounts 0 and 1; the payer rides along as
/// account 2, exactly like a real client transaction would carry one.
fn funded_payer(svm: &mut Svm<'_, CustomSvm>) -> Address {
    let payer = Address::new_unique();
    svm.set_account(
        payer,
        Account {
            lamports: 1_000_000_000,
            data: vec![],
            owner: solana_sdk_ids::system_program::id(),
            executable: false,
            rent_epoch: 0,
        },
    )
    .expect("payer account write should succeed");
    payer
}

fn main() {
    // --- Setup: roughly twenty lines to stand up the custom environment. ---
    let mut svm = Svm::<CustomSvm>::new_with_defaults();
    let probe = agave_feature_set::raise_cpi_nesting_limit_to_8::ID;
    let control = agave_feature_set::increase_tx_account_lock_limit::id();
    let custom_version = Version::base(ClusterSpecId::from(CustomSpecId::Custom))
        .clone()
        .with_feature(&probe, false);
    svm.set_feature_set((*custom_version.feature_set).clone())
        .expect("installing the custom version should succeed");
    svm.set_block_env_ext(L1SlotExt { l1_slot: 42 });
    CustomMoveSeven::register(&mut svm);
    let precompile_id = Address::new_unique();
    let invoked = Arc::new(AtomicBool::new(false));
    let mut precompile_set = PrecompileSet::<CustomSvm>::new();
    precompile_set.insert(Arc::new(FailingPrecompile {
        program_id: precompile_id,
        invoked: Arc::clone(&invoked),
    }));
    svm.set_precompile_provider(Arc::new(precompile_set));

    // --- Custom spec id converts into the tracked built-in profile. ---
    assert_eq!(ClusterSpecId::from(CustomSpecId::Custom), ClusterSpecId::Devnet);

    // --- Version override is observable through `version()`. ---
    assert!(
        !svm.version().feature_set.is_active(&probe),
        "the flipped gate must read back inactive"
    );
    assert!(svm.version().feature_set.is_active(&control), "every other gate must stay active");
    assert!(
        Version::base(ClusterSpecId::Devnet).feature_set.is_active(&probe),
        "the base table itself must be untouched by the override"
    );

    // --- Non-empty `BlockEnvExt` reads back through both accessors. ---
    assert_eq!(svm.block_env().ext, L1SlotExt { l1_slot: 42 });
    let via_host: BlockEnv<L1SlotExt> = Host::<CustomSvm>::block_env(&svm);
    assert_eq!(via_host.ext.l1_slot, 42);
    assert_eq!(via_host, svm.block_env());

    // --- Custom transaction handler dispatches on a standalone registry. ---
    // (The `Svm`-integrated registry is fixed to `DetachedTx` outcomes whose
    // fields are crate-internal, so an external crate proves handler dispatch
    // on its own `TxRegistry<CustomSvm, String>` instead.)
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = TxRegistry::<CustomSvm, String>::new();
    registry.register(TxTag::Custom(7), L1GreetingHandler { calls: Arc::clone(&calls) });
    let payer = Keypair::new();
    let probe_ix = transfer(&payer.pubkey(), &Address::new_unique(), 1);
    let output = registry
        .dispatch(TxTag::Custom(7), legacy_tx(&payer, probe_ix, svm.latest_blockhash()), &svm)
        .expect("custom handler dispatch should succeed");
    assert_eq!(output, "l1_slot=42");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let probe_ix = transfer(&payer.pubkey(), &Address::new_unique(), 1);
    let unknown = registry
        .dispatch(TxTag::Custom(9), legacy_tx(&payer, probe_ix, svm.latest_blockhash()), &svm)
        .expect_err("an unregistered tag must fail");
    assert!(
        matches!(unknown, TxError::UnknownTag(TxTag::Custom(9))),
        "expected UnknownTag(Custom(9)), got {unknown:?}"
    );

    // --- Macro-declared instruction executes with its state effect. ---
    let program_id = CustomMoveSeven::program_id();
    assert_eq!(CustomMoveSeven::COMPUTE_UNITS, 150);
    let payer = funded_payer(&mut svm);
    let (from, to) = {
        let from = Address::new_unique();
        let to = Address::new_unique();
        svm.set_account(from, program_owned_account(&program_id, 1_000_000))
            .expect("fixture account write should succeed");
        svm.set_account(to, program_owned_account(&program_id, 1_000_000))
            .expect("fixture account write should succeed");
        (from, to)
    };
    let move_ix = Instruction {
        program_id,
        accounts: vec![
            solana_instruction::account_meta::AccountMeta::new(from, false),
            solana_instruction::account_meta::AccountMeta::new(to, false),
            solana_instruction::account_meta::AccountMeta::new(payer, true),
        ],
        data: vec![],
    };
    let meta = svm.process_instruction(move_ix).expect("macro instruction should succeed");
    assert_eq!(svm.get_account(&from).expect("from exists").lamports, 1_000_000 - 7);
    assert_eq!(svm.get_account(&to).expect("to exists").lamports, 1_000_000 + 7);
    assert_eq!(svm.get_account(&to).expect("to exists").data[0], 0xAB);
    assert!(
        meta.compute_units_consumed >= 150,
        "expected at least 150 CUs, consumed {}",
        meta.compute_units_consumed
    );
    let failing_ix = Instruction {
        program_id,
        accounts: vec![
            solana_instruction::account_meta::AccountMeta::new(from, false),
            solana_instruction::account_meta::AccountMeta::new(to, false),
            solana_instruction::account_meta::AccountMeta::new(payer, true),
        ],
        data: vec![0xFF],
    };
    let failure = svm.process_instruction(failing_ix).expect_err("0xFF must fail");
    assert_eq!(failure.err, TransactionError::InstructionError(0, InstructionError::Custom(42)));
    assert_eq!(svm.get_account(&from).expect("from exists").lamports, 1_000_000 - 7);
    assert_eq!(svm.get_account(&to).expect("to exists").lamports, 1_000_000 + 7);

    // --- Custom precompile is registered (and, with the feature on, dispatched). ---
    assert!(
        svm.precompile_provider().program_ids().contains(&precompile_id),
        "the custom precompile id must be registered on the provider"
    );
    #[cfg(feature = "precompiles")]
    {
        use solana_svm_callback::InvokeContextCallback as _;
        assert!(svm.is_precompile(&precompile_id), "the custom id must dispatch as a precompile");
        let dispatch_ix = Instruction { program_id: precompile_id, accounts: vec![], data: vec![] };
        let failure =
            svm.process_instruction(dispatch_ix).expect_err("the custom precompile always fails");
        assert!(invoked.load(Ordering::SeqCst), "the custom precompile was not dispatched");
        assert_eq!(failure.err, TransactionError::InstructionError(0, InstructionError::Custom(4)));
    }
    #[cfg(not(feature = "precompiles"))]
    {
        use solana_svm_callback::InvokeContextCallback as _;
        assert!(
            !svm.is_precompile(&precompile_id),
            "without the feature the provider swap stays inert"
        );
    }

    // --- `commit()` keeps the staged post-state of a transfer. ---
    const FUNDS: u64 = 1_000_000_000;
    const FIRST_HOP: u64 = 64;
    const SECOND_HOP: u64 = 128;
    const SIGNATURE_FEE: u64 = 5_000;
    let wallet = Keypair::new();
    let wallet_address = wallet.pubkey();
    let recipient = Address::new_unique();
    let _ = svm.airdrop(&wallet_address, FUNDS).expect("airdrop should succeed");
    let _ = svm.airdrop(&recipient, FUNDS).expect("airdrop should succeed");
    let first = Transaction::new(
        &[&wallet],
        Message::new(&[transfer(&wallet_address, &recipient, FIRST_HOP)], Some(&wallet_address)),
        svm.latest_blockhash(),
    );
    let first_meta = svm
        .transact(first)
        .expect("transact should stage")
        .commit()
        .expect("transfer should succeed");
    assert_eq!(first_meta.fee, SIGNATURE_FEE);
    assert_eq!(svm.get_balance(&wallet_address), Some(FUNDS - SIGNATURE_FEE - FIRST_HOP));
    assert_eq!(svm.get_balance(&recipient), Some(FUNDS + FIRST_HOP));

    // --- `commit_with(&mut sink)` streams the exact account changes. ---
    let second = Transaction::new(
        &[&wallet],
        Message::new(&[transfer(&wallet_address, &recipient, SECOND_HOP)], Some(&wallet_address)),
        svm.latest_blockhash(),
    );
    let mut sink = RecordingSink::default();
    let second_meta = svm
        .transact(second)
        .expect("transact should stage")
        .commit_with(&mut sink)
        .expect("sink accepts every change")
        .expect("transfer should succeed");
    assert_eq!(second_meta.fee, SIGNATURE_FEE);
    assert_eq!(
        svm.get_balance(&wallet_address),
        Some(FUNDS - 2 * SIGNATURE_FEE - FIRST_HOP - SECOND_HOP)
    );
    assert_eq!(svm.get_balance(&recipient), Some(FUNDS + FIRST_HOP + SECOND_HOP));
    let mut seen = sink.seen.clone();
    seen.sort_by_key(|(address, _, _)| *address);
    let mut want = vec![
        (
            wallet_address,
            Some(FUNDS - SIGNATURE_FEE - FIRST_HOP),
            Some(FUNDS - 2 * SIGNATURE_FEE - FIRST_HOP - SECOND_HOP),
        ),
        (recipient, Some(FUNDS + FIRST_HOP), Some(FUNDS + FIRST_HOP + SECOND_HOP)),
    ];
    want.sort_by_key(|(address, _, _)| *address);
    assert_eq!(seen, want);

    println!(
        "custom_svm: every customisation observed (l1_slot=42, handler, instruction, precompile, commit, commit_with)"
    );
}
