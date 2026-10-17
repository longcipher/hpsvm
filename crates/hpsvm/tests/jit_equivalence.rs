//! Deep JIT equivalence gate, mirroring evm2's EEST JIT mode.
//!
//! evm2 runs its state/blockchain suites through a blocking, non-optimized
//! JIT backend (`hot_threshold: 0`) so JIT-vs-interpreter equivalence is a
//! first-class test mode. The hpsvm analog runs one workload matrix through
//! every [`JitPolicy`](hpsvm::JitPolicy) arm — `Eager` (the mainnet-fidelity
//! reference, always JIT on x86_64), `Off` (forced interpreter), `Lazy{0}`
//! (synchronous promotion) and `Lazy{8}` (background promotion) — and
//! asserts identical outcomes.
//!
//! Every arm builds its own VM from the same builder plan, so each arm gets
//! a fresh runtime environment (a per-arm custom syscall keeps it off the
//! shared default environment) and therefore distinct resident-store keys.
//! Without that isolation the process-global entry store would let arms
//! share entries and the comparison would be vacuous. Pre-state (funding,
//! program bytes, blockhash seed) is identical, so post-state is comparable
//! byte-for-byte.
//!
//! Hard gate: status, fee, logs, return data and touched post-state must be
//! identical. Compute units are asserted equal too and additionally printed
//! as a report table: sbpf meters JIT code in basic-block chunks, so any
//! future granularity divergence shows up here rather than silently.

use std::time::{Duration, Instant};

use agave_feature_set::FeatureSet;
use hpsvm::{HPSVM, HpsvmBuilder, JitPolicy};
use solana_account::Account;
use solana_address::{Address, address};
use solana_instruction::{Instruction, account_meta::AccountMeta};
use solana_keypair::Keypair;
use solana_message::{Message, VersionedMessage};
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::{Transaction, versioned::VersionedTransaction};

type TxResult = hpsvm::types::TransactionResult;

const POLICIES: [(JitPolicy, &str); 4] = [
    (JitPolicy::Eager, "eager"),
    (JitPolicy::Off, "off"),
    (JitPolicy::Lazy { hot_threshold: 0 }, "lazy0"),
    (JitPolicy::Lazy { hot_threshold: 8 }, "lazy8"),
];

fn arm(policy: JitPolicy) -> HPSVM {
    // All protocol features on (closest to mainnet fidelity: full SBPF
    // version, rent, budget) plus one dummy custom syscall. Any custom
    // syscall forces a fresh runtime environment per VM, so every arm gets
    // a distinct environment `Arc` and therefore distinct resident-store
    // keys — genuine per-policy entries, never vacuous cross-arm sharing.
    // (`FeatureSet::default()` would also isolate, but it downgrades the
    // SBPF version and newer programs fail to load there.)
    HpsvmBuilder::new()
        .with_feature_set(FeatureSet::all_enabled())
        .with_jit_policy(policy)
        .with_builtins()
        .with_custom_syscall("jit_probe_nop", register_probe_nop)
        .with_sysvars()
        .with_lamports(1_000_000_000)
        .with_sigverify(true)
        .with_blockhash_check(true)
        .build()
        .expect("policy arm VM should build")
}

solana_program_runtime::solana_sbpf::declare_builtin_function!(
    /// Dummy syscall existing only to give each matrix arm a distinct
    /// runtime environment. Never called by any probe program.
    SyscallProbeNop,
    fn rust(
        _invoke_context: &mut solana_program_runtime::invoke_context::InvokeContext<'_, '_>,
        _arg1: u64,
        _arg2: u64,
        _arg3: u64,
        _arg4: u64,
        _arg5: u64,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        Ok(0)
    }
);

fn register_probe_nop(
    loader: &mut solana_program_runtime::solana_sbpf::program::BuiltinProgram<
        solana_program_runtime::invoke_context::InvokeContext<'static, 'static>,
    >,
    name: &str,
) -> Result<(), solana_program_runtime::solana_sbpf::elf::ElfError> {
    loader.register_definition::<SyscallProbeNop>(name)
}

/// Outcome captured per policy arm for one workload run.
#[derive(Debug)]
struct ArmOutcome {
    name: &'static str,
    result: TxResult,
    /// (address, lamports, data) for every account the workload asserts on.
    post_state: Vec<(Address, u64, Vec<u8>)>,
}

/// Compares every arm against the `Eager` reference: status, fee,
/// compute units, logs, return data and post-state must match exactly.
fn assert_arms_match(workload: &'static str, outcomes: &[ArmOutcome], cu_table: &mut Vec<CuRow>) {
    let reference =
        outcomes.iter().find(|o| o.name == "eager").expect("eager reference arm must exist");
    for outcome in outcomes {
        let tag = format!("{workload}/{}", outcome.name);
        match (&reference.result, &outcome.result) {
            (Ok(expected), Ok(actual)) => {
                assert_eq!(actual.fee, expected.fee, "{tag}: fee diverged");
                assert_eq!(
                    actual.compute_units_consumed, expected.compute_units_consumed,
                    "{tag}: CU diverged (JIT meter granularity?)"
                );
                assert_eq!(actual.logs, expected.logs, "{tag}: logs diverged");
                assert_eq!(actual.return_data, expected.return_data, "{tag}: return data diverged");
                assert_eq!(
                    actual.inner_instructions.len(),
                    expected.inner_instructions.len(),
                    "{tag}: inner-instruction count diverged"
                );
            }
            (Err(expected), Err(actual)) => {
                assert_eq!(actual.err, expected.err, "{tag}: failure kind diverged");
                assert_eq!(
                    actual.meta.compute_units_consumed, expected.meta.compute_units_consumed,
                    "{tag}: failing-tx CU diverged"
                );
                assert_eq!(actual.meta.logs, expected.meta.logs, "{tag}: failing-tx logs diverged");
            }
            (expected, actual) => {
                panic!("{tag}: success/failure diverged: expected {expected:?}, got {actual:?}");
            }
        }
        assert_eq!(outcome.post_state, reference.post_state, "{tag}: post-state diverged");
    }
    let mut row = CuRow { workload, cus: Vec::new() };
    for outcome in outcomes {
        let cu = match &outcome.result {
            Ok(meta) => meta.compute_units_consumed,
            Err(failed) => failed.meta.compute_units_consumed,
        };
        row.cus.push((outcome.name, cu));
    }
    cu_table.push(row);
}

struct CuRow {
    workload: &'static str,
    cus: Vec<(&'static str, u64)>,
}

fn print_cu_table(cu_table: &[CuRow]) {
    println!("JIT equivalence CU report (compute units per workload x policy):");
    for row in cu_table {
        let cells: Vec<String> = row.cus.iter().map(|(name, cu)| format!("{name}={cu}")).collect();
        println!("  {:28} {}", row.workload, cells.join(" "));
    }
}

fn snapshot_account(svm: &HPSVM, address: Address) -> (Address, u64, Vec<u8>) {
    let account = svm.get_account(&address).unwrap_or_else(|| {
        panic!("expected account {address} to exist after {address}");
    });
    (address, account.lamports, account.data.clone())
}

fn fund(svm: &mut HPSVM, payer: &Address) {
    svm.airdrop(payer, 100_000_000).unwrap();
}

/// Deterministic address shared by every policy arm of one workload.
/// Random per-arm addresses would leak into program logs and break the
/// comparison; separate VMs never collide on the same address.
fn addr(seed: u8) -> Address {
    Address::new_from_array([seed; 32])
}

/// Deterministic payer shared by every policy arm of one workload.
fn payer(seed: u8) -> Keypair {
    Keypair::new_from_array([seed; 32])
}

fn memo_v3() -> &'static [u8] {
    include_bytes!("../elf/spl_memo-3.0.0.so")
}

fn memo_v1() -> &'static [u8] {
    include_bytes!("../elf/spl_memo-1.0.0.so")
}

fn memo_ix(program_id: Address, nonce: u8) -> Instruction {
    let mut data = b"jit-equivalence-probe".to_vec();
    data.push(nonce);
    Instruction { program_id, accounts: vec![], data }
}

fn send_labeled(svm: &mut HPSVM, payer: &Keypair, ixs: Vec<Instruction>) -> TxResult {
    let msg = Message::new_with_blockhash(&ixs, Some(&payer.pubkey()), &svm.latest_blockhash());
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[payer])
        .expect("tx should build");
    svm.send_transaction(tx)
}

/// System transfer (no SBF programs): the control arm.
#[test]
fn jit_off_matches_eager_for_system_transfer() {
    let mut cu_table = Vec::new();
    let mut outcomes = Vec::new();
    for (policy, name) in POLICIES {
        let mut svm = arm(policy);
        let payer = payer(12);
        let recipient = addr(11);
        fund(&mut svm, &payer.pubkey());
        let rate = 64;
        let tx = Transaction::new(
            &[&payer],
            Message::new(&[transfer(&payer.pubkey(), &recipient, rate)], Some(&payer.pubkey())),
            svm.latest_blockhash(),
        );
        let result = svm.send_transaction(tx);
        assert!(result.is_ok(), "{name}: transfer should succeed: {result:?}");
        let post_state =
            vec![snapshot_account(&svm, payer.pubkey()), snapshot_account(&svm, recipient)];
        outcomes.push(ArmOutcome { name, result, post_state });
    }
    assert_arms_match("system-transfer", &outcomes, &mut cu_table);
    print_cu_table(&cu_table);
}

/// Memo v3 (bpf_loader) and memo v1 (bpf_loader_deprecated): distinct
/// loaders, distinct ELFs, single-instruction transactions.
#[test]
fn jit_off_matches_eager_for_memo_programs() {
    for (elf, loader, label) in [
        (memo_v3(), solana_sdk_ids::bpf_loader::id(), "memo-v3"),
        (memo_v1(), solana_sdk_ids::bpf_loader_deprecated::id(), "memo-v1"),
    ] {
        let mut cu_table = Vec::new();
        let mut outcomes = Vec::new();
        for (policy, name) in POLICIES {
            let mut svm = arm(policy);
            let program_id = addr(21);
            svm.add_program_with_loader(program_id, elf, loader).expect("add memo");
            let payer = payer(22);
            fund(&mut svm, &payer.pubkey());
            let result = send_labeled(&mut svm, &payer, vec![memo_ix(program_id, 0)]);
            assert!(result.is_ok(), "{label}/{name} should succeed: {result:?}");
            let post_state = vec![snapshot_account(&svm, payer.pubkey())];
            outcomes.push(ArmOutcome { name, result, post_state });
        }
        assert_arms_match(label, &outcomes, &mut cu_table);
        print_cu_table(&cu_table);
    }
}

/// Three memo instructions (two programs) in one transaction, plus a
/// discard-then-commit pair to cover the rollback path under every policy.
#[test]
fn jit_off_matches_eager_for_multi_memo_and_discard() {
    let mut cu_table = Vec::new();
    let mut outcomes = Vec::new();
    for (policy, name) in POLICIES {
        let mut svm = arm(policy);
        let v3 = addr(31);
        let v1 = addr(32);
        svm.add_program_with_loader(v3, memo_v3(), solana_sdk_ids::bpf_loader::id())
            .expect("add v3");
        svm.add_program_with_loader(v1, memo_v1(), solana_sdk_ids::bpf_loader_deprecated::id())
            .expect("add v1");
        let payer = payer(32);
        fund(&mut svm, &payer.pubkey());
        // Discard path first: must return the same result and leave state
        // untouched under every policy.
        let discard_result = send_labeled(&mut svm, &payer, vec![memo_ix(v3, 10), memo_ix(v1, 11)]);
        assert!(discard_result.is_ok(), "{name}: discard probe should succeed");
        let before = svm.get_balance(&payer.pubkey());
        let discarded = svm
            .transact({
                let msg = Message::new_with_blockhash(
                    &[memo_ix(v3, 12)],
                    Some(&payer.pubkey()),
                    &svm.latest_blockhash(),
                );
                VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[&payer]).unwrap()
            })
            .expect("transact should stage")
            .discard();
        assert!(discarded.is_ok(), "{name}: discard should succeed");
        assert_eq!(svm.get_balance(&payer.pubkey()), before, "{name}: discard must roll back");
        let result =
            send_labeled(&mut svm, &payer, vec![memo_ix(v3, 0), memo_ix(v1, 1), memo_ix(v3, 2)]);
        assert!(result.is_ok(), "{name}: multi-memo should succeed: {result:?}");
        // Fold the discard probe into the comparison so rollback behavior is
        // gated too (same policy arm, same pre-state class).
        assert!(discard_result.is_ok(), "{name}: discard probe should succeed");
        let post_state = vec![snapshot_account(&svm, payer.pubkey())];
        outcomes.push(ArmOutcome { name, result, post_state });
    }
    assert_arms_match("multi-memo", &outcomes, &mut cu_table);
    print_cu_table(&cu_table);
}

/// Counter program (stateful SBF): two greets, post-state is program-owned
/// account data. Skipped when the SBF program was not built.
#[test]
fn jit_off_matches_eager_for_counter() {
    let Some(counter) = hpsvm_test_support::read_program(hpsvm_test_support::COUNTER) else {
        eprintln!("skipping: SBF program `counter.so` is unavailable");
        return;
    };
    let mut cu_table = Vec::new();
    let mut outcomes = Vec::new();
    for (policy, name) in POLICIES {
        let mut svm = arm(policy);
        let program_id = addr(41);
        let counter_address = addr(42);
        svm.add_program(program_id, &counter).expect("add counter");
        svm.set_account(
            counter_address,
            Account { lamports: 5, data: vec![0; 4], owner: program_id, ..Default::default() },
        )
        .unwrap();
        let payer = payer(42);
        fund(&mut svm, &payer.pubkey());
        for deduper in 0..2u8 {
            let ix = Instruction {
                program_id,
                accounts: vec![AccountMeta::new(counter_address, false)],
                data: vec![0, deduper],
            };
            let result = send_labeled(&mut svm, &payer, vec![ix]);
            assert!(result.is_ok(), "{name}: greet {deduper} should succeed: {result:?}");
            if deduper == 1 {
                let post_state = vec![
                    snapshot_account(&svm, payer.pubkey()),
                    snapshot_account(&svm, counter_address),
                ];
                outcomes.push(ArmOutcome { name, result, post_state });
            }
        }
    }
    assert_arms_match("counter", &outcomes, &mut cu_table);
    print_cu_table(&cu_table);
}

/// Failure program (always `Custom(0)`): the error path must be identical
/// under every policy, including logs and CU. Skipped when not built.
#[test]
fn jit_off_matches_eager_for_failing_program() {
    let Some(failure) = hpsvm_test_support::read_program(hpsvm_test_support::FAILURE) else {
        eprintln!("skipping: SBF program `failure.so` is unavailable");
        return;
    };
    let mut cu_table = Vec::new();
    let mut outcomes = Vec::new();
    for (policy, name) in POLICIES {
        let mut svm = arm(policy);
        let program_id = address!("HvrRMSshMx3itvsyWDnWg2E3cy5h57iMaR7oVxSZJDSA");
        svm.add_program(program_id, &failure).expect("add failure program");
        let payer = payer(52);
        fund(&mut svm, &payer.pubkey());
        let ix = Instruction { program_id, accounts: vec![], data: vec![] };
        let result = send_labeled(&mut svm, &payer, vec![ix]);
        assert!(result.is_err(), "{name}: failure program should fail");
        let post_state = vec![snapshot_account(&svm, payer.pubkey())];
        outcomes.push(ArmOutcome { name, result, post_state });
    }
    assert_arms_match("failure", &outcomes, &mut cu_table);
    print_cu_table(&cu_table);
}

/// Clock example (sysvar read + assert): identical slot/clock pre-state per
/// arm, identical outcome. Skipped when not built.
#[test]
fn jit_off_matches_eager_for_clock_example() {
    let Some(clock) = hpsvm_test_support::read_program(hpsvm_test_support::CLOCK_EXAMPLE) else {
        eprintln!("skipping: SBF program `hpsvm_clock_example.so` is unavailable");
        return;
    };
    let mut cu_table = Vec::new();
    let mut outcomes = Vec::new();
    for (policy, name) in POLICIES {
        let mut svm = arm(policy);
        let program_id = addr(51);
        svm.add_program(program_id, &clock).expect("add clock example");
        let payer = payer(62);
        fund(&mut svm, &payer.pubkey());
        let ix = Instruction { program_id, accounts: vec![], data: vec![] };
        let result = send_labeled(&mut svm, &payer, vec![ix]);
        assert!(result.is_ok(), "{name}: clock example should succeed: {result:?}");
        let post_state = vec![snapshot_account(&svm, payer.pubkey())];
        outcomes.push(ArmOutcome { name, result, post_state });
    }
    assert_arms_match("clock", &outcomes, &mut cu_table);
    print_cu_table(&cu_table);
}

/// Batch path: one conflict-free stage of transfers under every policy.
#[test]
fn jit_batch_matches_across_policies() {
    let mut cu_table = Vec::new();
    // Per-arm result vectors, compared index-wise against eager.
    let mut all_results: Vec<(&str, Vec<TxResult>, Vec<(Address, u64, Vec<u8>)>)> = Vec::new();
    for (policy, name) in POLICIES {
        let mut svm = arm(policy);
        let mut senders = Vec::new();
        for _ in 0..4 {
            let payer = payer(70 + senders.len() as u8);
            fund(&mut svm, &payer.pubkey());
            senders.push(payer);
        }
        let blockhash = svm.latest_blockhash();
        let txs: Vec<VersionedTransaction> = senders
            .iter()
            .map(|sender| {
                let recipient = addr(100 + senders.len() as u8);
                let msg = Message::new_with_blockhash(
                    &[transfer(&sender.pubkey(), &recipient, 7)],
                    Some(&sender.pubkey()),
                    &blockhash,
                );
                VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[sender]).unwrap()
            })
            .collect();
        let batch = svm.send_transaction_batch(txs).expect("batch should plan");
        assert_eq!(batch.results.len(), 4);
        let mut post_state = Vec::new();
        for sender in &senders {
            post_state.push(snapshot_account(&svm, sender.pubkey()));
        }
        all_results.push((name, batch.results, post_state));
    }
    let reference =
        all_results.iter().find(|(n, _, _)| *n == "eager").expect("eager arm must exist");
    for (name, results, post_state) in &all_results {
        assert_eq!(results.len(), reference.1.len(), "{name}: batch length diverged");
        for (index, (actual, expected)) in results.iter().zip(reference.1.iter()).enumerate() {
            match (expected, actual) {
                (Ok(e), Ok(a)) => {
                    assert_eq!(a.fee, e.fee, "{name}[{index}]: fee diverged");
                    assert_eq!(
                        a.compute_units_consumed, e.compute_units_consumed,
                        "{name}[{index}]: CU diverged"
                    );
                    assert_eq!(a.logs, e.logs, "{name}[{index}]: logs diverged");
                }
                (Err(e), Err(a)) => {
                    assert_eq!(a.err, e.err, "{name}[{index}]: failure kind diverged");
                }
                _ => panic!("{name}[{index}]: success/failure diverged"),
            }
        }
        assert_eq!(post_state, &reference.2, "{name}: batch post-state diverged");
    }
    let mut row = CuRow { workload: "batch-transfer", cus: Vec::new() };
    for (name, results, _) in &all_results {
        let total: u64 = results
            .iter()
            .map(|r| match r {
                Ok(m) => m.compute_units_consumed,
                Err(f) => f.meta.compute_units_consumed,
            })
            .sum();
        row.cus.push((name, total));
    }
    cu_table.push(row);
    print_cu_table(&cu_table);
}

/// Simulate (detached, no staging) under every policy.
#[test]
fn jit_simulate_matches_across_policies() {
    let mut cu_table = Vec::new();
    let mut outcomes = Vec::new();
    for (policy, name) in POLICIES {
        // Fresh arm per policy (fresh environment → genuine per-policy
        // entries, never vacuous cross-arm sharing).
        let mut svm = arm(policy);
        let program_id = addr(61);
        svm.add_program_with_loader(program_id, memo_v3(), solana_sdk_ids::bpf_loader::id())
            .expect("add memo");
        let payer = payer(82);
        fund(&mut svm, &payer.pubkey());
        let msg = Message::new_with_blockhash(
            &[memo_ix(program_id, 3)],
            Some(&payer.pubkey()),
            &svm.latest_blockhash(),
        );
        let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[&payer]).unwrap();
        let simulated = svm.simulate_transaction(tx).expect("simulate should succeed");
        // Wrap the simulated meta as an `Ok` outcome for the shared comparator.
        let result: TxResult = Ok(simulated.meta);
        let post_state = vec![snapshot_account(&svm, payer.pubkey())];
        outcomes.push(ArmOutcome { name, result, post_state });
    }
    assert_arms_match("simulate-memo", &outcomes, &mut cu_table);
    print_cu_table(&cu_table);
}

/// `Lazy{8}` promotion convergence: repeated executions stay identical
/// before and after the background promotion publishes.
#[test]
fn jit_lazy8_converges_and_stays_identical() {
    if !cfg!(target_arch = "x86_64") {
        eprintln!("skipping: promotion only exists on x86_64");
        return;
    }
    let mut eager = arm(JitPolicy::Eager);
    let mut lazy = arm(JitPolicy::Lazy { hot_threshold: 8 });
    let eager_id = addr(71);
    let lazy_id = addr(71);
    eager.add_program_with_loader(eager_id, memo_v3(), solana_sdk_ids::bpf_loader::id()).unwrap();
    lazy.add_program_with_loader(lazy_id, memo_v3(), solana_sdk_ids::bpf_loader::id()).unwrap();
    let eager_payer = payer(91);
    let lazy_payer = payer(91);
    fund(&mut eager, &eager_payer.pubkey());
    fund(&mut lazy, &lazy_payer.pubkey());
    let mut cu_table = Vec::new();
    for round in 0..10u8 {
        let eager_tx = {
            let msg = Message::new_with_blockhash(
                &[memo_ix(eager_id, round)],
                Some(&eager_payer.pubkey()),
                &eager.latest_blockhash(),
            );
            VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[&eager_payer]).unwrap()
        };
        let lazy_tx = {
            let msg = Message::new_with_blockhash(
                &[memo_ix(lazy_id, round)],
                Some(&lazy_payer.pubkey()),
                &lazy.latest_blockhash(),
            );
            VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[&lazy_payer]).unwrap()
        };
        let eager_result = eager.send_transaction(eager_tx);
        let lazy_result = lazy.send_transaction(lazy_tx);
        match (&eager_result, &lazy_result) {
            (Ok(e), Ok(l)) => {
                assert_eq!(l.fee, e.fee, "round {round}: fee diverged");
                assert_eq!(
                    l.compute_units_consumed, e.compute_units_consumed,
                    "round {round}: CU diverged"
                );
                assert_eq!(l.logs, e.logs, "round {round}: logs diverged");
                assert_eq!(l.return_data, e.return_data, "round {round}: return data diverged");
            }
            _ => panic!("round {round}: success/failure diverged"),
        }
        if round == 0 || round == 9 {
            let mut row = CuRow {
                workload: if round == 0 { "lazy8-round0" } else { "lazy8-round9" },
                cus: Vec::new(),
            };
            row.cus.push((
                "eager",
                match &eager_result {
                    Ok(m) => m.compute_units_consumed,
                    Err(f) => f.meta.compute_units_consumed,
                },
            ));
            row.cus.push((
                "lazy8",
                match &lazy_result {
                    Ok(m) => m.compute_units_consumed,
                    Err(f) => f.meta.compute_units_consumed,
                },
            ));
            cu_table.push(row);
        }
    }
    // After 10 interpreted runs past threshold 8, the entry must have been
    // promoted on the background pool.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if lazy.program_tier_stats().promotions_succeeded >= 1 {
            break;
        }
        if Instant::now() > deadline {
            panic!("lazy8 never promoted after 10 hot executions");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    print_cu_table(&cu_table);
}
