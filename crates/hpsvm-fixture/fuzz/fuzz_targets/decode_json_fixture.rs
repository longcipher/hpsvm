#![no_main]

//! Fuzzes the JSON fixture decoder.
//!
//! Fixtures are hand-edited JSON, so the decoder must survive arbitrary text
//! without panicking, and must never accept a document it cannot re-emit.

use hpsvm_fixture::Fixture;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // `from_slice` avoids failing on non-UTF-8 before the parser is reached,
    // which keeps the fuzzer focused on the JSON grammar itself.
    if let Ok(fixture) = serde_json::from_slice::<Fixture>(data) {
        let re_encoded = serde_json::to_string(&fixture).expect("a decoded fixture must re-encode");
        assert_eq!(
            serde_json::from_str::<Fixture>(&re_encoded).ok(),
            Some(fixture),
            "decode -> encode -> decode must be stable"
        );
    }
});
