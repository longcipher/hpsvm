use std::{collections::HashMap, fs};

use hpsvm_fixture::{Fixture, FixtureInput, FixtureRunner};
use solana_address::Address;

use crate::error::CliError;

pub(crate) fn parse_program_map(values: &[String]) -> Result<HashMap<Address, Vec<u8>>, CliError> {
    let mut parsed = HashMap::new();

    for value in values {
        let Some((program_id, path)) = value.split_once('=') else {
            return Err(CliError::InvalidProgramMapping { value: value.clone() });
        };

        let program_id = program_id.parse::<Address>().map_err(|error| {
            CliError::InvalidProgramId { value: program_id.to_string(), reason: error.to_string() }
        })?;
        parsed.insert(program_id, fs::read(path)?);
    }

    Ok(parsed)
}

pub(crate) fn preload_runner(
    mut runner: FixtureRunner,
    fixture: &Fixture,
    programs: &HashMap<Address, Vec<u8>>,
) -> FixtureRunner {
    for binding in fixture_programs(fixture) {
        if let Some(bytes) = programs.get(&binding.program_id) {
            runner = runner.with_program_elf(binding.program_id, bytes.clone());
        }
    }

    runner
}

pub(crate) fn fixture_programs(fixture: &Fixture) -> &[hpsvm_fixture::ProgramBinding] {
    match &fixture.input {
        FixtureInput::Transaction(transaction) => &transaction.programs,
        FixtureInput::Instruction(instruction) => &instruction.programs,
        _ => &[],
    }
}
