# HPSVM

[![DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/longcipher/hpsvm)
[![Context7](https://img.shields.io/badge/Website-context7.com-blue)](https://context7.com/longcipher/hpsvm)
[![crates.io](https://img.shields.io/crates/v/hpsvm.svg)](https://crates.io/crates/hpsvm)
[![docs.rs](https://docs.rs/hpsvm/badge.svg)](https://docs.rs/hpsvm)

![hpsvm](https://socialify.git.ci/longcipher/hpsvm/image?font=Source+Code+Pro&language=1&name=1&owner=1&pattern=Circuit+Board&theme=Auto)

## 📍 Overview

`hpsvm` is a fast and lightweight library for testing Solana programs. It works by creating an in-process Solana VM optimized for program developers. This makes it much faster to run and compile than alternatives like `solana-program-test` and `solana-test-validator`. In a further break from tradition, it has an ergonomic API with sane defaults and extensive configurability for those who want it.

`hpsvm` is optimized for low-overhead, in-process test execution. It does not try to emulate Sealevel-style concurrent scheduling inside a single VM instance. State-committing APIs such as `send_transaction` intentionally mutate one in-memory test environment in place.

This is a pure Rust library, making it ideal for Rust-native Solana development workflows.

## ✨ Features

- 🚀 **High Performance**: In-process VM avoids validator process and RPC overhead, so tests run significantly faster than external validators
- 🛠️ **Easy to Use**: Simple API with sensible defaults and comprehensive configuration options
- 🔧 **Pure Rust**: No external dependencies or runtime requirements beyond Rust
- 📊 **Comprehensive Testing**: Supports transactions, account management, and program execution
- 🔄 **Configurable**: Extensive options for customizing the test environment
- 📚 **Well Documented**: Full API documentation and examples

## 🚀 Getting Started

### Prerequisites

- Rust
- Solana CLI (for building test programs)

### 🔧 Installation

Add `hpsvm` as a development dependency to your Solana program project:

```sh
cargo add --dev hpsvm
```

To read through live RPC state while keeping execution local, add the companion crate as well:

```sh
cargo add --dev hpsvm-fork-rpc
```

### 🤖 Quick Example

Here's a minimal example that demonstrates creating a test environment, airdropping SOL, and executing a transfer transaction:

```rust
use hpsvm::HPSVM;
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;

// Create keypairs for testing
let from_keypair = Keypair::new();
let from = from_keypair.pubkey();
let to = Address::new_unique();

// Initialize the SVM with default configuration
let mut svm = HPSVM::new();

// Airdrop SOL to the sender account
svm.airdrop(&from, 10_000).unwrap();

// Create a transfer instruction
let instruction = transfer(&from, &to, 64);

// Build and sign the transaction
let tx = Transaction::new(
    &[&from_keypair],
    Message::new(&[instruction], Some(&from)),
    svm.latest_blockhash(),
);

// Execute the transaction
let tx_result = svm.send_transaction(tx).unwrap();

// Verify the results
let from_account = svm.get_account(&from).unwrap();
let to_account = svm.get_account(&to).unwrap();
assert_eq!(from_account.lamports, 4936);  // 10000 - 64 - fee
assert_eq!(to_account.lamports, 64);
```

### 📖 Usage

For more advanced usage, including custom configurations, program deployment, and complex transaction scenarios, see the [full documentation](https://docs.rs/hpsvm).

### Architecture Highlights

`hpsvm` keeps the `HPSVM` facade stable while exposing a few sharper seams for advanced test harnesses:

- `transact` computes an `ExecutionOutcome` without mutating the VM, and `commit_transaction` applies it explicitly when you want to persist the result.
- `with_account_source` lets the VM read missing accounts from an external source while keeping local writes in the in-memory overlay.
- `block_env` exposes the current blockhash and slot snapshot, and `with_inspector` installs lightweight top-level execution observers.

```rust
use hpsvm::HPSVM;
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;

let mut svm = HPSVM::new();
let payer = Keypair::new();
let recipient = Address::new_unique();

svm.airdrop(&payer.pubkey(), 10_000).unwrap();

let tx = Transaction::new(
    &[&payer],
    Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
    svm.latest_blockhash(),
);

let outcome = svm.transact(tx);
assert!(outcome.status().is_ok());
assert_eq!(svm.get_balance(&recipient), None);

let commit = svm.commit_transaction(outcome);
assert!(commit.is_ok());
assert_eq!(svm.get_balance(&recipient), Some(64));
assert_eq!(svm.block_env().latest_blockhash, svm.latest_blockhash());
```

### Forking RPC State

`hpsvm` can read missing accounts through a configured account source. The `hpsvm-fork-rpc` companion crate provides an RPC-backed source with a local cache:

```rust
use hpsvm::HPSVM;
use hpsvm_fork_rpc::RpcForkSource;

let source = RpcForkSource::builder()
    .with_rpc_url("http://127.0.0.1:8899")
    .with_slot(1)
    .build();

let svm = HPSVM::builder().with_account_source(source).build().unwrap();
```

### Top-Level Instruction Inspection

Use `with_inspector` when you need lightweight transaction observation without reaching into the lower-level invocation callback APIs:

```rust
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use hpsvm::{HPSVM, Inspector};
use solana_address::Address;

#[derive(Default)]
struct CountingInspector {
    seen: Arc<AtomicUsize>,
}

impl Inspector for CountingInspector {
    fn on_instruction(&self, _svm: &HPSVM, _index: usize, _program_id: &Address) {
        self.seen.fetch_add(1, Ordering::SeqCst);
    }
}

let inspector = CountingInspector::default();
let observed = Arc::clone(&inspector.seen);
let _svm = HPSVM::new().with_inspector(inspector);

assert_eq!(observed.load(Ordering::SeqCst), 0);
```

## 🛠️ Developing hpsvm

### Building Test Programs

The test suite uses Solana programs that need to be built first:

```bash
cd crates/hpsvm/test_programs
cargo build-sbf
```

### Running Tests

Run the full test suite:

```bash
cargo test
```

### Running Benchmarks

```bash
cargo bench
```

Build the benchmark helper program and profile the public HPSVM surface:

```bash
HPSVM_HOTPATH=1 cargo bench -p hpsvm --features hotpath --bench core_interfaces
```

Profile the steady-state transaction execution hot path:

```bash
HPSVM_HOTPATH=1 cargo bench -p hpsvm --features hotpath --bench max_perf
```

Export a deeper trace summary for the steady-state SBF execution path:

```bash
just bench-hotpath-trace
```

`core_interfaces` covers the main public APIs: `new`, `set_account`, `get_account`,
`expire_blockhash`, `add_program`, `add_program_from_file`, `airdrop`,
`send_transaction`, `simulate_transaction`, `transact` + `commit_transaction`,
`plan_transaction_batch`, and `send_transaction_batch`.

When `HPSVM_HOTPATH=1` is set, hotpath reports are written to `target/hotpath/*.json`.
Use `HPSVM_HOTPATH_LIMIT` to cap the number of reported functions, or `HOTPATH_OUTPUT_PATH`
to override the default report path.

When `HPSVM_TRACE_METRICS=1` is combined with the `register-tracing` feature, `just bench-hotpath-trace`
also writes `target/hotpath/max_perf.trace.json` and `target/hotpath/simple_bench.trace.json`.
These summaries report per-program invocation counts, CPI depth, register-frame counts, and
instruction-account counts so you can tell whether the `execute_instruction` hotspot is dominated
by nested CPIs or a single SBF program body in both the steady-state loop and the repeated
transaction-loop benchmark.

To compare default runtime benchmarks without hotpath overhead:

```bash
just bench-runtime
just bench-runtime-baseline-compare
```

This writes compact Criterion reports to `target/runtime-benchmarks/*.json` and a markdown summary
to `target/runtime-benchmarks/summary.md`. Use this runtime baseline before treating a hotpath-only
delta as a real VM slowdown.

To refresh the committed default runtime baselines:

```bash
just bench-runtime-baseline-refresh
```

To refresh the committed hotpath baselines:

```bash
just bench-baseline-refresh
```

To compare a fresh benchmark run with the committed baselines:

```bash
just bench-hotpath
just bench-baseline-compare
```

### Code Quality

Format code:

```bash
cargo fmt
```

Lint code:

```bash
cargo clippy
```

## 🙏 Acknowledgments

- Initially forked from [litesvm](https://github.com/LiteSVM/litesvm)
- Built for the Solana ecosystem
- Inspired by the need for faster, more ergonomic testing tools
- Thanks to the Solana community for their contributions and feedback
