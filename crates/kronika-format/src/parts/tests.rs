use super::*;

/// A minimal valid part: magic + one tiny section + catalog + tail.
fn sample_part() -> Vec<u8> {
    let section = *b"data";
    let mut part = Vec::new();
    part.extend_from_slice(&MAGIC);
    part.extend_from_slice(&section);
    let catalog = Catalog {
        entries: vec![Entry {
            type_id: 1_006_001,
            flags: 0,
            offset: 4,
            len: section.len() as u64,
            rows: 1,
            crc32c: crc32c(&section),
        }],
        min_ts: 1,
        max_ts: 2,
        format_version: crate::FORMAT_VERSION,
        window_count: 1,
    };
    part.extend_from_slice(&catalog.encode());
    part
}

fn frame(part: &[u8]) -> Vec<u8> {
    let mut out = FrameHeader {
        part_len: part.len() as u64,
    }
    .encode()
    .to_vec();
    out.extend_from_slice(part);
    out
}

const fn small_limits() -> JournalLimits {
    JournalLimits { max_part_len: 4096 }
}

#[test]
fn frame_header_layout_is_byte_exact() {
    let encoded = FrameHeader { part_len: 88 }.encode();
    assert_eq!(&encoded[..4], b"ZMSP");
    assert_eq!(&encoded[4..12], &88_u64.to_le_bytes());
    // The CRC pins the covered range: magic + length, little-endian.
    assert_eq!(
        &encoded[12..],
        &crc32c(&encoded[..12]).to_le_bytes(),
        "header crc covers exactly the first 12 bytes"
    );
    assert_eq!(
        FrameHeader::decode(encoded),
        Ok(FrameHeader { part_len: 88 })
    );
}

#[test]
fn frame_header_rejects_damage() {
    let mut bytes = FrameHeader { part_len: 7 }.encode();
    bytes[0] ^= 0xFF;
    assert!(matches!(
        FrameHeader::decode(bytes),
        Err(FrameError::BadMagic { .. })
    ));

    let mut bytes = FrameHeader { part_len: 7 }.encode();
    bytes[5] ^= 0x01;
    assert!(matches!(
        FrameHeader::decode(bytes),
        Err(FrameError::BadCrc { .. })
    ));
}

#[test]
fn validates_a_real_part_and_catches_section_corruption() {
    let part = sample_part();
    let catalog = validate_part(&part).expect("sample part is valid");
    assert_eq!(catalog.entries.len(), 1);

    // Corrupting the section body is caught by the section CRC even
    // though the catalog itself is intact.
    let mut corrupted = part;
    corrupted[5] ^= 0x01;
    assert!(matches!(
        validate_part(&corrupted),
        Err(PartError::SectionCrc { .. })
    ));
}

#[test]
fn catalog_validation_skips_section_body_crc() {
    // A part whose body is corrupt but whose catalog is intact: the full
    // check rejects it, the catalog-only check accepts it (the reader
    // re-verifies bodies on decode).
    let mut part = sample_part();
    part[5] ^= 0x01;
    assert!(matches!(
        validate_part(&part),
        Err(PartError::SectionCrc { .. })
    ));
    assert!(validate_part_catalog(&part).is_ok());
    // The catalog-only check still rejects a structural failure.
    let mut bad_magic = sample_part();
    bad_magic[0] ^= 0xFF;
    assert!(matches!(
        validate_part_catalog(&bad_magic),
        Err(PartError::BadMagic { .. })
    ));
}

#[test]
fn part_validation_rejects_duplicate_section_types() {
    let part = build_part(
        &[
            SectionInput {
                type_id: 1_006_001,
                rows: 1,
                body: b"first",
            },
            SectionInput {
                type_id: 1_006_001,
                rows: 1,
                body: b"second",
            },
        ],
        PartMeta {
            min_ts: 1,
            max_ts: 2,
        },
    );

    assert!(matches!(
        validate_part_catalog(&part),
        Err(PartError::Layout(_))
    ));
}

#[test]
fn part_validation_accepts_canonical_dictionary_tail() {
    let part = build_part(
        &[
            SectionInput {
                type_id: 1_006_001,
                rows: 1,
                body: b"data",
            },
            SectionInput {
                type_id: 3_001_001,
                rows: 1,
                body: b"strings",
            },
            SectionInput {
                type_id: 3_002_001,
                rows: 1,
                body: b"blobs",
            },
        ],
        PartMeta {
            min_ts: 1,
            max_ts: 2,
        },
    );

    assert!(validate_part(&part).is_ok());
}

#[test]
fn build_part_round_trips_through_validate_part() {
    let first: &[u8] = b"section-one-body";
    let second: &[u8] = b"second";
    let part = build_part(
        &[
            SectionInput {
                type_id: 1_006_001,
                rows: 3,
                body: first,
            },
            SectionInput {
                type_id: 1_021_001,
                rows: 1,
                body: second,
            },
        ],
        PartMeta {
            min_ts: 100,
            max_ts: 900,
        },
    );

    let catalog = validate_part(&part).expect("built part is valid");
    assert_eq!(catalog.entries.len(), 2);
    assert_eq!((catalog.min_ts, catalog.max_ts), (100, 900));
    assert_eq!(catalog.entries[0].type_id, 1_006_001);
    assert_eq!(catalog.entries[0].rows, 3);
    assert_eq!(catalog.entries[0].offset, MAGIC.len() as u64);

    // Each recorded (offset, len) slices back to the exact body that went in.
    for (entry, body) in catalog.entries.iter().zip([first, second]) {
        let start = usize::try_from(entry.offset).expect("offset fits usize");
        let len = usize::try_from(entry.len).expect("len fits usize");
        assert_eq!(&part[start..start + len], body);
    }
}

#[test]
fn build_part_accepts_no_sections() {
    let part = build_part(
        &[],
        PartMeta {
            min_ts: 0,
            max_ts: 0,
        },
    );
    let catalog = validate_part(&part).expect("empty part is valid");
    assert!(catalog.entries.is_empty());
}

#[test]
fn a_built_part_passes_the_journal_scan() {
    let part = build_part(
        &[SectionInput {
            type_id: 1_006_001,
            rows: 1,
            body: b"data",
        }],
        PartMeta {
            min_ts: 1,
            max_ts: 2,
        },
    );
    let report = scan_journal(&frame(&part), small_limits());
    assert!(report.is_clean());
    assert_eq!(report.parts.len(), 1);
}

#[test]
fn clean_journal_scans_clean() {
    let part = sample_part();
    let mut journal = Vec::new();
    journal.extend_from_slice(&frame(&part));
    journal.extend_from_slice(&frame(&part));

    let report = scan_journal(&journal, small_limits());
    assert!(report.is_clean());
    assert_eq!(report.parts.len(), 2);
    assert_eq!(report.valid_len, journal.len());
    for part_ref in &report.parts {
        let body = &journal[part_ref.offset..part_ref.offset + part_ref.len];
        assert_eq!(body, part.as_slice());
    }
}

#[test]
fn incomplete_final_frame_keeps_the_valid_prefix() {
    let part = sample_part();
    let mut journal = frame(&part);
    let full = frame(&part);
    journal.extend_from_slice(&full[..full.len() - 3]);

    let report = scan_journal(&journal, small_limits());
    assert_eq!(report.parts.len(), 1);
    assert_eq!(report.damages.len(), 1);
    assert_eq!(report.damages[0].kind, DamageKind::TornTail);
    assert_eq!(
        report.valid_len,
        frame(&part).len(),
        "truncation point is the end of the last valid frame"
    );
}

#[test]
fn middle_corruption_resyncs_and_keeps_both_sides() {
    let part = sample_part();
    let one = frame(&part);
    let mut journal = Vec::new();
    journal.extend_from_slice(&one);
    journal.extend_from_slice(&one);
    journal.extend_from_slice(&one);
    // Corrupt a byte inside the second frame's part body.
    let target = one.len() + FRAME_HEADER_LEN + 5;
    journal[target] ^= 0x01;

    let report = scan_journal(&journal, small_limits());
    assert_eq!(report.parts.len(), 2, "first and third parts survive");
    assert_eq!(report.damages.len(), 1);
    assert!(matches!(
        report.damages[0].kind,
        DamageKind::Middle { resumed_at } if resumed_at == 2 * one.len()
    ));
}

#[test]
fn corrupted_final_header_is_reported_without_truncation() {
    let part = sample_part();
    let one = frame(&part);
    let mut journal = Vec::new();
    journal.extend_from_slice(&one);
    journal.extend_from_slice(&one);
    // Corrupt the second frame's header magic: recovery cannot know where
    // that frame ends, and nothing valid follows it.
    let target = one.len();
    journal[target] ^= 0xFF;

    let report = scan_journal(&journal, small_limits());
    assert_eq!(report.parts.len(), 1);
    assert_eq!(report.damages.len(), 1);
    assert_eq!(report.damages[0].kind, DamageKind::DamagedTail);
    assert_eq!(report.valid_len, one.len());
}

#[test]
fn corrupted_final_body_with_intact_header_is_recoverable() {
    let part = sample_part();
    let one = frame(&part);
    let mut journal = Vec::new();
    journal.extend_from_slice(&one);
    journal.extend_from_slice(&one);
    // The header is intact and the frame ends exactly at the buffer end,
    // but the body is invalid. Treat it like an interrupted write and
    // keep only the valid prefix.
    let target = one.len() + FRAME_HEADER_LEN + 5;
    journal[target] ^= 0x01;

    let report = scan_journal(&journal, small_limits());
    assert_eq!(report.parts.len(), 1);
    assert_eq!(report.damages.len(), 1);
    assert_eq!(report.damages[0].kind, DamageKind::TornTail);
    assert_eq!(report.valid_len, one.len());
}

#[test]
fn resync_prefers_the_header_implied_boundary_over_embedded_frames() {
    // The embedded frame is legitimate section data, not a journal frame.
    let inner = frame(&sample_part());
    let mut tricky = Vec::new();
    tricky.extend_from_slice(&MAGIC);
    tricky.extend_from_slice(&inner);
    let catalog = Catalog {
        entries: vec![Entry {
            type_id: 1_000_001,
            flags: 0,
            offset: 4,
            len: inner.len() as u64,
            rows: 1,
            crc32c: crc32c(&inner),
        }],
        min_ts: 1,
        max_ts: 2,
        format_version: crate::FORMAT_VERSION,
        window_count: 1,
    };
    tricky.extend_from_slice(&catalog.encode());

    let plain = sample_part();
    let mut journal = Vec::new();
    journal.extend_from_slice(&frame(&tricky));
    journal.extend_from_slice(&frame(&plain));
    // Corrupt one byte of the outer catalog of the tricky part, past
    // the embedded frame.
    let target = FRAME_HEADER_LEN + 4 + inner.len() + 3;
    journal[target] ^= 0x01;

    let report = scan_journal(&journal, small_limits());
    assert_eq!(report.parts.len(), 1, "only the real second part");
    let recovered = &journal[report.parts[0].offset..report.parts[0].offset + report.parts[0].len];
    assert_eq!(recovered, plain.as_slice());
    assert!(matches!(
        report.damages[0].kind,
        DamageKind::Middle { resumed_at } if resumed_at == FRAME_HEADER_LEN + tricky.len()
    ));
}

#[test]
fn resync_searches_to_the_end_of_the_buffer() {
    // A long damaged region followed by a valid frame: the search must
    // not give up early, or later appends would be lost on reopen.
    let part = sample_part();
    let mut journal = frame(&part);
    journal.extend_from_slice(&[0xAB_u8; 2048]);
    journal.extend_from_slice(&frame(&part));

    let report = scan_journal(&journal, small_limits());
    assert_eq!(report.parts.len(), 2);
    assert!(matches!(report.damages[0].kind, DamageKind::Middle { .. }));
}

#[test]
fn oversized_length_claim_is_final_damage() {
    let part = sample_part();
    let mut journal = frame(&part);
    // A frame claiming a part over the configured limit, with a
    // valid CRC: damaged by definition, and nothing valid follows.
    journal.extend_from_slice(
        &FrameHeader {
            part_len: small_limits().max_part_len + 1,
        }
        .encode(),
    );

    let report = scan_journal(&journal, small_limits());
    assert_eq!(report.parts.len(), 1);
    assert_eq!(report.damages.len(), 1);
    assert_eq!(report.damages[0].kind, DamageKind::DamagedTail);
}
