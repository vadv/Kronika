Feature: What the collector writes and what it tells the operator

  The collector is judged by two artifacts: the segments on disk and the lines
  an operator reads in the log. Every scenario runs the real binary.

  Scenario: A finished segment lands on its UTC calendar path
    Given a collector that writes on every tick
    When it runs for 3 seconds
    Then a segment exists under a YYYY/MM/DD directory
    And every published segment file ends in .zms
    And the raw journal is named active.wal

  Scenario: Writing reports where the segment went and what it cost
    Given a collector that writes on every tick
    When it runs for 3 seconds
    Then the log has a segment_write_finish line
    And that line names the segment path, the reason, the section count, the byte size, and the elapsed time

  Scenario: A metric that cannot be collected is reported, not invented
    Given a procfs fixture without meminfo
    And a collector that writes on every tick
    When it runs for 3 seconds
    Then the log reports os_meminfo as degraded
    And the log still reports a segment written

  Scenario: Stopping the collector leaves the journal in place
    Given a collector that keeps one open segment
    When it runs for 3 seconds
    Then the raw journal is named active.wal
    And the log has no error line

  Scenario: A journal the collector cannot read is moved aside, not fatal
    Given a data root whose journal is corrupt
    When the collector runs for 3 seconds
    Then the log reports the journal as damaged
    And the corrupt bytes are kept next to the journal
    And a segment exists under a YYYY/MM/DD directory

  Scenario: Windows written before the damage still reach a segment
    Given a collector that keeps one open segment
    When it runs for 3 seconds
    And its journal is cut short
    And the collector runs again for 3 seconds
    Then the log reports the journal as damaged
    And a segment exists under a YYYY/MM/DD directory
    And that segment was written from the salvaged windows
