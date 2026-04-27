Feature: Feature set reconfiguration
  Scenario: Rebuilding materialized defaults after switching feature sets
    Given a default HPSVM instance with all features materialized
    When the VM feature set is replaced with the default disabled feature set
    Then the old active feature account should be removed
    And the SPL token program should use the legacy loader
