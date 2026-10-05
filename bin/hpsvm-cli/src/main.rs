#![allow(clippy::print_stderr, clippy::print_stdout, missing_docs)]

mod config;
mod cu;
mod error;
mod fixture;
mod program_map;
mod record;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use solana_address::Address;

use crate::{
    cu::report_compute_units,
    fixture::{compare_fixture, inspect_fixture, run_fixture},
    record::{RecordRequest, record_fixture},
};

#[derive(Debug, Parser)]
#[command(name = "hpsvm")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Cu(CuArgs),
    Fixture(FixtureArgs),
}

#[derive(Debug, Args)]
struct CuArgs {
    #[command(subcommand)]
    command: CuCommand,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum FixtureFormatArg {
    #[default]
    Hpsvm,
    Firedancer,
}

/// How the transaction handed to `fixture record` is encoded on disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum TransactionEncodingArg {
    /// Raw `wincode`-serialized `VersionedTransaction` bytes.
    #[default]
    Raw,
    /// Standard base64 with padding.
    Base64,
    /// Lowercase hex.
    Hex,
}

impl TransactionEncodingArg {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Base64 => "base64",
            Self::Hex => "hex",
        }
    }

    /// Decodes `encoded` into the raw transaction bytes.
    pub(crate) fn decode(self, encoded: &[u8]) -> Result<Vec<u8>, String> {
        match self {
            Self::Raw => Ok(encoded.to_vec()),
            Self::Base64 => decode_base64(encoded),
            Self::Hex => decode_hex(encoded),
        }
    }
}

fn decode_base64(encoded: &[u8]) -> Result<Vec<u8>, String> {
    const INVALID: &str = "invalid base64 input";

    // Tolerate the newlines and spaces that appear once base64 is wrapped in
    // config files or copied through a terminal.
    let text: String = encoded
        .iter()
        .map(|byte| char::from(*byte))
        .filter(|character| !character.is_ascii_whitespace())
        .collect();

    if !text.len().is_multiple_of(4) {
        return Err(format!("{INVALID}: length {} is not a multiple of 4", text.len()));
    }

    fn sextet(value: char) -> Option<u8> {
        match value {
            'A'..='Z' => Some(value as u8 - b'A'),
            'a'..='z' => Some(value as u8 - b'a' + 26),
            '0'..='9' => Some(value as u8 - b'0' + 52),
            '+' => Some(62),
            '/' => Some(63),
            _ => None,
        }
    }

    let symbols: Vec<char> = text.chars().collect();
    let mut decoded = Vec::with_capacity(symbols.len() / 4 * 3);
    for (index, group) in symbols.chunks(4).enumerate() {
        let padding = group.iter().filter(|value| **value == '=').count();
        if padding > 2 || (padding > 0 && index + 1 != symbols.len() / 4) {
            return Err(format!("{INVALID}: misplaced padding"));
        }

        // Padding must form a suffix of the final group; a `=` followed by a
        // data symbol is malformed, so reject it explicitly.
        if group.contains(&'=') {
            let first_padding =
                group.iter().position(|value| *value == '=').expect("padding was just found");
            if group[first_padding..].iter().any(|value| *value != '=') {
                return Err(format!("{INVALID}: data character after padding"));
            }
        }

        let mut accumulator = 0u32;
        for (offset, value) in group.iter().enumerate() {
            let bits = if *value == '=' { 0 } else { u32::from(sextet(*value).ok_or(INVALID)?) };
            accumulator |= bits << (18 - 6 * offset);
        }

        decoded.push((accumulator >> 16) as u8);
        if padding < 2 {
            decoded.push((accumulator >> 8) as u8);
        }
        if padding < 1 {
            decoded.push(accumulator as u8);
        }
    }

    Ok(decoded)
}

fn decode_hex(encoded: &[u8]) -> Result<Vec<u8>, String> {
    if !encoded.is_ascii() {
        return Err("invalid hex input: non-ascii byte".to_owned());
    }

    let text: String = encoded
        .iter()
        .map(|byte| char::from(*byte))
        .filter(|character| !character.is_ascii_whitespace())
        .collect();

    if !text.len().is_multiple_of(2) {
        return Err(format!("invalid hex input: length {} is odd", text.len()));
    }

    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            // `to_digit` accepts non-ASCII digit forms, so match an explicit
            // ascii hex alphabet instead.
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => Some(u32::from(byte - b'0')),
                b'a'..=b'f' => Some(u32::from(byte - b'a') + 10),
                b'A'..=b'F' => Some(u32::from(byte - b'A') + 10),
                _ => None,
            };
            let high = digit(pair[0]);
            let low = digit(pair[1]);
            match (high, low) {
                (Some(high), Some(low)) => Ok((high * 16 + low) as u8),
                _ => Err(format!(
                    "invalid hex input: {:?} is not a hex byte pair",
                    std::str::from_utf8(pair).unwrap_or("<non-utf8>")
                )),
            }
        })
        .collect()
}

#[derive(Debug, Subcommand)]
enum CuCommand {
    Report {
        fixture: PathBuf,
        #[arg(long = "fixture-format", value_enum, default_value_t)]
        fixture_format: FixtureFormatArg,
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long)]
        baseline_dir: Option<PathBuf>,
        #[arg(long = "program")]
        programs: Vec<String>,
        #[arg(long)]
        must_pass: bool,
    },
}

#[derive(Debug, Args)]
struct FixtureArgs {
    #[command(subcommand)]
    command: FixtureCommand,
}

#[derive(Debug, Subcommand)]
enum FixtureCommand {
    Inspect {
        fixture: PathBuf,
        #[arg(long = "fixture-format", value_enum, default_value_t)]
        fixture_format: FixtureFormatArg,
    },
    Run {
        fixture: PathBuf,
        #[arg(long = "fixture-format", value_enum, default_value_t)]
        fixture_format: FixtureFormatArg,
        #[arg(long = "program")]
        programs: Vec<String>,
    },
    Compare {
        fixture: PathBuf,
        #[arg(long = "fixture-format", value_enum, default_value_t)]
        fixture_format: FixtureFormatArg,
        #[arg(long = "baseline-program")]
        baseline_programs: Vec<String>,
        #[arg(long = "candidate-program")]
        candidate_programs: Vec<String>,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        ignore_compute_units: bool,
    },
    /// Record a new fixture by executing a transaction against a supplied pre-state.
    Record {
        /// File holding the signed transaction to execute.
        #[arg(long)]
        transaction: PathBuf,
        /// How `transaction` is encoded; `raw` is `wincode` bytes.
        #[arg(long, value_enum, default_value_t)]
        transaction_encoding: TransactionEncodingArg,
        /// JSON array of pre-execution accounts.
        #[arg(long)]
        accounts: PathBuf,
        /// Fixture to write; `.json` or `.bin`.
        #[arg(long, short)]
        output: PathBuf,
        /// Fixture name; defaults to the output file stem.
        #[arg(long)]
        name: Option<String>,
        /// Fixture tag; repeatable.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Free-form provenance string stored in the fixture header.
        #[arg(long)]
        source: Option<String>,
        /// Program ELF to bind, as `<program-id>=<path>`; repeatable.
        #[arg(long = "program")]
        programs: Vec<String>,
        /// Loader to bind recorded programs with; defaults to the upgradeable loader.
        #[arg(long)]
        loader: Option<Address>,
        /// Slot to record against; defaults to the fresh VM's slot.
        #[arg(long)]
        slot: Option<u64>,
        /// Record with signature verification enabled.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        sigverify: bool,
        /// Cap captured log bytes.
        #[arg(long)]
        log_bytes_limit: Option<usize>,
        /// Override the default compute unit limit.
        #[arg(long)]
        compute_unit_limit: Option<u64>,
        /// Record comparisons that ignore compute unit drift.
        #[arg(long)]
        ignore_compute_units: bool,
    },
}

#[cfg_attr(feature = "hotpath", hotpath::main)]
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg_attr(feature = "hotpath", hotpath::measure)]
fn run() -> Result<(), error::CliError> {
    match Cli::parse().command {
        Command::Cu(args) => match args.command {
            CuCommand::Report {
                fixture,
                fixture_format,
                output_dir,
                baseline_dir,
                programs,
                must_pass,
            } => report_compute_units(
                &fixture,
                fixture_format,
                &output_dir,
                baseline_dir.as_deref(),
                &programs,
                must_pass,
            ),
        },
        Command::Fixture(args) => match args.command {
            FixtureCommand::Inspect { fixture, fixture_format } => {
                inspect_fixture(&fixture, fixture_format)
            }
            FixtureCommand::Run { fixture, fixture_format, programs } => {
                run_fixture(&fixture, fixture_format, &programs)
            }
            FixtureCommand::Compare {
                fixture,
                fixture_format,
                baseline_programs,
                candidate_programs,
                config,
                ignore_compute_units,
            } => compare_fixture(
                &fixture,
                fixture_format,
                &baseline_programs,
                &candidate_programs,
                config.as_deref(),
                ignore_compute_units,
            ),
            FixtureCommand::Record {
                transaction,
                transaction_encoding,
                accounts,
                output,
                name,
                tags,
                source,
                programs,
                loader,
                slot,
                sigverify,
                log_bytes_limit,
                compute_unit_limit,
                ignore_compute_units,
            } => record_fixture(&RecordRequest {
                transaction,
                transaction_encoding,
                accounts,
                output,
                name,
                tags,
                source,
                programs,
                loader,
                slot,
                sigverify,
                log_bytes_limit,
                compute_unit_limit,
                ignore_compute_units,
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;
    use solana_address::Address;

    use super::{
        Cli, Command, CuArgs, CuCommand, FixtureArgs, FixtureCommand, FixtureFormatArg,
        TransactionEncodingArg, decode_base64, decode_hex,
    };

    /// Reference encoder, independent of the decoder under test.
    fn encode_base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = u32::from(chunk[0]);
            let b1 = u32::from(chunk.get(1).copied().unwrap_or_default());
            let b2 = u32::from(chunk.get(2).copied().unwrap_or_default());
            let triple = (b0 << 16) | (b1 << 8) | b2;
            let sextet = |shift: u32| char::from(ALPHABET[((triple >> shift) & 0x3f) as usize]);
            encoded.push(sextet(18));
            encoded.push(sextet(12));
            encoded.push(if chunk.len() > 1 { sextet(6) } else { '=' });
            encoded.push(if chunk.len() > 2 { sextet(0) } else { '=' });
        }
        encoded
    }

    #[test]
    fn base64_round_trips_every_padding_case() {
        // Lengths 0, 1, 2, 3 mod 3 exercise the no-pad, one-pad, and two-pad paths.
        for len in 0..=9usize {
            let bytes: Vec<u8> =
                (0..len).map(|index| (index as u8).wrapping_mul(37).wrapping_add(11)).collect();
            let encoded = encode_base64(&bytes);
            assert_eq!(
                decode_base64(encoded.as_bytes()).expect("valid base64 must decode"),
                bytes,
                "round trip failed for length {len} (encoded {encoded:?})"
            );
        }
    }

    #[test]
    fn base64_decodes_the_rfc4648_test_vector() {
        assert_eq!(decode_base64(b"TQ==").unwrap(), b"M");
        assert_eq!(decode_base64(b"TWE=").unwrap(), b"Ma");
        assert_eq!(decode_base64(b"TWFu").unwrap(), b"Man");
    }

    #[test]
    fn base64_tolerates_wrapping_whitespace() {
        assert_eq!(decode_base64(b"TWFu\n").unwrap(), b"Man");
        assert_eq!(decode_base64(b" T W F u ").unwrap(), b"Man");
    }

    #[test]
    fn base64_rejects_bad_input() {
        assert!(decode_base64(b"TWF").is_err(), "length not a multiple of 4");
        assert!(decode_base64(b"T*Fu").is_err(), "character outside the alphabet");
        assert!(decode_base64(b"=WFu").is_err(), "padding in a leading position");
        assert!(decode_base64(b"TW=u").is_err(), "padding in a middle position");
    }

    /// Byte 0xFB 0xFF encodes to the `+/` alphabet entries, so dropping either
    /// digit would make these two inputs decode to the wrong value.
    #[test]
    fn base64_decodes_the_plus_and_slash_alphabet_entries() {
        assert_eq!(decode_base64(b"+/8=").unwrap(), vec![0xfb_u8, 0xff]);
        assert!(decode_base64(b"-_8=").is_err(), "url-safe alphabet is not accepted");
    }

    #[test]
    fn hex_round_trips_and_rejects_bad_input() {
        assert_eq!(decode_hex(b"").unwrap(), Vec::<u8>::new());
        assert_eq!(decode_hex(b"00ff10").unwrap(), vec![0x00, 0xff, 0x10]);
        assert_eq!(decode_hex(b"00 FF\n10").unwrap(), vec![0x00, 0xff, 0x10]);
        assert_eq!(decode_hex(b"DEADbeef").unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
        assert!(decode_hex(b"abc").is_err(), "odd length");
        assert!(decode_hex(b"zz").is_err(), "non-hex digits");
    }

    /// Every hex digit band must be covered, so dropping one range cannot pass.
    #[test]
    fn hex_decodes_every_digit_band() {
        assert_eq!(decode_hex(b"09").unwrap(), vec![0x09]);
        assert_eq!(decode_hex(b"af").unwrap(), vec![0xaf]);
        assert_eq!(decode_hex(b"AF").unwrap(), vec![0xaf]);
    }

    /// `to_digit(16)` also accepts full-width and other Unicode digit forms, so
    /// pin down that non-ASCII input is rejected rather than silently decoded.
    #[test]
    fn hex_rejects_non_ascii_digit_lookalikes() {
        assert!(decode_hex("０ｆ".as_bytes()).is_err(), "full-width digits are not hex");
        assert!(decode_hex("٠f".as_bytes()).is_err(), "arabic-indic digits are not hex");
    }

    #[test]
    fn raw_encoding_is_passed_through_untouched() {
        let bytes = [0u8, 1, 2, 250, 251, 252];
        assert_eq!(TransactionEncodingArg::Raw.decode(&bytes).unwrap(), bytes);
        assert_eq!(TransactionEncodingArg::Hex.decode(b"000102fafbfc").unwrap(), bytes);
        assert_eq!(
            TransactionEncodingArg::Base64.decode(encode_base64(&bytes).as_bytes()).unwrap(),
            bytes
        );
    }

    #[test]
    fn encoding_names_are_stable_for_error_messages() {
        assert_eq!(TransactionEncodingArg::Raw.as_str(), "raw");
        assert_eq!(TransactionEncodingArg::Base64.as_str(), "base64");
        assert_eq!(TransactionEncodingArg::Hex.as_str(), "hex");
    }

    // ---------------------------------------------------------------------
    // clap surface
    // ---------------------------------------------------------------------

    #[test]
    fn parsing_requires_a_subcommand() {
        assert!(Cli::try_parse_from(["hpsvm"]).is_err());
        assert!(Cli::try_parse_from(["hpsvm", "nonsense"]).is_err());
    }

    #[test]
    fn fixture_format_defaults_to_hpsvm_and_parses_both_values() {
        let cli = Cli::try_parse_from(["hpsvm", "fixture", "inspect", "f.json"]).unwrap();
        match cli.command {
            Command::Fixture(FixtureArgs {
                command: FixtureCommand::Inspect { fixture_format, .. },
            }) => {
                assert_eq!(fixture_format, FixtureFormatArg::Hpsvm);
            }
            other => panic!("unexpected command {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "hpsvm",
            "fixture",
            "inspect",
            "f.fix",
            "--fixture-format",
            "firedancer",
        ])
        .unwrap();
        match cli.command {
            Command::Fixture(FixtureArgs {
                command: FixtureCommand::Inspect { fixture_format, .. },
            }) => {
                assert_eq!(fixture_format, FixtureFormatArg::Firedancer);
            }
            other => panic!("unexpected command {other:?}"),
        }

        assert!(
            Cli::try_parse_from([
                "hpsvm",
                "fixture",
                "inspect",
                "f.json",
                "--fixture-format",
                "bogus"
            ])
            .is_err()
        );
    }

    #[test]
    fn cu_report_requires_output_dir_and_defaults_everything_else() {
        let cli = Cli::try_parse_from(["hpsvm", "cu", "report", "f.json", "--output-dir", "out"])
            .unwrap();
        match cli.command {
            Command::Cu(CuArgs {
                command:
                    CuCommand::Report {
                        fixture,
                        output_dir,
                        baseline_dir,
                        programs,
                        must_pass,
                        fixture_format,
                    },
            }) => {
                assert_eq!(fixture, PathBuf::from("f.json"));
                assert_eq!(output_dir, PathBuf::from("out"));
                assert_eq!(baseline_dir, None);
                assert!(programs.is_empty());
                assert!(!must_pass);
                assert_eq!(fixture_format, FixtureFormatArg::Hpsvm);
            }
            other => panic!("unexpected command {other:?}"),
        }

        // `--output-dir` is mandatory.
        assert!(Cli::try_parse_from(["hpsvm", "cu", "report", "f.json"]).is_err());
    }

    #[test]
    fn cu_report_collects_repeatable_program_mappings() {
        let cli = Cli::try_parse_from([
            "hpsvm",
            "cu",
            "report",
            "f.json",
            "--output-dir",
            "out",
            "--program",
            "a=one.so",
            "--program",
            "b=two.so",
            "--baseline-dir",
            "base",
            "--must-pass",
        ])
        .unwrap();
        match cli.command {
            Command::Cu(CuArgs {
                command: CuCommand::Report { programs, baseline_dir, must_pass, .. },
            }) => {
                assert_eq!(programs, vec![String::from("a=one.so"), String::from("b=two.so")]);
                assert_eq!(baseline_dir, Some(PathBuf::from("base")));
                assert!(must_pass);
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn fixture_run_collects_repeatable_programs() {
        let cli = Cli::try_parse_from([
            "hpsvm",
            "fixture",
            "run",
            "dir",
            "--program",
            "id=path.so",
            "--program",
            "id2=path2.so",
        ])
        .unwrap();
        match cli.command {
            Command::Fixture(FixtureArgs { command: FixtureCommand::Run { programs, .. } }) => {
                assert_eq!(
                    programs,
                    vec![String::from("id=path.so"), String::from("id2=path2.so")]
                );
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn fixture_compare_parses_both_program_flags_config_and_ignore_compute_units() {
        let cli = Cli::try_parse_from([
            "hpsvm",
            "fixture",
            "compare",
            "f.json",
            "--baseline-program",
            "b=base.so",
            "--candidate-program",
            "c=cand.so",
            "--config",
            "cmp.json",
            "--ignore-compute-units",
        ])
        .unwrap();
        match cli.command {
            Command::Fixture(FixtureArgs {
                command:
                    FixtureCommand::Compare {
                        baseline_programs,
                        candidate_programs,
                        config,
                        ignore_compute_units,
                        ..
                    },
            }) => {
                assert_eq!(baseline_programs, vec![String::from("b=base.so")]);
                assert_eq!(candidate_programs, vec![String::from("c=cand.so")]);
                assert_eq!(config, Some(PathBuf::from("cmp.json")));
                assert!(ignore_compute_units);
            }
            other => panic!("unexpected command {other:?}"),
        }

        // All of those flags are optional.
        let cli = Cli::try_parse_from(["hpsvm", "fixture", "compare", "f.json"]).unwrap();
        match cli.command {
            Command::Fixture(FixtureArgs {
                command:
                    FixtureCommand::Compare {
                        baseline_programs,
                        candidate_programs,
                        config,
                        ignore_compute_units,
                        ..
                    },
            }) => {
                assert!(baseline_programs.is_empty());
                assert!(candidate_programs.is_empty());
                assert_eq!(config, None);
                assert!(!ignore_compute_units);
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn fixture_record_defaults_name_tags_source_and_flags() {
        let cli = Cli::try_parse_from([
            "hpsvm",
            "fixture",
            "record",
            "--transaction",
            "tx.bin",
            "--accounts",
            "acc.json",
            "-o",
            "out.json",
        ])
        .unwrap();
        match cli.command {
            Command::Fixture(FixtureArgs {
                command:
                    FixtureCommand::Record {
                        name,
                        tags,
                        source,
                        programs,
                        loader,
                        slot,
                        sigverify,
                        log_bytes_limit,
                        compute_unit_limit,
                        ignore_compute_units,
                        transaction_encoding,
                        ..
                    },
            }) => {
                assert_eq!(name, None);
                assert!(tags.is_empty());
                assert_eq!(source, None);
                assert!(programs.is_empty());
                assert_eq!(loader, None);
                assert_eq!(slot, None);
                // Signature verification is on by default.
                assert!(sigverify);
                assert_eq!(log_bytes_limit, None);
                assert_eq!(compute_unit_limit, None);
                assert!(!ignore_compute_units);
                assert_eq!(transaction_encoding, TransactionEncodingArg::Raw);
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn fixture_record_requires_transaction_accounts_and_output() {
        assert!(Cli::try_parse_from(["hpsvm", "fixture", "record"]).is_err());
        assert!(
            Cli::try_parse_from(["hpsvm", "fixture", "record", "--transaction", "tx.bin"]).is_err()
        );
        assert!(
            Cli::try_parse_from([
                "hpsvm",
                "fixture",
                "record",
                "--transaction",
                "tx.bin",
                "--accounts",
                "acc.json"
            ])
            .is_err()
        );
    }

    #[test]
    fn fixture_record_parses_every_optional_flag() {
        const LOADER: &str = "BPFLoaderUpgradeab1e11111111111111111111111";
        let cli = Cli::try_parse_from([
            "hpsvm",
            "fixture",
            "record",
            "--transaction",
            "tx.hex",
            "--transaction-encoding",
            "hex",
            "--accounts",
            "acc.json",
            "-o",
            "out.bin",
            "--name",
            "my-fixture",
            "--tag",
            "alpha",
            "--tag",
            "beta",
            "--source",
            "agave-test-validator",
            "--program",
            "p=prog.so",
            "--loader",
            LOADER,
            "--slot",
            "4242",
            "--sigverify",
            "false",
            "--log-bytes-limit",
            "1024",
            "--compute-unit-limit",
            "200000",
            "--ignore-compute-units",
        ])
        .unwrap();
        match cli.command {
            Command::Fixture(FixtureArgs {
                command:
                    FixtureCommand::Record {
                        transaction_encoding,
                        name,
                        tags,
                        source,
                        programs,
                        loader,
                        slot,
                        sigverify,
                        log_bytes_limit,
                        compute_unit_limit,
                        ignore_compute_units,
                        output,
                        ..
                    },
            }) => {
                assert_eq!(transaction_encoding, TransactionEncodingArg::Hex);
                assert_eq!(name.as_deref(), Some("my-fixture"));
                assert_eq!(tags, vec![String::from("alpha"), String::from("beta")]);
                assert_eq!(source.as_deref(), Some("agave-test-validator"));
                assert_eq!(programs, vec![String::from("p=prog.so")]);
                assert_eq!(loader, Some(parse_address(LOADER)));
                assert_eq!(slot, Some(4242));
                assert!(!sigverify);
                assert_eq!(log_bytes_limit, Some(1024));
                assert_eq!(compute_unit_limit, Some(200_000));
                assert!(ignore_compute_units);
                assert_eq!(output, PathBuf::from("out.bin"));
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn transaction_encoding_parses_every_value_and_rejects_unknown_ones() {
        for (name, expected) in [
            ("raw", TransactionEncodingArg::Raw),
            ("base64", TransactionEncodingArg::Base64),
            ("hex", TransactionEncodingArg::Hex),
        ] {
            let cli = Cli::try_parse_from([
                "hpsvm",
                "fixture",
                "record",
                "--transaction",
                "tx",
                "--transaction-encoding",
                name,
                "--accounts",
                "a",
                "-o",
                "o.json",
            ])
            .unwrap();
            match cli.command {
                Command::Fixture(FixtureArgs {
                    command: FixtureCommand::Record { transaction_encoding, .. },
                }) => assert_eq!(transaction_encoding, expected, "encoding {name}"),
                other => panic!("unexpected command {other:?}"),
            }
        }

        assert!(
            Cli::try_parse_from([
                "hpsvm",
                "fixture",
                "record",
                "--transaction",
                "tx",
                "--transaction-encoding",
                "cbor",
                "--accounts",
                "a",
                "-o",
                "o.json",
            ])
            .is_err()
        );
    }

    #[test]
    fn sigverify_accepts_an_explicit_true() {
        let cli = Cli::try_parse_from([
            "hpsvm",
            "fixture",
            "record",
            "--transaction",
            "tx",
            "--accounts",
            "a",
            "-o",
            "o.json",
            "--sigverify",
            "true",
        ])
        .unwrap();
        match cli.command {
            Command::Fixture(FixtureArgs { command: FixtureCommand::Record { sigverify, .. } }) => {
                assert!(sigverify)
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn a_malformed_loader_address_is_rejected_at_parse_time() {
        assert!(
            Cli::try_parse_from([
                "hpsvm",
                "fixture",
                "record",
                "--transaction",
                "tx",
                "--accounts",
                "a",
                "-o",
                "o.json",
                "--loader",
                "not-an-address",
            ])
            .is_err()
        );
    }

    #[test]
    fn non_numeric_slot_and_limit_values_are_rejected() {
        let base = [
            "hpsvm",
            "fixture",
            "record",
            "--transaction",
            "tx",
            "--accounts",
            "a",
            "-o",
            "o.json",
        ];
        let with = |extra: &[&str]| -> Vec<String> {
            base.iter().chain(extra.iter()).map(|arg| (*arg).to_string()).collect()
        };

        assert!(Cli::try_parse_from(with(&["--slot", "abc"])).is_err());
        assert!(Cli::try_parse_from(with(&["--compute-unit-limit", "lots"])).is_err());
        assert!(Cli::try_parse_from(with(&["--log-bytes-limit", "-1"])).is_err());
        // The same command with valid values still parses.
        assert!(Cli::try_parse_from(with(&["--slot", "1", "--log-bytes-limit", "0"])).is_ok());
    }

    /// Parses a base58 Solana address for the loader-arg assertions above.
    fn parse_address(value: &str) -> Address {
        use std::str::FromStr;
        Address::from_str(value).expect("test address must be valid base58")
    }
}
