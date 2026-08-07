Feature: What the collector writes and what it tells the operator

  The collector is judged by two artifacts: the segments on disk and the lines
  an operator reads in the log. Every scenario runs the real binary.

  Scenario: A sealed segment lands on its UTC calendar path
    Given a collector that seals on every tick
    When it runs for 3 seconds
    Then a segment exists under a YYYY/MM/DD directory
    And every published segment file ends in .zms
    And the raw journal is named active.wal

  Scenario: Sealing reports where the segment went and what it cost
    Given a collector that seals on every tick
    When it runs for 3 seconds
    Then the log has a segment_seal_finish line
    And that line names the segment path, the reason, the section count, the byte size, and the elapsed time

  Scenario: A metric that cannot be collected is reported, not invented
    Given a procfs fixture without meminfo
    And a collector that seals on every tick
    When it runs for 3 seconds
    Then the log reports os_meminfo as degraded
    And the log still reports a sealed segment

  Scenario: Stopping the collector leaves the journal in place
    Given a collector that keeps one open segment
    When it runs for 3 seconds
    Then the raw journal is named active.wal
    And the log has no error line
