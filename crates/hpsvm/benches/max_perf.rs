use criterion::{Criterion, criterion_group, criterion_main};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_signer::Signer;

mod common;

use common::{
    HotpathGuard, TraceMetricsGuard, counter_account, counter_program_path, make_counter_tx,
    new_benchmark_vm,
};

const NUM_GREETINGS: u8 = 255;

fn criterion_benchmark(c: &mut Criterion) {
    let _hotpath = HotpathGuard::new("max_perf");
    let _trace_metrics = TraceMetricsGuard::new("max_perf");
    let mut svm = new_benchmark_vm();
    _trace_metrics.install(&mut svm);
    let payer_kp = Keypair::new();
    let payer_pk = payer_kp.pubkey();
    let program_id = Address::new_unique();
    let so_path = counter_program_path();
    svm.add_program_from_file(program_id, &so_path).unwrap();
    svm.airdrop(&payer_pk, 100_000_000_000).unwrap();
    let counter_address = Address::new_unique();
    let latest_blockhash = svm.latest_blockhash();
    let tx =
        make_counter_tx(program_id, counter_address, &payer_pk, latest_blockhash, &payer_kp, 0);
    let mut group = c.benchmark_group("max_perf_comparison");
    group.bench_function("max_perf_hpsvm", |b| {
        b.iter(|| {
            let _ = svm.set_account(counter_address, counter_acc(program_id));
            for _ in 0..NUM_GREETINGS {
                svm.send_transaction(tx.clone()).unwrap();
            }
            assert_eq!(svm.get_account(&counter_address).unwrap().data[0], NUM_GREETINGS);
        })
    });
}

fn counter_acc(program_id: Address) -> solana_account::Account {
    counter_account(program_id)
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
