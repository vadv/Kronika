use bytes::Bytes;
use parquet::basic::{Compression, Encoding};
use parquet::column::page::Page;
use parquet::file::reader::{FileReader, SerializedFileReader};

use super::{FINAL_ZSTD_LEVEL, VerifiedSection, encode_final_batches};
use crate::os_loadavg::OsLoadavg;
use crate::{Section, Ts, decode_any};

fn row(load1: f64) -> OsLoadavg {
    OsLoadavg {
        ts: Ts(42),
        load1,
        load5: 2.0,
        load15: 0.5,
        running: 2,
        total: 345,
        scope: 0,
    }
}

fn decoded_batches(rows: &[OsLoadavg]) -> Vec<arrow_array::RecordBatch> {
    let body = OsLoadavg::encode(rows).expect("encode input section");
    decode_any(
        OsLoadavg::CONTRACT.type_id.get(),
        VerifiedSection::for_test(Bytes::from(body)),
    )
    .expect("decode input section")
    .batches
}

#[test]
fn finished_encoding_is_physical_and_boundary_deterministic() {
    let rows = [
        row(f64::from_bits(0x7ff8_0000_0000_0002)),
        row(-0.0),
        row(0.0),
        row(f64::from_bits(0x7ff8_0000_0000_0001)),
        row(-0.0),
    ];
    let one_batch = decoded_batches(&rows);
    let many_reversed = rows
        .iter()
        .rev()
        .flat_map(|row| decoded_batches(std::slice::from_ref(row)))
        .collect::<Vec<_>>();
    let type_id = OsLoadavg::CONTRACT.type_id.get();
    let one = encode_final_batches(type_id, one_batch).expect("write one batch");
    let many =
        encode_final_batches(type_id, many_reversed).expect("write reversed one-row batches");
    assert_eq!(one, many, "partition and input order must not affect bytes");

    let decoded = decode_any(type_id, VerifiedSection::for_test(Bytes::from(one.clone())))
        .expect("decode final section");
    assert_eq!(
        decoded.stats.rows,
        rows.len(),
        "duplicate rows are retained"
    );
    let typed = OsLoadavg::decode(VerifiedSection::for_test(Bytes::from(one.clone())))
        .expect("decode typed finished rows");
    assert_eq!(
        typed
            .iter()
            .map(|row| row.load1.to_bits())
            .collect::<Vec<_>>(),
        vec![
            (-0.0_f64).to_bits(),
            (-0.0_f64).to_bits(),
            0.0_f64.to_bits(),
            0x7ff8_0000_0000_0001,
            0x7ff8_0000_0000_0002,
        ],
        "canonical ordering preserves NaN payloads, signed zero, and duplicates"
    );
    assert_eq!(FINAL_ZSTD_LEVEL, 6);

    let reader = SerializedFileReader::new(Bytes::from(one)).expect("open Parquet metadata");
    let metadata = reader.metadata();
    assert_eq!(metadata.file_metadata().version(), 1);
    assert_eq!(metadata.file_metadata().created_by(), Some(""));
    assert_eq!(metadata.num_row_groups(), 1);
    let row_group = metadata.row_group(0);
    for column in row_group.columns() {
        assert!(matches!(column.compression(), Compression::ZSTD(_)));
        assert!(column.statistics().is_none());
        assert!(column.dictionary_page_offset().is_none());
        assert!(
            column
                .encodings()
                .iter()
                .all(|encoding| matches!(encoding, Encoding::PLAIN | Encoding::RLE)),
            "final columns use only PLAIN data and RLE levels"
        );
    }
    let group = reader.get_row_group(0).expect("row group");
    for column in 0..group.metadata().num_columns() {
        let mut pages = group.get_column_page_reader(column).expect("page reader");
        let mut data_pages = 0;
        while let Some(page) = pages.get_next_page().expect("read page") {
            if matches!(page, Page::DataPage { .. } | Page::DataPageV2 { .. }) {
                data_pages += 1;
            }
        }
        assert_eq!(data_pages, 1, "one data page per column chunk");
    }
}
