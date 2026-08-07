Feature: A journal the collector cannot read

  Whatever the reason a journal will not open, the collector keeps the frames
  that still read, sets the file aside and carries on. The proof is the segment
  it writes afterwards, not the line it logged about it.

  Scenario: A journal that will not open is moved aside, not fatal
    Given a data root whose active.wal is the journal magic followed by "not-a-valid-header"
    And a collector with these settings
      | variable                  | value |
      | KRONIKA_INTERVAL_S        | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES | 0     |
    When it runs for 3 seconds
    Then the log reports the journal as damaged
    And the damaged journal is kept as active.wal.damaged
    And a segment exists under a YYYY/MM/DD directory

  Scenario: Windows written before the damage still reach a segment
    Given a collector with these settings
      | variable                    | value      |
      | KRONIKA_INTERVAL_S          | 1          |
      | KRONIKA_SEGMENT_MAX_BYTES   | 1073741824 |
      | KRONIKA_SEGMENT_MAX_AGE_S   | 86400      |
      | KRONIKA_INSTANCE_INTERVAL_S | 0          |
      | KRONIKA_OS_CORE_INTERVAL_S  | 0          |
    When it runs for 4 seconds
    And the last 200 bytes of the journal are cut off
    And it starts again over the same data root and runs for 3 seconds
    Then the log reports the journal as damaged
    And some segment holds these sections
      | type_id | section           | min rows |
      | 1021001 | instance_metadata | 1        |
      | 1102001 | os_cpu            | 2        |
      | 1105001 | os_loadavg        | 1        |
