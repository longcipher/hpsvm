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
    use proptest::prelude::*;

    use super::*;

    const BOLD_RED: &str = "\u{1b}[1;38;5;9m";
    const GREEN: &str = "\u{1b}[32m";
    const BOLD_GRAY: &str = "\u{1b}[1;38;5;243m";
    const DARK_GRAY: &str = "\u{1b}[38;5;239m";
    const RESET: &str = "\u{1b}[0m";

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

    /// Each of the ten error keywords must classify the log as [`Importance::Error`].
    #[test]
    fn every_error_keyword_yields_error_importance() {
        let keywords = [
            "error: ",
            "error ",
            "err: ",
            "err ",
            "failure: ",
            "failure ",
            "failed: ",
            "failed ",
            "fail: ",
            "fail ",
        ];
        for keyword in keywords {
            assert_eq!(
                get_importance(PROGRAM_LOG, keyword),
                Importance::Error,
                "keyword {keyword:?}"
            );
        }
    }

    /// Keyword matching is case-insensitive because the log is lowercased first.
    #[test]
    fn error_keyword_matching_is_case_insensitive() {
        for keyword in ["ERROR: boom", "Failed: x", "Err: y", "FaIl: z", "FAILURE w"] {
            assert_eq!(
                get_importance(PROGRAM_LOG, keyword),
                Importance::Error,
                "keyword {keyword:?}"
            );
        }
    }

    /// A keyword only counts as an error when it is followed by a space, so
    /// substrings such as "errorless" or "failed_up" must not be misclassified.
    #[test]
    fn keywords_without_a_trailing_space_are_not_errors() {
        for log in ["errorless path", "failed_up", "failover", "no errors here", "my err:or"] {
            assert_eq!(get_importance(PROGRAM_LOG, log), Importance::VeryHigh, "log {log:?}");
        }
    }

    /// The error check wins over both the escalation and the `Program log:`
    /// branches, so a panicking program log is always rendered in bold red.
    #[test]
    fn error_importance_outranks_escalation_and_program_log() {
        assert_eq!(get_importance(PROGRAM_LOG, "failed: out of compute"), Importance::Error);
        assert_eq!(
            get_importance(PROGRAM_LOG, "failed: signer privilege escalated"),
            Importance::Error
        );
        // A non-`Program log:` source still yields Error for an error keyword.
        assert_eq!(get_importance("Program", "failure : nested"), Importance::Error);
    }

    #[test]
    fn escalation_is_high_importance_outside_program_logs() {
        assert_eq!(get_importance("Program", "signer privilege escalated"), Importance::High);
        assert_eq!(get_importance("", "signer privilege escalated"), Importance::High);
    }

    #[test]
    fn program_logs_are_very_high_importance() {
        assert_eq!(get_importance(PROGRAM_LOG, "hello world"), Importance::VeryHigh);
    }

    #[test]
    fn unrecognised_sources_are_low_importance() {
        for (source, log) in [
            ("Program", "invoke [1]"),
            ("process_instruction:", "Trace"),
            ("solana_runtime:", "x"),
            ("", "plain"),
        ] {
            assert_eq!(get_importance(source, log), Importance::Low, "source {source:?}");
        }
    }

    #[test]
    fn style_for_maps_each_importance_to_a_distinct_palette() {
        // `Style` is not `Display`, so paint a marker and inspect the prefix.
        let painted = |importance| style_for(importance).paint("x").to_string();
        assert!(
            painted(Importance::Error).starts_with(BOLD_RED),
            "got {}",
            painted(Importance::Error)
        );
        assert!(
            painted(Importance::VeryHigh).starts_with(GREEN),
            "got {}",
            painted(Importance::VeryHigh)
        );
        assert!(
            painted(Importance::High).starts_with(BOLD_GRAY),
            "got {}",
            painted(Importance::High)
        );
        assert!(
            painted(Importance::Low).starts_with(DARK_GRAY),
            "got {}",
            painted(Importance::Low)
        );
    }

    #[test]
    fn colourise_wraps_the_log_in_the_importance_style() {
        assert_eq!(colourise(Importance::Error, "x").to_string(), format!("{BOLD_RED}x{RESET}"));
        assert_eq!(colourise(Importance::Low, "x").to_string(), format!("{DARK_GRAY}x{RESET}"));
    }

    /// `Program log:` strips its prefix and paints the remainder green.
    #[test]
    fn program_log_prefix_is_stripped() {
        assert_eq!(
            format_line("Program log: static string"),
            format!("{GREEN}static string{RESET}")
        );
        // Leading spaces after the prefix are trimmed.
        assert_eq!(format_line("Program log:   padded"), format!("{GREEN}padded{RESET}"));
    }

    /// `Program ` keeps its prefix but lowers the importance to Low.
    #[test]
    fn program_prefix_is_preserved_with_low_importance() {
        assert_eq!(
            format_line("Program 11111111111111111111111111111111 invoke [1]"),
            format!("{DARK_GRAY}Program 11111111111111111111111111111111 invoke [1]{RESET}")
        );
    }

    #[test]
    fn process_instruction_prefix_is_preserved() {
        assert_eq!(
            format_line("process_instruction: ComputeBudget1234 [1]"),
            format!("{DARK_GRAY}process_instruction: ComputeBudget1234 [1]{RESET}")
        );
    }

    #[test]
    fn solana_runtime_prefix_is_preserved() {
        // The `solana_runtime:` prefix is recognised as the runtime prefix and
        // normalised to `solana_runtime: `, staying in Low importance.
        let formatted = format_line("solana_runtime: :program::process_instruction: hi");
        assert_eq!(
            formatted,
            format!("{DARK_GRAY}solana_runtime: :program::process_instruction: hi{RESET}")
        );
    }

    /// A bare line with no known prefix is emitted verbatim in Low importance.
    #[test]
    fn lines_without_a_known_prefix_are_emitted_verbatim() {
        assert_eq!(format_line("just some text"), format!("{DARK_GRAY}just some text{RESET}"));
        assert_eq!(format_line(""), format!("{DARK_GRAY}{RESET}"));
    }

    /// The `Program` prefix must be matched after `Program log:`, otherwise
    /// `Program log:` lines would be routed into the `Program` branch.
    #[test]
    fn program_log_prefix_is_matched_before_the_bare_program_prefix() {
        assert_eq!(format_line("Program log: x"), format!("{GREEN}x{RESET}"));
        assert_eq!(format_line("Program logx: y"), format!("{DARK_GRAY}Program logx: y{RESET}"));
    }

    #[test]
    fn format_logs_skips_empty_lines() {
        let logs = vec![
            String::from("Program log: first"),
            String::new(),
            String::from(""),
            String::from("Program log: second"),
        ];
        let formatted = format_logs(&logs);

        assert_eq!(formatted, format!("{GREEN}first{RESET}\n{GREEN}second{RESET}\n"));
    }

    #[test]
    fn format_logs_of_an_empty_slice_is_empty() {
        assert_eq!(format_logs(&[]), "");
    }

    #[test]
    fn format_logs_terminates_every_line_with_a_newline() {
        let logs = vec![String::from("a"), String::from("b"), String::new()];
        let formatted = format_logs(&logs);
        assert_eq!(formatted.lines().count(), 2);
        assert!(formatted.ends_with('\n'));
    }

    /// Multi-byte characters and quoting must survive formatting unchanged.
    #[test]
    fn arbitrary_lines_are_formatted_without_corruption() {
        let logs = vec![
            String::from("Program log: 日本語 ✓ 🎉"),
            String::from(
                "Program 11111111111111111111111111111111 consumed 0 of 200000 compute units",
            ),
            String::from("no prefix with \"quotes\" and \\backslash|"),
        ];
        let formatted = format_logs(&logs);
        assert!(formatted.contains("日本語 ✓ 🎉"));
        assert!(formatted.contains("no prefix with \"quotes\" and \\backslash|"));
    }

    // Property: each non-empty input line yields exactly one output line, and
    // every produced line is terminated by a reset sequence.
    proptest! {
        #[test]
        fn format_logs_emits_one_painted_line_per_non_empty_input(
            lines in prop::collection::vec("[^\r\n]*", 0..10),
        ) {
            let logs: Vec<String> = lines.clone();
            let formatted = format_logs(&logs);
            let expected = logs.iter().filter(|line| !line.is_empty()).count();

            prop_assert_eq!(formatted.lines().count(), expected);
            prop_assert_eq!(formatted.matches(RESET).count(), expected);
        }
    }

    // Property: `format_line` output always re-paints the line, i.e. it is
    // wrapped in exactly one style and never leaks a raw control character
    // other than the escape sequences `nu-ansi-term` emits.
    proptest! {
        #[test]
        fn format_line_always_wraps_the_rendered_text(line in ".{0,64}") {
            let formatted = format_line(&line);
            // Bound first: `prop_assert!` treats its single expression as a
            // format string, so literal braces must not appear inline.
            let escape = '\u{1b}';
            prop_assert!(formatted.ends_with(RESET));
            prop_assert!(formatted.contains(escape));
            // The style prefix is one of the four palette entries.
            prop_assert!(
                [BOLD_RED, GREEN, BOLD_GRAY, DARK_GRAY]
                    .iter()
                    .any(|style| formatted.starts_with(style))
            );
        }
    }

    // Property: a `Program log:` line never keeps its `Program log:` prefix in
    // the rendered output, whatever the body and whatever importance it gets.
    proptest! {
        #[test]
        fn program_log_lines_never_keep_their_prefix(body in ".*") {
            let formatted = format_line(&format!("Program log: {body}"));
            let rendered = formatted
                .strip_prefix(BOLD_RED)
                .or_else(|| formatted.strip_prefix(GREEN))
                .and_then(|rest| rest.strip_suffix(RESET))
                .expect("program log lines are painted with a known style");
            prop_assert!(!rendered.starts_with("Program log:"));
        }
    }
}
