use std::hint::black_box;

mod common;

use common::{
    HotpathGuard, counter_account, counter_program_path, make_counter_tx, new_benchmark_vm,
    read_counter_program,
};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use hpsvm::HPSVM;
use solana_account::Account;
use solana_address::Address;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_system_interface::instruction::transfer;
use solana_transaction::Transaction;

fn make_transfer_tx(
    from: &Keypair,
    to: &Address,
    lamports: u64,
    blockhash: solana_hash::Hash,
) -> Transaction {
    let msg = Message::new_with_blockhash(
        &[transfer(&from.pubkey(), to, lamports)],
        Some(&from.pubkey()),
        &blockhash,
    );
    Transaction::new(&[from], msg, blockhash)
}

fn build_transfer_case(lamports: u64) -> (HPSVM, Transaction) {
    let mut svm = new_benchmark_vm();
    let payer = Keypair::new();
    let recipient = Address::new_unique();

    svm.airdrop(&payer.pubkey(), 10_000_000).unwrap();
    let tx = make_transfer_tx(&payer, &recipient, lamports, svm.latest_blockhash());

    (svm, tx)
}

fn build_counter_case(deduper: u8) -> (HPSVM, Transaction) {
    let mut svm = new_benchmark_vm();
    let payer = Keypair::new();
    let payer_pk = payer.pubkey();
    let program_id = Address::new_unique();
    let counter_address = Address::new_unique();

    svm.add_program(program_id, &read_counter_program()).unwrap();
    svm.airdrop(&payer_pk, 1_000_000_000).unwrap();
    svm.set_account(counter_address, counter_account(program_id)).unwrap();
    let tx = make_counter_tx(
        program_id,
        counter_address,
        &payer_pk,
        svm.latest_blockhash(),
        &payer,
        deduper,
    );

    (svm, tx)
}

fn build_batch_case() -> (HPSVM, Vec<Transaction>) {
    let mut svm = new_benchmark_vm();
    let payer_a = Keypair::new();
    let payer_b = Keypair::new();
    let recipient_a = Address::new_unique();
    let recipient_b = Address::new_unique();
    let recipient_c = Address::new_unique();

    svm.airdrop(&payer_a.pubkey(), 10_000_000).unwrap();
    svm.airdrop(&payer_b.pubkey(), 10_000_000).unwrap();
    let blockhash = svm.latest_blockhash();

    let txs = vec![
        make_transfer_tx(&payer_a, &recipient_a, 10_000, blockhash),
        make_transfer_tx(&payer_b, &recipient_b, 10_000, blockhash),
        make_transfer_tx(&payer_a, &recipient_c, 5_000, blockhash),
    ];

    (svm, txs)
}

fn criterion_benchmark(c: &mut Criterion) {
    let _hotpath = HotpathGuard::new("core_interfaces");
    let program_bytes = read_counter_program();
    let program_path = counter_program_path();

    let mut group = c.benchmark_group("core_interfaces");

    group.bench_function("new", |b| {
        b.iter(|| {
            black_box(HPSVM::new());
        })
    });

    group.bench_function("set_account", |b| {
        b.iter_batched(
            || {
                let svm = new_benchmark_vm();
                let address = Address::new_unique();
                let owner = Address::new_unique();
                (svm, address, owner)
            },
            |(mut svm, address, owner)| {
                svm.set_account(
                    address,
                    Account {
                        lamports: 1_000_000,
                        data: vec![0_u8; 256],
                        owner,
                        ..Default::default()
                    },
                )
                .unwrap();
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("get_account", |b| {
        b.iter_batched(
            || {
                let mut svm = new_benchmark_vm();
                let address = Address::new_unique();
                let owner = Address::new_unique();
                svm.set_account(
                    address,
                    Account {
                        lamports: 1_000_000,
                        data: vec![7_u8; 256],
                        owner,
                        ..Default::default()
                    },
                )
                .unwrap();
                (svm, address)
            },
            |(svm, address)| {
                black_box(svm.get_account(&address).unwrap());
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("expire_blockhash", |b| {
        b.iter_batched(
            new_benchmark_vm,
            |mut svm| {
                svm.expire_blockhash();
                black_box(svm.latest_blockhash());
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("add_program", |b| {
        b.iter_batched(
            new_benchmark_vm,
            |mut svm| {
                svm.add_program(Address::new_unique(), &program_bytes).unwrap();
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("add_program_from_file", |b| {
        b.iter_batched(
            new_benchmark_vm,
            |mut svm| {
                svm.add_program_from_file(Address::new_unique(), &program_path).unwrap();
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("airdrop", |b| {
        b.iter_batched(
            || (new_benchmark_vm(), Address::new_unique()),
            |(mut svm, address)| {
                black_box(svm.airdrop(&address, 1_000_000).unwrap());
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("send_transaction/system_transfer", |b| {
        b.iter_batched(
            || build_transfer_case(10_000),
            |(mut svm, tx)| {
                black_box(svm.send_transaction(tx).unwrap());
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("send_transaction/program_invocation", |b| {
        b.iter_batched(
            || build_counter_case(0),
            |(mut svm, tx)| {
                black_box(svm.send_transaction(tx).unwrap());
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("simulate_transaction/system_transfer", |b| {
        b.iter_batched(
            || build_transfer_case(10_000),
            |(svm, tx)| {
                black_box(svm.simulate_transaction(tx).unwrap());
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("simulate_transaction/program_invocation", |b| {
        b.iter_batched(
            || build_counter_case(1),
            |(svm, tx)| {
                black_box(svm.simulate_transaction(tx).unwrap());
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("transact_commit/system_transfer", |b| {
        b.iter_batched(
            || build_transfer_case(10_000),
            |(mut svm, tx)| {
                let outcome = svm.transact(tx);
                black_box(svm.commit_transaction(outcome).unwrap());
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("plan_transaction_batch", |b| {
        b.iter_batched(
            build_batch_case,
            |(svm, txs)| {
                black_box(svm.plan_transaction_batch(txs).unwrap());
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("send_transaction_batch", |b| {
        b.iter_batched(
            build_batch_case,
            |(mut svm, txs)| {
                black_box(svm.send_transaction_batch(txs).unwrap());
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
