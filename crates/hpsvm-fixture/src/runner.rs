use std::collections::HashMap;

use hpsvm::HPSVM;
#[cfg(feature = "bin-codec")]
use solana_address::Address;
#[cfg(feature = "bin-codec")]
use solana_transaction::versioned::VersionedTransaction;

#[cfg(feature = "instruction-fixture")]
use crate::InstructionFixture;
use crate::{
    ExecutionSnapshot, Fixture, FixtureError, FixtureInput, ProgramBinding, ResultConfig,
    RuntimeFixtureConfig, TransactionFixture,
};

/// Applies a fixture's runtime configuration to a VM.
///
/// The single place the fixture runtime configuration is applied. A
/// recording and a replay both go through it, so a recorded baseline can
/// always be reproduced by a later replay.
pub fn configure_vm(vm: &mut HPSVM, runtime: &RuntimeFixtureConfig) {
    vm.set_sigverify(runtime.sigverify);
    vm.set_blockhash_check(runtime.blockhash_check);
    vm.set_log_bytes_limit(runtime.log_bytes_limit);
    if let Some(compute_unit_limit) = runtime.compute_unit_limit {
        vm.set_compute_unit_limit(compute_unit_limit);
    }
    vm.warp_to_slot(runtime.slot);
}

/// Binds a fixture's programs to their ELF images on `vm`.
///
/// A binding without a matching ELF is an error rather than a skip: silently
/// skipping it would run (or record) a baseline against a different program
/// than the one the fixture names.
pub fn bind_programs<S: std::hash::BuildHasher>(
    vm: &mut HPSVM,
    bindings: &[ProgramBinding],
    elfs: &HashMap<Address, Vec<u8>, S>,
) -> Result<(), FixtureError> {
    for binding in bindings {
        let Some(elf) = elfs.get(&binding.program_id) else {
            return Err(FixtureError::MissingProgramElf { program_id: binding.program_id });
        };
        vm.add_program_with_loader(binding.program_id, elf, binding.loader_id)?;
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureExecution {
    pub snapshot: ExecutionSnapshot,
    pub pass: Option<bool>,
}

#[must_use = "fixture runners must be used to execute fixtures"]
pub struct FixtureRunner {
    base_vm: HPSVM,
    program_elfs: HashMap<Address, Vec<u8>>,
    jit_policy: Option<hpsvm::JitPolicy>,
}

impl FixtureRunner {
    pub fn new(vm: HPSVM) -> Self {
        Self { base_vm: vm, program_elfs: HashMap::new(), jit_policy: None }
    }

    pub fn with_program_elf(mut self, program_id: Address, elf: Vec<u8>) -> Self {
        self.program_elfs.insert(program_id, elf);
        self
    }

    /// Overrides the execution policy for every fixture run.
    ///
    /// `None` (default) keeps the base VM's policy. `Some(Off)` forces the
    /// interpreter even on x86_64 — the equivalence-gate arm. `Some(Eager)`
    /// is the fidelity reference.
    pub fn with_jit_policy(mut self, policy: hpsvm::JitPolicy) -> Self {
        self.jit_policy = Some(policy);
        self
    }

    /// Sets the execution policy override (builder-style `with_jit_policy`
    /// counterpart for `&mut` use).
    pub fn set_jit_policy(&mut self, policy: hpsvm::JitPolicy) {
        self.jit_policy = Some(policy);
    }

    /// Binds every program the fixture declares to the ELF in `elfs`.
    ///
    /// A binding without a matching ELF is an error: the runner must not
    /// execute a fixture against a different program than the one it names.
    pub fn with_fixture_programs<S: std::hash::BuildHasher>(
        mut self,
        fixture: &Fixture,
        elfs: &HashMap<Address, Vec<u8>, S>,
    ) -> Result<Self, FixtureError> {
        for binding in fixture.programs() {
            let elf = elfs
                .get(&binding.program_id)
                .ok_or(FixtureError::MissingProgramElf { program_id: binding.program_id })?;
            self.program_elfs.insert(binding.program_id, elf.clone());
        }
        Ok(self)
    }

    pub fn run(&mut self, fixture: &Fixture) -> Result<FixtureExecution, FixtureError> {
        let snapshot = match &fixture.input {
            FixtureInput::Transaction(transaction) => self.run_transaction_fixture(transaction)?,
            #[cfg(feature = "instruction-fixture")]
            FixtureInput::Instruction(instruction) => self.run_instruction_fixture(instruction)?,
        };

        Ok(FixtureExecution { snapshot, pass: None })
    }

    pub fn run_and_validate(
        &mut self,
        fixture: &Fixture,
        config: &ResultConfig,
    ) -> Result<bool, FixtureError> {
        let snapshot = self.run(fixture)?.snapshot;
        Ok(snapshot.compare_with(
            &fixture.expectations.baseline,
            &fixture.expectations.compares,
            config,
        ))
    }

    fn run_transaction_fixture(
        &self,
        fixture: &TransactionFixture,
    ) -> Result<ExecutionSnapshot, FixtureError> {
        #[cfg(not(feature = "bin-codec"))]
        {
            let _ = (self, fixture);
            return Err(FixtureError::CodecDisabled { feature: "bin-codec" });
        }

        #[cfg(feature = "bin-codec")]
        {
            if fixture.runtime.blockhash_check {
                return Err(FixtureError::UnsupportedRuntimeConfig { field: "blockhash_check" });
            }

            let mut vm = self.base_vm.clone();
            if let Some(policy) = self.jit_policy {
                vm.set_jit_policy(policy);
            }
            configure_vm(&mut vm, &fixture.runtime);
            self.load_programs(&mut vm, &fixture.programs)?;

            for account in &fixture.pre_accounts {
                // The snapshot and the account share their field layout, so
                // the conversion is the only place that has to know it.
                vm.set_account(account.address, account.clone().into())?;
            }

            let tx: VersionedTransaction = wincode::deserialize(&fixture.transaction_bytes)
                .map_err(FixtureError::DecodeTransaction)?;
            Ok(ExecutionSnapshot::from_outcome(&vm.transact_detached(tx)))
        }
    }

    #[cfg(feature = "instruction-fixture")]
    fn run_instruction_fixture(
        &self,
        fixture: &InstructionFixture,
    ) -> Result<ExecutionSnapshot, FixtureError> {
        let mut vm = self.base_vm.clone();
        if let Some(policy) = self.jit_policy {
            vm.set_jit_policy(policy);
        }
        configure_vm(&mut vm, &fixture.runtime);
        self.load_programs(&mut vm, &fixture.programs)?;

        let outcome = vm.process_instruction_case(&fixture.instruction_case())?;
        Ok(ExecutionSnapshot::from_outcome(&outcome))
    }

    fn load_programs(
        &self,
        vm: &mut HPSVM,
        programs: &[ProgramBinding],
    ) -> Result<(), FixtureError> {
        bind_programs(vm, programs, &self.program_elfs)
    }
}
