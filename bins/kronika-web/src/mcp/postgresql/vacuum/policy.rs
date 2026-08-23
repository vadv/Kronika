use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

use super::super::{PostgresqlFailure, failure};
use crate::product_semantics::{SemanticDefinition, SemanticPolicy, VacuumMovement, VacuumRisk};

pub(super) struct Policies {
    pub(super) adjacency_factor: f64,
    pub(super) no_movement_samples: usize,
    pub(super) movements: BTreeMap<String, VacuumMovement>,
    default_risk: VacuumRisk,
    risk_order: BTreeMap<VacuumRisk, usize>,
    phase_risks: BTreeMap<String, VacuumRisk>,
    pub(super) definitions: Vec<Value>,
}

impl Policies {
    pub(super) fn load() -> Result<Self, PostgresqlFailure> {
        let adjacency = semantic("vacuum.episode_adjacency")?;
        let no_movement = semantic("vacuum.no_movement")?;
        let risk = semantic("vacuum.phase_risk")?;
        let SemanticPolicy::VacuumEpisode { adjacency_factor } = &adjacency.policy else {
            return Err(semantics_failure(
                "Vacuum adjacency policy has the wrong kind",
            ));
        };
        if !adjacency_factor.is_finite() || *adjacency_factor <= 0.0 {
            return Err(semantics_failure("Vacuum adjacency factor is invalid"));
        }
        let SemanticPolicy::VacuumNoMovement { samples, phases } = &no_movement.policy else {
            return Err(semantics_failure(
                "Vacuum no-movement policy has the wrong kind",
            ));
        };
        let no_movement_samples = usize::try_from(*samples)
            .ok()
            .filter(|samples| *samples > 0)
            .ok_or_else(|| semantics_failure("Vacuum no-movement sample count is invalid"))?;
        let movements = phases
            .iter()
            .cloned()
            .map(|movement| (movement.phase.clone(), movement))
            .collect::<BTreeMap<_, _>>();
        let SemanticPolicy::VacuumRisk {
            default,
            order,
            phases,
        } = &risk.policy
        else {
            return Err(semantics_failure("Vacuum risk policy has the wrong kind"));
        };
        let risk_order = order
            .iter()
            .copied()
            .enumerate()
            .map(|(position, risk)| (risk, position))
            .collect::<BTreeMap<_, _>>();
        let definitions = [adjacency, no_movement, risk]
            .into_iter()
            .map(|definition| {
                serde_json::to_value(definition)
                    .map_err(|error| semantics_failure(format!("serialize Vacuum policy: {error}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            adjacency_factor: *adjacency_factor,
            no_movement_samples,
            movements,
            default_risk: *default,
            risk_order,
            phase_risks: phases.clone(),
            definitions,
        })
    }

    pub(super) fn risk(&self, phase: &str) -> VacuumRisk {
        self.phase_risks
            .get(phase)
            .copied()
            .unwrap_or(self.default_risk)
    }

    pub(super) fn risk_position(&self, risk: VacuumRisk) -> usize {
        self.risk_order.get(&risk).copied().unwrap_or(usize::MAX)
    }
}

fn semantic(id: &str) -> Result<&'static SemanticDefinition, PostgresqlFailure> {
    crate::product_semantics::get(id)
        .map_err(|error| semantics_failure(error.to_string()))?
        .ok_or_else(|| semantics_failure(format!("missing accepted semantic {id}")))
}

pub(super) fn adjacency_limit(seconds: u64, factor: f64) -> Result<i64, PostgresqlFailure> {
    let duration = Duration::from_secs(seconds).mul_f64(factor);
    i64::try_from(duration.as_micros())
        .map_err(|_overflow| semantics_failure("Vacuum adjacency duration is too large"))
}

fn semantics_failure(message: impl Into<String>) -> PostgresqlFailure {
    failure("semantics_unreadable", message, None)
}
