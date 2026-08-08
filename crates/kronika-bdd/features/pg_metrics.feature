Feature: What a PostgreSQL server reports reaches the segment

  A real server answers the queries and the rows are asserted against the
  segment on disk, decoded back through the format, with dictionary text
  resolved.

  The section ids below name the layouts of the server this image installs
  (PostgreSQL 15). A different server major writes different layouts, and this
  scenario is meant to fail loudly rather than pass against a shape nobody
  asserted.

  Scenario: A live server's statistics become typed rows
    Given a PostgreSQL writing its log as stderr
    And the collector reaches PostgreSQL by DSN
    And a collector with these settings
      | variable                             | value |
      | KRONIKA_INTERVAL_S                   | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES            | 1     |
      | KRONIKA_PG_INTERVAL_S                | 0     |
      | KRONIKA_PG_RELATIONS_INTERVAL_S      | 0     |
      | KRONIKA_LOG_INTERVAL_S               | 3600  |
      | KRONIKA_OS_CORE_INTERVAL_S           | 3600  |
      | KRONIKA_OS_MOUNTTOPO_INTERVAL_S      | 3600  |
      | KRONIKA_OS_PROCESS_INTERVAL_S        | 3600  |
      | KRONIKA_OS_PROCESS_STATUS_INTERVAL_S | 3600  |
      | KRONIKA_OS_CGROUP_INTERVAL_S         | 3600  |
      | KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S | 3600  |
    When these statements run against PostgreSQL
      | statement                                                      |
      | create table bdd_orders (id bigint primary key, payload text)  |
      | insert into bdd_orders select i, repeat('x', 64) from generate_series(1, 500) i |
      | analyze bdd_orders                                             |
      | select count(*) from bdd_orders where id < 100                 |
    And it runs for 4 seconds
    Then some segment holds these sections
      | type_id | section                | min rows |
      | 1019001 | pg_settings            | 100      |
      | 1008001 | pg_stat_archiver       | 1        |
      | 1007001 | pg_stat_wal            | 1        |
      | 1005003 | pg_stat_database       | 1        |
      | 1001003 | pg_stat_activity       | 1        |
      | 1013002 | pg_stat_user_tables    | 1        |
      | 1014001 | pg_stat_user_indexes   | 1        |
    And some segment records these rows
      | type_id | column        | value          |
      | 1019001 | name          | shared_buffers |
      | 1005003 | datname       | postgres       |
      | 1013002 | relname       | bdd_orders     |
      | 1013002 | schemaname    | public         |
      | 1014001 | indexrelname  | bdd_orders_pkey |
      | 1014001 | amname        | btree          |

  Scenario: Without a configured server no PostgreSQL section is written
    Given a collector with these settings
      | variable                        | value |
      | KRONIKA_INTERVAL_S              | 1     |
      | KRONIKA_SEGMENT_MAX_BYTES       | 1     |
      | KRONIKA_PG_INTERVAL_S           | 0     |
      | KRONIKA_PG_RELATIONS_INTERVAL_S | 0     |
    When it runs for 3 seconds
    Then no segment holds these sections
      | type_id | section             |
      | 1019001 | pg_settings         |
      | 1005003 | pg_stat_database    |
      | 1013002 | pg_stat_user_tables |
