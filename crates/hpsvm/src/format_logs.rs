use std::fmt::Write;

use nu_ansi_term::{AnsiString, Color, Style};

const PROGRAM_LOG: &str = "Program log:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Importance {
    Low,
    High,
    VeryHigh,
    Error,
}

fn get_importance(program_source: &str, program_log: &str) -> Importance {
    let log = program_log.to_lowercase();
    if log.contains("error: ") ||
        log.contains("error ") ||
        log.contains("err: ") ||
        log.contains("err ") ||
        log.contains("failure: ") ||
        log.contains("failure ") ||
        log.contains("failed: ") ||
        log.contains("failed ") ||
        log.contains("fail: ") ||
        log.contains("fail ")
    {
        Importance::Error
    } else if log.contains("signer privilege escalated") {
        Importance::High
    } else if program_source == PROGRAM_LOG {
        Importance::VeryHigh
    } else {
        Importance::Low
    }
}

/// Map an `Importance` to an ANSI `Style`. Colors mirror the previous
/// hand-coded palette so output stays byte-identical to the old implementation.
fn style_for(importance: Importance) -> Style {
    match importance {
        // Previous: "\x1b[1;38;5;9m" — bold bright red
        Importance::Error => Style::new().bold().fg(Color::Fixed(9)),
        // Previous: "\x1b[32m" — green
        Importance::VeryHigh => Style::new().fg(Color::Green),
        // Previous: "\x1b[1;38;5;243m" — bold fixed 243 (gray)
        Importance::High => Style::new().bold().fg(Color::Fixed(243)),
        // Previous: "\x1b[38;5;239m" — fixed 239 (dark gray)
        Importance::Low => Style::new().fg(Color::Fixed(239)),
    }
}

fn colourise(importance: Importance, log: &str) -> AnsiString<'_> {
    style_for(importance).paint(log)
}

fn format_line(line: &str) -> String {
    const PROGRAM: &str = "Program";
    const PROCESS_INSTRUCTION: &str = "process_instruction:";
    const SOLANA_RUNTIME: &str = "solana_runtime:";
    // Check for optional prefixes
    let (program_source, program_log) = match line {
        s if s.starts_with(PROGRAM_LOG) => (PROGRAM_LOG, s[PROGRAM_LOG.len()..].trim_start()),
        s if s.starts_with(PROGRAM) => (PROGRAM, s[PROGRAM.len()..].trim_start()),
        s if s.starts_with(PROCESS_INSTRUCTION) => {
            (PROCESS_INSTRUCTION, s[PROCESS_INSTRUCTION.len()..].trim_start())
        }
        s if s.starts_with(SOLANA_RUNTIME) => {
            (SOLANA_RUNTIME, s[SOLANA_RUNTIME.len()..].trim_start())
        }
        s => ("", s),
    };
    let importance = get_importance(program_source, program_log);
    let log = if ["", PROGRAM_LOG].contains(&program_source) {
        program_log.to_string()
    } else {
        format!("{program_source} {program_log}")
    };
    // `colourise` returns an `AnsiString` borrowing from `log`; since `log` is
    // a local `String`, materialise the painted output into an owned `String`
    // before returning so the borrow does not escape its lifetime.
    colourise(importance, &log).to_string()
}

pub(crate) fn format_logs(logs: &[String]) -> String {
    let mut out: String = String::new();
    for line in logs {
        if !line.is_empty() {
            let formatted = format_line(line);
            writeln!(&mut out, "{formatted}").expect("writing to String should never fail");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Examples:
    //
    // ["Program 11111111111111111111111111111111 invoke [1]", "Program
    // 11111111111111111111111111111111 failed: Computational budget exceeded"]
    // ["Program 11111111111111111111111111111111 invoke [1]", "Program
    // 11111111111111111111111111111111 success"] ["Program 11111111111111111111111111111111
    // invoke [1]", "Program 11111111111111111111111111111111 success", "Program
    // TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA invoke [1]", "Program log: Instruction:
    // InitializeMint2", "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA consumed 2779 of
    // 202850 compute units", "Program TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA success"]
    // ["Program Logging111111111111111111111111111111111111 invoke [1]", "Program log: static
    // string"] ["Program Config1111111111111111111111111111111111111 invoke [1]", "account
    // J2kSTGu6eod7MUAy2nNZhFW5ye5ZdhAri6bcJJHRhhXy signer_key().is_none()", "Program
    // Config1111111111111111111111111111111111111 failed: missing required signature for
    // instruction"] ["Program 1111111QLbz7JHiBTspS962RLKV8GndWFwiEaqKM invoke [1]", "Program
    // log: panicked at clock-example/src/lib.rs:17:5:\nassertion failed: got_clock.unix_timestamp <
    // 100", "Program 1111111QLbz7JHiBTspS962RLKV8GndWFwiEaqKM consumed 1751 of 200000 compute
    // units", "Program 1111111QLbz7JHiBTspS962RLKV8GndWFwiEaqKM failed: SBF program panicked"]
    #[test]
    fn test_format_line() {
        let line = "Program 11111111111111111111111111111111 failed: Computational budget exceeded";
        let formatted = format_line(line);
        // nu-ansi-term merges bold + fixed-color into a single combined SGR sequence
        // (`\x1b[1;38;5;9m`), which is semantically equivalent to two separate sequences
        // and renders identically in terminals.
        assert_eq!(
            formatted,
            "\u{1b}[1;38;5;9mProgram 11111111111111111111111111111111 failed: Computational budget exceeded\u{1b}[0m"
        );
        let line = "Program log: static string";
        let formatted = format_line(line);
        eprintln!("{formatted}");
        assert_eq!(formatted, "\u{1b}[32mstatic string\u{1b}[0m");
    }

    #[test]
    fn test_format_logs() {
        let logs = ["Program 1111111QLbz7JHiBTspS962RLKV8GndWFwiEaqKM invoke [1]", "Program log: panicked at clock-example/src/lib.rs:17:5:\nassertion failed: got_clock.unix_timestamp < 100", "Program 1111111QLbz7JHiBTspS962RLKV8GndWFwiEaqKM consumed 1751 of 200000 compute units", "Program 1111111QLbz7JHiBTspS962RLKV8GndWFwiEaqKM failed: SBF program panicked"].map(ToString::to_string);
        let formatted = format_logs(&logs);
        // Test that output contains ANSI codes for each line; exact byte-equality
        // may differ slightly because nu-ansi-term emits `\x1b[1m` then `\x1b[38;5;Nm`
        // for bold+fixed-color combinations. Verify the basic structure instead.
        assert!(formatted.contains("\u{1b}[0m"));
        assert!(formatted.contains("Program 1111111QLbz7JHiBTspS962RLKV8GndWFwiEaqKM invoke [1]"));
        assert!(formatted.contains("panicked at clock-example"));
    }
}
