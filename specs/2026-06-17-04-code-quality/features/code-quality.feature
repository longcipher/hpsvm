Feature: Code quality and documentation
  As a contributor to hpsvm
  I want clean code structure and complete documentation
  So that the codebase is maintainable and the public API is usable

  @finding-8 @tech-debt
  Scenario: Transaction execution logic is extracted from lib.rs
    Given the 2880-line lib.rs module
    When the execution pipeline is extracted
    Then execute_sanitized_transaction, process_transaction, check_and_process_transaction
    And sanitize_transaction variants
    And free functions (validate_fee_payer, execution_diagnostics, etc.)
    Should live in separate modules
    And lib.rs should contain only the HPSVM struct and its public API methods

  @finding-9 @tech-debt
  Scenario: Duplicate execute_sanitized_transaction methods are unified
    Given execute_sanitized_transaction and execute_sanitized_transaction_readonly
    When reviewing their implementations
    Then they should share a single implementation
    And the mutable/readonly distinction should be handled at the call site

  @finding-10 @tech-debt
  Scenario: Token builder send() boilerplate is deduplicated
    Given the 20+ token instruction builders
    When each builder's send() method is reviewed
    Then a shared sign_and_send helper should handle common transaction construction
    And each builder should delegate to the helper

  @finding-11 @docs
  Scenario: Public types in types.rs have complete documentation
    Given the types.rs module
    When the #[expect(missing_docs)] attributes are removed
    Then each public struct and field should have doc comments
    And the doc comments should describe the field's purpose and constraints
