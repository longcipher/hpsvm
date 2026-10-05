#![no_main]

//! Differential fuzz target for the two fixture codecs.
//!
//! Both codecs must describe the *same* model: anything the binary codec can
//! decode, the JSON codec must decode to an equal fixture, and vice versa. A
//! disagreement means one codec silently drops or reinterprets a field, which
//! is exactly the class of bug that makes recorded fixtures untrustworthy.

use hpsvm_fixture::Fixture;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(from_binary) = wincode::deserialize::<Fixture>(data) else {
        return;
    };

    let json = serde_json::to_string(&from_binary).expect("a decoded fixture must encode to json");
    let Ok(from_json) = serde_json::from_str::<Fixture>(&json) else {
        panic!("binary-decoded fixture must be expressible in json");
    };

    assert_eq!(from_binary, from_json, "the two codecs must agree on the same model");

    // A JSON-decoded fixture must survive the binary codec as well.
    let binary = wincode::serialize(&from_json).expect("a json-decoded fixture must encode");
    assert_eq!(
        wincode::deserialize::<Fixture>(&binary).ok(),
        Some(from_json.clone()),
        "json -> binary -> json must be stable"
    );
});
