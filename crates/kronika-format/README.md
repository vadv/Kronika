# kronika-format

[Русская версия](README.ru.md)

`kronika-format` defines the binary contract that lets Kronika finish writing
a segment in one process and open it later in another. It is not a standalone
converter or a generic analytics format.

Inside Kronika, the crate connects the durable write and read paths:

```text
Linux / cgroup / PostgreSQL / PostgreSQL log
        |
        v
kronika-collector collects one window
        |
        v
kronika-registry encodes typed rows as section bodies
        |
        v
kronika-writer builds a self-contained ZMS part
        |
        v
$KRONIKA_OUT_DIR/active.wal
        |
        | write_segment()
        v
$KRONIKA_OUT_DIR/YYYY/MM/DD/N.zms
        |
        v
kronika-store / kronika-reader
        |
        v
kronika-web
```

The journal is always `$KRONIKA_OUT_DIR/active.wal`. Finished ZMS files use
the strict UTC calendar tree:

```text
$KRONIKA_OUT_DIR/
|-- active.wal
`-- YYYY/
    `-- MM/
        `-- DD/
            `-- N.zms
```

`N` is the decimal [`SegmentId`](../kronika-layout/src/time.rs): Unix
microseconds of the first collection window successfully appended to the
segment. `YYYY/MM/DD` is the UTC day derived from that id.

There are no separate files for string values. While a segment is open,
dictionary bodies live in the ZMS parts inside `active.wal`. On completion,
the writer decodes and normalizes them into at most one `dict.strings` and one
`dict.blobs` body in the finished `.zms`.

The format crate owns the `ZMS1` and `ZMSP` byte layouts, the end catalog,
CRC32C checksums, `StrId`, and the bounded dictionary model. It deliberately
does not own:

- Linux and PostgreSQL semantics and section schemas: those belong to
  [`kronika-registry`](../kronika-registry/README.md);
- Parquet encoding, buffering, journal I/O, and writing: those belong to
  [`kronika-writer`](../kronika-writer/README.md);
- collection intervals, rotation, and source limits: those belong to
  [`kronika-collector`](../../bins/kronika-collector/README.md);
- data-directory paths, strict discovery, and ownership:
  [`kronika-layout`](../kronika-layout/);
- typed queries, HTTP, retention, encryption, or remote storage.

## From a collection window to a segment

The current collector creates a fresh interner and row buffers for each
non-empty collection cycle. The registry sorts each snapshot section by its
contract key and encodes it. The writer adds dictionary sections, builds a
self-contained ZMS part, wraps it in a `ZMSP` frame, appends it to
`active.wal`, and calls `sync_data`.

The collector keeps appending windows until a size, age, forced-rotation, or
journal-cap condition closes the segment. Writing then:

1. validates each recorded part catalog and every body range it reads;
2. groups registered data sections by `type_id`, decodes their Parquet bodies,
   and combines their rows;
3. sorts each combined data section by its registry key and then every
   remaining column, producing a deterministic total order;
4. normalizes dictionary records by `str_id`, deduplicating exact repeats and
   rejecting conflicting values, metadata, or placement;
5. re-encodes each populated type as one canonical Parquet body and writes a
   packed ZMS with one end catalog;
6. synchronizes an invocation-owned sibling temporary file and publishes it
   with a no-replace hard link;
7. resets `active.wal` only after publication succeeds.

Writing therefore removes per-window Parquet framing, repeated catalog entries,
and repeated dictionary records. It does not copy journal section bodies into
the finished ZMS.

### Windows, parts, sections, and bodies

The format description uses four different levels:

- a **collection window** is one non-empty collector cycle;
- a **ZMS part** is a self-contained record of one window, with its own `ZMS1`,
  bodies, catalog, and tail index;
- a **section** is one catalog entry: `type_id`, offset, length, row count, and
  CRC32C;
- a **section body** is the byte range addressed by a catalog entry. The
  current writer places one self-contained Parquet file in that range.

While the segment is open, every part is wrapped in its own journal frame:

```text
active.wal
|
+-- ZMSP frame #1
|   `-- ZMS part for window #1
|       |-- "ZMS1"
|       |-- data section bodies
|       |-- dict.strings / dict.blobs bodies
|       |-- part catalog
|       `-- tail index
|
`-- ZMSP frame #2
    `-- ZMS part for window #2
        |-- "ZMS1"
        |-- data section bodies
        |-- dict.strings / dict.blobs bodies
        |-- part catalog
        `-- tail index
```

Within each part, the writer places non-empty data bodies in ascending
`type_id` order, followed by `dict.strings` and then `dict.blobs`. An absent
dictionary section occupies no space. The same canonical inventory applies to
the finished ZMS: every populated type occurs once, data types are ascending,
and the two optional dictionaries form the tail. Readers locate every body
through the catalog.

Completing the segment removes the individual part framing:

```text
active.wal                         YYYY/MM/DD/N.zms

ZMSP [part for window #1]             "ZMS1"
ZMSP [part for window #2] -- write_segment() -> one canonical body per populated data type
...                                  optional normalized dict.strings
                                     optional normalized dict.blobs
                                     one shared end catalog
                                     one tail index
```

The finished ZMS has no explicit window-boundary markers. Rows from all windows
of one type share a canonical body, and their originating part is not recorded.
The catalog describes physical sections, not window numbers.

## Finished segment v1 layout

All integers are little-endian. In canonical output from the current writer,
fields are packed without alignment padding. For `N` catalog entries and `B`
total bytes in all section bodies:

```text
offset  size          contents
0       4             "ZMS1"
4       B             section body 0, section body 1, ... in catalog order
4+B     32*N          catalog entries
...     40            catalog metadata
...     8             tail index
```

The exact file size is:

```text
zms_bytes = B + 32*N + 52
catalog_len = 32*N + 40
```

The 52 fixed bytes are the leading magic, catalog metadata, and tail index.
There is no outer compression layer. These equations describe canonical writer
output. `validate_catalog_layout`, part validation, and physical readers require
body ranges to be contiguous from the leading magic to the catalog. They reject
gaps, overlaps, trailing body bytes, repeated types, nonzero flags, empty
populated sections, and noncanonical section order.

### Catalog entry: 32 bytes

| Offset | Field | Type | Meaning |
| ---: | --- | --- | --- |
| 0 | `type_id` | `u32` | Section schema registered by `kronika-registry`. |
| 4 | `flags` | `u32` | Reserved; current writers store zero. |
| 8 | `offset` | `u64` | Absolute body offset from the first byte of the file. |
| 16 | `len` | `u64` | Body length in bytes. |
| 24 | `rows` | `u32` | Number of logical rows recorded for the body. |
| 28 | `crc32c` | `u32` | CRC32C of this section body. |

Each populated `type_id` appears exactly once. Data sections are ordered by
ascending `type_id`, followed by at most one `dict.strings` and at most one
`dict.blobs`. Different chart entities and rows from different collection
windows are coalesced inside the one body for their type.

### Catalog metadata: 32 bytes

| Offset | Field | Type | Meaning |
| ---: | --- | --- | --- |
| 0 | `min_ts` | `i64` | Earliest section timestamp, Unix microseconds. |
| 8 | `max_ts` | `i64` | Latest section timestamp, Unix microseconds. |
| 16 | `entry_count` | `u32` | Number of 32-byte entries before this block. |
| 20 | `format_version` | `u32` | Container layout version; current writers store `2`. |
| 24 | `crc32c` | `u32` | CRC32C of entries and metadata with this field zeroed. |
| 28 | `window_count` | `u32` | Collection windows coalesced into this container. `build_part` stores `1`; `write_segment` stores the exact number of journal parts; zero means unknown. |

### Tail index: 8 bytes

| Offset | Field | Type | Meaning |
| ---: | --- | --- | --- |
| 0 | `catalog_len` | `u32` | Entries plus the 32-byte metadata block; excludes the tail itself. |
| 4 | `magic` | 4 bytes | `"ZMS1"`. |

A reader therefore starts at the end, not at the first section:

1. read the final eight bytes;
2. use `catalog_len` to find the catalog start;
3. decode and CRC-check the catalog;
4. read a selected body by its absolute `offset` and `len`;
5. verify the body CRC before handing the bytes to the registry codec.

### Example: a snapshot and two dictionaries in one ZMS

The following simplified ZMS for a Linux host contains one
`os_process` body, one dictionary body for short string values, and one
dictionary body for large values. Let their sizes be `S`, `T`, and `L`, with
`B = S + T + L`.

```text
offset

0       +----------------------------------------------------------+
        | "ZMS1"                                                   | 4 bytes
4       +----------------------------------------------------------+
        | body #0: os_process, type_id=1_100_001             | S bytes
        | Parquet: ts | pid | comm=H | cmdline=Q | ...             |
4+S     +----------------------------------------------------------+
        | body #1: dict.strings, type_id=3_001_001                 | T bytes
        | Parquet: str_id | bytes                                  |
4+S+T   +----------------------------------------------------------+
        | body #2: dict.blobs, type_id=3_002_001                   | L bytes
        | Parquet: str_id | stored_bytes | full_len | ...          |
4+B     +----------------------------------------------------------+
        | catalog entry #0: type=1_100_001, offset=4, len=S        | 32 bytes
36+B    | catalog entry #1: type=3_001_001, offset=4+S, len=T      | 32 bytes
68+B    | catalog entry #2: type=3_002_001, offset=4+S+T, len=L    | 32 bytes
100+B   +----------------------------------------------------------+
        | catalog metadata                                         | 40 bytes
140+B   +----------------------------------------------------------+
        | catalog_len=136 | "ZMS1"                                 | 8 bytes
148+B   +----------------------------------------------------------+
```

For three bodies, `catalog_len = 3 * 32 + 40 = 136`, so the complete file is
`B + 148` bytes. If the window has no large values, the `dict.blobs` body and
its catalog entry are absent. Every non-empty window can contribute rows, but
the finished catalog still contains each populated `type_id` only once.

In current Kronika files, a data or dictionary body is normally a
self-contained Parquet file, so it has its own `PAR1 ... PAR1` framing. The ZMS
container treats those bytes as opaque; it does not parse column metadata. The
Parquet rows shown above are decoded contents, not literal plain text in the
file: internal encoding and Zstd mean that text bytes need not occur verbatim
in the ZMS.

The repository also contains a byte-exact
[`minimal.zms`](tests/fixtures/minimal.zms) fixture. It is 88 bytes:
`ZMS1`, one four-byte body `01 02 03 04`, one catalog entry, metadata, and the
tail. [`tests/fixture.rs`](tests/fixture.rs) records every offset and verifies
that the encoder reproduces the fixture byte for byte.

## What section bodies contain today

`kronika-format` permits opaque bodies, but Kronika uses self-contained
Parquet for snapshot and dictionary sections. Journal parts and finished
segments deliberately use different writer profiles:

- a collection-window body uses Zstd level 3 and sorts snapshot rows by the
  registry key and dictionary rows by `str_id`;
- a final final body uses Parquet 1.0, one row group, PLAIN values, RLE
  levels, and Zstd level 6; Parquet dictionary encoding, statistics, and offset
  indexes are disabled;
- canonical data rows use the registry key followed by every remaining column
  as a deterministic total order; normalized dictionary rows use `str_id`;
- a final body is limited to 65,536 rows and 8 MiB, while decode admission also
  limits a body to 16 row groups and 128 MiB of aggregate decoded work;
- redundant Arrow schema metadata is omitted and `created_by` is empty.

Before a window is appended, segment admission accumulates rows,
`List<i32>` child values, dictionary rows, and stored dictionary bytes. It also
proves the final one-page PLAIN profile for every physical column. For column
`i`, let `V_i` be worst-case PLAIN value bytes and `L_i` be level bytes:

```text
V_i < 1 MiB
page_i = V_i + L_i
body_bound = 64 KiB + sum(zstd_bound(page_i) + 4 KiB)
body_bound <= 8 MiB
```

For the pinned Zstandard contract:

```text
zstd_bound(n) = n + floor(n / 256)
              + (n < 128 KiB ? floor((128 KiB - n) / 2048) : 0)
```

The 4 KiB term bounds each page header and column-chunk metadata; 64 KiB bounds
Parquet file framing. Write recomputes these bounds as it decodes each part and
checks the actual encoded body against 8 MiB.

Omitting the embedded Arrow schema removes a duplicate logical schema from
every body; the exact saving depends on the section contract. The native
Parquet schema remains because the decoder needs the physical column layout.

There is no whole-file Zstd pass. Each Parquet body's header and footer, the ZMS
catalog, and the ZMS/ZMSP framing are outside a shared Zstd stream. A reader can
locate and verify one body without decompressing unrelated sections.

## Where string values physically live

A snapshot section does not store the original text in every record. A
`StrId` column stores only a `u64` equal to `xxh3_64(original_bytes)`. Zero
means "no value". The number is not a file offset: the original bytes must be
found under the same id in separate dictionary Parquet bodies.

The current writer emits two dictionary section types:

| `type_id` | Section | Columns after decoding |
| ---: | --- | --- |
| `3_001_001` | `dict.strings` | `str_id`, complete `bytes` |
| `3_002_001` | `dict.blobs` | `str_id`, `stored_bytes`, `full_len`, `truncated`, optional `full_sha256` |

Both sections are **inside the ZMS**, alongside snapshot sections.
`dict.blobs` is not a separate file, PostgreSQL TOAST, or external object
storage.

### The short string `postgres`

For the bytes `postgres`,
`H = xxh3_64(b"postgres") = 0x0939566173e67ada`. If two processes in one window
share that name, the data occupies one physical file as follows. Parquet contents are shown after decoding; unrelated
bodies are omitted:

```text
$KRONIKA_OUT_DIR/YYYY/MM/DD/N.zms  (one file)

[ "ZMS1" ]
[ os_process Parquet body, type_id=1_100_001
  after decoding:
  pid | comm
  101 | H
  102 | H ]
[ dict.strings Parquet body, type_id=3_001_001
  after decoding:
  str_id | bytes
  H      | b"postgres" ]
[ end catalog
  offset,len -> entire os_process body
  offset,len -> entire dict.strings body ]
[ tail index ]
```

The snapshot section therefore contains two logical `StrId` values represented
as `u64`, while the dictionary contains one record with the `postgres` bytes.
One level deeper, their placement is:

```text
ZMS catalog entry: type_id=3_001_001, offset=X, len=T
                            |
                            v
byte range [X, X+T): self-contained Parquet body
|-- "PAR1"
|-- row group #0
|   |-- str_id column chunk: encodes H
|   `-- bytes column chunk: encodes b"postgres"
|-- Parquet metadata and metadata length
`-- "PAR1"
```

The bytes belong to the `bytes` column chunk of the `dict.strings` body.
In a finished ZMS, Parquet places the PLAIN value in the column's data page and
Zstd level 6 compresses that page. The `os_process` body keeps only `H`.
The ZMS catalog stores the offset and length of the entire dictionary body, not
of one value; finer offsets belong to Parquet metadata. Resolving `H` therefore
requires decoding the dictionary body. The corresponding body inside a
journal part uses the collection-window Zstd level 3 profile.

The "after decoding" blocks show logical values, not on-disk bytes. The exact
cost of the two references need not be 16 bytes, and encoding and compression
mean that `postgres` need not occur as a plain byte sequence in the file.

Reading follows the end catalog:

```text
(1) tail index -> end catalog

(2) catalog -> os_process body
            -> decode Parquet
            -> comm=H

(3) catalog + H -> every dict.strings and dict.blobs body
                -> decode record with str_id=H
                -> b"postgres"
                -> comm="postgres"
```

ZMS has no global `StrId -> offset` index. A full dictionary read visits every
`dict.strings` and `dict.blobs` body and builds an in-memory map. The targeted
index path also reads dictionary bodies but retains only ids requested in
advance.

### Large and truncated values

Under the current collector limits, placement works as follows:

| Original value | Section | Bytes retained on disk |
| --- | --- | --- |
| `postgres`, 8 bytes | `dict.strings` | All 8 bytes. |
| Plan text, 20 KiB | `dict.blobs` | All 20 KiB, `full_len=20,480`, `truncated=false`, and no SHA-256. |
| `/proc/PID/cmdline`, 80 KiB | `dict.blobs` | The first 64 KiB, `full_len=81,920`, `truncated=true`, and SHA-256 of the original 80 KiB. |

A truncated value's `StrId` is computed over all original bytes, not the stored
prefix. The discarded suffix cannot be recovered from the ZMS. A source that
caps a value's length does so before interning, so the dictionary treats the
bytes it receives as the complete value.

The reusable model and the collector intentionally use different limits:

| Limit | `DictLimits::default()` | Current collector | Effect |
| --- | ---: | ---: | --- |
| Blob threshold | 4 KiB | 4 KiB | Values at or above it use `dict.blobs`. |
| Truncation limit | 1 MiB | 64 KiB | Longer values retain exactly this prefix. |
| Stored-byte cap | 64 MiB | 16 MiB | Rejects an ordinary new value when stored dictionary bytes would exceed the cap; required hot values are exempt. |

The collector values are currently fixed in code, not environment variables.
The byte cap counts stored value bytes after truncation, not Parquet metadata.
For an ordinary new value, exceeding it returns `DictError::Full`; it is not an
absolute encoded-section limit because required hot strings remain exempt. The
current collector does not automatically complete the segment at 16 MiB:
depending on the source, the cycle fails or individual records or text fields
are omitted.

`SegmentDicts` also models a `dict.hot_strings` subset, but the current writer
does not emit a third dictionary section. Those values are written into the
ordinary `dict.strings` body; there is no `dict.hot_strings` body to find in the
file.

### Where deduplication stops

One `SegmentDicts` instance detects hash collisions and keeps one copy of equal
bytes. The current collector, however, creates a new interner for every
collection cycle, so the journal can contain the same dictionary id in several
parts.

Write extends normalization across the complete segment. For one `str_id`, an
exactly repeated value with the same metadata and placement is retained once.
Different bytes or blob metadata, and any strings-versus-blobs placement
conflict, fail writing. The result is sorted by `str_id` and contains at most
one physical record per id in at most one `dict.strings` and one `dict.blobs`
body. Physical readers reject repeated dictionary section types, and dictionary
decoding requires ids inside each body to be strictly increasing.

## `active.wal` and crash recovery

`active.wal` is the root-level durable journal of an unfinished segment.
Journal version 1 starts with a checksummed 36-byte header:

```text
"KRNJNL1\0" | version: u32 | state: u8 | id_present: u8 | reserved: u16
             | segment_id: i64 | body_len: u64 | header_crc32c: u32
```

The header records an empty state or the exact `SegmentId` of the active
segment and the number of frame bytes that follow. Every frame then has a
16-byte header followed by one complete ZMS part:

```text
"ZMSP" | part_len: u64 | header_crc32c: u32 | ZMS part
```

The journal-header checksum covers its first 32 bytes; each frame-header
checksum covers its first 12 bytes. The ZMS part has its own `ZMS1`, bodies,
catalog, and tail, so a frame can be validated before it is accepted. A
canonical empty journal is the 36-byte empty header, never a zero-length file.

For a clean journal, let `P` be its frame count, `N_in` its total part-catalog
entry count, and `B_in` the bytes in all input bodies. Let `K` be the number of
populated types after coalescing and `B_out` the bytes in their canonical
re-encoded bodies:

```text
active_parts_bytes     = 36 + B_in + 32*N_in + 68*P
finished_zms_bytes       = B_out + 32*K + 52
journal_minus_finished   = (B_in - B_out) + 32*(N_in - K) + 68*P - 16
```

Every final body is at most 8 MiB, so the container also has the bound:

```text
B_out <= 8 MiB * K
finished_zms_bytes <= (8 MiB + 32) * K + 52
```

`K` has no repeated `type_id`. Unlike a copy-only write, the exact reduction
includes the changed Parquet encoding, removed per-window bodies, removed
catalog entries, normalized dictionaries, and removed frame overhead.

Version 1 has three absolute admission limits: one ZMS part is at most 64 MiB,
one journal contains at most 1,000,000 frames, and the physical journal file is
at most 1 GiB including the temporary reset marker. Runtime configuration may
only lower these limits. The production streaming scanner keeps one bounded
ZMS part body plus the returned frame references in memory. It stops at the
first damaged frame and does not search damaged bytes for another frame magic.
The resynchronizing in-memory scanner is a diagnostic API, not a recovery
policy.

`Journal::open` is fail-closed for version 1. It validates the complete header,
the recorded body length, and every frame. A headerless file, another version,
a torn header, a length mismatch, or any torn or damaged frame returns an error
and leaves the file unchanged. A zero-length file provably holds no data and is
re-initialized to the canonical empty header. The low-level scanner's
damage classifications are diagnostic data, not a repair policy for this
journal. Version 1 is the first and only supported journal format. Kronika
has not had a public release, and there is no alternate journal format or
migration path.

On collector startup, a valid active journal is completed under its stored
`SegmentId` and published at `YYYY/MM/DD/N.zms`. Only successful publication
allows reset. Reset first persists a commit marker. It then writes
`JournalHeader::EMPTY` and calls `sync_data` while the marker and frame body
remain, truncates the file to 36 bytes, and calls `sync_data` again. If the
process exits after the marker is durable, the next `Journal::open` validates
it and completes the reset.

The reset marker is 32 bytes and records the previous journal length,
`SegmentId`, and header checksum. The configured journal cap is a literal
physical-file limit: the writer reserves those 32 bytes before every append,
including the first frame.

## Where the bytes go

For the current implementation, most size variation comes from section
bodies, not the 52-byte container constant or 32-byte catalog entries.

### What already reduces size without losing data

| Mechanism | What is removed or compressed | Scope |
| --- | --- | --- |
| `StrId` and dictionaries | Repeated short text becomes a value in a `u64` column. | Parts are self-contained per window; write normalizes exact repeats to one record for the segment. |
| Section coalescing | Per-window Parquet headers, footers, and catalog entries are replaced by one body and entry per populated type. | All accepted windows in one finished segment. |
| Parquet and Zstd | Journal bodies use Zstd level 3; write re-encodes PLAIN columns with Zstd level 6. | Compression is local to each final type body. |
| Canonical sorting | Values with nearby keys become adjacent and output is deterministic. | Compression gain depends on the data and is not guaranteed. |
| Narrow column types | For example, `pid` is stored as `i32`, not `i64`, and text labels move to dictionaries. | Each section contract fixes its column types. |
| Omitted Arrow metadata | Each body omits a second logical Arrow schema and leaves `created_by` empty. | The physical schema and Parquet footer remain. |

### What reduces size by discarding data

These are admission limits, not compression:

| Mechanism | What is discarded | Cost |
| --- | --- | --- |
| `dict.blobs` truncation | The suffix after 64 KiB. | Full text cannot be recovered; its length and SHA-256 remain. |
| `*_MAX_TABLES`, `*_MAX_INDEXES`, `*_MAX_STATEMENTS`, `*_MAX_PLANS` | Objects below the top-N cutoff. | Their observations do not enter the ZMS. |
| Source intervals | Snapshots between polling times. | Time resolution decreases. |
| Plan-text budgets | Text beyond one read's limit. | Numeric plan statistics may remain without plan text. |

Segment age and size limits control how many accepted windows enter one
coalescing unit. They change the ZMS count, final body compression, and how much
per-window structure and dictionary repetition write can remove. They do not
discard accepted rows.

### How write removes repetition across collection windows

Each collection window writes a separate Parquet body for every non-empty
type. A one-row snapshot still pays for a Parquet schema, column metadata, and
footer while it remains in `active.wal`. Dictionary sections are also emitted
per window. For `postgres`, the logical contents before and after write are:

```text
before completing the segment:

active.wal
|-- ZMSP [part #1:
|          os_process body #1: pid=101, H; pid=102, H
|          dict.strings #1: H -> b"postgres"]
`-- ZMSP [part #2:
           os_process body #2: pid=103, H
           dict.strings #2: H -> b"postgres"]

after completing the segment (`write_segment`):

"ZMS1"
|-- one canonical os_process body:
|     pid=101, H; pid=102, H; pid=103, H
|-- one normalized dict.strings body: H -> b"postgres"
|-- shared catalog
`-- tail index

3 StrId references | 1 unique H | 1 physical dictionary record
```

Write achieves this by decoding Parquet, combining bodies with the same
`type_id`, validating dictionary equality and placement, sorting rows into a
canonical total order, and encoding again. Admission closes the accumulated
segment before an incoming window would exceed a final row, list-value,
dictionary, page, or body limit. A window that cannot fit by itself is rejected
before journal append.

### Additional approaches that are not implemented

| Approach | Source of the saving | Constraint |
| --- | --- | --- |
| One interner for the open segment | A repeated value is not emitted into the next window's dictionary. | Current ZMS parts are self-contained. If a later window refers to an earlier dictionary, isolated frame reads and crash recovery need a new contract. |
| Higher final Parquet Zstd level | Pages inside one body may become smaller. | Costs more CPU; final bodies already use level 6, while collection-window bodies use level 3. |
| Outer compression for the whole ZMS | One stream can see repeated dictionaries, footers, and similar windows. | Direct body access by `offset` is lost; this would replace ZMS access semantics and is outside this research. |

Write already removes structural repetition and dictionary duplicates. The
remaining approaches would change part self-containment, CPU cost, or direct
section access.

## Parameters that affect file size

The format layout itself has no compression knobs. Operators control the
amount and grouping of collected data through `kronika-collector`:

| Control | What it changes |
| --- | --- |
| Per-source read intervals | A longer interval produces fewer snapshots and Parquet footers, at the cost of time resolution. |
| Per-source row ceilings | Lower ceilings reduce high-cardinality rows and the dictionary entries behind them, at the cost of the rows past the cut. |
| Segment size and age | They change file granularity and the time span one file covers, not Parquet compression. |
| Journal size | A hard physical `active.wal` limit; exhaustion causes an early write. It is not a target ZMS size. |

The variables behind these controls, with their defaults, are listed in the
[collector README](../../bins/kronika-collector/README.md).

Changing source limits trades observability for disk use. Segment age and
rotation size change how many windows write coalesces, so they can affect final
compression and the amount of removable per-window structure without changing
the writing contract. See the
[collector configuration](../../bins/kronika-collector/README.md)
for every source interval and validation rule.

## Integrity, limits, and compatibility

CRC32C covers every section body, the catalog, and each `ZMSP` header. It
detects accidental corruption; it is not authentication, a signature, or
encryption.

`kronika-format` validates framing, catalog length and checksum, section
bounds, and section checksums for complete parts. Higher layers add policy:
the current finished reader accepts container version 1 and caps catalog,
section, row, and row-group sizes. An incompatible section schema receives a
new `type_id`; changing the ZMS framing requires a new container version.

Sources of truth:

- [`src/lib.rs`](src/lib.rs), [`src/catalog.rs`](src/catalog.rs), and
  [`src/parts.rs`](src/parts.rs) for the byte contract;
- [`src/dictionary.rs`](src/dictionary.rs) for dictionary invariants;
- [`kronika-registry`](../kronika-registry/README.md) for section schemas and
  Parquet limits;
- [`kronika-writer`](../kronika-writer/README.md) for append and write behavior.
