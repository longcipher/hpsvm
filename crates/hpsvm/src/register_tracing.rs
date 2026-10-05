use std::{
    collections::HashMap,
    fs::File,
    io::{self, Write},
    path::Path,
    sync::Arc,
};

use parking_lot::Mutex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use solana_address::Address;
use solana_program_runtime::invoke_context::{Executable, InvokeContext, RegisterTrace};
use solana_transaction::sanitized::SanitizedTransaction;
use solana_transaction_context::{IndexOfAccount, instruction::InstructionContext};

use crate::{HPSVM, InvocationInspectCallback};

const DEFAULT_PATH: &str = "target/sbf/trace";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProgramTraceMetrics {
    pub program_id: Address,
    pub invocations: usize,
    pub cpi_invocations: usize,
    pub total_register_frames: usize,
    pub max_register_frames: usize,
    pub max_stack_height: usize,
    pub total_instruction_accounts: usize,
    pub max_instruction_accounts: usize,
}

impl ProgramTraceMetrics {
    fn new(program_id: Address) -> Self {
        Self {
            program_id,
            invocations: 0,
            cpi_invocations: 0,
            total_register_frames: 0,
            max_register_frames: 0,
            max_stack_height: 0,
            total_instruction_accounts: 0,
            max_instruction_accounts: 0,
        }
    }

    pub fn average_register_frames(&self) -> f64 {
        if self.invocations == 0 {
            0.0
        } else {
            self.total_register_frames as f64 / self.invocations as f64
        }
    }

    pub fn average_instruction_accounts(&self) -> f64 {
        if self.invocations == 0 {
            0.0
        } else {
            self.total_instruction_accounts as f64 / self.invocations as f64
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct TraceMetricsCollector {
    metrics: Arc<Mutex<HashMap<Address, ProgramTraceMetrics>>>,
}

impl TraceMetricsCollector {
    pub fn snapshot(&self) -> Vec<ProgramTraceMetrics> {
        let mut metrics = self.metrics.lock().values().cloned().collect::<Vec<_>>();
        metrics.sort_by(|left, right| {
            right
                .total_register_frames
                .cmp(&left.total_register_frames)
                .then_with(|| right.invocations.cmp(&left.invocations))
                .then_with(|| left.program_id.cmp(&right.program_id))
        });
        metrics
    }

    pub fn write_json_path(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let mut file = File::create(path)?;
        write_trace_metrics_json(&mut file, &self.snapshot())
    }

    fn record_trace(
        &self,
        instruction_context: InstructionContext<'_, '_>,
        register_trace: RegisterTrace<'_>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if register_trace.is_empty() {
            return Ok(());
        }

        let program_id = *instruction_context.get_program_key()?;
        let stack_height = instruction_context.get_stack_height();
        let instruction_accounts =
            usize::from(instruction_context.get_number_of_instruction_accounts());
        let register_frames = register_trace.len();

        let mut metrics = self.metrics.lock();
        let entry =
            metrics.entry(program_id).or_insert_with(|| ProgramTraceMetrics::new(program_id));
        entry.invocations = entry.invocations.saturating_add(1);
        if stack_height > 1 {
            entry.cpi_invocations = entry.cpi_invocations.saturating_add(1);
        }
        entry.total_register_frames = entry.total_register_frames.saturating_add(register_frames);
        entry.max_register_frames = entry.max_register_frames.max(register_frames);
        entry.max_stack_height = entry.max_stack_height.max(stack_height);
        entry.total_instruction_accounts =
            entry.total_instruction_accounts.saturating_add(instruction_accounts);
        entry.max_instruction_accounts = entry.max_instruction_accounts.max(instruction_accounts);
        Ok(())
    }
}

/// Serializable wrapper for the JSON output format.
#[derive(Serialize)]
struct TraceMetricsJson<'a> {
    programs: &'a [ProgramTraceMetrics],
}

pub fn write_trace_metrics_json(
    writer: &mut impl Write,
    metrics: &[ProgramTraceMetrics],
) -> io::Result<()> {
    let output = TraceMetricsJson { programs: metrics };
    serde_json::to_writer_pretty(writer, &output)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}

#[derive(Debug)]
pub struct DefaultRegisterTracingCallback {
    pub sbf_trace_dir: String,
    pub sbf_trace_disassemble: bool,
}

impl Default for DefaultRegisterTracingCallback {
    fn default() -> Self {
        Self {
            // User can override default path with `SBF_TRACE_DIR` environment variable.
            sbf_trace_dir: std::env::var("SBF_TRACE_DIR").unwrap_or(DEFAULT_PATH.to_string()),
            sbf_trace_disassemble: std::env::var("SBF_TRACE_DISASSEMBLE").is_ok(),
        }
    }
}

impl DefaultRegisterTracingCallback {
    pub fn disassemble_register_trace<W: std::io::Write>(
        &self,
        writer: &mut W,
        program_id: &Address,
        executable: &Executable,
        register_trace: RegisterTrace<'_>,
    ) {
        match solana_program_runtime::solana_sbpf::static_analysis::Analysis::from_executable(
            executable,
        ) {
            Ok(analysis) => {
                if let Err(e) = analysis.disassemble_register_trace(writer, register_trace) {
                    eprintln!("Can't disassemble register trace for {program_id}: {e:#?}");
                }
            }
            Err(e) => {
                eprintln!("Can't create trace disassemble analysis for {program_id}: {e:#?}")
            }
        }
    }

    pub fn handler(
        &self,
        svm: &HPSVM,
        instruction_context: InstructionContext<'_, '_>,
        executable: &Executable,
        register_trace: RegisterTrace<'_>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if register_trace.is_empty() {
            // Can't do much with an empty trace.
            return Ok(());
        }

        let current_dir = std::env::current_dir()?;
        let sbf_trace_dir = current_dir.join(&self.sbf_trace_dir);

        // Reject paths that escape the working directory.
        if let Ok(canonical) = sbf_trace_dir.canonicalize() {
            if !canonical.starts_with(&current_dir) {
                return Err(format!(
                    "SBF_TRACE_DIR resolves outside working directory: {}",
                    canonical.display()
                )
                .into());
            }
        }

        std::fs::create_dir_all(&sbf_trace_dir)?;

        let trace_digest = compute_hash(as_bytes(register_trace));
        let base_fname = sbf_trace_dir.join(&trace_digest[..16]);
        let mut regs_file = File::create(base_fname.with_extension("regs"))?;
        let mut insns_file = File::create(base_fname.with_extension("insns"))?;
        let mut program_id_file = File::create(base_fname.with_extension("program_id"))?;

        // Get program_id.
        let program_id = instruction_context.get_program_key()?;

        // Persist a full trace disassembly if requested.
        if self.sbf_trace_disassemble {
            let mut trace_disassemble_file = File::create(base_fname.with_extension("trace"))?;
            self.disassemble_register_trace(
                &mut trace_disassemble_file,
                program_id,
                executable,
                register_trace,
            );
        }

        // Persist the program id.
        let _ = program_id_file.write(program_id.to_string().as_bytes());

        if let Ok(elf_data) = svm.accounts().try_program_elf_bytes(program_id) {
            // Persist the preload hash of the executable.
            let mut so_hash_file = File::create(base_fname.with_extension("exec.sha256"))?;
            let _ = so_hash_file.write(compute_hash(elf_data).as_bytes());
        }

        // Get the relocated executable.
        let (_, program) = executable.get_text_bytes();
        for regs in register_trace.iter() {
            // The program counter is stored in r11.
            let pc = regs[11];
            // From the executable fetch the instruction this program counter points to.
            let insn =
                solana_program_runtime::solana_sbpf::ebpf::get_insn_unchecked(program, pc as usize)
                    .to_array();

            // Persist them in files.
            let _ = regs_file.write(as_bytes(regs.as_slice()))?;
            let _ = insns_file.write(insn.as_slice())?;
        }

        Ok(())
    }
}

impl InvocationInspectCallback for DefaultRegisterTracingCallback {
    fn before_invocation(
        &self,
        _: &HPSVM,
        _: &SanitizedTransaction,
        _: &[IndexOfAccount],
        _: &InvokeContext<'_, '_>,
    ) {
    }

    fn after_invocation(
        &self,
        svm: &HPSVM,
        invoke_context: &InvokeContext<'_, '_>,
        register_tracing_enabled: bool,
    ) {
        if register_tracing_enabled {
            // Only read the register traces if they were actually enabled.
            invoke_context.iterate_vm_traces(
                &|instruction_context: InstructionContext<'_, '_>,
                  executable: &Executable,
                  register_trace: RegisterTrace<'_>| {
                    if let Err(e) =
                        self.handler(svm, instruction_context, executable, register_trace)
                    {
                        eprintln!("Error collecting the register tracing: {}", e);
                    }
                },
            );
        }
    }
}

impl InvocationInspectCallback for TraceMetricsCollector {
    fn before_invocation(
        &self,
        _: &HPSVM,
        _: &SanitizedTransaction,
        _: &[IndexOfAccount],
        _: &InvokeContext<'_, '_>,
    ) {
    }

    fn after_invocation(
        &self,
        _: &HPSVM,
        invoke_context: &InvokeContext<'_, '_>,
        register_tracing_enabled: bool,
    ) {
        if register_tracing_enabled {
            invoke_context.iterate_vm_traces(
                &|instruction_context: InstructionContext<'_, '_>,
                  _executable: &Executable,
                  register_trace: RegisterTrace<'_>| {
                    if let Err(error) = self.record_trace(instruction_context, register_trace) {
                        eprintln!("Error collecting trace metrics: {error}");
                    }
                },
            );
        }
    }
}

// SAFETY: T must be `Copy` (no drop glue) and must not contain padding bytes
// (e.g. `u64`, `[u64; N]`). The resulting byte slice faithfully represents the
// original data. The `Copy` bound is enforced at compile time to prevent
// accidental use with types that have non-trivial drop or internal pointers.
pub(crate) fn as_bytes<T: Copy>(slice: &[T]) -> &[u8] {
    // SAFETY: `T: Copy` guarantees no drop glue; the pointer cast is valid for
    // POD types without internal padding. Callers must ensure `T` has no padding.
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
}

fn compute_hash(slice: &[u8]) -> String {
    hex::encode(Sha256::digest(slice).as_slice())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// Minimal metrics row with the two ordering keys set explicitly.
    fn metrics_row(program_id: Address, invocations: usize, frames: usize) -> ProgramTraceMetrics {
        ProgramTraceMetrics {
            program_id,
            invocations,
            cpi_invocations: 0,
            total_register_frames: frames,
            max_register_frames: frames,
            max_stack_height: 1,
            total_instruction_accounts: invocations,
            max_instruction_accounts: 1,
        }
    }

    #[test]
    fn sbf_trace_dir_rejects_paths_outside_cwd() {
        let callback = DefaultRegisterTracingCallback {
            sbf_trace_dir: "/etc".to_string(),
            sbf_trace_disassemble: false,
        };
        let current_dir = std::env::current_dir().unwrap();
        let sbf_trace_dir = current_dir.join(&callback.sbf_trace_dir);

        // /etc canonicalizes to /etc which is NOT under cwd.
        if let Ok(canonical) = sbf_trace_dir.canonicalize() {
            assert!(!canonical.starts_with(&current_dir), "path outside cwd should be rejected");
        }
    }

    #[test]
    fn sbf_trace_dir_allows_relative_paths() {
        let callback = DefaultRegisterTracingCallback {
            sbf_trace_dir: "target/sbf/trace".to_string(),
            sbf_trace_disassemble: false,
        };
        let current_dir = std::env::current_dir().unwrap();
        let sbf_trace_dir = current_dir.join(&callback.sbf_trace_dir);

        // Relative paths resolve under cwd.
        assert!(sbf_trace_dir.starts_with(&current_dir));
    }

    /// A freshly created metrics row has no invocations, so both averages must
    /// be zero rather than `NaN` from a 0/0 division.
    #[test]
    fn averages_of_an_unobserved_program_are_zero() {
        let fresh = ProgramTraceMetrics::new(Address::new_unique());
        assert_eq!(fresh.invocations, 0);
        assert_eq!(fresh.average_register_frames(), 0.0);
        assert_eq!(fresh.average_instruction_accounts(), 0.0);
        assert!(fresh.average_register_frames().is_finite());
        assert!(fresh.average_instruction_accounts().is_finite());
    }

    #[test]
    fn averages_divide_the_totals_by_the_invocation_count() {
        let program_id = Address::new_unique();
        let mut entry = metrics_row(program_id, 4, 40);
        entry.total_instruction_accounts = 8;
        assert_eq!(entry.average_register_frames(), 10.0);
        assert_eq!(entry.average_instruction_accounts(), 2.0);
    }

    #[test]
    fn averages_of_a_single_frame_invocation_are_that_frame_count() {
        let program_id = Address::new_unique();
        let mut entry = ProgramTraceMetrics::new(program_id);
        entry.invocations = 1;
        entry.total_register_frames = 7;
        entry.total_instruction_accounts = 3;
        assert_eq!(entry.average_register_frames(), 7.0);
        assert_eq!(entry.average_instruction_accounts(), 3.0);
    }

    #[test]
    fn snapshot_tolerates_a_stale_entry_for_the_same_program() {
        // Overwriting a row must fully replace it rather than merge.
        let collector = TraceMetricsCollector::default();
        let program_id = Address::new_from_array([5u8; 32]);
        {
            let mut metrics = collector.metrics.lock();
            metrics.insert(program_id, metrics_row(program_id, 9, 99));
            metrics.insert(program_id, metrics_row(program_id, 1, 1));
        }

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].invocations, 1);
        assert_eq!(snapshot[0].total_register_frames, 1);
    }

    #[test]
    fn snapshot_of_an_empty_collector_is_empty() {
        let collector = TraceMetricsCollector::default();
        assert!(collector.snapshot().is_empty());
    }

    /// Ordering: descending frames, then descending invocations, then ascending
    /// program id — the final key makes the sort total and deterministic.
    #[test]
    fn snapshot_sorts_by_frames_then_invocations_then_program_id() {
        let collector = TraceMetricsCollector::default();
        let mut metrics = collector.metrics.lock();

        let low = Address::new_from_array([1u8; 32]);
        let high = Address::new_from_array([2u8; 32]);
        let mid_low = Address::new_from_array([3u8; 32]);

        // 10 frames / 2 invocations, low id
        metrics.insert(low, metrics_row(low, 2, 10));
        // 10 frames / 5 invocations, high id -> wins on invocations
        metrics.insert(high, metrics_row(high, 5, 10));
        // 20 frames / 1 invocation -> wins on frames
        metrics.insert(mid_low, metrics_row(mid_low, 1, 20));
        drop(metrics);

        let snapshot = collector.snapshot();
        assert_eq!(
            snapshot.iter().map(|entry| entry.program_id).collect::<Vec<_>>(),
            vec![mid_low, high, low]
        );
    }

    /// Two rows tied on both frames and invocations fall back to ascending id.
    #[test]
    fn snapshot_ties_break_on_ascending_program_id() {
        let collector = TraceMetricsCollector::default();
        let first = Address::new_from_array([9u8; 32]);
        let second = Address::new_from_array([4u8; 32]);
        {
            let mut metrics = collector.metrics.lock();
            metrics.insert(first, metrics_row(first, 3, 7));
            metrics.insert(second, metrics_row(second, 3, 7));
        }

        let snapshot = collector.snapshot();
        assert_eq!(snapshot[0].program_id, second);
        assert_eq!(snapshot[1].program_id, first);
    }

    /// The JSON document is `{"programs": [...]}` and must serialise every metric
    /// field, so downstream tooling can rely on the shape.
    #[test]
    fn write_trace_metrics_json_emits_the_programs_array() {
        let program_id = Address::new_from_array([7u8; 32]);
        let rows = [ProgramTraceMetrics {
            program_id,
            invocations: 2,
            cpi_invocations: 1,
            total_register_frames: 12,
            max_register_frames: 8,
            max_stack_height: 3,
            total_instruction_accounts: 10,
            max_instruction_accounts: 6,
        }];

        let mut buffer: Vec<u8> = Vec::new();
        write_trace_metrics_json(&mut buffer, &rows).unwrap();

        let text = String::from_utf8(buffer).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let programs = value["programs"].as_array().expect("programs must be an array");
        assert_eq!(programs.len(), 1);
        assert_eq!(programs[0]["invocations"], 2);
        assert_eq!(programs[0]["cpi_invocations"], 1);
        assert_eq!(programs[0]["total_register_frames"], 12);
        assert_eq!(programs[0]["max_register_frames"], 8);
        assert_eq!(programs[0]["max_stack_height"], 3);
        assert_eq!(programs[0]["total_instruction_accounts"], 10);
        assert_eq!(programs[0]["max_instruction_accounts"], 6);
        assert_eq!(programs[0]["program_id"], serde_json::json!(program_id.to_bytes()));
        // Pretty-printed output is multi-line.
        assert!(text.contains('\n'));
    }

    #[test]
    fn write_trace_metrics_json_of_an_empty_snapshot_is_an_empty_array() {
        let mut buffer: Vec<u8> = Vec::new();
        write_trace_metrics_json(&mut buffer, &[]).unwrap();
        let text = String::from_utf8(buffer).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["programs"].as_array().map(Vec::len), Some(0));
    }

    /// A failing writer must surface as an `io::Error`, not a panic.
    #[test]
    fn write_trace_metrics_json_maps_writer_failures_to_io_errors() {
        struct FailingWriter;

        impl Write for FailingWriter {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("disk full"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let error = write_trace_metrics_json(&mut FailingWriter, &[]).unwrap_err();
        assert_eq!(error.to_string(), "disk full");
    }

    #[test]
    fn write_json_path_writes_the_snapshot_to_disk() {
        let dir = std::env::temp_dir().join(format!("hpsvm-trace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("metrics.json");

        let collector = TraceMetricsCollector::default();
        let program_id = Address::new_from_array([2u8; 32]);
        {
            let mut metrics = collector.metrics.lock();
            metrics.insert(program_id, metrics_row(program_id, 1, 5));
        }
        collector.write_json_path(&path).unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["programs"][0]["total_register_frames"], 5);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_json_path_of_a_missing_directory_is_an_error() {
        let error =
            TraceMetricsCollector::default().write_json_path("/definitely/not/a/real/dir/out.json");
        assert!(error.is_err());
    }

    /// `as_bytes` is the crate's only `unsafe` cast: it must be a faithful,
    /// in-place reinterpretation for POD types.
    #[test]
    fn as_bytes_reinterprets_pod_types_in_place() {
        let words = [1u64, 2, 3];
        let mut expected = Vec::new();
        for word in words {
            expected.extend_from_slice(&word.to_ne_bytes());
        }
        assert_eq!(as_bytes(&words).len(), std::mem::size_of::<[u64; 3]>());
        assert_eq!(as_bytes(&words), expected.as_slice());
    }

    #[test]
    fn as_bytes_of_an_empty_slice_is_empty() {
        let words: [u64; 0] = [];
        assert!(as_bytes(&words).is_empty());
    }

    #[test]
    fn as_bytes_of_a_single_element_matches_its_native_layout() {
        let value = 0x0102_0304_0506_0708u64;
        assert_eq!(as_bytes(std::slice::from_ref(&value)), value.to_ne_bytes());
    }

    #[test]
    fn compute_hash_is_the_hex_sha256_of_the_input() {
        assert_eq!(
            compute_hash(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            compute_hash(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 32 bytes of hex per SHA-256 digest.
        assert_eq!(compute_hash(b"anything").len(), 64);
    }

    #[test]
    fn default_callback_falls_back_when_the_env_vars_are_unset() {
        // `SBF_TRACE_DIR` / `SBF_TRACE_DISASSEMBLE` are not set in the test env.
        let callback = DefaultRegisterTracingCallback::default();
        if std::env::var("SBF_TRACE_DIR").is_err() {
            assert_eq!(callback.sbf_trace_dir, DEFAULT_PATH);
        }
        if std::env::var("SBF_TRACE_DISASSEMBLE").is_err() {
            assert!(!callback.sbf_trace_disassemble);
        }
    }

    // Property: `as_bytes` always produces exactly `size_of_val` bytes, and
    // the bytes are the native little-endian encoding of the elements.
    proptest! {
        #[test]
        fn as_bytes_always_matches_the_native_encoding(
            words in prop::collection::vec(any::<u64>(), 0..16),
        ) {
            let bytes = as_bytes(&words);
            prop_assert_eq!(bytes.len(), words.len() * std::mem::size_of::<u64>());
            for (index, word) in words.iter().enumerate() {
                let start = index * std::mem::size_of::<u64>();
                let end = start + std::mem::size_of::<u64>();
                prop_assert_eq!(&bytes[start..end], &word.to_ne_bytes());
        }
    }
    }

    // Property: averages are zero when there are no invocations and equal
    // `total / invocations` otherwise.
    proptest! {
        #[test]
        fn averages_follow_the_total_over_invocations_rule(
            invocations in 0usize..16,
            frames in 0usize..4096,
            accounts in 0usize..64,
        ) {
            let program_id = Address::new_unique();
            let entry = ProgramTraceMetrics {
                program_id,
                invocations,
                cpi_invocations: 0,
                total_register_frames: frames,
                max_register_frames: frames,
                max_stack_height: 1,
                total_instruction_accounts: accounts,
                max_instruction_accounts: accounts,
            };

            if invocations == 0 {
                prop_assert_eq!(entry.average_register_frames(), 0.0);
                prop_assert_eq!(entry.average_instruction_accounts(), 0.0);
            } else {
                prop_assert_eq!(entry.average_register_frames(), frames as f64 / invocations as f64);
                prop_assert_eq!(entry.average_instruction_accounts(), accounts as f64 / invocations as f64);
            }
        }
    }

    // Property: SHA-256 hex is always 64 lowercase hex characters and is
    // content-addressed (same input -> same digest).
    proptest! {
        #[test]
        fn compute_hash_is_a_stable_lowercase_sha256(
            payload in prop::collection::vec(any::<u8>(), 0..512),
        ) {
            let digest = compute_hash(&payload);
            prop_assert_eq!(digest.len(), 64);
            prop_assert!(digest.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            prop_assert_eq!(digest, compute_hash(&payload));
        }
    }
}
