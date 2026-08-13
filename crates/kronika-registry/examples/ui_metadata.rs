//! Writes the registry metadata compiled into the forensic interface bundle.

use std::io::{self, Write as _};

use arrow_array as _;
use arrow_ord as _;
use arrow_schema as _;
use arrow_select as _;
use bytes as _;
use kronika_derive as _;
use kronika_registry::{logical_section_name, registry};
use parquet as _;
use serde_json::json;
use thrift as _;

fn main() -> io::Result<()> {
    let layouts = registry()
        .iter()
        .map(|contract| {
            json!({
                "typeId": contract.type_id.get().to_string(),
                "logicalName": logical_section_name(contract.type_id.get()),
                "identity": contract.identity,
                "columns": contract.columns.iter().map(|column| column.name).collect::<Vec<_>>(),
                "columnMetadata": contract.columns.iter().map(|column| json!({
                    "name": column.name,
                    "type": column.ty.code(),
                    "class": column.class.code(),
                    "unit": column.unit.map(|unit| unit.code()),
                })).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    write!(out, "export const registry = ")?;
    serde_json::to_writer(&mut out, &layouts)?;
    writeln!(out, " as const;")?;
    writeln!(
        out,
        "export type RegistryLayout = (typeof registry)[number];"
    )
}
