use hpsvm::HPSVM;
use solana_address::address;

// https://github.com/HPSVM/litesvm/issues/140
#[test]
fn test_dflow_load() {
    let mut svm = HPSVM::new();
    let program_bytes =
        include_bytes!("../test_programs/DF1ow3DqMj3HvTj8i8J9yM2hE9hCrLLXpdbaKZu4ZPnz.so");
    svm.add_program(address!("DF1ow3DqMj3HvTj8i8J9yM2hE9hCrLLXpdbaKZu4ZPnz"), program_bytes)
        .unwrap();
}
