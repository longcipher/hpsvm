Feature: Fee payer visibility
  Scenario: Fee payer is recorded in ExecutionResult on successful transactions
    Given a default HPSVM instance with all features materialized
    And a funded sender account
    When a successful system transfer is executed via transact
    Then the ExecutionOutcome should contain the fee payer address
    And the fee payer should match the transaction fee payer
