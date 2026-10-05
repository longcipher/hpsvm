use crate::{Fixture, FixtureError};

pub(crate) fn load(path: &std::path::Path) -> Result<Fixture, FixtureError> {
    let bytes = std::fs::read(path)?;
    wincode::deserialize(&bytes).map_err(FixtureError::DecodeFixture)
}

pub(crate) fn save(fixture: &Fixture, path: &std::path::Path) -> Result<(), FixtureError> {
    let bytes = wincode::serialize(fixture).map_err(FixtureError::EncodeFixture)?;
    std::fs::write(path, bytes)?;
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

    fn snapshot(
        status: ExecutionStatus,
        compute_units: u64,
        accounts: Vec<AccountSnapshot>,
    ) -> ExecutionSnapshot {
        ExecutionSnapshot::from_fields(ExecutionSnapshotFields {
            status,
            included: compute_units % 2 == 0,
            compute_units_consumed: compute_units,
            // `overflow-checks` is on, so the fee must be derived with a
            // wrapping multiply rather than `compute_units * 3`.
            fee: compute_units.wrapping_mul(3),
            logs: (0..(compute_units % 4)).map(|index| format!("line {index}")).collect(),
            return_data: None,
            inner_instructions: Vec::new(),
            post_accounts: accounts,
        })
    }

    /// Builds a fixture with fully generated contents so the round-trip property
    /// exercises every encoded field.
    fn generated_fixture(
        name: String,
        slot: u64,
        compute_units: u64,
        accounts: Vec<(u8, u64, Vec<u8>)>,
        compares: Vec<Compare>,
    ) -> Fixture {
        let post_accounts: Vec<AccountSnapshot> = accounts
            .into_iter()
            .map(|(seed, lamports, data)| {
                AccountSnapshot::new(
                    address(seed),
                    lamports,
                    address(seed.wrapping_add(1)),
                    false,
                    0,
                    data,
                )
            })
            .collect();

        Fixture::new(
            FixtureHeader::new(name, FixtureKind::Transaction)
                .source("proptest")
                .tag("alpha")
                .tag("beta"),
            FixtureInput::Transaction(TransactionFixture::new(
                RuntimeFixtureConfig::new(slot, Some(1024), true, false)
                    .with_compute_unit_limit(1_000_000),
                vec![ProgramBinding::new(address(1), address(2), Some(String::from("app")))],
                post_accounts.clone(),
                vec![0xde, 0xad, 0xbe, 0xef],
            )),
            FixtureExpectations::new(
                snapshot(
                    if compute_units % 3 == 0 {
                        ExecutionStatus::Success
                    } else {
                        ExecutionStatus::Failure {
                            kind: String::from("InstructionError"),
                            message: String::from("boom"),
                        }
                    },
                    compute_units,
                    post_accounts,
                ),
                compares,
            ),
        )
    }

    fn roundtrip(fixture: &Fixture) -> Fixture {
        let path = std::env::temp_dir().join(format!("hpsvm-bin-{}.bin", Address::new_unique()));
        save(fixture, &path).expect("binary save must succeed");
        let loaded = load(&path).expect("binary load must succeed");
        std::fs::remove_file(&path).ok();
        loaded
    }

    // The binary codec is the on-disk format for fixtures, so it must be a
    // lossless round trip for every field, for any generated fixture.
    proptest! {
        #[test]
        fn binary_codec_round_trips_any_generated_fixture(
            name in "[a-zA-Z0-9_-]{0,24}",
            slot in any::<u64>(),
            compute_units in any::<u64>(),
            accounts in prop::collection::vec(
                (any::<u8>(), any::<u64>(), prop::collection::vec(any::<u8>(), 0..24)),
                0..6,
            ),
            compares in prop::collection::vec(prop_oneof![
                Just(Compare::Status),
                Just(Compare::Included),
                Just(Compare::ComputeUnits),
                Just(Compare::Fee),
                Just(Compare::ReturnData),
                Just(Compare::Logs),
                Just(Compare::InnerInstructionCount),
                Just(Compare::Accounts(crate::AccountCompareScope::All)),
                Just(Compare::Accounts(crate::AccountCompareScope::Only(vec![address(1)]))),
                Just(Compare::Accounts(crate::AccountCompareScope::AllExcept(vec![address(2)]))),
            ], 0..6),
        ) {
            let fixture = generated_fixture(name, slot, compute_units, accounts, compares);
            prop_assert_eq!(roundtrip(&fixture), fixture);
        }
    }

    // Encoding must be deterministic: the same fixture always produces the same
    // bytes, otherwise baseline diffs would be noise.
    proptest! {
        #[test]
        fn binary_encoding_is_deterministic(
            compute_units in any::<u64>(),
            accounts in prop::collection::vec(any::<u8>(), 0..4),
        ) {
            let accounts: Vec<(u8, u64, Vec<u8>)> =
                accounts.into_iter().map(|seed| (seed, 1, vec![seed])).collect();
            let fixture =
                generated_fixture(String::from("det"), 1, compute_units, accounts, Vec::new());

            let first = wincode::serialize(&fixture).expect("serialize should succeed");
            let second = wincode::serialize(&fixture).expect("serialize should succeed");
            prop_assert_eq!(first, second);
            prop_assert_eq!(roundtrip(&fixture), fixture);
        }
    }

    // Arbitrary byte slices must never panic in the decoder.
    proptest! {
        #[test]
        fn binary_decode_never_panics_on_arbitrary_bytes(
            bytes in prop::collection::vec(any::<u8>(), 0..256),
        ) {
            let _ = wincode::deserialize::<Fixture>(&bytes);
        }
    }

    // Truncating a valid encoding must fail cleanly rather than panic, and a
    // slice that was not truncated must still decode.
    proptest! {
        #[test]
        fn truncated_encodings_never_panic(keep in 0usize..64) {
            let fixture =
                generated_fixture(String::from("trunc"), 1, 7, vec![(1, 2, vec![3])], Vec::new());
            let encoded = wincode::serialize(&fixture).expect("serialize should succeed");

            if keep < encoded.len() {
                let _ = wincode::deserialize::<Fixture>(&encoded[..keep]);
            } else {
                prop_assert!(wincode::deserialize::<Fixture>(&encoded).is_ok());
            }
        }
    }

    #[test]
    fn save_reports_an_unwritable_path() {
        let fixture = generated_fixture(String::from("io"), 1, 1, Vec::new(), Vec::new());
        let path = std::env::temp_dir()
            .join(format!("hpsvm-missing-dir-{}/out.bin", Address::new_unique()));
        assert!(matches!(save(&fixture, &path).unwrap_err(), FixtureError::Io(_)));
    }

    #[test]
    fn load_reports_a_missing_file() {
        let path = std::env::temp_dir().join(format!("hpsvm-absent-{}.bin", Address::new_unique()));
        assert!(matches!(load(&path).unwrap_err(), FixtureError::Io(_)));
    }
}
