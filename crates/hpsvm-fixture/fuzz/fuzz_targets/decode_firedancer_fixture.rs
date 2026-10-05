#![no_main]

//! Fuzzes the Firedancer protobuf adapter.
//!
//! `.fix` fixtures are produced externally, so both the protobuf decode *and*
//! the proto -> hpsvm model conversion must be total functions over the input
//! space: every rejection has to come back as a typed `AdapterError`, never a
//! panic. The conversion is where the interesting rejections live (bad address
//! lengths, out-of-range account indices, compute-unit underflow, inconsistent
//! status pairs).

use hpsvm_fixture::{AdapterError, FiredancerFixture, Fixture};
use libfuzzer_sys::fuzz_target;
use mollusk_svm_fuzz_fixture_firedancer as fd_codec;
use prost::Message;

fuzz_target!(|data: &[u8]| {
    let Ok(proto) = fd_codec::proto::InstrFixture::decode(data) else {
        return;
    };

    // The upstream `From<ProtoFixture> for Fixture` unwraps `input` and
    // `output`, so a proto missing either aborts inside the protobuf crate
    // before the adapter runs. Those inputs are out of scope here; everything
    // else is fuzzed.
    if proto.input.is_none() || proto.output.is_none() {
        return;
    }

    let fixture = FiredancerFixture::from_proto(proto.clone());

    // `to_model` must never panic on any decodable proto.
    let _ = fixture.to_model();
    assert_eq!(fixture.as_proto(), &proto);

    match Fixture::try_from(fixture) {
        Ok(model) => {
            let hpsvm_fixture::FixtureInput::Instruction(instruction) = &model.input else {
                panic!("firedancer import must produce an instruction fixture");
            };

            // Every instruction account must resolve to a supplied pre-account.
            for account in &instruction.accounts {
                assert!(
                    instruction.pre_accounts.iter().any(|pre| pre.address == account.pubkey),
                    "instruction account {} is not in pre_accounts",
                    account.pubkey
                );
            }

            // The consumed compute units must fit inside the recorded budget,
            // otherwise replaying the fixture could not reproduce the baseline.
            let budget = instruction
                .runtime
                .compute_unit_limit
                .expect("an imported fixture always records its compute unit budget");
            assert!(
                model.expectations.baseline.compute_units_consumed <= budget,
                "consumed {} exceeds budget {budget}",
                model.expectations.baseline.compute_units_consumed
            );

            // An imported fixture is always included and carries the firedancer
            // provenance markers.
            assert!(model.expectations.baseline.included);
            assert_eq!(model.header.source.as_deref(), Some("firedancer"));
            assert_eq!(model.header.tags, vec![String::from("external:firedancer")]);
        }
        Err(error) => {
            // Every rejection must be a typed adapter error with a message.
            assert!(!error.to_string().is_empty(), "adapter errors must render a message");

            assert!(
                matches!(
                    error,
                    AdapterError::MissingField { .. } |
                        AdapterError::InvalidAddressLength { .. } |
                        AdapterError::InvalidInstructionAccountIndex { .. } |
                        AdapterError::MissingInstructionAccount { .. } |
                        AdapterError::UnsupportedSeedAddress { .. } |
                        AdapterError::InconsistentComputeUnits { .. } |
                        AdapterError::InconsistentExecutionStatus { .. }
                ),
                "unexpected adapter error variant: {error:?}"
            );
        }
    }
});
