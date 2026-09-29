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

#[derive(Parser)]
#[command(name = "hpsvm")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Cu(CuArgs),
    Fixture(FixtureArgs),
}

#[derive(Args)]
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

#[derive(Subcommand)]
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

#[derive(Args)]
struct FixtureArgs {
    #[command(subcommand)]
    command: FixtureCommand,
}

#[derive(Subcommand)]
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
    use super::{TransactionEncodingArg, decode_base64, decode_hex};

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
}
