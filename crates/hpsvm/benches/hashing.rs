use std::{
    hash::{BuildHasher, BuildHasherDefault, DefaultHasher},
    hint::black_box,
};

use criterion::{Criterion, criterion_group, criterion_main};
use solana_address::Address;

mod common;

use common::HotpathGuard;

#[cfg_attr(feature = "hotpath", hotpath::measure)]
#[inline(never)]
fn std_default(address: &Address, hash_builder: &BuildHasherDefault<DefaultHasher>) -> u64 {
    hash_builder.hash_one(address)
}

#[cfg(feature = "hashbrown")]
#[cfg_attr(feature = "hotpath", hotpath::measure)]
#[inline(never)]
fn hashbrown(address: &Address, hash_builder: &hashbrown::DefaultHashBuilder) -> u64 {
    hash_builder.hash_one(address)
}

fn criterion_benchmark(c: &mut Criterion) {
    let _hotpath = HotpathGuard::new("hashing");
    let address = Address::new_unique();

    let mut group = c.benchmark_group("hashers");

    group.bench_function("default", |b| {
        let hash_builder = BuildHasherDefault::<DefaultHasher>::default();

        b.iter(|| {
            black_box(std_default(&address, &hash_builder));
        })
    });

    #[cfg(feature = "hashbrown")]
    group.bench_function("foldhash", |b| {
        let hash_builder = hashbrown::DefaultHashBuilder::default();

        b.iter(|| {
            black_box(hashbrown(&address, &hash_builder));
        })
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
