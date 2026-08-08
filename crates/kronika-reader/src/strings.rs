//! Resolving the dictionary ids that sections carry instead of text.

use std::collections::HashMap;

use arrow_array::{Array as _, BinaryArray, UInt64Array};
use kronika_registry::Bytes;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use crate::error::ReaderError;

/// One segment's string dictionary.
///
/// A `StrId` cell holds an id, not text; the text lives once per segment here.
#[derive(Debug, Default, Clone)]
pub struct Strings {
    by_id: HashMap<u64, String>,
}

impl Strings {
    /// The text interned under `id`, or `None` when the segment never used it.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&str> {
        self.by_id.get(&id).map(String::as_str)
    }

    /// Decode a `dict.strings` section body.
    ///
    /// Non-UTF-8 bytes are kept lossily: the dictionary stores what a log line
    /// held, and a reader that refused the segment over one bad byte would be
    /// worse than one that shows the replacement character.
    ///
    /// # Errors
    ///
    /// Returns [`ReaderError::Dictionary`] when the body is not Parquet, and
    /// [`ReaderError::DictionaryShape`] when it is Parquet without the columns
    /// a dictionary has.
    pub(crate) fn decode(body: Bytes) -> Result<Self, ReaderError> {
        let reader = ParquetRecordBatchReaderBuilder::try_new(body)
            .map_err(ReaderError::Dictionary)?
            .build()
            .map_err(ReaderError::Dictionary)?;
        let mut by_id = HashMap::new();
        for batch in reader {
            let batch = batch.map_err(|error| ReaderError::Dictionary(error.into()))?;
            let ids = batch
                .column_by_name("str_id")
                .and_then(|column| column.as_any().downcast_ref::<UInt64Array>())
                .ok_or(ReaderError::DictionaryShape("no str_id column"))?;
            let bytes = batch
                .column_by_name("bytes")
                .and_then(|column| column.as_any().downcast_ref::<BinaryArray>())
                .ok_or(ReaderError::DictionaryShape("no bytes column"))?;
            for row in 0..batch.num_rows() {
                by_id.insert(
                    ids.value(row),
                    String::from_utf8_lossy(bytes.value(row)).into_owned(),
                );
            }
        }
        Ok(Self { by_id })
    }
}
