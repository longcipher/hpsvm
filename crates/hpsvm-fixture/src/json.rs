use crate::{Fixture, FixtureError};

pub(crate) fn load(path: &std::path::Path) -> Result<Fixture, FixtureError> {
    if path.extension().and_then(|value| value.to_str()) != Some("json") {
        return Err(FixtureError::UnsupportedFormat { path: path.display().to_string() });
    }

    let file = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&file)?)
}

pub(crate) fn save(fixture: &Fixture, path: &std::path::Path) -> Result<(), FixtureError> {
    let json = serde_json::to_string_pretty(fixture)?;
    std::fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use solana_address::Address;

    use super::*;
    use crate::{
        AccountSnapshot, Compare, ExecutionSnapshot, ExecutionSnapshotFields, ExecutionStatus,
        FixtureExpectations, FixtureHeader, FixtureInput, FixtureKind, ProgramBinding,
        RuntimeFixtureConfig, TransactionFixture,
    };

    fn address(seed: u8) -> Address {
        Address::new_from_array([seed; 32])
    }

    fn generated_fixture(name: String, compute_units: u64, data: Vec<u8>) -> Fixture {
        let account = AccountSnapshot::new(address(1), 12, address(2), false, 0, data.clone());
        Fixture::new(
            FixtureHeader::new(name, FixtureKind::Transaction).source("json").tag("t"),
            FixtureInput::Transaction(TransactionFixture::new(
                RuntimeFixtureConfig::new(9, Some(2048), true, false)
                    .with_compute_unit_limit(1_000_000),
                vec![ProgramBinding::new(address(1), address(2), Some(String::from("role")))],
                vec![account.clone()],
                data,
            )),
            FixtureExpectations::new(
                ExecutionSnapshot::from_fields(ExecutionSnapshotFields {
                    status: ExecutionStatus::Success,
                    included: true,
                    compute_units_consumed: compute_units,
                    fee: compute_units,
                    logs: vec![String::from("a"), String::from("b")],
                    return_data: None,
                    inner_instructions: Vec::new(),
                    post_accounts: vec![account],
                }),
                Compare::everything(),
            ),
        )
    }

    fn roundtrip(fixture: &Fixture) -> Fixture {
        let path = std::env::temp_dir().join(format!("hpsvm-json-{}.json", Address::new_unique()));
        save(fixture, &path).expect("json save must succeed");
        let loaded = load(&path).expect("json load must succeed");
        std::fs::remove_file(&path).ok();
        loaded
    }

    // The JSON codec is the human-editable fixture format, so it must be a
    // lossless round trip for every field.
    proptest! {
        #[test]
        fn json_codec_round_trips_any_generated_fixture(
            name in "[a-zA-Z0-9_-]{0,24}",
            compute_units in any::<u64>(),
            data in prop::collection::vec(any::<u8>(), 0..32),
        ) {
            let fixture = generated_fixture(name, compute_units, data);
            prop_assert_eq!(roundtrip(&fixture), fixture);
        }
    }

    /// `compute_unit_limit` is `#[serde(default)]`, so a JSON document that
    /// predates the field must still load.
    #[test]
    fn a_baseline_without_a_compute_unit_limit_still_loads() {
        let mut fixture = generated_fixture(String::from("legacy"), 1, Vec::new());
        let FixtureInput::Transaction(transaction) = &mut fixture.input else {
            unreachable!("transaction fixture");
        };
        transaction.runtime.compute_unit_limit = None;

        let json = serde_json::to_string(&fixture).unwrap();
        assert!(
            json.contains(r#""compute_unit_limit":null"#),
            "an unset limit is written as null: {json}"
        );

        let decoded: Fixture = serde_json::from_str(&json).unwrap();
        let FixtureInput::Transaction(transaction) = &decoded.input else {
            unreachable!("transaction fixture");
        };
        assert_eq!(transaction.runtime.compute_unit_limit, None);

        // The field is `#[serde(default)]`, so a document that predates it
        // still loads once the key is removed entirely.
        let mut document: serde_json::Value = serde_json::from_str(&json).unwrap();
        let runtime = document["input"]["Transaction"]["runtime"]
            .as_object_mut()
            .expect("runtime is an object");
        assert_eq!(runtime.remove("compute_unit_limit"), Some(serde_json::Value::Null));
        let legacy = document.to_string();
        assert!(!legacy.contains("compute_unit_limit"), "the key must be gone: {legacy}");

        let decoded: Fixture = serde_json::from_str(&legacy).unwrap();
        let FixtureInput::Transaction(transaction) = &decoded.input else {
            unreachable!("transaction fixture");
        };
        assert_eq!(transaction.runtime.compute_unit_limit, None);
    }

    /// Pretty-printed output must be multi-line so diffs stay readable.
    #[test]
    fn json_save_is_pretty_printed() {
        let path =
            std::env::temp_dir().join(format!("hpsvm-pretty-{}.json", Address::new_unique()));
        save(&generated_fixture(String::from("pretty"), 1, vec![1]), &path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains('\n'));
        assert!(text.starts_with("{\n"));

        std::fs::remove_file(path).ok();
    }

    // Arbitrary text must never panic the JSON decoder.
    proptest! {
        #[test]
        fn json_decode_never_panics_on_arbitrary_text(
            text in prop::collection::vec(any::<char>(), 0..128)
                .prop_map(|chars| chars.into_iter().collect::<String>()),
        ) {
            let _ = serde_json::from_str::<Fixture>(&text);
        }
    }

    #[test]
    fn json_load_rejects_a_non_json_extension() {
        let path =
            std::env::temp_dir().join(format!("hpsvm-wrong-ext-{}.bin", Address::new_unique()));
        std::fs::write(&path, b"{}").unwrap();

        match load(&path).expect_err("a .bin path should be refused") {
            FixtureError::UnsupportedFormat { path: reported } => {
                assert_eq!(reported, path.display().to_string());
            }
            other => panic!("expected FixtureError::UnsupportedFormat, got {other:?}"),
        }

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn json_load_reports_malformed_json() {
        let path = std::env::temp_dir().join(format!("hpsvm-bad-{}.json", Address::new_unique()));
        std::fs::write(&path, b"{ nope").unwrap();

        assert!(matches!(load(&path).unwrap_err(), FixtureError::Json(_)));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn json_load_reports_a_missing_file() {
        let path =
            std::env::temp_dir().join(format!("hpsvm-absent-{}.json", Address::new_unique()));
        assert!(matches!(load(&path).unwrap_err(), FixtureError::Io(_)));
    }

    #[test]
    fn json_save_reports_an_unwritable_path() {
        let path = std::env::temp_dir()
            .join(format!("hpsvm-absent-dir-{}/out.json", Address::new_unique()));
        let fixture = generated_fixture(String::from("io"), 1, Vec::new());
        assert!(matches!(save(&fixture, &path).unwrap_err(), FixtureError::Io(_)));
    }
}
