Feature: Quick correctness and tooling fixes
  As a developer using hpsvm
  I want correctness bugs fixed and tooling gaps closed
  So that the codebase is reliable and onboarding is smooth

  @finding-1 @correctness
  Scenario: Fee payer is recorded in ExecutionResult on successful transactions
    Given a default HPSVM instance
    And a funded sender account
    When a successful system transfer is executed
    Then the ExecutionResult should contain the fee payer address
    And the fee payer should match the transaction fee payer

  @finding-2 @performance
  Scenario: Pre-rent-state lookups are batched for external account source
    Given a default HPSVM with an external account source
    And a transaction touching multiple writable accounts
    When the rent state check runs
    Then the external account source should receive a single batch fetch call
    And not one call per writable account

  @finding-3 @DX
  Scenario: Justfile setup installs all required tools
    Given a fresh development environment
    When the setup recipe runs
    Then rumdl should be available
    And cargo-tarpaulin should be available

  @finding-4 @DX
  Scenario: CI does not run duplicate test steps
    Given the CI workflow file
    When inspecting the test steps
    Then cargo test --features precompiles should not appear separately from just ci
    And SPL token tests should not duplicate what just ci covers

  @finding-12 @correctness
  Scenario: Unsafe as_bytes transmute has safety documentation
    Given the register_tracing module
    When reviewing the as_bytes function
    Then it should have a SAFETY comment explaining invariants

  @finding-13 @security
  Scenario: register_tracing validates SBF_TRACE_DIR path
    Given the DefaultRegisterTracingCallback
    When SBF_TRACE_DIR is set to an absolute system path
    Then the handler should reject paths outside the working directory

  @finding-14 @dependency
  Scenario: ansi_term replaced with anstyle
    Given the format_logs module
    When formatting transaction logs
    Then anstyle should be used instead of ansi_term
    And color output should be identical

  @finding-15 @dependency
  Scenario: serde_yaml replaced with serde_yml
    Given the CLI config loading code
    When parsing a YAML configuration file
    Then serde_yml should be used instead of serde_yaml

  @finding-16 @dependency
  Scenario: ed25519-dalek updated to v2
    Given the precompiles test suite
    When running ed25519 signature verification tests
    Then ed25519-dalek v2 should be used
    And all tests should pass

  @finding-18 @correctness
  Scenario: Loader chunk offset uses u64 to prevent overflow
    Given the load_upgradeable_buffer function
    When deploying a program larger than 4GB worth of chunks
    Then the offset should not wrap to zero
    And an error should be returned for oversized programs
