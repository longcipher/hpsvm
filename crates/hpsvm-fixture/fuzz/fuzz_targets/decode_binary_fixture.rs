#![no_main]

//! Fuzzes the `wincode` binary fixture decoder.
//!
//! `hpsvm-fixture` reads `.bin` fixtures straight off disk, so a truncated,
//! corrupted, or hostile file must produce a clean `DecodeFixture` error rather
//! than a panic, an abort, or an unbounded allocation.

use hpsvm_fixture::Fixture;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Anything that decodes must be re-encodable: this catches decoders that
    // accept a value the encoder could not have produced (a lost invariant).
    if let Ok(fixture) = wincode::deserialize::<Fixture>(data) {
        let re_encoded = wincode::serialize(&fixture).expect("a decoded fixture must re-encode");
        assert_eq!(
            wincode::deserialize::<Fixture>(&re_encoded).ok(),
            Some(fixture),
            "decode -> encode -> decode must be stable"
        );
    }
});
