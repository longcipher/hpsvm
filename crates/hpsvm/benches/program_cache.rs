//! Program-cache bench: cold vs warm `add_program`, rebuild after mutation,
//! and `prewarm_programs` amortization.
//!
//! Uses the vendored memo ELFs (always available, no SBF toolchain needed)
//! so the bench runs on every host. Cold loads miss the resident store and
//! pay ELF parse + verify + JIT; warm loads re-link the resident `Arc`.

use criterion::{Criterion, criterion_group, criterion_main};
use solana_address::Address;

mod common;

use common::new_benchmark_vm;

fn memo_v3() -> &'static [u8] {
    include_bytes!("../elf/spl_memo-3.0.0.so")
}

fn memo_v1() -> &'static [u8] {
    include_bytes!("../elf/spl_memo-1.0.0.so")
}

fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("program_cache");

    // Cold: fresh minimal VM (fresh environment → store miss) + add_program.
    // The minimal VM skips default programs so VM construction stays cheap
    // and the measured cost is the ELF pipeline.
    group.bench_function("cold_add_program", |b| {
        b.iter(|| {
            let mut svm =
                hpsvm::Hpsvm::builder().with_sysvars().build().expect("minimal VM should build");
            let id = Address::new_unique();
            svm.add_program_with_loader(id, memo_v3(), solana_sdk_ids::bpf_loader::id())
                .expect("cold add should succeed");
            std::hint::black_box(id);
        });
    });

    // Warm: same VM, same bytes, new address → resident hit (no parse/verify/JIT).
    group.bench_function("warm_add_program", |b| {
        let mut svm = new_benchmark_vm();
        let loader = solana_sdk_ids::bpf_loader::id();
        svm.add_program_with_loader(Address::new_unique(), memo_v3(), loader)
            .expect("setup add should succeed");
        b.iter(|| {
            let id = Address::new_unique();
            svm.add_program_with_loader(id, memo_v3(), loader).expect("warm add should succeed");
            std::hint::black_box(id);
        });
    });

    // Single-entry reload after mutation via the public `set_account` path:
    // only the changed program pays ELF parse + verify + JIT again.
    // (The full `rebuild_program_cache` sweep is `pub(crate)`, so it cannot
    // be driven from an external bench; its re-link behavior is pinned by
    // the `rebuild_relinks_unchanged_programs` unit test instead.)
    group.bench_function("rebuild_after_mutation", |b| {
        let mut svm = new_benchmark_vm();
        let loader = solana_sdk_ids::bpf_loader::id();
        let ids: Vec<Address> = (0..8).map(|_| Address::new_unique()).collect();
        for id in &ids {
            svm.add_program_with_loader(*id, memo_v3(), loader).expect("setup");
        }
        let mut toggle = false;
        b.iter(|| {
            toggle = !toggle;
            let elf = if toggle { memo_v3() } else { memo_v1() };
            // `set_account` with executable bytes triggers the load path;
            // alternating ELFs forces exactly one reload per iteration.
            let lamports = svm.minimum_balance_for_rent_exemption(elf.len());
            let account = solana_account::Account {
                lamports,
                data: elf.to_vec(),
                owner: loader,
                executable: true,
                rent_epoch: 0,
            };
            svm.set_account(ids[0], account).expect("upgrade should succeed");
        });
    });

    // Prewarm amortization: resolve the stage's program set before the first
    // transaction.
    group.bench_function("prewarm_programs", |b| {
        let mut svm = new_benchmark_vm();
        let loader = solana_sdk_ids::bpf_loader::id();
        let ids: Vec<Address> = (0..8).map(|_| Address::new_unique()).collect();
        for id in &ids {
            svm.add_program_with_loader(*id, memo_v3(), loader).expect("setup");
        }
        b.iter(|| {
            svm.prewarm_programs(&ids).expect("prewarm should succeed");
        });
    });

    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
