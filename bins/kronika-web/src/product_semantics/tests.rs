use super::{
    EventTier, SemanticOrigin, SemanticPolicy, SemanticUnit, ThresholdOperator, VacuumRisk, all,
    get,
};

#[test]
fn bundled_registry_parses_into_typed_definitions() {
    let definitions = all().expect("bundled product semantics");
    assert_eq!(
        definitions.len(),
        23,
        "unexpected semantic definition count"
    );

    let definition = get("value_tone.query_duration_ms")
        .expect("product semantics")
        .expect("query duration semantic");
    assert_eq!(definition.origin, SemanticOrigin::AcceptedPresentation);
    assert_eq!(definition.unit, Some(SemanticUnit::Milliseconds));
    assert_eq!(definition.thresholds.len(), 2);
    assert_eq!(definition.thresholds[0].operator, ThresholdOperator::Gte);
    assert!((definition.thresholds[0].value - 5_000.0).abs() < f64::EPSILON);
    assert!(matches!(
        definition.policy,
        SemanticPolicy::NumericValueTone {
            ref field,
            active_client_only: true,
        } if field == "query_duration_ms"
    ));

    let serialized = serde_json::to_value(definition).expect("serialize semantic definition");
    assert_eq!(serialized["id"], "value_tone.query_duration_ms");
    assert_eq!(serialized["policy"]["kind"], "numeric_value_tone");
}

#[test]
fn registry_keeps_index_semantics_in_the_index_crate() {
    let definitions = all().expect("bundled product semantics");
    assert!(
        definitions
            .iter()
            .all(|definition| !definition.id.starts_with("finding.")
                && !definition.id.starts_with("health.")),
        "indexed finding and health semantics must not be copied here"
    );
}

#[test]
fn vacuum_relation_and_event_policies_keep_their_origins() {
    let vacuum = get("vacuum.phase_risk")
        .expect("product semantics")
        .expect("vacuum risk semantic");
    let SemanticPolicy::VacuumRisk {
        default,
        order,
        phases,
    } = &vacuum.policy
    else {
        panic!("vacuum risk has the wrong typed policy");
    };
    assert_eq!(*default, VacuumRisk::Ordinary);
    assert_eq!(
        order,
        &[
            VacuumRisk::Dangerous,
            VacuumRisk::Heavy,
            VacuumRisk::Ordinary
        ]
    );
    assert_eq!(phases["truncating heap"], VacuumRisk::Dangerous);

    let relation = get("relation.index_state_severity")
        .expect("product semantics")
        .expect("relation severity semantic");
    assert_eq!(relation.origin, SemanticOrigin::KronikaDerived);
    let SemanticPolicy::RelationSeverity { states } = &relation.policy else {
        panic!("relation severity has the wrong typed policy");
    };
    assert_eq!(
        states
            .iter()
            .map(|state| state.severity)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );

    let event = get("event.pg_log_errors.tier")
        .expect("product semantics")
        .expect("event tier semantic");
    let SemanticPolicy::EventTier {
        tiers,
        fallback,
        provenance,
        ..
    } = &event.policy
    else {
        panic!("event tier has the wrong typed policy");
    };
    assert_eq!(*provenance, SemanticOrigin::Recorded);
    assert_eq!(*fallback, EventTier::Notable);
    assert_eq!(tiers[1], EventTier::Critical);
}
