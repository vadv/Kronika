use kronika_reader::Row;
use serde_json::{Value, json};

use super::{
    CounterReadings, IdentityCell, OrderedNumber, PageContext, Plan, counter_delta,
    preferred_column,
};

const STATEMENTS: &str = "pg_stat_statements";
const PLANS: &str = "pg_store_plans";

#[derive(Clone, Copy)]
pub(super) struct Columns {
    calls: Option<&'static str>,
    execution: Option<&'static str>,
    rows: Option<&'static str>,
}

impl Columns {
    pub(super) fn new(logical_name: &str, plan: &Plan) -> Option<Self> {
        matches!(logical_name, STATEMENTS | PLANS).then(|| Self {
            calls: plan.contract.column("calls").map(|column| column.name),
            execution: preferred_column(plan, "total_exec_time", "total_time"),
            rows: plan.contract.column("rows").map(|column| column.name),
        })
    }

    fn projection(self) -> Vec<&'static str> {
        [self.calls, self.execution, self.rows]
            .into_iter()
            .flatten()
            .collect()
    }
}

pub(super) fn add_projection(logical_name: &str, plan: &mut Plan) {
    if let Some(columns) = Columns::new(logical_name, plan) {
        plan.add_projection_columns(&columns.projection());
    }
}

#[derive(Default)]
struct Sum {
    value: f64,
    seen: bool,
    unavailable: bool,
}

impl Sum {
    fn add(&mut self, value: Option<f64>) {
        let Some(value) = value.filter(|value| value.is_finite()) else {
            self.unavailable = true;
            return;
        };
        let total = self.value + value;
        if total.is_finite() {
            self.value = total;
            self.seen = true;
        } else {
            self.unavailable = true;
        }
    }

    fn json(&self) -> Value {
        if self.seen && !self.unavailable {
            json!(self.value)
        } else {
            Value::Null
        }
    }
}

#[derive(Default)]
pub(super) struct Dense {
    call_rate: Sum,
    execution_rate: Sum,
    row_rate: Sum,
    calls: Sum,
    execution: Sum,
}

impl Dense {
    pub(super) fn new(logical_name: &str) -> Option<Self> {
        matches!(logical_name, STATEMENTS | PLANS).then(Self::default)
    }

    pub(super) fn add(
        &mut self,
        columns: Columns,
        context: &PageContext<'_>,
        row: &Row,
        identity: &[IdentityCell],
    ) {
        let elapsed = context.elapsed_for(row).filter(|elapsed| *elapsed > 0);
        let before = elapsed.and_then(|_| context.predecessor(row, identity));
        let calls = delta(row, before, columns.calls);
        let execution = delta(row, before, columns.execution);
        let rows = delta(row, before, columns.rows);

        self.call_rate.add(rate(calls, elapsed));
        self.execution_rate.add(rate(execution, elapsed));
        self.row_rate.add(rate(rows, elapsed));
        self.calls.add(calls);
        self.execution.add(execution);
    }

    pub(super) fn json(&self) -> Value {
        let mean = if self.calls.seen
            && self.execution.seen
            && !self.calls.unavailable
            && !self.execution.unavailable
            && self.calls.value > 0.0
        {
            json!(self.execution.value / self.calls.value)
        } else {
            Value::Null
        };
        json!({
            "call_rate": self.call_rate.json(),
            "exec_time_rate": self.execution_rate.json(),
            "mean_exec": mean,
            "row_rate": self.row_rate.json(),
        })
    }
}

fn delta(row: &Row, before: Option<&CounterReadings>, column: Option<&'static str>) -> Option<f64> {
    let column = column?;
    let before = before?;
    row.get(column)
        .zip(before.get(column))
        .and_then(|(now, earlier)| counter_delta(now, earlier))
        .map(OrderedNumber::as_f64)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "snapshot rates are emitted as finite JSON numbers"
)]
fn rate(value: Option<f64>, elapsed: Option<i64>) -> Option<f64> {
    let rate = value? * 1_000_000.0 / elapsed? as f64;
    rate.is_finite().then_some(rate)
}
