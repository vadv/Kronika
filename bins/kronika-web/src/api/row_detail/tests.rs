use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentId, WriterOwner};
use kronika_registry::os_topology::OsTopology;
use kronika_registry::{Section as _, StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};

use super::{RowDetailRequest, read_row_detail};
use crate::api::ApiError;

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const TYPE_ID: u32 = OsTopology::CONTRACT.type_id.get();

struct Fixture {
    directory: tempfile::TempDir,
    _writer: WriterOwner,
    journal: Journal,
}

impl Fixture {
    fn new(text: &[u8], limits: DictLimits) -> Self {
        let directory = tempfile::tempdir().expect("temporary data root");
        let root = DataRoot::open(directory.path()).expect("open data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire writer");
        let journal = Journal::open(&writer, JournalConfig::default()).expect("open journal");
        let mut fixture = Self {
            directory,
            _writer: writer,
            journal,
        };
        fixture.append(100, 7, text, limits);
        fixture
    }

    fn root(&self) -> &std::path::Path {
        self.directory.path()
    }

    fn append(&mut self, timestamp: i64, cpu_id: i32, text: &[u8], limits: DictLimits) {
        let mut interner = Interner::new(limits);
        let text = StrId(interner.intern(text).expect("intern text").get());
        let dictionary = dict::encode(interner.window()).expect("encode dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(OsTopology {
                ts: Ts(timestamp),
                cpu_id,
                model_name: text,
                mhz_max: Some(3_600.0),
                core_id: cpu_id,
                socket_id: 0,
                numa_node: 0,
                scope: 0,
            })
            .expect("buffer topology row");
        let part = buffers
            .flush(&dictionary)
            .expect("encode row")
            .expect("row is present");
        let id = SegmentId::new(SEGMENT_ID).expect("segment id");
        self.journal.append(id, &part).expect("append row");
    }
}

fn request() -> RowDetailRequest {
    RowDetailRequest {
        segment_id: SEGMENT_ID,
        type_id: TYPE_ID,
        row_ordinal: 0,
        timestamp_us: 100,
        fields: vec!["cpu_id".to_owned(), "mhz_max".to_owned()],
        text_field: Some("model_name".to_owned()),
        byte_offset: None,
        byte_limit: 5,
        cursor: None,
    }
}

#[test]
fn exact_row_text_cursor_pins_active_prefix_and_binds_the_query() {
    let mut fixture = Fixture::new(b"hello world", DictLimits::default());
    let first = read_row_detail(fixture.root(), &request()).expect("first chunk");
    assert_eq!(first.row["type_id"], TYPE_ID.to_string());
    assert_eq!(first.row["ordinal"], "0");
    assert_eq!(first.row["timestamp"], "100");
    assert_eq!(first.row["values"], serde_json::json!([7, 3600.0]));
    assert_eq!(first.text_chunk.as_ref().unwrap()["representation"], "utf8");
    assert!(
        first.text_chunk.as_ref().unwrap()["str_id"]
            .as_str()
            .is_some()
    );
    assert_eq!(first.text_chunk.as_ref().unwrap()["utf8"], "hello");
    assert_eq!(first.text_chunk.as_ref().unwrap()["stored_len"], "11");
    assert_eq!(first.text_chunk.as_ref().unwrap()["source_full_len"], "11");
    assert_eq!(first.text_chunk.as_ref().unwrap()["chunk_truncated"], true);
    assert_eq!(
        first.text_chunk.as_ref().unwrap()["source_truncated"],
        false
    );
    let cursor = first.next_cursor.expect("continuation cursor");
    let pinned = first.active_position.expect("active prefix");

    fixture.append(101, 8, b"later row", DictLimits::default());
    let mut continuation = request();
    continuation.cursor = Some(cursor);
    let second = read_row_detail(fixture.root(), &continuation).expect("second chunk");
    assert_eq!(second.active_position, Some(pinned));
    assert_eq!(second.text_chunk.as_ref().unwrap()["byte_offset"], "5");
    assert_eq!(second.text_chunk.as_ref().unwrap()["utf8"], " worl");

    continuation.byte_limit = 4;
    assert!(matches!(
        read_row_detail(fixture.root(), &continuation),
        Err(ApiError::BadCursor)
    ));
    continuation.byte_limit = 5;
    continuation.byte_offset = Some(0);
    assert!(matches!(
        read_row_detail(fixture.root(), &continuation),
        Err(ApiError::BadCursor)
    ));

    let mut changed_projection = request();
    changed_projection.fields = vec!["cpu_id".to_owned()];
    changed_projection.cursor = continuation.cursor;
    assert!(matches!(
        read_row_detail(fixture.root(), &changed_projection),
        Err(ApiError::BadCursor)
    ));
}

#[test]
fn blob_chunk_is_lossless_and_keeps_source_truncation_facts() {
    let limits = DictLimits::new(4, 8).expect("small test limits");
    let bytes = [0xff, 0x00, b'a', b'b', b'c', b'd', b'e', b'f', b'g', b'h'];
    let fixture = Fixture::new(&bytes, limits);
    let mut request = request();
    request.byte_limit = 8;
    let detail = read_row_detail(fixture.root(), &request).expect("blob detail");
    let chunk = detail.text_chunk.expect("text chunk");

    assert_eq!(chunk["storage"], "blob");
    assert_eq!(chunk["representation"], "base64");
    assert_eq!(chunk["base64"], "/wBhYmNkZWY=");
    assert_eq!(chunk["stored_len"], "8");
    assert_eq!(chunk["source_full_len"], "10");
    assert_eq!(chunk["chunk_truncated"], false);
    assert_eq!(chunk["source_truncated"], true);
    assert_eq!(chunk["source_sha256"].as_str().map(str::len), Some(64));
    assert!(detail.next_cursor.is_none());
    assert!(detail.source_truncated);
}

#[test]
fn exact_locator_and_text_field_type_are_enforced() {
    let fixture = Fixture::new(b"recorded", DictLimits::default());
    let mut row_only = request();
    row_only.text_field = None;
    let detail = read_row_detail(fixture.root(), &row_only).expect("projected row only");
    assert!(detail.text_chunk.is_none());
    assert!(detail.next_cursor.is_none());

    row_only.byte_offset = Some(0);
    assert!(matches!(
        read_row_detail(fixture.root(), &row_only),
        Err(ApiError::BadFilter(parameter)) if parameter == "byte_offset"
    ));

    let mut mismatch = request();
    mismatch.timestamp_us = 99;
    assert!(matches!(
        read_row_detail(fixture.root(), &mismatch),
        Err(ApiError::BadCursor)
    ));

    mismatch = request();
    mismatch.row_ordinal = 1;
    assert!(matches!(
        read_row_detail(fixture.root(), &mismatch),
        Err(ApiError::BadCursor)
    ));

    mismatch = request();
    mismatch.text_field = Some("cpu_id".to_owned());
    assert!(matches!(
        read_row_detail(fixture.root(), &mismatch),
        Err(ApiError::BadFilter(parameter)) if parameter == "text_field"
    ));
}
