//! Table and schema names shared by every workload module, so schema
//! creation and the sessions that query those tables never drift apart.

/// The name of schema `schema`.
pub(crate) fn schema_name(schema: u32) -> String {
    format!("tenant_{schema}")
}

/// The schema-qualified name of table `table` in schema `schema`.
pub(crate) fn table_name(schema: u32, table: u32) -> String {
    format!("{}.t{table}", schema_name(schema))
}

#[cfg(test)]
mod tests;
