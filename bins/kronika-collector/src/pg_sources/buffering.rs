//! Interning the strings of a `PostgreSQL` read and buffering its rows.

use anyhow::Result;
use kronika_registry::StrId;
use kronika_source_pg::activity::{self, ActivityVersion};
use kronika_source_pg::archiver;
use kronika_source_pg::database::{self, DatabaseVersion};
use kronika_source_pg::io::{self, IoVersion};
use kronika_source_pg::prepared_xacts;
use kronika_source_pg::progress_vacuum;
use kronika_source_pg::settings::{self, SettingsRow};
use kronika_source_pg::statements::{self, StatementsVersion};
use kronika_source_pg::store_plans;
use kronika_source_pg::user_indexes::{self, UserIndexesVersion};
use kronika_source_pg::user_tables::{self, UserTablesVersion};
use kronika_source_pg::wal::WalSnapshot;
use kronika_writer::{Interner, SectionBuffers};

use super::PgRows;
use crate::buffering::buffer_row;
use crate::logging::{LogLevel, field, log_event};

/// Move one read of the server into the window.
///
/// `settings` is the running configuration and is buffered only when a segment
/// is opening: it changes rarely, and every segment has to carry it to be
/// readable on its own.
///
/// # Errors
///
/// Returns an error when a string cannot be interned or a section buffer is
/// full.
pub(crate) fn push_pg_sources(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    rows: &PgRows,
    settings: &[SettingsRow],
) -> Result<()> {
    push_settings(buffers, interner, settings)?;
    if let Some(row) = &rows.archiver {
        buffer_row(buffers, archiver::to_archiver(row, intern(interner))?)?;
    }
    match rows.wal {
        Some(WalSnapshot::V1(row)) => buffer_row(buffers, row)?,
        Some(WalSnapshot::V2(row)) => buffer_row(buffers, row)?,
        None => {}
    }
    for row in &rows.prepared_xacts {
        buffer_row(
            buffers,
            prepared_xacts::to_prepared_xacts(row, intern(interner))?,
        )?;
    }
    push_database(buffers, interner, rows)?;
    push_io(buffers, interner, rows)?;
    push_activity(buffers, interner, rows)?;
    for row in &rows.progress_vacuum {
        buffer_row(
            buffers,
            progress_vacuum::to_progress_vacuum(row, intern(interner))?,
        )?;
    }
    push_statements(buffers, interner, rows)?;
    push_store_plans(buffers, interner, rows)?;
    push_user_tables(buffers, interner, rows)?;
    push_user_indexes(buffers, interner, rows)
}

fn push_settings(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    settings: &[SettingsRow],
) -> Result<()> {
    for row in settings {
        buffer_row(buffers, settings::to_section(row, intern(interner))?)?;
    }
    Ok(())
}

fn push_database(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    rows: &PgRows,
) -> Result<()> {
    let Some((version, collected)) = &rows.database else {
        return Ok(());
    };
    for row in collected {
        match version {
            DatabaseVersion::V1 => buffer_row(buffers, database::to_v1(row, intern(interner))?)?,
            DatabaseVersion::V2 => buffer_row(buffers, database::to_v2(row, intern(interner))?)?,
            DatabaseVersion::V3 => buffer_row(buffers, database::to_v3(row, intern(interner))?)?,
            DatabaseVersion::V4 => buffer_row(buffers, database::to_v4(row, intern(interner))?)?,
        }
    }
    Ok(())
}

fn push_io(buffers: &mut SectionBuffers, interner: &mut Interner, rows: &PgRows) -> Result<()> {
    let Some((version, collected)) = &rows.io else {
        return Ok(());
    };
    for row in collected {
        match version {
            IoVersion::V1 => buffer_row(buffers, io::to_v1(row, intern(interner))?)?,
            IoVersion::V2 => buffer_row(buffers, io::to_v2(row, intern(interner))?)?,
        }
    }
    Ok(())
}

/// A read the server could not answer in full is dropped, not written short:
/// half an activity snapshot would read as an idle server.
fn push_activity(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    rows: &PgRows,
) -> Result<()> {
    let Some(read) = &rows.activity else {
        return Ok(());
    };
    if read.truncated {
        log_event(
            LogLevel::Warn,
            "pg_activity_truncated",
            &[field("source_rows", read.source_rows)],
        );
        return Ok(());
    }
    for row in &read.rows {
        match read.version {
            ActivityVersion::V1 => buffer_row(buffers, activity::to_v1(row, intern(interner))?)?,
            ActivityVersion::V2 => buffer_row(buffers, activity::to_v2(row, intern(interner))?)?,
            ActivityVersion::V3 => buffer_row(buffers, activity::to_v3(row, intern(interner))?)?,
        }
    }
    Ok(())
}

fn push_statements(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    rows: &PgRows,
) -> Result<()> {
    let Some((version, collected)) = &rows.statements else {
        return Ok(());
    };
    for row in collected {
        match version {
            StatementsVersion::V1 => {
                buffer_row(buffers, statements::to_v1(row, intern(interner))?)?;
            }
            StatementsVersion::V2 => {
                buffer_row(buffers, statements::to_v2(row, intern(interner))?)?;
            }
            StatementsVersion::V3 => {
                buffer_row(buffers, statements::to_v3(row, intern(interner))?)?;
            }
            StatementsVersion::V4 => {
                buffer_row(buffers, statements::to_v4(row, intern(interner))?)?;
            }
            StatementsVersion::V5 => {
                buffer_row(buffers, statements::to_v5(row, intern(interner))?)?;
            }
            StatementsVersion::V6 => {
                buffer_row(buffers, statements::to_v6(row, intern(interner))?)?;
            }
        }
    }
    Ok(())
}

fn push_store_plans(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    rows: &PgRows,
) -> Result<()> {
    for row in &rows.store_plans_ossc {
        buffer_row(buffers, store_plans::to_ossc(row, intern(interner))?)?;
    }
    for row in &rows.store_plans_vadv {
        buffer_row(buffers, store_plans::to_vadv(row, intern(interner))?)?;
    }
    Ok(())
}

fn push_user_tables(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    rows: &PgRows,
) -> Result<()> {
    let Some((version, collected)) = &rows.user_tables else {
        return Ok(());
    };
    for row in collected {
        match version {
            UserTablesVersion::V1 => {
                buffer_row(buffers, user_tables::to_v1(row, intern(interner))?)?;
            }
            UserTablesVersion::V2 => {
                buffer_row(buffers, user_tables::to_v2(row, intern(interner))?)?;
            }
            UserTablesVersion::V3 => {
                buffer_row(buffers, user_tables::to_v3(row, intern(interner))?)?;
            }
            UserTablesVersion::V4 => {
                buffer_row(buffers, user_tables::to_v4(row, intern(interner))?)?;
            }
        }
    }
    Ok(())
}

fn push_user_indexes(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    rows: &PgRows,
) -> Result<()> {
    let Some((version, collected)) = &rows.user_indexes else {
        return Ok(());
    };
    for row in collected {
        match version {
            UserIndexesVersion::V1 => {
                buffer_row(buffers, user_indexes::to_v1(row, intern(interner))?)?;
            }
            UserIndexesVersion::V2 => {
                buffer_row(buffers, user_indexes::to_v2(row, intern(interner))?)?;
            }
        }
    }
    Ok(())
}

/// The interner in the shape every `to_*` builder expects.
fn intern(interner: &mut Interner) -> impl FnMut(&[u8]) -> Result<StrId> + '_ {
    move |value: &[u8]| {
        interner
            .intern(value)
            .map(|id| StrId(id.get()))
            .map_err(|err| anyhow::anyhow!("intern a PostgreSQL string: {err}"))
    }
}
