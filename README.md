# HPSVM

## 📍 Overview

`hpsvm` is a fast and lightweight library for testing Solana programs. It works by creating an in-process Solana VM optimized for program developers. This makes it much faster to run and compile than alternatives like `solana-program-test` and `solana-test-validator`. In a further break from tradition, it has an ergonomic API with sane defaults and extensive configurability for those who want it.

## 🚀 Getting Started

### 🔧 Installation

```sh
cargo add --dev hpsvm
```

### 🤖 Minimal Example

```rust
use hpsvm::HPSVM;
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;

let from_keypair = Keypair::new();
let from = from_keypair.pubkey();
let to = Address::new_unique();

let mut svm = HPSVM::new();
svm.airdrop(&from, 10_000).unwrap();

let instruction = transfer(&from, &to, 64);
let tx = Transaction::new(
    &[&from_keypair],
    Message::new(&[instruction], Some(&from)),
    svm.latest_blockhash(),
);
let tx_res = svm.send_transaction(tx).unwrap();

let from_account = svm.get_account(&from);
let to_account = svm.get_account(&to);
assert_eq!(from_account.unwrap().lamports, 4936);
assert_eq!(to_account.unwrap().lamports, 64);
```

### 🛠️ Developing hpsvm

#### Run the tests

The tests in this repo use some test programs you need to build first (Solana CLI >= 1.18.8 required):

```cd crates/hpsvm/test_programs && cargo build-sbf```

Then just run `cargo test`.
