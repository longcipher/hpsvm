#[cfg(feature = "bin-codec")]
use solana_transaction::versioned::VersionedTransaction;

use crate::{
    AccountSnapshot, Compare, ExecutionSnapshot, FixtureHeader, FixtureKind, ProgramBinding,
    RuntimeFixtureConfig,
};
#[cfg(feature = "bin-codec")]
use crate::{Fixture, FixtureError, FixtureExpectations, FixtureInput, TransactionFixture};

#[derive(Debug, Default, Clone)]
#[must_use = "capture builders do nothing unless you finish them into a fixture"]
pub struct CaptureBuilder {
    header: Option<FixtureHeader>,
    runtime: Option<RuntimeFixtureConfig>,
    programs: Vec<ProgramBinding>,
    pre_accounts: Vec<AccountSnapshot>,
    baseline: Option<ExecutionSnapshot>,
    compares: Vec<Compare>,
}

impl CaptureBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            header: Some(FixtureHeader {
                schema_version: 1,
                name: name.into(),
                kind: FixtureKind::Transaction,
                source: None,
                tags: Vec::new(),
            }),
            ..Self::default()
        }
    }

    pub fn source(mut self, source: impl Into<String>) -> Self {
        if let Some(header) = self.header.as_mut() {
            header.source = Some(source.into());
        }
        self
    }

    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        if let Some(header) = self.header.as_mut() {
            header.tags.push(tag.into());
        }
        self
    }

    pub fn runtime(mut self, runtime: RuntimeFixtureConfig) -> Self {
        self.runtime = Some(runtime);
        self
    }

    pub fn programs(mut self, programs: Vec<ProgramBinding>) -> Self {
        self.programs = programs;
        self
    }

    pub fn pre_accounts(mut self, pre_accounts: Vec<AccountSnapshot>) -> Self {
        self.pre_accounts = pre_accounts;
        self
    }

    pub fn baseline(mut self, baseline: ExecutionSnapshot) -> Self {
        self.baseline = Some(baseline);
        self
    }

    pub fn compares(mut self, compares: Vec<Compare>) -> Self {
        self.compares = compares;
        self
    }

    #[cfg(feature = "bin-codec")]
    pub fn capture_transaction(self, tx: &VersionedTransaction) -> Result<Fixture, FixtureError> {
        let transaction_bytes = wincode::serialize(tx).map_err(FixtureError::EncodeTransaction)?;
        let header = self.header.ok_or(FixtureError::MissingField { field: "header" })?;
        let runtime = self.runtime.ok_or(FixtureError::MissingField { field: "runtime" })?;
        let baseline = self.baseline.ok_or(FixtureError::MissingField { field: "baseline" })?;

        Ok(Fixture {
            header,
            input: FixtureInput::Transaction(TransactionFixture {
                runtime,
                programs: self.programs,
                pre_accounts: self.pre_accounts,
                transaction_bytes,
            }),
            expectations: FixtureExpectations {
                baseline,
                compares: if self.compares.is_empty() {
                    Compare::everything()
                } else {
                    self.compares
                },
            },
        })
    }
}

#[cfg(all(test, feature = "bin-codec"))]
mod tests {
    use super::*;
    use crate::{ExecutionSnapshotFields, ExecutionStatus};

    fn transaction() -> VersionedTransaction {
        use solana_keypair::Keypair;
        use solana_message::Message;
        use solana_signer::Signer;

        let payer = Keypair::new();
        VersionedTransaction::try_new(
            solana_message::VersionedMessage::Legacy(Message::new(&[], Some(&payer.pubkey()))),
            &[&payer],
        )
        .expect("an empty-instruction transaction should build")
    }

    fn baseline() -> ExecutionSnapshot {
        ExecutionSnapshot::from_fields(ExecutionSnapshotFields {
            status: ExecutionStatus::Success,
            included: true,
            compute_units_consumed: 10,
            fee: 20,
            logs: Vec::new(),
            return_data: None,
            inner_instructions: Vec::new(),
            post_accounts: Vec::new(),
        })
    }

    fn complete_builder() -> CaptureBuilder {
        CaptureBuilder::new("case")
            .runtime(RuntimeFixtureConfig::new(0, None, true, false))
            .baseline(baseline())
    }

    #[test]
    fn new_prefills_a_transaction_header_and_empty_compares() {
        let builder = CaptureBuilder::new("my-case");
        assert!(builder.compares.is_empty());
        assert!(builder.pre_accounts.is_empty());
        assert!(builder.programs.is_empty());
    }

    #[test]
    fn new_defaults_the_kind_to_transaction() {
        let fixture = complete_builder().capture_transaction(&transaction()).unwrap();
        assert_eq!(fixture.header.kind, FixtureKind::Transaction);
        assert_eq!(fixture.header.schema_version, 1);
    }

    #[test]
    fn source_and_tag_are_recorded_in_the_header() {
        let fixture = complete_builder()
            .source("agave-test-validator")
            .tag("alpha")
            .tag("beta")
            .capture_transaction(&transaction())
            .unwrap();

        assert_eq!(fixture.header.source, Some(String::from("agave-test-validator")));
        assert_eq!(fixture.header.tags, vec![String::from("alpha"), String::from("beta")]);
    }

    /// An empty `compares` list falls back to comparing everything, so a
    /// captured fixture never silently skips validation.
    #[test]
    fn empty_compares_falls_back_to_compare_everything() {
        let fixture = complete_builder().capture_transaction(&transaction()).unwrap();
        assert_eq!(fixture.expectations.compares, Compare::everything());
    }

    #[test]
    fn explicit_compares_are_preserved_verbatim() {
        let fixture = complete_builder()
            .compares(vec![Compare::Status, Compare::Logs])
            .capture_transaction(&transaction())
            .unwrap();
        assert_eq!(fixture.expectations.compares, vec![Compare::Status, Compare::Logs]);
    }

    #[test]
    fn programs_and_pre_accounts_are_stored_in_order() {
        let program_id = solana_address::Address::new_unique();
        let pre = AccountSnapshot::new(
            solana_address::Address::new_unique(),
            5,
            solana_address::Address::new_unique(),
            false,
            0,
            vec![7],
        );

        let fixture = complete_builder()
            .programs(vec![ProgramBinding::new(
                program_id,
                solana_address::Address::new_unique(),
                None,
            )])
            .pre_accounts(vec![pre.clone()])
            .capture_transaction(&transaction())
            .unwrap();

        let FixtureInput::Transaction(transaction_fixture) = &fixture.input else {
            panic!("expected a transaction fixture");
        };
        assert_eq!(transaction_fixture.programs.len(), 1);
        assert_eq!(transaction_fixture.programs[0].program_id, program_id);
        assert_eq!(transaction_fixture.pre_accounts, vec![pre]);
    }

    #[test]
    fn programs_and_pre_accounts_overwrite_rather_than_append() {
        let first = ProgramBinding::new(
            solana_address::Address::new_unique(),
            solana_address::Address::new_unique(),
            None,
        );
        let second = ProgramBinding::new(
            solana_address::Address::new_unique(),
            solana_address::Address::new_unique(),
            None,
        );

        let fixture = complete_builder()
            .programs(vec![first.clone()])
            .programs(vec![second])
            .capture_transaction(&transaction())
            .unwrap();

        let FixtureInput::Transaction(transaction_fixture) = &fixture.input else {
            panic!("expected a transaction fixture");
        };
        // `programs` replaces the whole list rather than appending, so only the
        // second call's bindings survive.
        assert_eq!(transaction_fixture.programs.len(), 1);
        assert_ne!(transaction_fixture.programs[0].program_id, first.program_id);
    }

    /// `CaptureBuilder::default()` has no header, so capture must report the
    /// missing field rather than panicking.
    #[test]
    fn default_builder_reports_the_missing_header() {
        let error = CaptureBuilder::default()
            .runtime(RuntimeFixtureConfig::new(0, None, true, false))
            .baseline(baseline())
            .capture_transaction(&transaction())
            .expect_err("missing header should fail");

        assert!(
            matches!(error, FixtureError::MissingField { field } if field == "header"),
            "got {error:?}"
        );
    }

    #[test]
    fn a_builder_without_a_runtime_reports_the_missing_runtime() {
        let error = CaptureBuilder::new("case")
            .baseline(baseline())
            .capture_transaction(&transaction())
            .expect_err("missing runtime should fail");

        assert!(
            matches!(error, FixtureError::MissingField { field } if field == "runtime"),
            "got {error:?}"
        );
    }

    #[test]
    fn a_builder_without_a_baseline_reports_the_missing_baseline() {
        let error = CaptureBuilder::new("case")
            .runtime(RuntimeFixtureConfig::new(0, None, true, false))
            .capture_transaction(&transaction())
            .expect_err("missing baseline should fail");

        assert!(
            matches!(error, FixtureError::MissingField { field } if field == "baseline"),
            "got {error:?}"
        );
    }

    /// The missing-field error must name the field so callers can report it.
    #[test]
    fn the_missing_field_error_names_the_field() {
        let error = CaptureBuilder::default().capture_transaction(&transaction()).unwrap_err();
        match error {
            FixtureError::MissingField { field } => assert_eq!(field, "header"),
            other => panic!("expected FixtureError::MissingField, got {other:?}"),
        }
    }

    /// Setting `source`/`tag` on a header-less builder must not resurrect it:
    /// the builder only mutates an existing header.
    #[test]
    fn source_and_tag_are_ignored_without_a_header() {
        let error = CaptureBuilder::default()
            .source("ignored")
            .tag("ignored")
            .runtime(RuntimeFixtureConfig::new(0, None, true, false))
            .baseline(baseline())
            .capture_transaction(&transaction())
            .unwrap_err();

        assert!(
            matches!(error, FixtureError::MissingField { field } if field == "header"),
            "got {error:?}"
        );
    }

    /// The serialized transaction must be recoverable, which is what the runner
    /// depends on at replay time.
    #[test]
    fn the_transaction_bytes_round_trip() {
        let tx = transaction();
        let fixture = complete_builder().capture_transaction(&tx).unwrap();

        let FixtureInput::Transaction(transaction_fixture) = &fixture.input else {
            panic!("expected a transaction fixture");
        };
        let decoded: VersionedTransaction =
            wincode::deserialize(&transaction_fixture.transaction_bytes).unwrap();
        assert_eq!(decoded, tx);
    }

    /// The builder is `Clone` and cloning must not share state.
    #[test]
    fn cloning_a_builder_does_not_share_state() {
        let builder = complete_builder();
        // `tag` consumes and returns the builder, so the clone is what is
        // mutated; the original must be unaffected.
        let tagged = builder.clone().tag("only-on-clone");

        assert!(builder.compares.is_empty());
        assert!(builder.pre_accounts.is_empty());
        assert!(tagged.compares.is_empty());
        assert!(tagged.pre_accounts.is_empty());
        let tagged_tags = &tagged.header.as_ref().expect("header is set").tags;
        assert_eq!(tagged_tags, &[String::from("only-on-clone")]);
        let original_tags = &builder.header.as_ref().expect("header is set").tags;
        assert!(original_tags.is_empty(), "the original must keep its own tags");
    }
}
