use criterion::{Criterion, criterion_group, criterion_main};
use solana_address::Address;
use solana_keypair::Keypair;
use solana_signer::Signer;

mod common;

use common::{
    HotpathGuard, TraceMetricsGuard, counter_account, make_counter_tx, new_benchmark_vm,
    read_counter_program,
};

const NUM_GREETINGS: u8 = 255;

fn criterion_benchmark(c: &mut Criterion) {
    let _hotpath = HotpathGuard::new("simple_bench");
    let _trace_metrics = TraceMetricsGuard::new("simple_bench");
    let mut svm = new_benchmark_vm();
    _trace_metrics.install(&mut svm);
    let payer_kp = Keypair::new();
    let payer_pk = payer_kp.pubkey();
    let program_id = Address::new_unique();

    svm.add_program(program_id, &read_counter_program()).unwrap();
    svm.airdrop(&payer_pk, 1000000000).unwrap();
    let counter_address = Address::new_unique();
    c.bench_function("simple_bench", |b| {
        b.iter(|| {
            let _ = svm.set_account(counter_address, counter_acc(program_id));
            svm.expire_blockhash();
            let latest_blockhash = svm.latest_blockhash();
            for deduper in 0..NUM_GREETINGS {
                let tx = make_counter_tx(
                    program_id,
                    counter_address,
                    &payer_pk,
                    latest_blockhash,
                    &payer_kp,
                    deduper,
                );
                svm.send_transaction(tx).unwrap();
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
