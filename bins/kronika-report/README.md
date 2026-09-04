# kronika-report

`kronika-report <input>.zms <output>.html` reads exactly one finished standalone
ZMS and writes one self-contained HTML report. The input may have any `.zms`
basename, such as `incident.zms`; the command derives its internal segment
identity from the validated ZMS catalog.

An already-sliced ZMS can retain nearby rows needed for interval calculations.
Pass `--from <unix_microseconds> --to-exclusive <unix_microseconds>` before the
two paths to expose only the requested half-open time range in report
navigation. Both bounds must be positive JavaScript-safe integers. Without
these options, navigation uses the complete ZMS time range.

The generated file embeds the production report UI, its browser query module,
the source ZMS, and the canonical isolated IDX derived from that ZMS. It has no
server, external sidecar, authentication, or network dependency.
