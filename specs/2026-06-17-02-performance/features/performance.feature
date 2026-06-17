Feature: Batch execution performance
  As a developer running high-throughput test suites
  I want batch execution to minimize cloning and unnecessary work
  So that test suites complete faster

  @finding-5 @performance
  Scenario: Batch workers share AccountsDb via Arc instead of cloning
    Given a batch stage with N conflict-free transactions
    When the batch stage executes in parallel
    Then each worker should receive a shared reference to the AccountsDb
    And each worker should produce a delta of modified accounts
    And the staging loop should merge deltas back

  @finding-6 @performance
  Scenario: Transaction diagnostics are computed only when requested
    Given a default HPSVM instance
    When a transaction is executed without diagnostics enabled
    Then the execution should not compute pre/post account diffs
    And the execution should not compute token balances
    When diagnostics are explicitly requested
    Then the full diagnostics should be computed
