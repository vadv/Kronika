use std::path::Path;
use std::sync::Arc;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::pg_log::PgLogLifecycle;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};

use crate::config::SOURCE_POSTGRESQL;
use crate::mcp::State;
use crate::route::{Filter, Route, Window};

const SEGMENT_ID: i64 = 1_710_000_000_000_000;

#[test]
fn metric_history_route_pins_the_captured_active_prefix() {
    let route = super::history_route(
        10,
        20,
        Some((7, 99)),
        "os_process",
        vec!["utime".to_owned()],
        vec![Filter {
            column: "pid".to_owned(),
            value: "42".to_owned(),
        }],
    );
    let Route::Hour(request) = route else {
        panic!("metric history uses the shared Hour reader");
    };

    assert_eq!(request.active_segment, Some((7, 99)));
}

#[test]
fn metric_history_after_read_check_rejects_a_changed_active_prefix() {
    let mut fixture = Fixture::new();
    fixture.append(SEGMENT_ID + 10);
    let state = fixture.state();
    let window = Window {
        from: Some(SEGMENT_ID),
        to: Some(SEGMENT_ID + 100),
    };
    let catalog = super::catalog(&state, window, &|| false).expect("initial catalog");
    let expected = super::catalog_segments(&catalog.records);

    fixture.append(SEGMENT_ID + 20);
    let error = super::ensure_history_source_unchanged(&state, window, &expected, &|| false)
        .expect_err("changed active prefix is rejected");

    assert_eq!((error.code, error.retryable), ("source_changed", true));
}

struct Fixture {
    directory: tempfile::TempDir,
    _writer: WriterOwner,
    journal: Journal,
    address: SegmentAddress,
    interner: Interner,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary history data root");
        let root = DataRoot::open(directory.path()).expect("open history data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire history writer");
        let journal =
            Journal::open(&writer, JournalConfig::default()).expect("open history journal");
        let address = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("segment id"))
            .expect("segment address");
        Self {
            directory,
            _writer: writer,
            journal,
            address,
            interner: Interner::new(DictLimits::default()),
        }
    }

    fn state(&self) -> State {
        State {
            data_root: self.root().to_owned(),
            sources: SOURCE_POSTGRESQL,
            synthetic_demo: false,
            heavy_scans: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn append(&mut self, timestamp: i64) {
        let source = self.intern(b"postgresql.csv");
        let message = self.intern(b"recorded lifecycle event");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(PgLogLifecycle {
                ts: Ts(timestamp),
                system_identifier: Some(7),
                source_file: source,
                kind: 0,
                pid: Some(42),
                signal: None,
                shutdown_mode: None,
                message,
                query_detail: None,
            })
            .expect("Lifecycle row fits");
        let dictionary = dict::encode(self.interner.window()).expect("encode history dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode history fixture")
            .expect("nonempty history fixture");
        let journal = &mut self.journal;
        let segment_id = self.address.id;
        self.interner
            .flush_window(|_window| journal.append(segment_id, &part).map(|_part| ()))
            .expect("append history fixture");
    }

    fn intern(&mut self, value: &[u8]) -> StrId {
        StrId(
            self.interner
                .intern(value)
                .expect("intern history text")
                .get(),
        )
    }
}
