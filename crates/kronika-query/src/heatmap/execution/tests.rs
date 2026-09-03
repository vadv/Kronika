use std::error::Error;

use kronika_reader::ReaderError;

use super::super::HeatmapError;
use crate::QueryError;

#[test]
fn storage_error_keeps_its_source_chain() {
    let reader = HeatmapError::storage(
        0,
        QueryError::from(ReaderError::Io(std::io::Error::from(
            std::io::ErrorKind::Interrupted,
        ))),
    )
    .into_query();
    assert!(reader.source_changed_during_read());

    let remote = HeatmapError::storage(0, RemoteError).into_query();
    let heatmap = remote.source().expect("heatmap wrapper");
    assert!(
        heatmap
            .source()
            .is_some_and(|source| source.is::<RemoteError>())
    );
}

#[derive(Debug)]
struct RemoteError;

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("remote object failed")
    }
}

impl Error for RemoteError {}
