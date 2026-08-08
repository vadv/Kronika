//! The two `PostgreSQL` protocol paths used by the collector.
//!
//! Typed metric rows use one-shot unnamed Extended Protocol through
//! [`tokio_postgres::Client::query_typed_raw`]. Small administrative reads may
//! use Simple Protocol when their text representation is unambiguous. Neither
//! path creates named server-side statement state.

use std::pin::pin;
use std::time::{Duration, Instant};
use std::{error::Error, fmt};

use anyhow::{Context as _, Result};
use futures_util::TryStreamExt as _;
use tokio_postgres::types::{BorrowToSql, Type};
use tokio_postgres::{Client, Row, RowStream, SimpleQueryMessage, SimpleQueryStream};

/// Target row count for one collector-side `PostgreSQL` batch.
pub const BATCH_ROWS: usize = 256;

/// Target decoded payload size for one collector-side `PostgreSQL` batch.
pub const BATCH_LOGICAL_BYTES: usize = 512 * 1024;

/// Maximum characters retained from a potentially unbounded text value.
pub const TEXT_PREFIX_CHARS: usize = 65_536;

/// Cumulative deadline for opening and consuming one bounded row stream.
pub const QUERY_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Measurements made while one SQL statement is consumed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct QueryStats {
    /// Result rows decoded from `PostgreSQL`.
    pub rows: u64,
    /// Approximate decoded application payload, not network or TLS bytes.
    pub application_payload_from_postgres_bytes: u64,
    /// Query text and parameter payload supplied by the application, not wire bytes.
    pub application_payload_to_postgres_bytes: u64,
    /// Batches handed to the collector before another row was fetched.
    pub batches: u64,
    /// Time spent encoding batches for the journal.
    pub encode_elapsed: Duration,
    /// Time spent appending encoded batches to the journal.
    pub append_elapsed: Duration,
    /// Encoded part bytes attributable to this query.
    pub encoded_bytes: u64,
    /// Bytes appended to the WAL attributable to this query.
    pub wal_bytes_appended: u64,
    sink_elapsed: Duration,
}

impl QueryStats {
    /// Account for a query before sending it.
    const fn sent(&mut self, sql: &str, parameter_bytes: usize) {
        self.application_payload_to_postgres_bytes = self
            .application_payload_to_postgres_bytes
            .saturating_add(usize_to_u64(sql.len().saturating_add(parameter_bytes)));
    }

    /// Database fetch time with synchronous encoding and append time removed.
    #[must_use]
    pub const fn fetch_elapsed(self, total: Duration) -> Duration {
        total.saturating_sub(self.sink_elapsed)
    }

    /// Account for one batch encoded and appended outside the stream helper.
    pub const fn record_batch_write(&mut self, elapsed: Duration, write: BatchWrite) {
        self.wrote_batch(elapsed, write);
    }

    /// Account for batch sink work that failed before bytes were written.
    pub const fn record_failed_batch(&mut self, elapsed: Duration) {
        self.attempted_batch(elapsed);
    }

    /// Account for one message returned by Simple Protocol.
    pub fn received_simple(&mut self, message: &SimpleQueryMessage) {
        let SimpleQueryMessage::Row(row) = message else {
            return;
        };
        self.rows = self.rows.saturating_add(1);
        let bytes = (0..row.len())
            .filter_map(|index| row.get(index))
            .map(str::len)
            .fold(0_usize, usize::saturating_add);
        self.application_payload_from_postgres_bytes = self
            .application_payload_from_postgres_bytes
            .saturating_add(usize_to_u64(bytes));
    }

    fn received(&mut self, row: &Row) -> usize {
        let bytes = logical_row_bytes(row);
        self.rows = self.rows.saturating_add(1);
        self.application_payload_from_postgres_bytes = self
            .application_payload_from_postgres_bytes
            .saturating_add(usize_to_u64(bytes));
        bytes
    }

    const fn wrote_batch(&mut self, elapsed: Duration, write: BatchWrite) {
        self.attempted_batch(elapsed);
        self.encode_elapsed = self.encode_elapsed.saturating_add(write.encode_elapsed);
        self.append_elapsed = self.append_elapsed.saturating_add(write.append_elapsed);
        self.encoded_bytes = self.encoded_bytes.saturating_add(write.encoded_bytes);
        self.wal_bytes_appended = self
            .wal_bytes_appended
            .saturating_add(write.wal_bytes_appended);
    }

    const fn attempted_batch(&mut self, elapsed: Duration) {
        self.batches = self.batches.saturating_add(1);
        self.sink_elapsed = self.sink_elapsed.saturating_add(elapsed);
    }
}

/// One bounded set of owned rows.
#[derive(Debug)]
pub struct Batch<T> {
    /// Owned rows in arrival order.
    pub rows: Vec<T>,
    /// Approximate decoded application payload in the rows.
    pub logical_bytes: usize,
}

/// Journal work attributable to one `PostgreSQL` batch.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BatchWrite {
    /// Time spent converting and encoding the batch.
    pub encode_elapsed: Duration,
    /// Time spent appending the encoded part.
    pub append_elapsed: Duration,
    /// Encoded part bytes.
    pub encoded_bytes: u64,
    /// Bytes added to the WAL.
    pub wal_bytes_appended: u64,
}

/// A streamed query failed either at `PostgreSQL` or in its synchronous sink.
#[derive(Debug)]
pub enum BatchError<E> {
    /// The cumulative query/fetch deadline elapsed.
    Timeout,
    /// The `PostgreSQL` protocol or row stream failed.
    PostgreSql(tokio_postgres::Error),
    /// A returned row did not match the source's declared shape.
    Decode(anyhow::Error),
    /// The collector could not consume the current retained batch.
    Sink(E),
}

/// A typed row failed to decode after PostgreSQL returned it successfully.
#[derive(Debug)]
pub struct DecodeError(anyhow::Error);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "decode a PostgreSQL result row: {:#}", self.0)
    }
}

impl Error for DecodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

/// A small-query stream failed at the transport, protocol, or server boundary.
#[derive(Debug)]
pub struct StreamError(tokio_postgres::Error);

impl StreamError {
    /// The underlying PostgreSQL error used for failure classification.
    #[must_use]
    pub const fn postgres(&self) -> &tokio_postgres::Error {
        &self.0
    }
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Error for StreamError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

/// One live `PostgreSQL` connection and its generation.
#[derive(Debug, Clone, Copy)]
pub struct Session<'a> {
    client: &'a Client,
    generation: u64,
}

impl<'a> Session<'a> {
    /// Wrap a connected client.
    #[must_use]
    pub const fn new(client: &'a Client, generation: u64) -> Self {
        Self { client, generation }
    }

    /// Generation assigned when this connection was opened.
    #[must_use]
    pub const fn generation(self) -> u64 {
        self.generation
    }

    /// Start a one-shot unnamed typed Extended Protocol query.
    ///
    /// # Errors
    ///
    /// Returns the transport, protocol, or parameter-encoding error.
    pub async fn typed_stream<P, I>(
        self,
        sql: &str,
        params: I,
        parameter_bytes: usize,
        stats: &mut QueryStats,
    ) -> Result<RowStream, tokio_postgres::Error>
    where
        P: BorrowToSql,
        I: IntoIterator<Item = (P, Type)>,
    {
        stats.sent(sql, parameter_bytes);
        self.client.query_typed_raw(sql, params).await
    }

    /// Start a Simple Protocol query for a small administrative response.
    ///
    /// # Errors
    ///
    /// Returns the transport or protocol error.
    pub async fn simple_stream(
        self,
        sql: &str,
        stats: &mut QueryStats,
    ) -> Result<SimpleQueryStream, tokio_postgres::Error> {
        stats.sent(sql, 0);
        self.client.simple_query_raw(sql).await
    }
}

/// Consume a typed query into memory for a known-small result.
///
/// # Errors
///
/// Returns a `PostgreSQL` protocol/decoding error.
pub async fn read_all<P, I, T>(
    session: Session<'_>,
    sql: &str,
    params: I,
    parameter_bytes: usize,
    stats: &mut QueryStats,
    mut decode: impl FnMut(&Row) -> Result<T>,
) -> Result<Vec<T>>
where
    P: BorrowToSql,
    I: IntoIterator<Item = (P, Type)>,
{
    let stream = session
        .typed_stream(sql, params, parameter_bytes, stats)
        .await
        .map_err(StreamError)?;
    let mut stream = pin!(stream);
    let mut rows = Vec::new();
    while let Some(row) = stream.try_next().await.map_err(StreamError)? {
        stats.received(&row);
        rows.push(decode(&row).map_err(DecodeError)?);
    }
    Ok(rows)
}

/// Consume exactly one typed row.
///
/// # Errors
///
/// Returns a `PostgreSQL` error or an error when the result does not contain
/// exactly one row.
pub async fn read_one<P, I, T>(
    session: Session<'_>,
    sql: &str,
    params: I,
    parameter_bytes: usize,
    stats: &mut QueryStats,
    decode: impl FnMut(&Row) -> Result<T>,
) -> Result<T>
where
    P: BorrowToSql,
    I: IntoIterator<Item = (P, Type)>,
{
    let mut rows = read_all(session, sql, params, parameter_bytes, stats, decode).await?;
    anyhow::ensure!(
        rows.len() == 1,
        "PostgreSQL returned {} rows, expected one",
        rows.len()
    );
    Ok(rows.remove(0))
}

/// Consume zero or one typed row.
///
/// # Errors
///
/// Returns a `PostgreSQL` error or an error when the result contains more than
/// one row.
pub async fn read_optional<P, I, T>(
    session: Session<'_>,
    sql: &str,
    params: I,
    parameter_bytes: usize,
    stats: &mut QueryStats,
    decode: impl FnMut(&Row) -> Result<T>,
) -> Result<Option<T>>
where
    P: BorrowToSql,
    I: IntoIterator<Item = (P, Type)>,
{
    let mut rows = read_all(session, sql, params, parameter_bytes, stats, decode).await?;
    anyhow::ensure!(
        rows.len() <= 1,
        "PostgreSQL returned {} rows, expected at most one",
        rows.len()
    );
    Ok(rows.pop())
}

/// Decode and deliver bounded batches before fetching more rows.
///
/// # Errors
///
/// Returns [`BatchError::Timeout`] when the cumulative query/fetch deadline
/// elapses, [`BatchError::PostgreSql`] for a stream failure,
/// [`BatchError::Decode`] for a row-shape mismatch, and [`BatchError::Sink`]
/// when the synchronous sink rejects a retained batch.
pub async fn read_batched<P, I, T, E>(
    session: Session<'_>,
    sql: &str,
    params: I,
    parameter_bytes: usize,
    stats: &mut QueryStats,
    decode: impl FnMut(&Row) -> Result<T>,
    sink: impl FnMut(Batch<T>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>>
where
    P: BorrowToSql,
    I: IntoIterator<Item = (P, Type)>,
{
    read_batched_inner(session, sql, params, parameter_bytes, stats, decode, sink).await
}

async fn read_batched_inner<P, I, T, E>(
    session: Session<'_>,
    sql: &str,
    params: I,
    parameter_bytes: usize,
    stats: &mut QueryStats,
    mut decode: impl FnMut(&Row) -> Result<T>,
    mut sink: impl FnMut(Batch<T>) -> Result<BatchWrite, E>,
) -> Result<(), BatchError<E>>
where
    P: BorrowToSql,
    I: IntoIterator<Item = (P, Type)>,
{
    let mut deadline = tokio::time::Instant::now() + QUERY_FETCH_TIMEOUT;
    let stream = tokio::time::timeout_at(
        deadline,
        session.typed_stream(sql, params, parameter_bytes, stats),
    )
    .await
    .map_err(|_elapsed| BatchError::Timeout)?
    .map_err(BatchError::PostgreSql)?;
    let mut stream = pin!(stream);
    let mut pending = PendingBatch::new();
    while let Some(row) = tokio::time::timeout_at(deadline, stream.try_next())
        .await
        .map_err(|_elapsed| BatchError::Timeout)?
        .map_err(BatchError::PostgreSql)?
    {
        let bytes = stats.received(&row);
        let decoded = decode(&row).map_err(BatchError::Decode)?;
        if let Some(batch) = pending.push(decoded, bytes) {
            extend_fetch_deadline(&mut deadline, deliver_batch(batch, stats, &mut sink)?);
        }
    }
    if let Some(batch) = pending.finish() {
        let _sink_elapsed = deliver_batch(batch, stats, &mut sink)?;
    }
    Ok(())
}

fn extend_fetch_deadline(deadline: &mut tokio::time::Instant, sink_elapsed: Duration) {
    if let Some(extended) = deadline.checked_add(sink_elapsed) {
        *deadline = extended;
    }
}

const fn batch_limit_reached(rows: usize, logical_bytes: usize) -> bool {
    rows >= BATCH_ROWS || logical_bytes >= BATCH_LOGICAL_BYTES
}

struct PendingBatch<T> {
    rows: Vec<T>,
    logical_bytes: usize,
}

impl<T> PendingBatch<T> {
    fn new() -> Self {
        Self {
            rows: Vec::with_capacity(BATCH_ROWS),
            logical_bytes: 0,
        }
    }

    fn push(&mut self, row: T, logical_bytes: usize) -> Option<Batch<T>> {
        if !self.rows.is_empty()
            && (self.rows.len() >= BATCH_ROWS
                || self.logical_bytes.saturating_add(logical_bytes) > BATCH_LOGICAL_BYTES)
        {
            let batch = self.take();
            self.logical_bytes = logical_bytes;
            self.rows.push(row);
            return Some(batch);
        }
        self.logical_bytes = self.logical_bytes.saturating_add(logical_bytes);
        self.rows.push(row);
        batch_limit_reached(self.rows.len(), self.logical_bytes).then(|| self.take())
    }

    fn finish(mut self) -> Option<Batch<T>> {
        (!self.rows.is_empty()).then(|| self.take())
    }

    fn take(&mut self) -> Batch<T> {
        Batch {
            rows: std::mem::replace(&mut self.rows, Vec::with_capacity(BATCH_ROWS)),
            logical_bytes: std::mem::take(&mut self.logical_bytes),
        }
    }
}

fn deliver_batch<T, E>(
    batch: Batch<T>,
    stats: &mut QueryStats,
    sink: &mut impl FnMut(Batch<T>) -> Result<BatchWrite, E>,
) -> Result<Duration, BatchError<E>> {
    let started = Instant::now();
    match sink(batch) {
        Ok(write) => {
            let elapsed = started.elapsed();
            stats.wrote_batch(elapsed, write);
            Ok(elapsed)
        }
        Err(error) => {
            stats.attempted_batch(started.elapsed());
            Err(BatchError::Sink(error))
        }
    }
}

/// Read one integer from an exact one-row, one-column Simple response.
///
/// # Errors
///
/// Returns a `PostgreSQL` error or an error for a missing, repeated, null, or
/// non-integer scalar value.
pub async fn read_simple_i32(
    session: Session<'_>,
    sql: &str,
    stats: &mut QueryStats,
) -> Result<i32> {
    let stream = session
        .simple_stream(sql, stats)
        .await
        .map_err(StreamError)?;
    let mut stream = pin!(stream);
    let mut value = None;
    while let Some(message) = stream.try_next().await.map_err(StreamError)? {
        if let SimpleQueryMessage::Row(row) = message {
            anyhow::ensure!(value.is_none(), "PostgreSQL returned more than one row");
            let text = row.get(0).context("the scalar column was NULL")?;
            stats.rows = stats.rows.saturating_add(1);
            stats.application_payload_from_postgres_bytes = stats
                .application_payload_from_postgres_bytes
                .saturating_add(usize_to_u64(text.len()));
            value = Some(
                text.parse::<i32>()
                    .context("parse the PostgreSQL integer scalar")?,
            );
        }
    }
    value.context("PostgreSQL returned no scalar row")
}

/// Consume the row messages from one known-small Simple Protocol query.
///
/// # Errors
///
/// Returns a protocol error or the decoder's error.
pub async fn read_simple_rows<T>(
    session: Session<'_>,
    sql: &str,
    stats: &mut QueryStats,
    mut decode: impl FnMut(&tokio_postgres::SimpleQueryRow) -> Result<T>,
) -> Result<Vec<T>> {
    let stream = session
        .simple_stream(sql, stats)
        .await
        .map_err(StreamError)?;
    let mut stream = pin!(stream);
    let mut rows = Vec::new();
    while let Some(message) = stream.try_next().await.map_err(StreamError)? {
        stats.received_simple(&message);
        if let SimpleQueryMessage::Row(row) = message {
            rows.push(decode(&row).map_err(DecodeError)?);
        }
    }
    Ok(rows)
}

fn logical_row_bytes(row: &Row) -> usize {
    row.columns()
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let ty = column.type_();
            if matches!(
                *ty,
                Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN
            ) {
                return row
                    .try_get::<_, Option<&str>>(index)
                    .ok()
                    .flatten()
                    .map_or(0, str::len);
            }
            if *ty == Type::BYTEA {
                return row
                    .try_get::<_, Option<&[u8]>>(index)
                    .ok()
                    .flatten()
                    .map_or(0, <[u8]>::len);
            }
            fixed_width(ty)
        })
        .fold(0_usize, usize::saturating_add)
}

fn fixed_width(ty: &Type) -> usize {
    if matches!(*ty, Type::BOOL | Type::CHAR) {
        1
    } else if *ty == Type::INT2 {
        2
    } else if matches!(*ty, Type::INT4 | Type::OID | Type::FLOAT4 | Type::DATE) {
        4
    } else if matches!(
        *ty,
        Type::INT8 | Type::FLOAT8 | Type::TIME | Type::TIMESTAMP | Type::TIMESTAMPTZ
    ) {
        8
    } else {
        0
    }
}

const fn usize_to_u64(value: usize) -> u64 {
    value as u64
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, DuplexStream};
    use tokio_postgres::{Config, NoTls};

    use super::{
        BATCH_LOGICAL_BYTES, BATCH_ROWS, Batch, BatchError, BatchWrite, PendingBatch,
        QUERY_FETCH_TIMEOUT, QueryStats, Session, batch_limit_reached, deliver_batch,
        extend_fetch_deadline, read_batched,
    };

    #[test]
    fn batch_limits_cover_both_rows_and_payload() {
        assert!(!batch_limit_reached(
            BATCH_ROWS - 1,
            BATCH_LOGICAL_BYTES - 1
        ));
        assert!(batch_limit_reached(BATCH_ROWS, 1));
        assert!(batch_limit_reached(1, BATCH_LOGICAL_BYTES));
    }

    #[test]
    fn batching_preserves_order_across_a_row_boundary() {
        let mut pending = PendingBatch::new();
        let mut first = None;
        for row in 0..=BATCH_ROWS {
            if let Some(batch) = pending.push(row, 1) {
                assert!(first.replace(batch).is_none(), "only one full batch");
            }
        }
        let first = first.expect("the row bound emits a batch");
        let last = pending.finish().expect("one row remains");
        assert_eq!(first.rows, (0..BATCH_ROWS).collect::<Vec<_>>());
        assert_eq!(first.logical_bytes, BATCH_ROWS);
        assert_eq!(last.rows, vec![BATCH_ROWS]);
        assert_eq!(last.logical_bytes, 1);
    }

    #[test]
    fn one_oversized_row_is_delivered_as_its_own_batch() {
        let mut pending = PendingBatch::new();
        let batch = pending
            .push("large", BATCH_LOGICAL_BYTES + 1)
            .expect("the payload bound emits immediately");
        assert_eq!(batch.rows, ["large"]);
        assert_eq!(batch.logical_bytes, BATCH_LOGICAL_BYTES + 1);
        assert!(pending.finish().is_none());
    }

    #[test]
    fn multirow_metric_collectors_use_the_bounded_stream_helper() {
        for source in [
            include_str!("database.rs"),
            include_str!("io.rs"),
            include_str!("prepared_xacts.rs"),
            include_str!("progress_vacuum.rs"),
        ] {
            assert!(source.contains("query::read_batched("));
            assert!(!source.contains("query::read_all("));
        }
    }

    #[test]
    fn write_accounting_keeps_storage_and_fetch_time_separate() {
        let mut stats = QueryStats::default();
        stats.record_batch_write(
            Duration::from_millis(11),
            BatchWrite {
                encode_elapsed: Duration::from_millis(3),
                append_elapsed: Duration::from_millis(5),
                encoded_bytes: 700,
                wal_bytes_appended: 900,
            },
        );
        assert_eq!(stats.batches, 1);
        assert_eq!(stats.encode_elapsed, Duration::from_millis(3));
        assert_eq!(stats.append_elapsed, Duration::from_millis(5));
        assert_eq!(stats.encoded_bytes, 700);
        assert_eq!(stats.wal_bytes_appended, 900);
        assert_eq!(
            stats.fetch_elapsed(Duration::from_millis(19)),
            Duration::from_millis(8)
        );
    }

    #[test]
    fn a_failed_sink_attempt_counts_without_claiming_written_bytes() {
        let mut stats = QueryStats::default();
        let result = deliver_batch(
            Batch {
                rows: vec![1],
                logical_bytes: 8,
            },
            &mut stats,
            &mut |_batch| {
                std::thread::sleep(Duration::from_millis(1));
                Err("journal full")
            },
        );
        assert!(matches!(result, Err(BatchError::Sink("journal full"))));
        assert_eq!(stats.batches, 1);
        assert!(stats.sink_elapsed >= Duration::from_millis(1));
        assert_eq!(stats.encode_elapsed, Duration::ZERO);
        assert_eq!(stats.append_elapsed, Duration::ZERO);
        assert_eq!(stats.encoded_bytes, 0);
        assert_eq!(stats.wal_bytes_appended, 0);
    }

    #[test]
    fn public_failed_batch_hook_separates_sink_time_from_fetch_time() {
        let mut stats = QueryStats::default();
        stats.record_failed_batch(Duration::from_millis(7));

        assert_eq!(stats.batches, 1);
        assert_eq!(
            stats.fetch_elapsed(Duration::from_millis(19)),
            Duration::from_millis(12)
        );
        assert_eq!(stats.encoded_bytes, 0);
        assert_eq!(stats.wal_bytes_appended, 0);
    }

    #[test]
    fn synchronous_sink_time_extends_the_fetch_deadline() {
        let mut deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let before = deadline;
        extend_fetch_deadline(&mut deadline, Duration::from_secs(7));
        assert_eq!(deadline.duration_since(before), Duration::from_secs(7));
    }

    #[test]
    fn the_cumulative_fetch_deadline_is_thirty_seconds() {
        assert_eq!(QUERY_FETCH_TIMEOUT, Duration::from_secs(30));
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_typed_stream_returns_the_timeout_variant() {
        let (client, driver, server) = connect_to_probe_mode(false).await;
        let mut stats = QueryStats::default();
        let result = read_batched(
            Session::new(&client, 1),
            "SELECT 1::int8",
            std::iter::empty::<(String, tokio_postgres::types::Type)>(),
            0,
            &mut stats,
            |_row| Ok(()),
            |_batch| Ok::<BatchWrite, ()>(BatchWrite::default()),
        )
        .await;
        assert!(matches!(result, Err(BatchError::Timeout)));
        assert_eq!(stats.rows, 0);
        assert_eq!(stats.batches, 0);
        assert!(stats.application_payload_to_postgres_bytes > 0);
        server.abort();
        driver.abort();
    }

    #[tokio::test]
    async fn simple_stream_sends_only_a_simple_query_message() {
        let (client, driver, server) = connect_to_probe().await;
        let mut stats = QueryStats::default();
        let stream = Session::new(&client, 1)
            .simple_stream("SHOW server_version_num", &mut stats)
            .await
            .expect("the simple request is accepted");
        drop(stream);
        drop(client);

        let messages = server.await.expect("the probe task completes");
        driver.abort();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].0, b'Q');
        assert!(messages.iter().all(|(tag, _body)| *tag != b'P'));
        assert!(messages.iter().all(|(tag, _body)| *tag != b'C'));
    }

    #[tokio::test]
    async fn typed_stream_uses_unnamed_parse_bind_without_statement_close() {
        let (client, driver, server) = connect_to_probe().await;
        let mut stats = QueryStats::default();
        let session = Session::new(&client, 9);
        assert_eq!(
            session.generation(),
            9,
            "generation-scoped caches must see the connection generation"
        );
        let stream = session
            .typed_stream(
                "SELECT $1::int8",
                std::iter::once((7_i64, tokio_postgres::types::Type::INT8)),
                size_of::<i64>(),
                &mut stats,
            )
            .await
            .expect("the typed request is accepted");
        drop(stream);
        drop(client);

        let messages = server.await.expect("the probe task completes");
        driver.abort();
        assert_eq!(
            messages.iter().map(|(tag, _body)| *tag).collect::<Vec<_>>(),
            [b'P', b'B', b'D', b'E', b'S']
        );
        let parse = &messages[0].1;
        assert_eq!(
            parse.first(),
            Some(&0),
            "Parse statement name must be empty"
        );
        let bind = &messages[1].1;
        assert_eq!(bind.first(), Some(&0), "Bind portal name must be empty");
        assert_eq!(bind.get(1), Some(&0), "Bind statement name must be empty");
        assert!(messages.iter().all(|(tag, _body)| *tag != b'C'));
    }

    async fn connect_to_probe() -> (
        tokio_postgres::Client,
        tokio::task::JoinHandle<Result<(), tokio_postgres::Error>>,
        tokio::task::JoinHandle<Vec<(u8, Vec<u8>)>>,
    ) {
        connect_to_probe_mode(true).await
    }

    async fn connect_to_probe_mode(
        respond: bool,
    ) -> (
        tokio_postgres::Client,
        tokio::task::JoinHandle<Result<(), tokio_postgres::Error>>,
        tokio::task::JoinHandle<Vec<(u8, Vec<u8>)>>,
    ) {
        let (client_io, server_io) = tokio::io::duplex(16 * 1024);
        let server = tokio::spawn(protocol_probe(server_io, respond));
        let mut config = Config::new();
        config.user("kronika-protocol-test");
        config.dbname("postgres");
        let (client, connection) = config
            .connect_raw(client_io, NoTls)
            .await
            .expect("the probe performs a valid startup handshake");
        let driver = tokio::spawn(connection);
        (client, driver, server)
    }

    async fn protocol_probe(mut io: DuplexStream, respond: bool) -> Vec<(u8, Vec<u8>)> {
        let startup_len = io.read_u32().await.expect("read startup length");
        let mut startup = vec![0; usize::try_from(startup_len - 4).expect("startup fits")];
        io.read_exact(&mut startup)
            .await
            .expect("read startup body");

        write_backend(&mut io, b'R', &0_i32.to_be_bytes()).await;
        write_backend(&mut io, b'Z', b"I").await;

        let mut messages = Vec::new();
        loop {
            let tag = io.read_u8().await.expect("read frontend tag");
            let len = io.read_u32().await.expect("read frontend length");
            let mut body = vec![0; usize::try_from(len - 4).expect("message fits")];
            io.read_exact(&mut body).await.expect("read frontend body");
            messages.push((tag, body));
            if matches!(tag, b'Q' | b'S') {
                break;
            }
        }

        if messages.first().is_some_and(|(tag, _body)| *tag == b'P') {
            if !respond {
                std::future::pending::<()>().await;
            }
            write_backend(&mut io, b'1', b"").await;
            write_backend(&mut io, b'2', b"").await;
            write_backend(&mut io, b'n', b"").await;
            write_backend(&mut io, b'C', b"SELECT 0\0").await;
            write_backend(&mut io, b'Z', b"I").await;
        }
        messages
    }

    async fn write_backend(io: &mut DuplexStream, tag: u8, body: &[u8]) {
        io.write_u8(tag).await.expect("write backend tag");
        let len = u32::try_from(body.len() + 4).expect("test message fits");
        io.write_u32(len).await.expect("write backend length");
        io.write_all(body).await.expect("write backend body");
        io.flush().await.expect("flush backend message");
    }
}
