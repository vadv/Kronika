use std::collections::{HashMap, HashSet};

use kronika_registry::{ColumnClass, registry};
use serde_json::json;

use super::{
    ConversionContext, ConversionContextBuilder, Entity, FiniteValue, I64String, Metric,
    MetricClass, MetricUnit, Ranking, RawQuery, RelationLevel, Surface, Top, U64String,
    metric_definitions, normalize, surface_definitions,
};

const HOUR: &str = "1699999200000000";

fn raw(surface: Surface, metric: Option<Metric>, level: Option<RelationLevel>) -> RawQuery {
    RawQuery {
        hour: HOUR.to_owned(),
        surface: surface.as_str().to_owned(),
        metric: metric.map(|value| value.as_str().to_owned()),
        level: level.map(|value| value.as_str().to_owned()),
        top: None,
    }
}

#[test]
fn registry_has_exact_shipped_matrix() {
    assert_eq!(surface_definitions().len(), 8);
    assert_eq!(metric_definitions().len(), 37);
    let pairs: HashSet<_> = metric_definitions()
        .iter()
        .map(|definition| (definition.surface, definition.metric))
        .collect();
    assert_eq!(pairs.len(), 37);
    assert_eq!(Metric::ALL.len(), 32);
    assert_eq!(
        metric_definitions()
            .iter()
            .filter(|definition| definition.class == MetricClass::Cumulative)
            .count(),
        35
    );
    assert_eq!(
        metric_definitions()
            .iter()
            .filter(|definition| definition.class == MetricClass::Gauge)
            .count(),
        2
    );

    let counts: HashMap<_, _> = Surface::ALL
        .into_iter()
        .map(|surface| {
            let count = metric_definitions()
                .iter()
                .filter(|definition| definition.surface == surface)
                .count();
            (surface, count)
        })
        .collect();
    assert_eq!(counts[&Surface::PostgreSqlStatements], 7);
    assert_eq!(counts[&Surface::PostgreSqlPlans], 5);
    assert_eq!(counts[&Surface::PostgreSqlTables], 5);
    assert_eq!(counts[&Surface::PostgreSqlIndexes], 3);
    assert_eq!(counts[&Surface::Processes], 6);
    assert_eq!(counts[&Surface::PostgreSqlDatabases], 5);
    assert_eq!(counts[&Surface::CgroupCpu], 2);
    assert_eq!(counts[&Surface::CgroupIo], 4);
}

#[test]
fn all_valid_pairs_normalize_and_all_cross_surface_pairs_fail() {
    let valid: HashSet<_> = metric_definitions()
        .iter()
        .map(|definition| (definition.surface, definition.metric))
        .collect();
    let mut accepted = 0;
    let mut rejected = 0;
    for surface in Surface::ALL {
        for metric in Metric::ALL {
            let result = normalize(raw(surface, Some(metric), None));
            if valid.contains(&(surface, metric)) {
                let query = result.expect("registered pair must normalize");
                assert_eq!(query.selection().surface(), surface);
                assert_eq!(query.selection().metric(), metric);
                accepted += 1;
            } else {
                let error = result.expect_err("cross-surface pair must fail");
                assert_eq!(
                    error.message(),
                    format!("metric {metric} is not valid for surface {surface}")
                );
                rejected += 1;
            }
        }
    }
    assert_eq!(accepted, 37);
    assert_eq!(rejected, 219);
}

#[test]
fn level_expansion_is_exactly_sixty_one_selections() {
    let mut selections = HashSet::new();
    for definition in metric_definitions() {
        let levels: &[Option<RelationLevel>] = if matches!(
            definition.surface,
            Surface::PostgreSqlTables | Surface::PostgreSqlIndexes
        ) {
            &[
                Some(RelationLevel::Object),
                Some(RelationLevel::Schema),
                Some(RelationLevel::Database),
                Some(RelationLevel::Tablespace),
            ]
        } else {
            &[None]
        };
        for level in levels {
            let query = normalize(raw(definition.surface, Some(definition.metric), *level))
                .expect("registered selection must normalize");
            selections.insert(query.selection());
        }
    }
    assert_eq!(selections.len(), 61);
    assert_eq!(selections.len() * Top::ALL.len(), 244);
}

#[test]
fn defaults_match_every_surface() {
    let expected = [
        (Surface::PostgreSqlStatements, Metric::ExecTime, None),
        (Surface::PostgreSqlPlans, Metric::ExecTime, None),
        (
            Surface::PostgreSqlTables,
            Metric::Writes,
            Some(RelationLevel::Object),
        ),
        (
            Surface::PostgreSqlIndexes,
            Metric::IdxScan,
            Some(RelationLevel::Object),
        ),
        (Surface::Processes, Metric::Cpu, None),
        (Surface::PostgreSqlDatabases, Metric::Commits, None),
        (Surface::CgroupCpu, Metric::CgCpu, None),
        (Surface::CgroupIo, Metric::CgRead, None),
    ];
    for (surface, metric, level) in expected {
        let query = normalize(raw(surface, None, None)).expect("default must normalize");
        assert_eq!(query.selection().metric(), metric);
        assert_eq!(query.selection().level(), level);
        assert_eq!(query.top(), Top::TwentyFive);
    }
}

#[test]
fn top_choices_are_closed() {
    let mut accepted = 0;
    for surface in Surface::ALL {
        for top in Top::ALL {
            let mut input = raw(surface, None, None);
            input.top = Some(top.get() as i64);
            assert_eq!(normalize(input).expect("shipped top").top(), top);
            accepted += 1;
        }
    }
    assert_eq!(accepted, 32);
    for invalid in [i64::MIN, -1, 0, 9, 11, 24, 26, 99, 101, i64::MAX] {
        let mut input = raw(Surface::CgroupCpu, None, None);
        input.top = Some(invalid);
        assert!(normalize(input).is_err(), "top {invalid} must fail");
    }
}

#[test]
fn omitted_optional_inputs_default_but_explicit_null_is_invalid() {
    let omitted = serde_json::from_value::<RawQuery>(json!({
        "hour": HOUR,
        "surface": "cgroup_cpu"
    }))
    .expect("omitted optionals");
    assert_eq!(normalize(omitted).expect("defaults").top(), Top::TwentyFive);

    for property in ["metric", "level", "top"] {
        let mut value = json!({
            "hour": HOUR,
            "surface": "cgroup_cpu"
        });
        value[property] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<RawQuery>(value).is_err(),
            "explicit null {property} must not act like omission"
        );
    }
}

#[test]
fn hour_is_canonical_aligned_and_checked() {
    for invalid in [
        "",
        "00",
        "01",
        "-0",
        "+0",
        " 0",
        "0 ",
        "3600000001",
        "9223372036854775808",
        "-9223372036854775809",
        "9223372036800000000",
    ] {
        let mut input = raw(Surface::CgroupCpu, None, None);
        input.hour = invalid.to_owned();
        assert!(normalize(input).is_err(), "hour {invalid:?} must fail");
    }

    for valid in [
        "-9223372036800000000",
        "-3600000000",
        "0",
        "3600000000",
        HOUR,
        "9223372033200000000",
    ] {
        let mut input = raw(Surface::CgroupCpu, None, None);
        input.hour = valid.to_owned();
        let query = normalize(input).expect("aligned canonical hour");
        assert_eq!(query.hour().start().to_string(), valid);
        assert_eq!(query.hour().end(), query.hour().start() + 3_599_999_999);
    }
}

#[test]
fn levels_are_relation_only() {
    for surface in [
        Surface::PostgreSqlStatements,
        Surface::PostgreSqlPlans,
        Surface::Processes,
        Surface::PostgreSqlDatabases,
        Surface::CgroupCpu,
        Surface::CgroupIo,
    ] {
        for level in RelationLevel::ALL {
            assert!(normalize(raw(surface, None, Some(level))).is_err());
        }
    }
}

#[test]
fn physical_recipes_and_layout_availability_match_registry() {
    for definition in metric_definitions() {
        let query = normalize(raw(definition.surface, Some(definition.metric), None))
            .expect("registered pair");
        let recipe = query.recipe().expect("registered recipe");
        assert!(!recipe.section.is_empty());
        assert!(!recipe.fields.is_empty());
        assert_eq!(recipe.surface, definition.surface);
        assert_eq!(recipe.metric, definition.metric);
        assert_eq!(recipe.class, definition.class);
        assert_eq!(recipe.fields, definition.fields);
        assert!(
            registry()
                .iter()
                .any(|contract| recipe.supports_layout(contract)),
            "{}/{} has no compatible stored layout",
            definition.surface,
            definition.metric
        );
        for contract in registry()
            .iter()
            .filter(|contract| recipe.supports_layout(contract))
        {
            for field in recipe.fields {
                let Some(column) = contract.column(field) else {
                    continue;
                };
                let expected = match recipe.class {
                    MetricClass::Cumulative => ColumnClass::Cumulative,
                    MetricClass::Gauge => ColumnClass::Gauge,
                };
                assert_eq!(column.class, expected, "{field} class in stored layout");
            }
            for name in recipe.labels.iter().chain(recipe.groups) {
                assert!(
                    contract.column(name).is_some(),
                    "{name} must exist in stored layout {}",
                    contract.type_id.get()
                );
            }
        }
    }

    let statements = normalize(raw(
        Surface::PostgreSqlStatements,
        Some(Metric::ExecTime),
        None,
    ))
    .expect("statement execution time")
    .recipe()
    .expect("recipe");
    let statement_layouts: Vec<_> = registry()
        .iter()
        .filter(|contract| statements.supports_layout(contract))
        .map(|contract| contract.type_id.get())
        .collect();
    assert_eq!(
        statement_layouts,
        [1_002_006, 1_002_005, 1_002_004, 1_002_003, 1_002_002]
    );

    let autovacuum = normalize(raw(
        Surface::PostgreSqlTables,
        Some(Metric::AutovacuumTime),
        None,
    ))
    .expect("autovacuum time")
    .recipe()
    .expect("recipe");
    let table_layouts: Vec<_> = registry()
        .iter()
        .filter(|contract| autovacuum.supports_layout(contract))
        .map(|contract| contract.type_id.get())
        .collect();
    assert_eq!(table_layouts, [1_013_008]);
}

#[test]
fn conversion_context_chooses_latest_usable_values_within_hour_end() {
    let hour = normalize(raw(Surface::Processes, None, None))
        .expect("query")
        .hour();
    let mut builder = ConversionContextBuilder::new(hour);
    builder.observe_block_size(hour.end() - 10, Some(4_096));
    builder.observe_block_size(hour.end() - 5, Some(8_192));
    builder.observe_block_size(hour.end() - 8, Some(2_048));
    builder.observe_block_size(hour.end() - 1, Some(0));
    builder.observe_block_size(hour.end(), None);
    builder.observe_block_size(hour.end() + 1, Some(16_384));
    builder.observe_clock_ticks_per_sec(hour.end() - 10, Some(100));
    builder.observe_clock_ticks_per_sec(hour.end() - 5, Some(250));
    builder.observe_clock_ticks_per_sec(hour.end() - 8, Some(50));
    builder.observe_clock_ticks_per_sec(hour.end() + 1, Some(1_000));
    let context = builder.finish();
    assert_eq!(context.block_size(), Some(8_192));
    assert_eq!(context.clock_ticks_per_sec(), Some(250));
}

#[test]
fn invalid_latest_metadata_falls_back_to_earlier_usable_value() {
    let hour = normalize(raw(Surface::Processes, None, None))
        .expect("query")
        .hour();
    let mut builder = ConversionContextBuilder::new(hour);
    builder.observe_block_size(hour.end() - 30, Some(8_192));
    builder.observe_block_size(hour.end() - 20, None);
    builder.observe_block_size(hour.end() - 10, None);
    builder.observe_clock_ticks_per_sec(hour.end() - 30, Some(100));
    builder.observe_clock_ticks_per_sec(hour.end() - 20, Some(0));
    builder.observe_clock_ticks_per_sec(hour.end() - 10, None);
    let context = builder.finish();
    assert_eq!(context.block_size(), Some(8_192));
    assert_eq!(context.clock_ticks_per_sec(), Some(100));
}

#[test]
fn absent_conversion_metadata_preserves_raw_values_and_units() {
    let block_recipe = normalize(raw(
        Surface::PostgreSqlStatements,
        Some(Metric::SharedRead),
        None,
    ))
    .expect("block metric")
    .recipe()
    .expect("recipe");
    let raw_blocks = block_recipe.resolve(ConversionContext::default());
    assert_eq!(raw_blocks.scale(42.0), Ok(42.0));
    assert_eq!(raw_blocks.definition.cell_unit, MetricUnit::CountPerSecond);
    assert_eq!(raw_blocks.definition.total_unit, MetricUnit::Count);

    let cpu_recipe = normalize(raw(Surface::Processes, Some(Metric::Cpu), None))
        .expect("cpu metric")
        .recipe()
        .expect("recipe");
    let raw_ticks = cpu_recipe.resolve(ConversionContext::default());
    assert_eq!(raw_ticks.scale(250.0), Ok(250.0));
    assert_eq!(raw_ticks.definition.cell_unit, MetricUnit::CountPerSecond);
    assert_eq!(raw_ticks.definition.total_unit, MetricUnit::Count);
    assert!(
        raw_ticks
            .definition
            .cell_formula
            .starts_with("Sum of usable member raw nonnegative endpoint tick deltas"),
        "grouped raw CPU cells must describe aggregation across members"
    );
}

#[test]
fn available_conversion_metadata_changes_values_units_and_formulas() {
    let hour = normalize(raw(Surface::Processes, None, None))
        .expect("query")
        .hour();
    let mut builder = ConversionContextBuilder::new(hour);
    builder.observe_block_size(hour.end(), Some(8_192));
    builder.observe_clock_ticks_per_sec(hour.end(), Some(250));
    let context = builder.finish();

    let block_metric = normalize(raw(
        Surface::PostgreSqlStatements,
        Some(Metric::SharedRead),
        None,
    ))
    .expect("block metric")
    .recipe()
    .expect("recipe")
    .resolve(context);
    assert_eq!(block_metric.scale(2.0), Ok(16_384.0));
    assert_eq!(
        block_metric.definition.cell_unit,
        MetricUnit::BytesPerSecond
    );
    assert_eq!(block_metric.definition.total_unit, MetricUnit::Bytes);
    assert!(
        block_metric
            .definition
            .total_formula
            .contains("latest usable recorded block size at or before hour_end"),
        "block total formula must state its conversion"
    );

    let cpu_metric = normalize(raw(Surface::Processes, Some(Metric::Cpu), None))
        .expect("CPU metric")
        .recipe()
        .expect("recipe")
        .resolve(context);
    assert_eq!(cpu_metric.scale(250.0), Ok(1.0));
    assert_eq!(
        cpu_metric.definition.cell_unit,
        MetricUnit::SecondsPerSecond
    );
    assert_eq!(cpu_metric.definition.total_unit, MetricUnit::Seconds);
    assert!(
        cpu_metric
            .definition
            .total_formula
            .contains("latest usable recorded clock rate at or before hour_end"),
        "CPU total formula must state its conversion"
    );
    assert!(
        cpu_metric
            .definition
            .cell_formula
            .starts_with("Sum of usable member nonnegative endpoint tick deltas"),
        "grouped CPU cells must describe aggregation across members"
    );

    let rss_metric = normalize(raw(Surface::Processes, Some(Metric::Rss), None))
        .expect("RSS metric")
        .recipe()
        .expect("recipe")
        .resolve(context);
    assert_eq!(rss_metric.scale(1.0), Ok(1_024.0));
    assert!(
        rss_metric.definition.total_formula.contains("1024"),
        "RSS total formula must state its conversion"
    );
    assert!(
        rss_metric
            .definition
            .cell_formula
            .starts_with("Sum of each member's last usable reading"),
        "grouped RSS cells must describe aggregation across members"
    );

    let grouped_blocks = normalize(raw(
        Surface::PostgreSqlTables,
        Some(Metric::HeapRead),
        Some(RelationLevel::Schema),
    ))
    .expect("grouped block metric")
    .recipe()
    .expect("recipe")
    .resolve(context);
    assert!(
        grouped_blocks
            .definition
            .cell_formula
            .starts_with("Sum of usable member nonnegative endpoint block deltas"),
        "grouped block cells must describe aggregation across members"
    );
}

#[test]
fn non_finite_raw_or_converted_values_are_explicit_failures() {
    let raw_metric = normalize(raw(Surface::CgroupCpu, None, None))
        .expect("raw metric")
        .recipe()
        .expect("recipe")
        .resolve(ConversionContext::default());
    assert!(
        raw_metric.scale(f64::INFINITY).is_err(),
        "stored infinity must fail publication"
    );

    let hour = normalize(raw(Surface::PostgreSqlStatements, None, None))
        .expect("query")
        .hour();
    let mut builder = ConversionContextBuilder::new(hour);
    builder.observe_block_size(hour.end(), Some(u128::MAX));
    let converted = normalize(raw(
        Surface::PostgreSqlStatements,
        Some(Metric::SharedRead),
        None,
    ))
    .expect("converted metric")
    .recipe()
    .expect("recipe")
    .resolve(builder.finish());
    assert!(
        converted.scale(f64::MAX).is_err(),
        "conversion overflow must fail publication"
    );
}

#[test]
fn recipes_encode_labels_groups_intervals_and_entity_kinds() {
    let statement = normalize(raw(Surface::PostgreSqlStatements, None, None))
        .expect("statement")
        .recipe()
        .expect("recipe");
    assert_eq!(statement.labels, ["datname", "usename"]);
    assert!(statement.groups.is_empty());
    assert_eq!(statement.intervals, 60);
    assert_eq!(statement.entity_kind(), "postgresql_statement");

    let process = normalize(raw(Surface::Processes, None, None))
        .expect("process")
        .recipe()
        .expect("recipe");
    assert!(process.labels.is_empty());
    assert_eq!(process.groups, ["comm"]);
    assert_eq!(process.entity_kind(), "process_command");

    let table = normalize(raw(
        Surface::PostgreSqlTables,
        None,
        Some(RelationLevel::Tablespace),
    ))
    .expect("tablespace")
    .recipe()
    .expect("recipe");
    assert!(table.labels.is_empty());
    assert_eq!(table.groups, ["tablespace"]);
    assert_eq!(table.intervals, 12);
    assert_eq!(table.entity_kind(), "postgresql_tablespace");
}

#[test]
fn wire_scalars_and_finite_values_are_exact() {
    for surface in Surface::ALL {
        assert_eq!(
            serde_json::to_value(surface).expect("surface wire value"),
            json!(surface.as_str())
        );
    }
    for metric in Metric::ALL {
        assert_eq!(
            serde_json::to_value(metric).expect("metric wire value"),
            json!(metric.as_str())
        );
    }
    for (level, expected) in [
        (RelationLevel::Object, "object"),
        (RelationLevel::Schema, "schema"),
        (RelationLevel::Database, "database"),
        (RelationLevel::Tablespace, "tablespace"),
    ] {
        assert_eq!(
            serde_json::to_value(level).expect("level wire value"),
            expected
        );
    }
    for (class, expected) in [
        (MetricClass::Cumulative, "cumulative"),
        (MetricClass::Gauge, "gauge"),
    ] {
        assert_eq!(
            serde_json::to_value(class).expect("class wire value"),
            expected
        );
    }
    let units = [
        (MetricUnit::Count, "count"),
        (MetricUnit::CountPerSecond, "count_per_second"),
        (MetricUnit::Bytes, "bytes"),
        (MetricUnit::BytesPerSecond, "bytes_per_second"),
        (MetricUnit::Milliseconds, "milliseconds"),
        (MetricUnit::MillisecondsPerSecond, "milliseconds_per_second"),
        (MetricUnit::Seconds, "seconds"),
        (MetricUnit::SecondsPerSecond, "seconds_per_second"),
        (MetricUnit::Microseconds, "microseconds"),
        (MetricUnit::MicrosecondsPerSecond, "microseconds_per_second"),
        (MetricUnit::Nanoseconds, "nanoseconds"),
        (MetricUnit::NanosecondsPerSecond, "nanoseconds_per_second"),
    ];
    for (unit, expected) in units {
        assert_eq!(
            serde_json::to_value(unit).expect("unit wire value"),
            expected
        );
    }
    for (ranking, expected) in [
        (Ranking::WholeWindowDeltaDesc, "whole_window_delta_desc"),
        (Ranking::WholeWindowMaxDesc, "whole_window_max_desc"),
        (
            Ranking::SumMemberWindowDeltaDesc,
            "sum_member_window_delta_desc",
        ),
        (
            Ranking::SumMemberWindowMaxDesc,
            "sum_member_window_max_desc",
        ),
    ] {
        assert_eq!(
            serde_json::to_value(ranking).expect("ranking wire value"),
            expected
        );
    }
    assert_eq!(
        serde_json::to_value(I64String::new(-42)).expect("signed wire value"),
        json!("-42")
    );
    assert_eq!(
        serde_json::to_value(U64String::new(u64::MAX)).expect("unsigned wire value"),
        json!(u64::MAX.to_string())
    );
    assert_eq!(FiniteValue::new(1.25).map(FiniteValue::get), Some(1.25));
    assert_eq!(FiniteValue::new(f64::NAN), None);
    assert_eq!(FiniteValue::new(f64::INFINITY), None);
}

#[test]
fn semantic_entity_kinds_have_exact_wire_identities() {
    let entities = [
        Entity::PostgreSqlStatement {
            query_id: None,
            role_oid: 1,
            database_oid: 2,
            top_level: None,
            database_name: None,
            role_name: None,
        },
        Entity::PostgreSqlPlan {
            role_oid: 1,
            database_oid: 2,
            entry_query_id: I64String::new(3),
            plan_id: I64String::new(4),
            database_name: None,
            role_name: None,
        },
        Entity::PostgreSqlTable {
            database_oid: 1,
            relation_oid: 2,
            database_name: "db".to_owned(),
            schema_name: "public".to_owned(),
            relation_name: "table".to_owned(),
        },
        Entity::PostgreSqlIndex {
            database_oid: 1,
            index_oid: 2,
            database_name: "db".to_owned(),
            schema_name: "public".to_owned(),
            table_name: "table".to_owned(),
            index_name: "index".to_owned(),
        },
        Entity::ProcessCommand {
            command: "postgres".to_owned(),
        },
        Entity::PostgreSqlDatabase {
            database_oid: 1,
            database_name: None,
        },
        Entity::CgroupCpu {
            path: "/system.slice".to_owned(),
        },
        Entity::CgroupIoDevice {
            path: "/system.slice".to_owned(),
            major: 8,
            minor: 0,
        },
        Entity::PostgreSqlRelationDatabase {
            database_name: "db".to_owned(),
        },
        Entity::PostgreSqlRelationSchema {
            database_name: "db".to_owned(),
            schema_name: "public".to_owned(),
        },
        Entity::PostgreSqlTablespace {
            tablespace_name: None,
        },
    ];
    let expected = [
        "postgresql_statement",
        "postgresql_plan",
        "postgresql_table",
        "postgresql_index",
        "process_command",
        "postgresql_database",
        "cgroup_cpu",
        "cgroup_io_device",
        "postgresql_relation_database",
        "postgresql_relation_schema",
        "postgresql_tablespace",
    ];
    for (entity, expected) in entities.into_iter().zip(expected) {
        let encoded = serde_json::to_value(entity).expect("semantic entity wire value");
        assert_eq!(encoded["kind"], expected);
    }
}
