Feature: Transaction errors
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
