//! Terminal instructions for storage inspection and standalone slices.

pub(crate) const HELP: &str =
    "kronika-dump - inspect recorded metrics or extract a standalone ZMS file

Usage:
  kronika-dump <DIR> [--section <ID> | --index] [--json]
               [--limit <N>] [--from <MICROSECONDS>] [--to <MICROSECONDS>]
  kronika-dump slice --from <RFC3339> --to <RFC3339> --out <FILE.zms>
  kronika-dump -h | --help
  kronika-dump --version

Start with an existing collector recording:
  kronika-dump /path/to/recording
  kronika-dump /path/to/recording --json
  kronika-dump /path/to/recording --index
  kronika-dump /path/to/recording --section 1100001 --limit 10

DIR is the real storage directory used by kronika-collector, containing
YYYY/MM/DD/*.zms and, while collecting, active.wal. A standalone .zms file,
a flat directory of arbitrary .zms files, or a symlink is not a storage root.
Both finished segments and the committed current journal are readable while
the collector runs. Use sudo if your account cannot read the private storage.
Inspection needs no environment variables, database connection, or web server.

Inspection options:
  No display option   List each segment's time bounds, physical section IDs,
                      row counts, section bytes, and physical overhead bytes.
  --section ID        Decode rows of one physical numeric type_id, resolving
                      dictionary references. Pick an ID from the default list.
                      Example: 1100001 is the OS process section.
  --index             Print derived series/index summaries. With --json, emit
                      their individual points and finding locators. Cannot be
                      combined with --section; does not write an .idx file.
  --json              One JSON object per line (NDJSON), instead of text tables.
  --limit N           Rows per segment for --section only. Default: 20.
                      N is a nonnegative integer; 0 prints every row.
  --from MICROSECONDS Inclusive earliest Unix timestamp; default: no lower bound.
  --to MICROSECONDS   Inclusive latest Unix timestamp; default: no upper bound.
                      These signed integer bounds select intersecting segments;
                      they do not trim individual section rows. Units are
                      MICROSECONDS, not seconds or RFC3339. Either may be used.
  -h, --help          Print this help and exit without opening storage.
  --version           Print the binary name and version and exit.

To start collection in another terminal, use your chosen recording directory:
  sudo env KRONIKA_STORAGE_DIR=/path/to/recording kronika-collector
Then inspect with sudo kronika-dump /path/to/recording. OS and PostgreSQL
recordings use the same inspection and slice commands; PostgreSQL collection
is configured on the collector, not on this command.

Inspection prints data to stdout; text-mode scan warnings and errors use
stderr. With --json, scan warnings are also JSON objects on stdout. It exits
after reading; a closed output pipe is successful. Other failures exit nonzero.
";

pub(crate) const SLICE_HELP: &str = r#"kronika-dump slice - extract a time interval into one standalone ZMS file

Usage:
  KRONIKA_STORAGE_DIR=<DIR> kronika-dump slice \
    --from <RFC3339> --to <RFC3339> --out <FILE.zms>
  kronika-dump slice -h | --help

Example: extract 19:00:00 through 19:59:59 UTC, including both whole seconds:
  sudo env KRONIKA_STORAGE_DIR=/path/to/recording kronika-dump slice \
    --from 2026-09-05T19:00:00Z --to 2026-09-05T19:59:59Z \
    --out incident.zms
  sudo chown "$(id -u):$(id -g)" incident.zms
  kronika-report incident.zms incident.html

Required environment:
  KRONIKA_STORAGE_DIR  Existing real collector storage directory, not a .zms
                       file or symlink. No default. Both finished segments and
                       the committed current journal are read. Read permission
                       is required; no database or web connection is used.

Required options (each exactly once, in any order):
  --from RFC3339       Inclusive first whole second.
  --to RFC3339         Inclusive last whole second; must be at or after --from.
                      Include a timezone: Z or an offset such as +03:00.
                      Fractional seconds and Unix numeric timestamps are refused.
                      --from equal to --to selects that entire second.
  --out FILE.zms       Exact new output path with a .zms suffix. Its parent
                      directory must exist and be writable. Existing paths are
                      refused; output is never overwritten.
  -h, --help          Print slice help without configuration or storage access.

An interval with no recorded rows fails. The file can retain up to 30 seconds
of nearby samples on each side for interval calculations. On success, stdout
reports bytes, rows, sections, and requested/actual bounds in Unix microseconds.
The requested_to_exclusive value is one second after the requested --to.

To keep an HTML report's visible bounds at exactly 19:00-20:00 UTC:
  kronika-report --from 1788634800000000 --to-exclusive 1788638400000000 \
    incident.zms incident.html
The report bounds are a pair of Unix MICROSECOND values, [from, to-exclusive).

Scratch and temporary output files are created beside --out, on its filesystem;
TMPDIR does not move them. Allow free space for both work files and the result.
The completed ZMS is validated before publication. The command exits when done;
errors go to stderr and return nonzero. Ctrl-C interrupts a running command.
"#;
