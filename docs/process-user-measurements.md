# Process user-reference measurements

These measurements exercise the production journal encoder, append path,
finished-segment writer, reader, and string dictionary. They were recorded on
Linux 6.17.10-100.fc41.x86_64, an AMD Ryzen 9 8945HS, with an optimized build
and a 100 Hz process CPU clock. Reproduce them with:

```text
CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu cargo test --release --ignore-rust-version -p kronika-collector process_user_references_report_production_storage_and_resource_costs -- --nocapture --test-threads=1
```

The local Rust compiler predates the workspace MSRV, so the measurement uses
`--ignore-rust-version`; CI uses the repository toolchain. Each artifact below
contains only `os_user` and its `dict.strings` data. Process rows are unchanged,
so this is the feature's direct storage contribution.

| Case | UID observations | Mapping rows | Raw `os_user` body | Raw dictionary body | Raw WAL | Finished `os_user` | Finished dictionary | Marginal finished bytes | Whole ZMS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 processes sharing one UID for 120 ticks | 120,000 | 1 | 1,171 B | 463 B | 1,794 B | 517 B | 245 B | 826 B | 870 B |
| Several ordinary UIDs | 16 | 16 | 1,343 B | 648 B | 2,151 B | 694 B | 428 B | 1,186 B | 1,230 B |
| Maximum distinct observed UIDs and names | 4,096 | 4,096 | 55,368 B | 43,684 B | 99,212 B | 42,414 B | 43,219 B | 85,697 B | 85,741 B |

`Marginal finished bytes` includes the two catalog entries. Raw WAL and whole
ZMS include their normal framing and file metadata. The shared-UID case proves
that process count and repeated ticks do not increase mapping row count.

Capture took 131 microseconds for the 120,000 repeated observations, 5
microseconds for 16 UIDs, and 1,240 microseconds at the 4,096-UID bound. Each
was below one 10 ms CPU clock tick. Production encoding, append, completion,
and read validation took 9,248, 7,017, and 10,630 microseconds respectively.

The optimized test process reached 15,832 KiB, 17,048 KiB, and 20,496 KiB for
the three sequential cases. The last value conservatively includes the test
harness, earlier allocator state, and production Parquet writer, and remains
below the collector's 25,600 KiB limit. Capture itself grew peak RSS by 128 KiB
in the first case and did not raise the already established peak in the next
two cases.

An unresolved UID emits no reference row. A malformed record is rejected while
a separate valid record remains available. A source larger than 256 KiB is
rejected as a whole. These cases add no user-reference WAL or ZMS bytes; process
metrics continue without name enrichment. The same test validates recovery and
forced rollover separately: each resulting segment contains exactly one
mapping and a resolvable segment-local dictionary entry.

The results apply to the approved `/etc/passwd` source. NSS-only, LDAP, SSSD,
and other dynamic identities remain numeric. A UID remap after its first use in
an open segment is visible from the next segment, by design.
