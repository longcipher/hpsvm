use std::collections::HashMap;

use hpsvm::HPSVM;
use solana_account::Account;
use solana_address::Address;
use solana_transaction::versioned::VersionedTransaction;

use crate::{
    ExecutionSnapshot, Fixture, FixtureError, FixtureInput, InstructionFixture, ResultConfig,
    TransactionFixture,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureExecution {
    pub snapshot: ExecutionSnapshot,
    pub pass: Option<bool>,
}

#[must_use = "fixture runners must be used to execute fixtures"]
pub struct FixtureRunner {
    base_vm: HPSVM,
    program_elfs: HashMap<Address, Vec<u8>>,
}

impl FixtureRunner {
    pub fn new(vm: HPSVM) -> Self {
        Self { base_vm: vm, program_elfs: HashMap::new() }
    }

    pub fn with_program_elf(mut self, program_id: Address, elf: Vec<u8>) -> Self {
        self.program_elfs.insert(program_id, elf);
        self
    }

    pub fn run(&mut self, fixture: &Fixture) -> Result<FixtureExecution, FixtureError> {
        let snapshot = match &fixture.input {
            FixtureInput::Transaction(transaction) => self.run_transaction_fixture(transaction)?,
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
        if fixture.runtime.blockhash_check {
            return Err(FixtureError::UnsupportedRuntimeConfig { field: "blockhash_check" });
        }

        let mut vm = self.base_vm.clone();
        Self::configure_vm(&mut vm, fixture.runtime);
        self.load_programs(&mut vm, &fixture.programs)?;

        for account in &fixture.pre_accounts {
            vm.set_account(
                account.address,
                Account {
                    lamports: account.lamports,
                    data: account.data.clone(),
                    owner: account.owner,
                    executable: account.executable,
                    rent_epoch: account.rent_epoch,
                },
            )?;
        }

        let tx: VersionedTransaction = bincode::deserialize(&fixture.transaction_bytes)
            .map_err(FixtureError::DecodeTransaction)?;
        Ok(ExecutionSnapshot::from_outcome(&vm.transact(tx)))
    }

    fn run_instruction_fixture(
        &self,
        fixture: &InstructionFixture,
    ) -> Result<ExecutionSnapshot, FixtureError> {
        let mut vm = self.base_vm.clone();
        Self::configure_vm(&mut vm, fixture.runtime);
        self.load_programs(&mut vm, &fixture.programs)?;

        let outcome = vm.process_instruction_case(&fixture.instruction_case())?;
        Ok(ExecutionSnapshot::from_outcome(&outcome))
    }

    fn configure_vm(vm: &mut HPSVM, runtime: crate::RuntimeFixtureConfig) {
        vm.set_sigverify(runtime.sigverify);
        vm.set_blockhash_check(runtime.blockhash_check);
        vm.set_log_bytes_limit(runtime.log_bytes_limit);
        vm.warp_to_slot(runtime.slot);
    }

    fn load_programs(
        &self,
        vm: &mut HPSVM,
        programs: &[crate::ProgramBinding],
    ) -> Result<(), FixtureError> {
        for program in programs {
            let elf = self
                .program_elfs
                .get(&program.program_id)
                .ok_or(FixtureError::MissingProgramElf { program_id: program.program_id })?;
            vm.add_program_with_loader(program.program_id, elf.as_slice(), program.loader_id)?;
        }

        Ok(())
    }
}
