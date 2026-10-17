# hpsvm-fd-fixture

Fork of [`mollusk-svm-fuzz-fixture-firedancer`](https://crates.io/crates/mollusk-svm-fuzz-fixture-firedancer) 0.16.2
(Anza, Apache-2.0), maintained by the
[hpsvm](https://github.com/longcipher/hpsvm) project.

**Why it exists.** Upstream pins `agave-feature-set`, `solana-account`, and
`solana-transaction-context` to `4.3.0`, which cannot resolve against the agave
`4.5.0-alpha` line: a semver range excludes pre-releases, and Cargo will not
keep two semver-compatible copies of the same crate. Every published upstream
version (through 0.16.2) is affected.

**What changed.** The three Solana runtime requirements (`agave-feature-set`,
`solana-account`, `solana-transaction-context`), widened so the crate resolves
against the agave `4.5.0-alpha` line, plus one import path (`InstructionAccount`
moved under `instruction_accounts::` in `solana-transaction-context` 4.x). No
logic was modified — the symbol table in `vendor/PATCHES.md` verifies every
symbol the crate uses still exists on the 4.5 line.

**Who consumes it.** `hpsvm-fixture`'s optional `fd-codec` feature (and
`hpsvm-cli`'s `fd-compat` feature on top of it). Nothing in the default feature
set depends on this crate.

**Maintenance.** Source lives in `vendor/hpsvm-fd-fixture` of the hpsvm
repository; see `vendor/PATCHES.md` for how to refresh it from a new upstream
release and when to retire it.
