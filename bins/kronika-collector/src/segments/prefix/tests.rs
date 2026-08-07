use kronika_format::{
    FrameHeader, JOURNAL_HEADER_LEN, JournalHeader, JournalState, PartMeta, SectionInput,
    build_part,
};

use super::readable_prefix;

const SEGMENT_ID: i64 = 1_709_164_800_000_000;

fn part(ts: i64) -> Vec<u8> {
    build_part(
        &[SectionInput {
            type_id: 1_105_001,
            rows: 1,
            body: b"loadavg-row",
        }],
        PartMeta {
            min_ts: ts,
            max_ts: ts,
        },
    )
}

fn framed(parts: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = parts
        .iter()
        .flat_map(|part| {
            let mut frame = FrameHeader {
                part_len: part.len() as u64,
            }
            .encode()
            .to_vec();
            frame.extend_from_slice(part);
            frame
        })
        .collect();
    let mut out = JournalHeader {
        state: JournalState::Active {
            segment_id: SEGMENT_ID,
        },
        body_len: body.len() as u64,
    }
    .encode()
    .to_vec();
    out.extend_from_slice(&body);
    out
}

#[test]
fn an_empty_file_holds_nothing_to_read() {
    let prefix = readable_prefix(&[].as_slice()).expect("an empty source reads");
    assert_eq!(prefix.segment_id, None);
    assert!(prefix.parts.is_empty());
}

#[test]
fn an_intact_journal_reads_to_its_end() {
    let journal = framed(&[part(1_000), part(2_000)]);
    let prefix = readable_prefix(&journal.as_slice()).expect("an intact journal reads");
    assert_eq!(prefix.segment_id, Some(SEGMENT_ID));
    assert_eq!(prefix.parts.len(), 2);
    let last = prefix.parts.last().expect("two frames");
    assert_eq!(
        last.offset + last.len,
        journal.len(),
        "nothing is left over"
    );
}

#[test]
fn a_journal_cut_mid_frame_reads_up_to_the_cut() {
    let journal = framed(&[part(1_000), part(2_000)]);
    let first_end = {
        let prefix = readable_prefix(&journal.as_slice()).expect("intact");
        let first = prefix.parts[0];
        first.offset + first.len
    };
    // Keep the first frame whole and cut into the middle of the second.
    let torn = &journal[..first_end + 8];
    let prefix = readable_prefix(&torn).expect("a torn journal reads");
    assert_eq!(prefix.segment_id, Some(SEGMENT_ID));
    assert_eq!(prefix.parts.len(), 1, "only the complete frame");
    assert_eq!(prefix.parts[0].offset + prefix.parts[0].len, first_end);
}

#[test]
fn a_journal_whose_frames_are_corrupt_reads_only_the_frames_before_the_damage() {
    let mut journal = framed(&[part(1_000), part(2_000)]);
    let second_frame_at = {
        let prefix = readable_prefix(&journal.as_slice()).expect("intact");
        let first = prefix.parts[0];
        first.offset + first.len
    };
    journal[second_frame_at + 1] ^= 0xFF;
    let prefix = readable_prefix(&journal.as_slice()).expect("a damaged journal reads");
    assert_eq!(prefix.parts.len(), 1);
}

#[test]
fn garbage_from_the_first_byte_holds_nothing_to_read() {
    let garbage = vec![0xAB_u8; JOURNAL_HEADER_LEN * 4];
    let prefix = readable_prefix(&garbage.as_slice()).expect("garbage reads");
    assert_eq!(prefix.segment_id, None);
    assert!(prefix.parts.is_empty());
}

#[test]
fn an_empty_header_names_no_segment_to_salvage_into() {
    let empty = JournalHeader::EMPTY.encode().to_vec();
    let prefix = readable_prefix(&empty.as_slice()).expect("an empty header reads");
    assert_eq!(prefix.segment_id, None);
    assert!(prefix.parts.is_empty());
}
