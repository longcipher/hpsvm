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

#[cfg(test)]
mod tests {
    use hpsvm::HPSVM;
    use hpsvm_fixture::{
        AccountSnapshot, CaptureBuilder, Compare, ExecutionSnapshot, FixtureError, ProgramBinding,
        RuntimeFixtureConfig,
    };
    use solana_address::Address;
    use solana_keypair::Keypair;
    use solana_message::Message;
    use solana_signer::Signer;
    use solana_system_interface::instruction::transfer;
    use solana_transaction::versioned::VersionedTransaction;

    use super::*;
    use crate::error::CliError;

    fn temp_elf_path(contents: &[u8]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("hpsvm-cli-prog-{}.so", Address::new_unique()));
        std::fs::write(&path, contents).expect("elf fixture should write");
        path
    }

    /// Captures a fixture that binds `program_id` to a (deliberately invalid)
    /// ELF, so the runner can only proceed when an ELF is injected.
    fn fixture_binding(program_id: Address) -> Fixture {
        let mut svm = HPSVM::new();
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        svm.airdrop(&payer.pubkey(), 10_000).unwrap();
        svm.airdrop(&recipient, 1).unwrap();

        let tx = VersionedTransaction::from(solana_transaction::Transaction::new(
            &[&payer],
            Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
            svm.latest_blockhash(),
        ));
        let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx.clone()));

        let snapshot = |svm: &HPSVM, address: Address| {
            AccountSnapshot::from_readable(address, &svm.get_account(&address).unwrap())
        };

        CaptureBuilder::new("binding")
            .runtime(RuntimeFixtureConfig::new(svm.block_env().slot, None, true, false))
            .programs(vec![ProgramBinding::new(
                program_id,
                Address::new_unique(),
                Some(String::from("candidate")),
            )])
            .pre_accounts(vec![snapshot(&svm, payer.pubkey()), snapshot(&svm, recipient)])
            .baseline(baseline)
            .compares(Compare::everything())
            .capture_transaction(&tx)
            .expect("fixture capture must succeed")
    }

    fn minimal_fixture() -> Fixture {
        let mut svm = HPSVM::new();
        let payer = Keypair::new();
        let recipient = Address::new_unique();
        svm.airdrop(&payer.pubkey(), 10_000).unwrap();
        svm.airdrop(&recipient, 1).unwrap();

        let tx = VersionedTransaction::from(solana_transaction::Transaction::new(
            &[&payer],
            Message::new(&[transfer(&payer.pubkey(), &recipient, 64)], Some(&payer.pubkey())),
            svm.latest_blockhash(),
        ));
        let baseline = ExecutionSnapshot::from_outcome(&svm.transact(tx.clone()));

        CaptureBuilder::new("minimal")
            .runtime(RuntimeFixtureConfig::new(svm.block_env().slot, None, true, false))
            .baseline(baseline)
            .compares(Compare::everything())
            .capture_transaction(&tx)
            .expect("fixture capture must succeed")
    }

    #[test]
    fn a_mapping_without_an_equals_sign_is_rejected() {
        let error = parse_program_map(&[String::from("no-equals")])
            .expect_err("mapping without `=` should fail");
        match error {
            CliError::InvalidProgramMapping { value } => assert_eq!(value, "no-equals"),
            other => panic!("expected CliError::InvalidProgramMapping, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_mapping_is_rejected() {
        let error = parse_program_map(&[String::new()]).expect_err("empty mapping should fail");
        assert!(matches!(error, CliError::InvalidProgramMapping { .. }));
    }

    #[test]
    fn an_unparseable_program_id_is_rejected() {
        let path = temp_elf_path(b"\x7fELF");
        let mapping = format!("not-an-address={}", path.display());
        let error = parse_program_map(&[mapping]).expect_err("bad program id should fail");

        match error {
            CliError::InvalidProgramId { value, reason } => {
                assert_eq!(value, "not-an-address");
                assert!(!reason.is_empty());
            }
            other => panic!("expected CliError::InvalidProgramId, got {other:?}"),
        }

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn an_unreadable_elf_path_is_an_io_error() {
        let program_id = Address::new_unique();
        let mapping = format!("{program_id}=/definitely/not/a/real/file.so");
        assert!(matches!(parse_program_map(&[mapping]).unwrap_err(), CliError::Io(_)));
    }

    #[test]
    fn a_valid_mapping_loads_the_elf_bytes() {
        let program_id = Address::new_unique();
        let path = temp_elf_path(b"\x7fELFpayload");
        let mapping = format!("{program_id}={}", path.display());

        let parsed = parse_program_map(&[mapping]).expect("valid mapping should parse");

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.get(&program_id).map(Vec::as_slice), Some(&b"\x7fELFpayload"[..]));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn several_mappings_are_collected() {
        let first = Address::new_unique();
        let second = Address::new_unique();
        let path_a = temp_elf_path(b"aaa");
        let path_b = temp_elf_path(b"bb");

        let parsed = parse_program_map(&[
            format!("{first}={}", path_a.display()),
            format!("{second}={}", path_b.display()),
        ])
        .expect("valid mappings should parse");

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed.get(&first).map(Vec::as_slice), Some(&b"aaa"[..]));
        assert_eq!(parsed.get(&second).map(Vec::as_slice), Some(&b"bb"[..]));

        std::fs::remove_file(path_a).ok();
        std::fs::remove_file(path_b).ok();
    }

    /// A repeated program id is last-write-wins: `HashMap::insert` overwrites.
    #[test]
    fn a_duplicate_program_id_keeps_the_last_mapping() {
        let program_id = Address::new_unique();
        let first = temp_elf_path(b"first");
        let second = temp_elf_path(b"second");

        let parsed = parse_program_map(&[
            format!("{program_id}={}", first.display()),
            format!("{program_id}={}", second.display()),
        ])
        .expect("valid mappings should parse");

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.get(&program_id).map(Vec::as_slice), Some(&b"second"[..]));

        std::fs::remove_file(first).ok();
        std::fs::remove_file(second).ok();
    }

    #[test]
    fn an_empty_mapping_list_yields_an_empty_map() {
        assert!(parse_program_map(&[]).unwrap().is_empty());
    }

    /// Only the first `=` splits, so a path containing `=` is preserved whole.
    #[test]
    fn a_path_containing_an_equals_sign_is_preserved() {
        let program_id = Address::new_unique();
        let path =
            std::env::temp_dir().join(format!("hpsvm-cli-eq-{}=name.so", Address::new_unique()));
        std::fs::write(&path, b"payload").expect("elf should write");

        let mapping = format!("{program_id}={}", path.display());
        let parsed = parse_program_map(&[mapping]).expect("valid mapping should parse");

        assert_eq!(parsed.get(&program_id).map(Vec::as_slice), Some(&b"payload"[..]));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn fixture_programs_returns_the_transaction_bindings() {
        let program_id = Address::new_unique();
        let fixture = fixture_binding(program_id);
        let bindings = fixture_programs(&fixture);

        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].program_id, program_id);
    }

    #[test]
    fn fixture_programs_is_empty_when_nothing_is_bound() {
        assert!(fixture_programs(&minimal_fixture()).is_empty());
    }

    /// A bound program id present in the map is preloaded, so the runner gets
    /// past the missing-ELF check and fails on the (deliberately bogus) ELF.
    #[test]
    fn preload_runner_injects_a_matching_elf() {
        let program_id = Address::new_unique();
        let fixture = fixture_binding(program_id);
        let mut programs = HashMap::new();
        programs.insert(program_id, b"not-a-real-elf".to_vec());

        let mut runner = preload_runner(FixtureRunner::new(HPSVM::new()), &fixture, &programs);
        let result = runner.run(&fixture);

        assert!(
            !matches!(result, Err(FixtureError::MissingProgramElf { .. })),
            "the injected ELF must satisfy the binding, got {result:?}"
        );
    }

    /// A map that does not cover a bound program id is skipped, leaving the
    /// runner reporting the missing program.
    #[test]
    fn preload_runner_skips_unbound_program_ids() {
        let fixture = fixture_binding(Address::new_unique());
        let programs = HashMap::new();

        let mut runner = preload_runner(FixtureRunner::new(HPSVM::new()), &fixture, &programs);
        let result = runner.run(&fixture);

        assert!(
            matches!(result, Err(FixtureError::MissingProgramElf { .. })),
            "expected MissingProgramElf, got {result:?}"
        );
    }

    /// A map entry for an unbound id must not satisfy a different binding.
    #[test]
    fn preload_runner_ignores_a_map_entry_for_another_program() {
        let bound = Address::new_unique();
        let fixture = fixture_binding(bound);
        let mut programs = HashMap::new();
        programs.insert(Address::new_unique(), b"unrelated".to_vec());

        let mut runner = preload_runner(FixtureRunner::new(HPSVM::new()), &fixture, &programs);
        let result = runner.run(&fixture);

        assert!(matches!(result, Err(FixtureError::MissingProgramElf { .. })), "got {result:?}");
    }

    #[test]
    fn preload_runner_over_a_fixture_without_bindings_is_a_no_op() {
        let fixture = minimal_fixture();
        let programs = HashMap::new();

        let mut runner = preload_runner(FixtureRunner::new(HPSVM::new()), &fixture, &programs);
        let result = runner.run(&fixture);

        assert!(result.is_ok(), "expected a clean run, got {result:?}");
    }
}
