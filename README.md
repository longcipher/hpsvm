# HPSVM

[![DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/longcipher/hpsvm)
[![Context7](https://img.shields.io/badge/Website-context7.com-blue)](https://context7.com/longcipher/hpsvm)
[![crates.io](https://img.shields.io/crates/v/hpsvm.svg)](https://crates.io/crates/hpsvm)
[![docs.rs](https://docs.rs/hpsvm/badge.svg)](https://docs.rs/hpsvm)

![hpsvm](https://socialify.git.ci/longcipher/hpsvm/image?font=Source+Code+Pro&language=1&name=1&owner=1&pattern=Circuit+Board&theme=Auto)

## 📍 Overview

`hpsvm` is a fast and lightweight library for testing Solana programs. It works by creating an in-process Solana VM optimized for program developers. This makes it much faster to run and compile than alternatives like `solana-program-test` and `solana-test-validator`. In a further break from tradition, it has an ergonomic API with sane defaults and extensive configurability for those who want it.

This is a pure Rust library, making it ideal for Rust-native Solana development workflows.

## ✨ Features

- 🚀 **High Performance**: In-process VM runs tests significantly faster than external validators
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
