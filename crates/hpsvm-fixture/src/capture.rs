use solana_transaction::versioned::VersionedTransaction;

use crate::{
    AccountSnapshot, Compare, ExecutionSnapshot, Fixture, FixtureError, FixtureExpectations,
    FixtureHeader, FixtureInput, FixtureKind, ProgramBinding, RuntimeFixtureConfig,
    TransactionFixture,
};

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

    pub fn capture_transaction(self, tx: &VersionedTransaction) -> Result<Fixture, FixtureError> {
        let transaction_bytes = bincode::serialize(tx).map_err(FixtureError::EncodeTransaction)?;
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
