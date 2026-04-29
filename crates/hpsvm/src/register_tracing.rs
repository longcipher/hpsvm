use std::{
    collections::HashMap,
    fs::File,
    io::{self, Write},
    path::Path,
    sync::{Arc, Mutex},
};

use sha2::{Digest, Sha256};
use solana_address::Address;
use solana_program_runtime::invoke_context::{Executable, InvokeContext, RegisterTrace};
use solana_transaction::sanitized::SanitizedTransaction;
use solana_transaction_context::{IndexOfAccount, InstructionContext};

use crate::{HPSVM, InvocationInspectCallback};

const DEFAULT_PATH: &str = "target/sbf/trace";

#[derive(Debug, Clone, PartialEq, Eq)]
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
        let mut metrics = self
            .metrics
            .lock()
            .expect("trace metrics mutex should not be poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        metrics.sort_by(|left, right| {
            right
                .total_register_frames
                .cmp(&left.total_register_frames)
                .then_with(|| right.invocations.cmp(&left.invocations))
                .then_with(|| left.program_id.to_string().cmp(&right.program_id.to_string()))
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

        let mut metrics = self.metrics.lock().expect("trace metrics mutex should not be poisoned");
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

pub fn write_trace_metrics_json(
    writer: &mut impl Write,
    metrics: &[ProgramTraceMetrics],
) -> io::Result<()> {
    writeln!(writer, "{{")?;
    writeln!(writer, "  \"programs\": [")?;
    for (index, metric) in metrics.iter().enumerate() {
        let suffix = if index + 1 == metrics.len() { "" } else { "," };
        writeln!(writer, "    {{")?;
        writeln!(writer, "      \"program_id\": \"{}\",", metric.program_id)?;
        writeln!(writer, "      \"invocations\": {},", metric.invocations)?;
        writeln!(writer, "      \"cpi_invocations\": {},", metric.cpi_invocations)?;
        writeln!(writer, "      \"total_register_frames\": {},", metric.total_register_frames)?;
        writeln!(
            writer,
            "      \"avg_register_frames\": {:.2},",
            metric.average_register_frames()
        )?;
        writeln!(writer, "      \"max_register_frames\": {},", metric.max_register_frames)?;
        writeln!(writer, "      \"max_stack_height\": {},", metric.max_stack_height)?;
        writeln!(
            writer,
            "      \"total_instruction_accounts\": {},",
            metric.total_instruction_accounts
        )?;
        writeln!(
            writer,
            "      \"avg_instruction_accounts\": {:.2},",
            metric.average_instruction_accounts()
        )?;
        writeln!(
            writer,
            "      \"max_instruction_accounts\": {}",
            metric.max_instruction_accounts
        )?;
        writeln!(writer, "    }}{suffix}")?;
    }
    writeln!(writer, "  ]")?;
    writeln!(writer, "}}")
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

pub(crate) fn as_bytes<T>(slice: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
}

fn compute_hash(slice: &[u8]) -> String {
    hex::encode(Sha256::digest(slice).as_slice())
}
