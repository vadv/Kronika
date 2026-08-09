use super::bounds::{
    FinalPlainColumnSize, final_single_batch_plain_body_bound, zstd_compress_bound,
};
use super::{CodecError, FINAL_FILE_FRAMING_BOUND, FINAL_PAGE_FRAMING_BOUND};

#[test]
fn single_batch_bound_covers_partitioned_zstd_frames() {
    let partitions = [0, 1, 255, 256, 131_071, 131_072];
    let page_input = partitions.iter().sum::<usize>();
    let pages = partitions.len();
    let bound = final_single_batch_plain_body_bound(
        [FinalPlainColumnSize::new("value", page_input, 0)],
        pages,
    )
    .expect("partitioned column fits");
    let exact_compression_bounds = partitions
        .into_iter()
        .map(|bytes| zstd_compress_bound(bytes).expect("small frame bound fits"))
        .sum::<usize>();
    let partitioned_bound =
        FINAL_FILE_FRAMING_BOUND + exact_compression_bounds + pages * FINAL_PAGE_FRAMING_BOUND;

    assert!(bound >= partitioned_bound);
}

#[test]
fn single_batch_bound_rejects_zero_pages_and_overflow() {
    assert!(matches!(
        final_single_batch_plain_body_bound([FinalPlainColumnSize::new("value", 1, 0)], 0,),
        Err(CodecError::InvalidPageLayout)
    ));
    assert!(matches!(
        final_single_batch_plain_body_bound([FinalPlainColumnSize::new("value", usize::MAX, 1)], 1,),
        Err(CodecError::SectionTooLarge { .. })
    ));
}
