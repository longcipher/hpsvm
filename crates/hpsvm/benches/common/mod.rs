#![allow(dead_code)]

use std::{env, path::PathBuf};

use hpsvm::HPSVM;
#[cfg(feature = "register-tracing")]
use hpsvm::register_tracing::TraceMetricsCollector;
use solana_account::Account;
use solana_address::Address;
use solana_instruction::{Instruction, account_meta::AccountMeta};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_transaction::Transaction;

const COUNTER_PROGRAM_RELATIVE_PATH: &str = "test_programs/target/deploy/counter.so";
const HOTPATH_ENV_VAR: &str = "HPSVM_HOTPATH";
const HOTPATH_LIMIT_ENV_VAR: &str = "HPSVM_HOTPATH_LIMIT";
const TRACE_METRICS_ENV_VAR: &str = "HPSVM_TRACE_METRICS";

pub struct HotpathGuard {
    #[cfg(feature = "hotpath")]
    _inner: Option<hotpath::HotpathGuard>,
}

impl HotpathGuard {
    pub fn new(name: &'static str) -> Self {
        #[cfg(feature = "hotpath")]
        {
            if env::var_os(HOTPATH_ENV_VAR).is_none() {
                return Self { _inner: None };
            }

            let limit = env::var(HOTPATH_LIMIT_ENV_VAR)
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(20);
            let output_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/hotpath")
                .join(format!("{name}.json"));
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent)
                    .expect("hotpath benchmark output directory should be creatable");
            }
            let guard = hotpath::HotpathGuardBuilder::new(name)
                .percentiles(&[50.0, 95.0, 99.0])
                .format(hotpath::Format::JsonPretty)
                .output_path(&output_path)
                .limit(limit)
                .build();

            return Self { _inner: Some(guard) };
        }

        #[cfg(not(feature = "hotpath"))]
        {
            let _ = name;
            Self {}
        }
    }
}

pub struct TraceMetricsGuard {
    #[cfg(feature = "register-tracing")]
    collector: Option<TraceMetricsCollector>,
    #[cfg(feature = "register-tracing")]
    output_path: Option<PathBuf>,
}

impl TraceMetricsGuard {
    pub fn new(name: &'static str) -> Self {
        #[cfg(feature = "register-tracing")]
        {
            if env::var_os(TRACE_METRICS_ENV_VAR).is_none() {
                return Self { collector: None, output_path: None };
            }

            let output_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/hotpath")
                .join(format!("{name}.trace.json"));
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent)
                    .expect("trace metrics output directory should be creatable");
            }

            return Self {
                collector: Some(TraceMetricsCollector::default()),
                output_path: Some(output_path),
            };
        }

        #[cfg(not(feature = "register-tracing"))]
        {
            let _ = name;
            Self {}
        }
    }

    pub fn install(&self, svm: &mut HPSVM) {
        #[cfg(feature = "register-tracing")]
        if let Some(collector) = &self.collector {
            svm.set_invocation_inspect_callback(collector.clone());
        }

        #[cfg(not(feature = "register-tracing"))]
        let _ = svm;
    }
}

impl Drop for TraceMetricsGuard {
    fn drop(&mut self) {
        #[cfg(feature = "register-tracing")]
        if let (Some(collector), Some(output_path)) = (&self.collector, &self.output_path) {
            collector
                .write_json_path(output_path)
                .expect("trace metrics output should be writable");
        }
    }
}

pub fn new_benchmark_vm() -> HPSVM {
    #[cfg(feature = "register-tracing")]
    let mut svm = if env::var_os(TRACE_METRICS_ENV_VAR).is_some() {
        HPSVM::new_debuggable(true)
    } else {
        HPSVM::new()
    };

    #[cfg(not(feature = "register-tracing"))]
    let mut svm = HPSVM::new();

    svm.set_blockhash_check(false);
    svm.set_sigverify(false);
    svm.set_transaction_history(0);
    svm
}

pub fn counter_program_path() -> PathBuf {
    let mut so_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    so_path.push(COUNTER_PROGRAM_RELATIVE_PATH);
    so_path
}

pub fn read_counter_program() -> Vec<u8> {
    std::fs::read(counter_program_path()).expect("counter program bytes should be available")
}

pub fn make_counter_tx(
    program_id: Address,
    counter_address: Address,
    payer_pk: &Address,
    blockhash: solana_hash::Hash,
    payer_kp: &Keypair,
    deduper: u8,
) -> Transaction {
    let msg = Message::new_with_blockhash(
        &[Instruction {
            program_id,
            accounts: vec![AccountMeta::new(counter_address, false)],
            data: vec![0, deduper],
        }],
        Some(payer_pk),
        &blockhash,
    );
    Transaction::new(&[payer_kp], msg, blockhash)
}

pub fn counter_account(program_id: Address) -> Account {
    Account {
        lamports: 5,
        data: vec![0_u8; std::mem::size_of::<u32>()],
        owner: program_id,
        ..Default::default()
    }
}
