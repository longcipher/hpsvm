Feature: Test coverage for critical paths
  As a developer maintaining hpsvm
  I want BDD and unit test coverage for critical paths
  So that regressions are caught automatically

  @finding-7 @test-coverage
  Scenario: Transaction compute budget exceeded error is handled
    Given a default HPSVM instance
    And a transaction that exceeds the compute budget
    When the transaction is executed
    Then the result should be an error
    And the error should indicate compute budget exceeded

  @finding-7 @test-coverage
  Scenario: Transaction with insufficient funds fails gracefully
    Given a default HPSVM instance
    And a sender account with zero lamports
    When a transfer instruction is executed
    Then the result should be an error
    And the error should indicate insufficient funds

  @finding-7 @test-coverage
  Scenario: Transaction with invalid program fails gracefully
    Given a default HPSVM instance
    And a transaction targeting a non-existent program
    When the transaction is executed
    Then the result should be an error
    And the error should indicate invalid program for instruction

  @finding-7 @test-coverage
  Scenario: Transaction with expired blockhash fails
    Given a default HPSVM instance
    And a transaction with an expired blockhash
    When the transaction is executed
    Then the result should be an error
    And the error should indicate blockhash not found

  @finding-17 @test-coverage
  Scenario: RentPaying account cannot be credited
    Given a RentPaying account with 1000 lamports
    When a credit transition is attempted
    Then the transition should be rejected

  @finding-17 @test-coverage
  Scenario: RentPaying account can be debited
    Given a RentPaying account with 1000 lamports
    When a debit transition is attempted
    Then the transition should be allowed

  @finding-17 @test-coverage
  Scenario: Any state can transition to RentExempt
    Given an account in any rent state
    When transitioning to RentExempt
    Then the transition should be allowed

  @finding-17 @test-coverage
  Scenario: Incinerator address bypasses rent state checks
    Given the incinerator address
    When any rent state transition is attempted
    Then the transition should be allowed
