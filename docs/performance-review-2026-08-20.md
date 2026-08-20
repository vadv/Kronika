# Rust performance review — 2026-08-20

Scope: every non-test `.rs` file in the workspace at `main` (791180e).
Method: ten reviewers read one area each end to end; every finding was then
re-checked against the code by an independent verifier that tried to refute
it (checked callers, buffer bounds, line numbers). Findings the verifier
refuted are not listed. Two areas (write path, web snapshot endpoints) did
not get the verification pass; their findings are listed separately and
should be re-read before acting on them.

This document only records problems. No fix is applied here; the fix
direction on each finding is one line and non-binding.

Path legend: `per-request` — cost paid on every web request; `per-tick` —
every 300 s collection cycle; `per-line` — every followed log line;
`startup`/`cold` — rarely.

## Summary

| Area | high | medium | low | verified |
|---|---|---|---|---|
| kronika-reader, kronika-index, kronika-dump | 2 | 3 | 3 | yes |
| kronika-registry | 1 | 2 | 3 | yes |
| kronika-web: everything else | 4 | 3 | 0 | yes |
| kronika-web: /snapshot, relation, search | — | — | — | no (16 findings) |
| kronika-source-log | 0 | 4 | 5 | yes |
| kronika-source-os | 1 | 1 | 7 | yes |
| kronika-source-pg | 0 | 2 | 1 | yes |
| kronika-collector | 0 | 4 | 2 | yes |
| kronika-layout, kronika-format, kronika-derive | 0 | 1 | 4 | yes |
| kronika-writer, kronika-store | — | — | — | no (5 findings) |
| **Total verified** | **8** | **20** | **25** | 53 |

## Read path (kronika-reader, kronika-index, kronika-dump)

### `crates/kronika-index/src/build.rs:230` — catalog_segments(..min_ts) walks and re-parses all history per active-index build

**high** · per-request

build_selected_from_reader lists every finished segment since the beginning of the store (range ..segment_ref.min_ts()) although only the 15-minute comparison window plus one predecessor is consumed. With list_segments re-opening and re-parsing the catalog of every overlapping unit (see lib.rs finding), a store with months of 300s segments pays tens of thousands of file opens and catalog parses on every request that touches the active segment.

Fix direction: Bound the listing range to (window_start - one segment margin)..min_ts instead of unbounded start.

### `crates/kronika-reader/src/lib.rs:227` — list_segments re-opens and re-parses every catalog scan_catalogs already parsed

**high** · per-request

scan_catalogs (kronika-store local.rs:110, FinishedValidation::Catalog) already opened each .zms and fully parsed its catalog to produce unit.summary, but the summary drops the entry table. list_segments then does open_finished + read_catalog again for every overlapping unit solely to compute the per-type sections table (sections_of). Web calls catalog_segments(..) full-range per request (bins/kronika-web/src/api.rs:235, api/hour.rs:55), so every finished segment gets two opens and two full catalog parses per request; Segment::open later parses a third time.

Fix direction: Carry per-type rows/bytes aggregates (or the parsed catalog) out of the scan into FinalUnit/SegmentRef instead of a second open+parse.

### `crates/kronika-index/src/detect/direct.rs:593` — find_active_backends re-decodes activity sections and dictionary already decoded in same build

**medium** · per-request

Within one index build (build_selected / build_selected_from_reader, run per request for the active segment since active indexes are never persisted), active_snapshots() repeats exactly what build.rs active_backend_points() already did for the same type_id: visit_rows("state"), dictionary_for(ids) (full dict.strings section read + CRC + two parquet decodes), then visit_rows("ts","state"). Combined with finding below, each pg_stat_activity section body is read from disk, CRC32C-checked and parquet-decoded 4 times, and the ~1MiB dictionary section twice, per build.

Fix direction: Compute per-timestamp active-backend samples once per build and share them between the series block and the finding detector.

### `crates/kronika-index/src/build.rs:572` — active_backend_points decodes the same section twice (state pass, then ts+state pass)

**medium** · per-request

visit_rows(type_id, ["state"]) at line 572 fully reads+CRCs+parquet-decodes the section to collect StrIds, then visit_rows(type_id, ["ts","state"]) at line 591 reads+CRCs+decodes the identical body again. Each Segment::visit_rows call allocates a fresh Vec of the compressed body, checksums it, and re-parses the parquet footer. Same double-pass pattern is copied in detect/direct.rs active_snapshots (lines 598 and 616).

Fix direction: One pass collecting (ts, state_id) pairs, resolve the dictionary afterwards, then count — halves section decodes.

### `crates/kronika-index/src/detect/direct.rs:401` — pg_stat_database section decoded twice per build with overlapping projections

**medium** · per-request

transaction_points (build.rs:508, columns ts/datid/xact_commit/xact_rollback) and find_deadlocks (direct.rs:401, columns ts/datid/deadlocks) each independently read the full compressed section, CRC it, and run a parquet decode within the same index build. Two full body reads and decodes where one pass with the union projection feeds both consumers.

Fix direction: Single visit_rows with the union of columns feeding both the TPS series and deadlock detection.

### `crates/kronika-index/src/build.rs:366` — instance_metadata section decoded three times per index build

**low** · per-request

One build visits the same one-row instance_metadata section via health_metadata (build.rs:366), visit_stall_snapshots for environment/btime (build.rs:675), and postgres_cpus (direct.rs:573). Each visit pays a section read, CRC, thrift footer preflight, and arrow reader construction; the row is one, so metadata parsing dominates and is paid 3x. predecessor_health_seed repeats the same on the predecessor segment.

Fix direction: Read instance_metadata once per build into one struct covering all consumers (ts, enabled, cpus, interval, environment, btime).

### `crates/kronika-reader/src/dictionary.rs:137` — decode_selected parses the parquet footer of each dictionary section three times

**low** · per-request

dictionary_builder runs plain_parquet_decode_profile (thrift footer walk) plus ParquetRecordBatchReaderBuilder::try_new_with_options (footer parse #2) at line 115, then line 137 constructs a second builder over the same Bytes, parsing the footer and rebuilding the arrow schema a third time. dictionary_for is called per request from web (api/query.rs:211, api/hour/lanes.rs:262) and twice per index build, so the redundant metadata parses multiply.

Fix direction: Reuse the first builder's ArrowReaderMetadata for the value-projection reader instead of re-parsing.

### `bins/kronika-dump/src/render.rs:305` — dump section command decodes full dictionary and materializes all rows despite --limit

**low** · cold

render::section always calls segment.dictionary() (complete dict.strings + dict.blobs decode, ~MiB scale) even for sections with no StrId columns, and segment.rows(type_id) (line 310) collects every row of the section into a Vec of Rows before take(limit) trims the printout. Dumping 10 rows of a large section still decodes and holds the entire section's rows plus the whole dictionary in memory — the dump binary's peak RSS scales with section size instead of the limit.

Fix direction: visit_rows with the limit, resolve only the StrIds actually printed via dictionary_for, and take the total row count from rows_of.

## Registry codecs (kronika-registry)

### `crates/kronika-registry/src/generic.rs:290` — visit_rows allocates a full-contract-width Vec<Cell> per row regardless of projection

**high** · per-request

In visit_rows_with the inner row loop does `let mut cells = vec![Cell::Null; contract.columns.len()]` for every visited row. Cell is 32 bytes (holds a Vec<i32> variant), so for pg_stat_statements V6 (55 columns) that is ~1.8 KB heap-allocated per row even when the web projects 2-3 columns (kronika-web and kronika-index callers typically project 1-8 columns). A 65k-row section scan performs 65k allocations and ~115 MB of churn per request; reader.rows() keeps all of it live. This is the production web decode path (kronika-web api/* -> kronika-reader::visit_rows -> kronika_registry::visit_rows).

Fix direction: Size the cells vector to the projection (Row carries the projected column positions), keeping full width only for the decode_rows all-columns wrapper.

### `crates/kronika-registry/src/generic.rs:233` — Parquet footer thrift is parsed twice per section decode (preflight + reader builder)

**medium** · per-request

visit_rows_with calls validate_parquet_decode_work (parse_footer -> FileMetaData::read_from_in_protocol, which heap-allocates a String per schema element and per column-chunk path across all row groups) and then ParquetRecordBatchReaderBuilder::try_new_with_options re-parses the same footer from scratch. capped_reader in codec/decode.rs:21-23 has the identical double parse for decode_section/decode_batches. For wide contracts (55 columns x up to 16 row groups) that is hundreds of redundant String allocations plus a second full thrift walk per section per request.

Fix direction: Parse the footer once (ParquetMetaDataReader) and hand the metadata to the builder via new_with_metadata, as finalize.rs::section_reader_builder already does.

### `crates/kronika-registry/src/generic.rs:88` — Row::get does a linear column-name scan on every call, multiplied per row by web visitors

**medium** · per-request

Row::get walks contract.columns with a string compare per position on every lookup. kronika-web visitors (api/hour/lanes.rs, api/rows.rs) and kronika-index detect call row.get(name) several times inside the per-row visit_rows closure, so a 40-column contract with 8 gets per row costs ~300 string compares per row, repeated over tens of thousands of rows per request. The name-to-position mapping is invariant for the whole scan.

Fix direction: Expose a once-per-scan name-to-position resolution (e.g. a contract position lookup callers hoist, or hand visitors the projected indices) so per-row access is cells[at].

### `crates/kronika-registry/src/generic.rs:306` — cell_at repeats the dyn-Any array downcast for every cell of every row

**low** · per-request

The row loop at generic.rs:289-300 calls cell_at per projected column per row, and cell_at re-does `array.as_any().downcast_ref::<...>()` (via typed()) on the same batch array for every row. For an 8-column projection over a 65k-row scan that is ~0.5M redundant TypeId-compare downcasts per section on the web request path; the downcast result is invariant for the lifetime of the batch and can be established once per batch alongside the `arrays` vector built at line 279.

Fix direction: Downcast each projected column once per batch into a typed accessor (small enum mirroring ColumnType) and have the row loop read through it.

### `crates/kronika-registry/src/codec/columns.rs:208` — validate_list_i32_array materializes an Arc'd child slice and re-downcasts per row

**low** · per-tick

The validation loop calls `array.value(i)` (allocates a new ArrayData + Arc slice per row), downcasts it, and checks its null_count per row. The child downcast and null check can be done once on `array.values()` for the whole column, and per-row lengths already come from value_length(i) without materializing slices. This runs per batch in segment finalize (encode_final_sections_to -> validate_list_i32_batch in both the ordered and column-projection paths) and on every typed decode/encode of list-bearing sections such as pg_locks, so it is O(rows) avoidable allocations under the 25 MiB collector. ListColumn::value at columns.rs:126 repeats the same per-row value()/downcast pattern.

Fix direction: Downcast array.values() once, check its null_count once, and keep only the per-row value_length checks in the loop.

### `crates/kronika-registry/src/codec/columns.rs:78` — write_list_i32 takes owned Vec<i32> rows, forcing the derived encoder to clone every list

**low** · per-tick

write_list_i32 accepts `impl Iterator<Item = Vec<i32>>` but only reads row.len() and iterates the values. The derive-generated encoder (crates/kronika-derive/src/generate.rs:73) therefore emits `rows.iter().map(|r| r.field.clone())`, heap-cloning each row's blocked_by vector on every pg_locks encode each 300 s tick. A borrowed item type removes the clone entirely.

Fix direction: Change the parameter to iterate borrowed slices (Item = &[i32] or AsRef<[i32]>) and drop the .clone() in the generated encoder.

## Web: routing, hour, history, catalog (kronika-web: everything else)

### `bins/kronika-web/src/api/query.rs:192` — Dictionary sections re-read, re-CRCed and re-scanned for every 16-row chunk

**high** · per-request

chunk_dictionary() calls Segment::dictionary_for per chunk, and every call re-reads the full dict.strings/dict.blobs bodies from disk, CRC32Cs them, and rescans the id column (kronika-reader/src/segment.rs:339 -> finished_body). Callers invoke it every ROW_CHUNK_ROWS=16 rows: history.rs:220 (emit_chunk, used by /history and the /hour series path via stream_plans), rows.rs:251 (asc) and rows.rs:325 (desc). Streaming a string-bearing section (pg_stat_activity, pg_stat_statements) of 65k rows costs ~4096 full dictionary-section reads (dictionary is ~1 MiB); a 1000-row page costs ~63.

Fix direction: Read and verify the dictionary bodies once per stream and resolve chunk ids against the retained verified bytes (or raise chunk size substantially).

### `bins/kronika-web/src/api/rows.rs:308` — Desc paging re-reads the whole section body for every <=16-row chunk

**high** · per-request

desc() loops `while upper > plan.start_row` calling segment.visit_rows(lower, limit<=16); each call reads the entire section body (up to MAX_SECTION_BYTES = 8 MiB) via read_exact_at, CRC32Cs it, and rebuilds a ParquetRecordBatchReaderBuilder, just to decode 16 rows. One desc page of page_size=1000 performs ~63 full body reads + CRCs + reader builds; chunk_rows is capped at 16 regardless of page_size, so the cap makes it worse, not better.

Fix direction: Read/verify the body once per plan and decode backwards from it (or set chunk size = page_size+1 so one visit covers the page).

### `bins/kronika-web/src/api.rs:235` — explicit_segment lists the whole store to find one segment by id

**high** · per-request

explicit_segment_with_listing calls reader.catalog_segments(..) with an unbounded range; list_segments (kronika-reader/src/lib.rs:220-240) then does open_finished + read_catalog + validate_finished_file for EVERY finished segment in the store. index.rs:32, history.rs:32 and rows.rs:46 immediately discard the listing and keep only the one segment whose id was in the URL. With a month of 300s segments that is thousands of file opens and catalog parses per /index, /history and /rows request; the id needed for the match is in unit.address before any file is opened.

Fix direction: Locate the unit by id in the scan summaries first and open/read only that segment's catalog.

### `bins/kronika-web/src/api/hour.rs:55` — Hour endpoint opens every finished segment's catalog for the whole store

**high** · per-request

prepare() calls reader.catalog_segments(..) full-range on every /hour request (the UI landing call). available_hours needs only min_ts/max_ts, which live in the pre-scanned unit summaries, yet catalog_segments opens each finished file and parses its catalog to fill SegmentRef.sections for all segments — including the vast majority outside the requested window that are filtered out three lines later.

Fix direction: Compute hours from scan summaries and open catalogs only for segments overlapping the window.

### `bins/kronika-web/src/api/hour/lanes.rs:311` — Lane points recomputed and re-sorted over all accumulated segments (quadratic)

**medium** · per-request

State/Counters BTreeMaps carry every timestamp ever seen across the request, and collect() -> current_points() calls points() per segment, which walks all 15 maps in full, allocates a Vec of lane points for the ENTIRE accumulated window, sorts it, then filters down to just the current segment's [min_ts, max_ts]. With S segments in a wide from/to window this is O(S^2) point construction plus S sorts, and memory grows with the whole window although rate continuity only needs the last sample per map.

Fix direction: After emitting a segment, prune each map to its last sample and compute/sort points only for the new segment's range.

### `bins/kronika-web/src/api/hour.rs:349` — facts() fully re-scans os_cpu per segment before collect() scans it again

**medium** · per-request

emit_lanes calls lanes::facts(&segment), which visit_rows the entire os_cpu section (every per-CPU row) just to collect distinct cpu_ids, then lanes::collect -> read_cpu visits the same section again. Each visit_rows is a full body read + CRC32C + Parquet decode from kronika-reader, so the largest OS section is read and decoded twice per segment on every /hour request.

Fix direction: Collect cpu ids and ticks during read_cpu's pass and derive capacity before rate computation, dropping the separate facts scan of os_cpu.

### `bins/kronika-web/src/api/hour/lanes.rs:249` — read_activity buffers every pg_stat_activity row of a segment in memory

**medium** · per-request

read_activity pushes every visited Row into `rows: Vec<Row>` before resolving the dictionary. Each Row carries one Cell per contract column (Cell is ~32 bytes and pg_stat_activity has dozens of columns, mostly Cell::Null padding for unprojected ones), so a section at the 65,536-row cap holds tens of MiB transiently — an input-proportional buffer in a deliberately memory-bounded binary, while only 6 fields per row are ever consumed.

Fix direction: Buffer a small per-row tuple of just the needed cells (ts, ids, leader/xact presence) instead of full-width Rows.

## Log source (kronika-source-log)

### `crates/kronika-source-log/src/tail.rs:405` — Every retained line is heap-copied a second time in add_to_open

**medium** · per-line

`take_physical_line` already produces an owned String (String::from_utf8 consumes partial.bytes with no copy), but add_to_open borrows it via truncate() and then does `retained.to_owned()`, allocating and memcpy-ing the full line again. In the common case nothing is cut (retained.len() == text.len()), so this is one avoidable malloc+copy per physical line of every followed log — the hottest multiplier in the crate.

Fix direction: Truncate the owned `text` in place (text.truncate(retained.len())) and push it, instead of to_owned() on the borrowed slice.

### `crates/kronika-source-log/src/postgres/normalize.rs:289` — to_ascii_lowercase() re-allocated inside the permission-object loop

**medium** · per-line

replace_word_patterns computes `out.to_ascii_lowercase()` inside the `for object in [...]` loop, so every WARNING+ record pays up to 5 full lowercase copies (each up to ~5 KiB) plus 5 substring scans just to test the five 'permission denied for ...' markers. On an error-flooded server this runs for every error line in the batch.

Fix direction: Hoist the lowercase copy out of the loop (compute once before iterating the object markers).

### `crates/kronika-source-log/src/postgres/normalize.rs:59` — normalize_error allocates ~12 full copies of the message per call

**medium** · per-line

The chain strip_at_character().to_owned() -> replace_quoted x2 -> replace_delimited x2 -> replace_word_patterns (to_owned + 8 replace_after calls + replace_wal_address) -> final truncate().to_owned() builds a fresh full-size String at every step. Worse, replace_after (line 302) returns `value.to_owned()` even when the prefix is absent, so the 8 marker probes each copy the whole message on a miss. Runs for every WARNING+ record and, via normalize_sql, for every slow-query statement (input up to 5 KiB).

Fix direction: Make the replace_* passes operate on/return Cow<str> (borrow on no-match) or mutate one working String in place instead of reallocating per pass.

### `crates/kronika-source-log/src/postgres/csvlog.rs:55` — csvlog split materializes all 23+ fields as owned Strings per record

**medium** · per-line

split() copies the whole record char-by-char into a Vec<String> of every column (~26 allocations per record on PG13+), although parse() reads only 9 columns by index; the 9 used ones are then copied again by bounded(). Combined with the joined() copy in the caller, each csvlog record's text is duplicated three times and 17 columns are allocated purely to be dropped.

Fix direction: Split into borrowed &str ranges (owned only for fields containing "" escapes) and stop materializing columns beyond the ones parse() reads.

### `crates/kronika-source-log/src/postgres/events.rs:331` — Events::add takes &PgRecord, forcing clones of up-to-5 KiB strings

**low** · per-line

add() receives a reference although the only caller (PgLog::read_batch, postgres.rs:235-237) drops `parsed` right after the call. Consequently add_error clones sqlstate/detail/hint/context/statement/database/username plus re-copies message as sample (each up to MAX_TEXT_BYTES=5120 B) for every new error group, and add_log clone_froms detail/context/statement for every lock-wait and temp-file record. All of these could be moves.

Fix direction: Pass PgRecord by value into Events::add and move its fields into the event structs.

### `crates/kronika-source-log/src/postgres.rs:263` — record.joined() allocates a full record copy even for single-line records

**low** · per-line

For csvlog every record is re-joined with lines.join("\n"), which always allocates, yet the overwhelmingly common case is a single-line record where record.first() would borrow. This is one full-record copy per csvlog record before parsing even starts.

Fix direction: Borrow first() when lines.len() == 1 (Cow), joining only true multi-line records.

### `crates/kronika-source-log/src/tail.rs:339` — Quote-parity memchr pass over every byte for formats that ignore it

**low** · per-line

keep_partial runs quote_parity() (a memchr_iter count of '"') over every chunk of every followed file, but raw_quotes_odd is consumed only by the csvlog continues; stderr, jsonlog and pgbouncer ignore the parameter. Non-CSV logs — including the continuously tailed PgBouncer log — pay an extra full scan of all input bytes for a value nobody reads.

Fix direction: Gate quote tracking on a flag set by the format (only csvlog needs parity).

### `crates/kronika-source-log/src/postgres/stderr.rs:44` — stderr continues runs five substring searches per line, repeated in parse

**low** · per-line

For every physical line of a stderr log, continues() calls line.contains() for all five PARTS markers (each a full scan on a miss, the normal case for severity lines). parse() then re-searches the same five markers via find_marker over every continuation line it just classified, so marker scanning is done twice per continuation line.

Fix direction: Do one scan (e.g. locate ':' candidates once, or have continues/parse share the found marker position).

### `crates/kronika-source-log/src/tail.rs:251` — 64 KiB read buffer allocated and zeroed on every read_batch, even idle polls

**low** · per-tick

scan() creates vec![0u8; 65536] unconditionally before checking whether there is anything to read, so every poll of every followed file — including idle polls where scan_offset >= size — pays a 64 KiB zeroed allocation. Tailing polls continuously between ticks, multiplying this across files and polls.

Fix direction: Keep the buffer as a reusable field on Tail (or allocate lazily only when scan_offset < size).

## OS source (kronika-source-os)

### `crates/kronika-source-os/src/cgroup.rs:605` — collect_workloads buffers every /proc/PID/cgroup content in a Vec<String>

**high** · per-tick

All membership file contents are materialized before parsing (memberships.push(content) at line 612). With 30k processes at ~200-1000 bytes each of raw cgroup text this holds 6-30 MB simultaneously — alone enough to breach the 25 MiB RSS budget. collect_workload_memberships already accepts IntoIterator<Item: AsRef<str>>, so nothing requires materialization; each content could be parsed into the bounded WorkloadCgroupPaths sets and dropped. (The collector bin mirrors this same buffering in os_sources/process.rs:71.)

Fix direction: Stream: read each PID's cgroup file, feed it to parse_self_cgroup/paths.insert, drop it — pass a lazy iterator instead of a Vec

### `crates/kronika-source-os/src/fs.rs:149` — read_raw allocates a fresh String (plus PathBuf join) for every procfs read

**medium** · per-tick

Each read starts with String::new() and grows via read_to_string (~5-7 reallocs+memcpys for a typical 1-4 KB status file since Take<&mut File> gives no size hint and procfs stat size is 0), plus a PathBuf allocation from root.join(). The per-process loop does ~7 reads per PID (stat, status, io, schedstat, cmdline, comm, cgroup); at 30k processes that is 200k+ transient String/PathBuf allocations per tick with zero reuse across iterations.

Fix direction: Add a read_into(&mut String) variant (clear + read) so the process loop reuses one buffer; optionally pre-size with with_capacity

### `crates/kronika-source-os/src/proc/process.rs:70` — Six format! path Strings heap-allocated per process per tick

**low** · per-tick

read_process_with_cgroup and its helpers build "{pid}/stat", "{pid}/status", "{pid}/io", "{pid}/schedstat", "{pid}/cmdline", "{pid}/comm" (and the caller adds "{pid}/cgroup") as fresh Strings on every call. All downstream consumers (read_required, read_raw) take &str, and the owned form is only needed on error paths. 30k processes x ~7 formats = ~200k short-lived allocations per tick purely for path text.

Fix direction: Reuse one String per process (or per loop): clear + write!(buf, "{pid}/{file}"); own the path only in the error branch

### `crates/kronika-source-os/src/proc/process/parse.rs:35` — parse_stat collects all ~44 stat fields into a Vec per process

**low** · per-tick

rest.split_whitespace().collect::<Vec<&str>>() allocates a ~700-byte Vec for every process just to index 18 fixed positions (max index 39). At 30k processes that is 30k Vec allocations per tick; a fixed [&str; 40] filled from the iterator (or positional next() consumption) needs no heap at all.

Fix direction: Fill a stack array of the first 40 tokens from the iterator instead of collect()

### `crates/kronika-source-os/src/cgroup.rs:86` — WorkloadCgroupPaths::insert allocates path.to_owned() before the dedup check

**low** · per-tick

paths.insert(path.to_owned()) builds an owned String even when the path is already in the BTreeSet. In production 30k process memberships collapse onto a few dozen distinct cgroup paths, so nearly all 30k+ inserts per tick allocate a String only to drop it immediately.

Fix direction: Check paths.contains(path) (Borrow<str>) first; to_owned() only on actual insert

### `crates/kronika-source-os/src/cgroup.rs:308` — parse_self_cgroup allocates two Strings per cgroup line, even for repeats

**low** · per-tick

For every line of every process's membership file, normalize_self_cgroup_path returns an owned String (though most paths need no rewriting), and set_exact_path then calls path.to_owned() again — including the current == path branch, which reallocates and replaces an identical stored String. Called once per membership in collect_workload_memberships: with 30k processes (and multiple controller lines each on v1) this is 60k-600k avoidable allocations per tick.

Fix direction: Return Cow/&str from normalize when unchanged; in set_exact_path skip reallocation when current == path

### `crates/kronika-source-os/src/proc/process.rs:215` — read_cmdline copies the command line twice (replace then trim().to_owned())

**low** · per-tick

content.replace('\0', " ") allocates a full copy, then .trim().to_owned() allocates a second copy of what can be a multi-KB cmdline — two extra full copies per process on top of the read buffer, x30k processes per tick.

Fix direction: Trim the raw content first (as &str, including NULs in the trim set), then do a single replace — one allocation

### `crates/kronika-source-os/src/fs.rs:137` — ProcFs/SysFs read() copies the whole trimmed content into a second String

**low** · per-tick

read() (and SysFs::read at line 237) does trimmed.to_owned() over the freshly read String, doubling allocations and memcpy for every trimmed read. SysFs::read is the workhorse of cgroup collection: up to 512 workload cgroups x ~6-10 controller files per tick, each paying read-buffer + full copy.

Fix direction: Trim in place: truncate for trailing whitespace, drain(..n) for leading, return the original String

### `crates/kronika-source-os/src/cgroup.rs:1008` — read_first_v1 retries up to 5 controller-dir spellings per file per cgroup

**low** · per-tick

Every v1 metric read probes candidate mount spellings (cpu,cpuacct / cpuacct,cpu / cpu / cpuacct / "") with a format! + open() each until one succeeds. The winning spelling is fixed for the mount's lifetime, yet read_cpu_v1 alone issues 5 read_first_v1 calls per cgroup; on hosts where the first candidate misses this wastes up to ~20 failed open() syscalls and format Strings per cgroup, x512 cgroups per tick.

Fix direction: Resolve each controller's directory once per collection (as bind_v1_controller_root does) and reuse it for all cgroups/files

## PostgreSQL source (kronika-source-pg)

### `crates/kronika-source-pg/src/query.rs:450` — read_batched builds a CancelToken and a tokio timer for every fetched row

**medium** · per-tick

The fetch loop wraps every stream.try_next() in timeout_at(session, ...), and timeout_at (line 295) calls session.client.cancel_token() on each invocation. In tokio-postgres 0.7 cancel_token() clones SocketConfig, which heap-allocates hostname: Option<String> (and Addr/PathBuf for unix sockets); tokio::time::timeout_at additionally registers/deregisters a timer-wheel entry per call. The send_cancel future built from the token is dropped unused on every successful row. This runs once per row across every batched source in the crate: up to pg_stat_statements.max (~5000) statements rows, table/index counts per database (can be 10k+), activity rows, etc. — tens of thousands of needless allocations plus timer ops per 300s tick, purely on the happy path.

Fix direction: Create the cancel token once per query outside the row loop (or build it lazily only in the Err(elapsed) branch of timeout_at_with_cancel).

### `crates/kronika-source-pg/src/statements.rs:646` — Per-row column lookup by name is O(columns^2) string comparisons per row

**medium** · per-tick

row_from_pg decodes ~55 fields with row.try_get("name"). tokio-postgres RowIndex for str resolves each name with columns.iter().position(|d| d.as_name() == self) — a linear scan with string equality (plus a second case-insensitive scan on miss). With ~55 columns that is ~1.5k name comparisons per row; at pg_stat_statements.max = 5000 rows that is ~7.5M comparisons per tick for this one source. The same name-based decode pattern is in every decoder: store_plans.rs (ossc/datasentinel/vadv ~35-40 cols), database.rs (~35), user_tables.rs row_from_pg at line 491 (~50 cols × table count per DB, can exceed statements in row volume), user_indexes.rs, activity.rs, locks.rs, io.rs, progress_vacuum.rs. The crate composes the SQL itself, so the column order is fully under its control and stable for the whole stream.

Fix direction: Resolve column name→ordinal once per stream (from the first row's columns()) and decode by usize index, or decode positionally since the SQL fixes column order.

### `crates/kronika-source-pg/src/user_tables.rs:493` — database.name.clone() allocates the same constant String for every table row

**low** · per-tick

row_from_pg stores datname: database.name.clone() into every UserTablesRow — one fresh heap String of an identical value per table, per database, per tick (a database with 10k+ tables pays 10k+ allocations that the interner immediately collapses back to one StrId in to_v1..to_v4). The same pattern is at user_indexes.rs:218 (one clone per index row) and settings.rs:88-89 (datname.to_owned() + usename.to_owned() for each of ~350 pg_settings rows). Retained memory stays bounded by the 256-row batch, so this is allocator churn, not RSS growth.

Fix direction: Drop the constant from the per-row struct: pass datname/datid alongside the batch to the to_vN converters (or store a pre-interned StrId / shared Arc<str> in the row).

## Collector binary (kronika-collector)

### `bins/kronika-collector/src/log_sources.rs:366` — log.offsets rewritten and fsynced after every acknowledged log batch

**medium** · per-tick

save_offsets() is called inside the per-batch ack loop (lines 366 and 434). Offsets::save (crates/kronika-source-log/src/offsets.rs:65) creates a temp file, renders the whole offsets map, write_all + sync_all (fsync) + rename — per batch. Steady state that is one fsync per followed file per log tick (10s); during catch-up a single source can ack up to 64 batches per cycle (MAX_SOURCE_READ_BYTES 256MiB / MAX_READ_BYTES 4MiB), i.e. 64 fsync+rename cycles back to back on the log path. The module's own contract (log_sources.rs:461) says a lost save only duplicates rows already in the WAL, so per-batch durability buys nothing.

Fix direction: Save offsets once at the end of each collect pass (per source or per cycle) instead of after every batch.

### `bins/kronika-collector/src/os_sources.rs:279` — /proc/self/mountinfo parsed on ticks that never use it

**medium** · per-tick

collect_os_sources calls procfs_sections::mountinfo_entries(fs) unconditionally after the early-return guard, but the result is only consumed when OsCore (diskstats device filter) or OsMountTopo (mountinfo rows) is due — exactly what the comment above claims and the code does not do. With default intervals (tick 5s, os_processes 5s, os_core 10s) roughly every other tick is OsProcesses/OsProcessStatus/OsCgroupMapping-only and still reads and parses the whole mount table (hundreds of lines on k8s nodes, one Vec<MountEntry> with 4 heap Strings per entry) plus resolve_major_zero sysfs reads, then throws it away.

Fix direction: Gate the mountinfo_entries call on due.has(OsCore) || due.has(OsMountTopo).

### `bins/kronika-collector/src/os_sources.rs:280` — Dead field mount_entries deep-clones the whole mount table per window

**medium** · per-tick

os.mount_entries.clone_from(&mounts) deep-clones every MountEntry (4+ Strings each: root, mount_point, fstype, source) into an OsSources field that is never read anywhere in the crate — grep finds only the declaration (line 105), the empty init (line 143), and this clone. Pure allocation churn plus retained memory for the lifetime of the OsSources value, on every window that collects any OS source, under a 25 MiB RSS ceiling.

Fix direction: Delete the mount_entries field and the clone_from line.

### `bins/kronika-collector/src/pg_sources.rs:1544` — settings comparison clones every SettingsRow just to ignore ts

**medium** · per-tick

settings_equal_ignoring_ts does `let mut normalized = left.clone(); normalized.ts = right.ts;` per row. SettingsRow carries ~7 heap Strings (datname, usename, name, setting, unit, source, sourcefile, ...), and pg_settings returns ~350+ rows, so each Pg-instance tick (default 30s) that reaches read_settings allocates and immediately drops ~2500 Strings solely to compare rows while ignoring the timestamp.

Fix direction: Compare fields directly (all except ts) without cloning, e.g. a helper comparing borrowed fields.

### `bins/kronika-collector/src/main.rs:506` — detect_container re-reads /proc/1/cgroup for every buffered window

**low** · per-tick

buffer_pg_batch (main.rs:505-506) and buffer_window (main.rs:779-780) each call ProcFs::from_env() + detect_container(&fs) per window. On a bare-metal host detect_container does two env lookups, a /.dockerenv stat, and an open+read+scan of /proc/1/cgroup (crates/kronika-source-os/src/scope.rs:55). Every admitted PG batch is its own window (~15-30 per Pg tick: archiver, bgwriter, checkpointer, wal, database, io, activity, locks, statements sub-batches, relations...), plus one per log batch window — so dozens of identical /proc reads per tick for a value that cannot change during the process lifetime. For non-opening PG batches the result is not even used.

Fix direction: Detect once at startup (store in Config or a field) and pass the bool down.

### `bins/kronika-collector/src/os_sources/process.rs:79` — Per-pid clone of /proc/<pid>/cgroup contents on cgroup ticks

**low** · per-tick

When cgroup collection is due (in-container, every 30s), the membership String read from /proc/<pid>/cgroup is cloned into cgroup_memberships for every pid, only because read_process_with_cgroup takes Option<String> by value (crates/kronika-source-os/src/proc/process.rs:63). One extra heap String per process per cgroup tick; on a node-agent watching thousands of pids that is thousands of avoidable allocations per tick.

Fix direction: Change read_process_with_cgroup to borrow the membership (Option<&str>) and move the owned String into the vec.

## Segment layout and binary format (kronika-layout, kronika-format, kronika-derive)

### `crates/kronika-layout/src/root.rs:410` — open_zms re-resolves year/month/day directories for every segment open

**medium** · per-request

`open_day` performs 3 openat syscalls (plus 3 format! String allocations from year/month/day_component and one more for zms_name) on every open_zms/open_idx call, and the API exposes no way to reuse the verified day descriptor. Web handlers open many segments per request (e.g. process_summary.rs loops reader.open_segment -> open_finished -> open_zms), so a day view over 288 five-minute segments costs ~900 redundant openat/close pairs per request for segments that all live in the same day directory.

Fix direction: Expose a day-handle (or batch open) so consecutive opens in the same UtcDay reuse one verified descriptor.

### `crates/kronika-layout/src/root/names.rs:130` — parse_leaf allocates a Vec<&str> per directory entry on every scan

**low** · per-request

`let fields: Vec<&str> = name.split('.').collect();` heap-allocates a Vec for every leaf entry of every day directory. The web binary runs a full DataRoot::scan per request (bins/kronika-web/src/api.rs:234 -> Reader::open -> scan_catalogs -> root.scan at crates/kronika-store/src/local/scan.rs:253), so with thousands of segment/idx/tmp files this is thousands of short-lived allocations per request, plus twice at collector startup. The split has at most 6 components and only fixed shapes are accepted.

Fix direction: Match components without collecting, e.g. chained split_once / splitn into a fixed-size array.

### `crates/kronika-layout/src/root/names.rs:175` — parse_canonical_i64/u64 allocate a String round-trip per name component

**low** · per-request

`(parsed.to_string() == value).then_some(parsed)` (also line 183) allocates a String for every id/pid/seq component of every leaf name — up to 3 allocations per temporary file, 1 per final file, on every layout scan, which the web runs per request. The only non-canonical form the round-trip still catches after the manual prefix checks is a leading '+' accepted by u64::from_str; an ASCII-digit byte check would reject it without allocating.

Fix direction: Replace the to_string comparison with a bytes().all(is_ascii_digit) pre-check (plus optional leading '-' for i64).

### `crates/kronika-derive/src/generate.rs:73` — Generated ListI32 encode clones every row's Vec<i32>

**low** · per-tick

The derive emits `rows.iter().map(|r| r.#field.clone())` because kronika_registry::write_list_i32 takes `impl Iterator<Item = Vec<i32>>`, yet its body only reads len() and iterates values (crates/kronika-registry/src/codec/columns.rs:76). Every encode of a section with a ListI32 column (e.g. pg_locks blocked_by) does one heap allocation + copy per row per tick in the collector, then immediately drops it.

Fix direction: Change write_list_i32 to accept borrowed rows (Iterator<Item = &[i32]>, use append_slice) and drop the clone in the derive.

### `crates/kronika-format/src/parts/frame.rs:311` — build_part allocates a temporary catalog buffer despite write_encoded existing

**low** · per-tick

`out.extend_from_slice(&catalog.encode())` builds a second catalog-sized Vec (entries*32 + 40 + 8 bytes) and copies it into `out`, although `Catalog::write_encoded` was written precisely to avoid the second buffer (catalog.rs:184, doc: "without allocating a second catalog-sized buffer") and Vec<u8> implements io::Write. One avoidable allocation + copy per journal part appended, i.e. per collector tick and per recovery rewrite.

Fix direction: Call catalog.write_encoded(&mut out) instead of extend_from_slice(&catalog.encode()).

## Unverified areas

The verifier pass did not run for these two areas. Line numbers and claims
come from a single reviewer and were not independently re-checked.

## Web: snapshot endpoints (kronika-web: /snapshot, relation, search) — unverified

### `bins/kronika-web/src/api/snapshot/relation.rs:3370` — scan_context restarts visit_rows per 16-row chunk: O(N/16) full section re-reads

per-request · unverified

The relation drill-down page loop calls source_segment.visit_rows(offset, SNAPSHOT_CHUNK_ROWS=16) in a while loop, restarting the scan for every chunk. On a finished segment each visit_rows call re-reads the ENTIRE section body from disk (finished_body: Vec alloc + read_exact_at), CRC32Cs it, and rebuilds a Parquet reader (segment.rs:240-252, generic.rs:225-273). A 10k-row pg_stat_user_tables section (~1-2 MB body) costs ~625 full body reads + CRCs + reader builds per context, per page request. Each chunk additionally triggers a full dictionary-section read at line 3400. scan_page (snapshot.rs:2306) and scan_history_plan (relation.rs:3100) already do the correct single-pass with internal chunk accumulation.

Fix direction: One visit_rows pass over the section accumulating chunks inside the visitor, like scan_page does.

### `bins/kronika-web/src/api/snapshot.rs:1107` — Dictionary section re-read + CRC once per 16-row chunk on snapshot emit and page scan

per-request · unverified

emit_context_chunk calls retained_dictionary per SNAPSHOT_CHUNK_ROWS=16 chunk; each call is Segment::dictionary_for, which reads and CRC-verifies the entire dict.strings and dict.blobs bodies from disk (segment.rs:339-362, finished_body). The dict section holds all query texts and can be MBs. The default full-snapshot emit of any timed/partitioned section pays rows/16 full dictionary-body reads; the same pattern hits the paged path in rank_page_chunk (line 2361) whenever a Bytes filter, search, or text ordering is active — a 5k-row section with search means ~312 full dictionary reads per request.

Fix direction: Raise the chunk size substantially (memory stays bounded) or collect ids across the scan and resolve once per source.

### `bins/kronika-web/src/api/snapshot/relation.rs:3149` — History scans resolve chunk_dictionary per 16-row chunk: full dict body read each time

per-request · unverified

process_history_chunk (and process_tablespace_history_chunk at line 2689) call query::chunk_dictionary once per 16-row chunk; chunk_dictionary collects all StrIds of the chunk and calls dictionary_for, which re-reads + CRCs the whole dictionary sections of the segment. A relation history stream over several segments of a few thousand rows each performs hundreds of full dictionary-body reads per request, dominating stream latency.

Fix direction: Bigger chunks or one id-collection pass per plan with a single dictionary resolution per segment.

### `bins/kronika-web/src/api/snapshot.rs:1767` — timed_contexts + shared_moments scan each source's timestamp column 3-4x per plan

per-request · unverified

Per plan: shared_moments scans the anchor for moments at `at` (line 1816), discards the previous it already computed, then rescans ALL sources for moments at current-1 (lines 1840-1853); timed_contexts then rescans every source a third time via Self::moments (line 1767) only to check the source carries the current moment, plus the collect() scan for previous readings. Each scan is a full section-body read + CRC + Parquet decode of the timestamp column. Roughly half of these scans are redundant: a single moments pass per source already yields current, previous, and carries-current, and Moments.previous is thrown away.

Fix direction: Compute per-source Moments once per plan and reuse for the previous-moment, currency check, and context construction.

### `bins/kronika-web/src/api/snapshot.rs:2003` — collect_partition does one full section scan per (partition, predecessor) pair

per-request · unverified

partition_rate_state loops over selected partitions and, for each, calls collect_partition per before-source; collect_partition scans the WHOLE section (visit_rows 0..MAX at line 2189) filtering to a single partition. With P databases sharing the same predecessor segment, the same section body is read, CRC'd and decoded P times per plan on every pg_stat_user_tables/indexes snapshot or relation page. The visitor already sees the partition column, so one scan could collect readings for all partitions keyed by a partition->wanted-timestamp map.

Fix direction: Group partitions by before-source and collect all of them in one scan with a partition->at map.

### `bins/kronika-web/src/api/snapshot/relation.rs:3274` — emit_relation_page renders full ~70-field metrics map for every aggregate, not just the page

per-request · unverified

Before sorting, every aggregate (every table/index/group in the database — can be thousands) gets a BTreeMap<String, Option<Metric>> built by evaluating all relation_fields (~70 for tables) and cloning each field name String. Only rows[start..end] (page_size, e.g. 200) are ever emitted; the sort uses only the single `sort` metric. 10k tables x 70 fields = 700k metric evaluations + String clones + BTreeMap inserts where ~14k suffice.

Fix direction: Build RelationRow with key + sort metric only; compute the metrics map lazily for the emitted page slice.

### `bins/kronika-web/src/api/snapshot.rs:1800` — postgres_block_size recomputed per context: full pg_settings scan + whole-dict read each

per-request · unverified

timed_contexts computes block_size (and clock_ticks_per_second) inside the per-source context loop, so each (layout x source) context for pg_stat_statements/pg_store_plans re-runs postgres_block_size: a full pg_settings section scan, then resolved_dictionary over ~700 ids which re-reads + CRCs the entire dict.strings body (which also holds all query texts, potentially MBs) — all to obtain one constant that is fixed per segment. With 2 layouts x 3 sources that is 6 pg_settings scans + 6 full dictionary-body reads per page request. Same pattern in the untimed branch at lines 1641/1649, which also re-opens the anchor segment twice per context.

Fix direction: Compute block size and clock ticks once per unique source segment (or per request) and share across contexts.

### `bins/kronika-web/src/api/snapshot.rs:3592` — search_clause_matches re-derives clause columns per row (Vec alloc + linear contract scans)

per-request · unverified

For every scanned row and every non-quantity clause, search_clause_columns runs search_fields().iter().find(), then builds a fresh Vec<&str> with plan.contract.column() linear lookups (~50 name compares per column). On a 50k-row page scan with 2 clauses this is 100k Vec allocations and millions of string compares, although PageContext already precomputes an aggregated search_columns list — just not per clause.

Fix direction: Precompute per-clause column slices once per context and index them from the row-match path.

### `bins/kronika-web/src/api/snapshot.rs:3621` — searchable_text allocates a String per cell per clause per scanned row

per-request · unverified

Search matching converts every candidate cell to an owned String: numeric cells via to_string(), dictionary strings via to_owned() of bytes the dictionary already holds borrowed (Resolved<'_>). Called from search_clause_matches for each (row, clause, column) triple during page scans — hundreds of thousands of short-lived heap allocations per search request, purely to run an equality/glob check that could operate on borrowed &str and a stack-formatted number.

Fix direction: Match on borrowed &str from the dictionary and format numbers into a stack buffer (or compare numerically).

### `bins/kronika-web/src/api/snapshot.rs:3658` — GlobPattern::matches collects candidate into Vec<char> on every call

per-request · unverified

Every pattern-clause match allocates a Vec<char> of the candidate text (query text can be hundreds of chars) before running the glob automaton; this runs once per row per pattern clause during page scans and relation aggregation — one heap allocation plus full char widening per row, multiplied by the searchable-column count.

Fix direction: Drive the backtracking over char_indices byte offsets instead of materializing Vec<char>.

### `bins/kronika-web/src/api/snapshot/relation.rs:3445` — Throwaway Aggregate built per row to evaluate the no_scans derived filter

per-request · unverified

When relation_filters is non-empty (the no_scans index view), scan_context constructs a full Aggregate per row — five BTreeMaps populated with ~40 entries via add_index, plus several String allocations from text_cell (indexrelname/relname/tablespace/amname/indexdef) — only to read back the single idx_scan rate in matches_derived_filters. Per-row cost is dozens of map inserts and heap allocations where a direct counter_delta on idx_scan answers the predicate.

Fix direction: Evaluate the idx_scan delta directly for the derived filter instead of building an Aggregate.

### `bins/kronika-web/src/api/snapshot/relation.rs:3422` — Full object GroupKey (up to 7 Strings) built per row even for database/schema grouping

per-request · unverified

scan_context always calls GroupKey::from_row with RelationGroup::Object, resolving and allocating datname, schemaname, relname (+ indexrelname for indexes) Strings for every eligible row; for_group (line 3465) then discards all but datid/datname (Database) or +schemaname (Schema). On a grouped page over thousands of rows this wastes 2-5 String allocations plus dictionary UTF-8 copies per row.

Fix direction: Build the key directly at the requested group granularity (Object detail only needed on the filter branch).

### `bins/kronika-web/src/api/snapshot.rs:2521` — collect() buffers all matching rows in a Vec before building the readings map

per-request · unverified

The previous-readings collector pushes every row at the wanted timestamp into `rows` (full-width Row with a Cell per contract column) and only afterwards converts them into the Readings map, roughly doubling peak memory during rate preparation — for pg_stat_statements this is thousands of ~50-cell rows held twice. The sibling collect_partition (line 2189) already builds the map inline inside the visitor with no intermediate Vec.

Fix direction: Build the Readings map directly inside the visit_rows closure like collect_partition.

### `bins/kronika-web/src/api/snapshot/relation.rs:2398` — Tablespace history scans every selected segment twice (discover pass + fill pass)

per-request · unverified

tablespace_history_segments runs collect_tablespace_moments over the same `selected` list twice back-to-back (loops at 2386 and 2398), each call opening the segment and fully reading + CRCing + decoding every layout's section, because keys discovered late in pass one need predecessors from segments scanned earlier. One pass keeping the top-2 pre-window moments for every (datid, type_id) pair (a tiny map) would collect the same data and halve the full-segment scans on the tablespace history endpoint.

Fix direction: Single pass recording top-2 pre-from moments for all pairs unconditionally; filter to discovered keys afterwards.

### `bins/kronika-web/src/api/snapshot.rs:1317` — emit_page opens each context's segment three times per request

per-request · unverified

The same source segment is re-opened via reader.open_segment for the ProcessUsers map (line 1317), again in scan_page (line 2267), and again in emit_page_rows (line 1531); cursor_anchor adds a fourth. Every open re-opens the file, re-reads and re-decodes the tail catalog, and re-validates file identity (Segment::open, read_catalog). With several contexts this is a dozen-plus redundant catalog reads per page request.

Fix direction: Open each unique source segment once per request and pass &Segment (or cache per context) through the phases.

### `bins/kronika-web/src/api/snapshot.rs:3492` — product_limbs heap-allocates several Vecs per comparison on the quantity-search row path

per-request · unverified

Each exact quantity comparison (SearchMetricValue::matches -> compare_products) clones the numerator/denominator factor Vecs (lines 3128-3131) and product_limbs allocates a fresh `multiplied` Vec per factor per side (~6-10 small allocations per row). rate_metric additionally builds two 2-element Vecs per row. On a 50k-row scan with one quantity clause that is ~500k transient allocations; the limb count is statically bounded (<=5 factors x 4 limbs), so fixed-size arrays suffice. The identical duplicated function in relation.rs:1970 has the same cost on relation search.

Fix direction: Use fixed-size limb arrays (or a u256-style two-u128 multiply) and avoid cloning factor Vecs per row.

## Write path (kronika-writer, kronika-store) — unverified

### `crates/kronika-store/src/local/scan.rs:193` — Journal scan reads every part body from disk twice per request

per-request · unverified

scan_journal_reader_bounded_from first runs scan_journal_frames -> kronika-format scan_journal_streaming_strict_from, which already reads every part body (into one reused part_buf) and fully validates it including section CRCs via validate_part. The following loop then allocates a fresh `vec![0_u8; part_ref.len]` (up to MAX_PART_LEN = 64 MiB per part) and reads the same body from disk a second time, only to call validate_part_catalog — a strictly weaker check that touches just the 4-byte magic and the tail catalog region. Every web request goes through this (kronika-web -> reader.catalog_segments -> scan_catalogs -> scan_journal), so the entire valid journal body (cap 1 GiB) is read twice and re-allocated per part on each request.

Fix direction: Have the streaming scan hand back the decoded catalogs (or catalog byte ranges) it already validated, or read only magic + catalog/tail region in the second pass (catalog_len is already known from active_part_catalog_metadata_bytes).

### `crates/kronika-store/src/local.rs:112` — scan_catalogs discards prior summaries; all finished catalogs re-read per request

per-request · unverified

scan_catalogs passes `&[]` as previous_finished to complete_scan_cached_with_warnings, so every request re-opens each finished .zms, re-reads its encoded catalog bytes, and recomputes two SHA-256 digests plus the type bloom in read_zms_summary/CatalogSummary — O(finished segments) file opens, reads, and hashing per web request, growing with retention. The reuse machinery (address+identity merge against previous FinalUnits) already exists in complete_scan_cached, but it is hardwired to FinishedValidation::Complete, so catalog-mode callers like kronika-reader cannot reach it and pay full rediscovery each time.

Fix direction: Expose a catalogs-mode completion accepting previous finished units (reuse the existing identity-match merge) so unchanged segments keep their Arc<CatalogSummary> across requests.

### `crates/kronika-writer/src/segment.rs:268` — Finalizer re-reads full compressed section bodies once per projected column

cold · unverified

The open closure in spool_data_section returns Bytes::from(read_section_body(..)) — a fresh heap Vec of the entire compressed section body read from the journal file. encode_final_sections_to (kronika-registry finalize.rs) invokes open roughly once per column per section (sort-key pass + tie refinement + one pass per output column), so a 20-column type re-reads and re-allocates every constituent section body ~20x during segment finalization. Since the Parquet reader with a ProjectionMask only fetches the projected column chunk via ChunkReader::get_bytes, all bytes outside that chunk are read from disk and thrown away on every open; for a near-full journal this multiplies finalization read I/O to tens of GB while memory alone stays bounded.

Fix direction: Back open() with a positional ChunkReader over the journal file range (read_part_range per requested byte range) instead of slurping the full body; keep the one full verified read for CRC/decode-work validation.

### `crates/kronika-writer/src/segment.rs:219` — Segment finalization writes every section twice via the spool file

cold · unverified

write_tmp streams all data and dictionary sections into a spool temp file (in Reverse(bytes) order), then re-reads the whole spool and copies it 64 KiB at a time into the destination temp — every final segment byte is written twice and read once extra per finalization. The copy exists only to reorder sections into ascending type_id, but that order is available for free: plan.by_type is a BTreeMap iterated ascending, and both dictionary ids (3_001_001, 3_002_001) sort after every data type id (max ~2_2xx_xxx), so sections could stream directly into the destination in final order; SectionSink already yields len/crc as it writes and the catalog goes at the end regardless. The Reverse(bytes) pre-sort buys nothing since the spool is fully rewritten anyway.

Fix direction: Stream sections straight into the destination temp in ascending type_id order (data types in BTreeMap order, then dict.strings, dict.blobs) and drop the spool file and copy pass.

### `crates/kronika-store/src/local/scan.rs:221` — O(n^2) metadata re-accounting: full active fold after each appended part

per-request · unverified

Inside the per-part admission loop, active_metadata_bytes(&active, active.capacity()) is recomputed after every push, and that helper folds over ALL retained ActiveParts summing catalog-entry capacities (budget.rs:224-243). A full journal rescan of n parts therefore does O(n^2) work; at a few hundred parts it is noise, but the admitted cap is MAX_JOURNAL_PARTS = 1_000_000 and a journal dense with small parts (frequent log-tail flushes) reaches tens of milliseconds of pure accounting per web request.

Fix direction: Track the running total incrementally: add the new part's catalog-entry bytes (already computed as part_metadata) plus any capacity delta instead of re-folding the whole vector each iteration.
