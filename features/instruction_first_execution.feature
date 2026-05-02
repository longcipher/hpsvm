Feature: Instruction-first execution
  Scenario: Processing one instruction commits state and records diagnostics
    Given a default HPSVM instance with all features materialized
    When a direct system transfer instruction is processed
    Then the recipient account should be committed
    And the instruction metadata should include account diagnostics
