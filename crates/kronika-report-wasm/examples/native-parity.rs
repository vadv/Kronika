//! Native byte oracle for the browser adapter harness.

use std::io::Write as _;

use kronika_api as _;
use kronika_layout as _;
use kronika_query::{SOURCE_OS, SOURCE_POSTGRESQL};
use kronika_report as _;
use wasm_bindgen as _;

const SEGMENT_ID: &str = "1709164800000000";
const ZMS: &[u8] = include_bytes!("../../../bins/kronika-report/tests/fixtures/standalone.zms");
const IDX: &[u8] = include_bytes!("../../../bins/kronika-report/tests/fixtures/standalone.idx");

fn argument(position: usize, name: &str) -> Result<String, std::io::Error> {
    std::env::args()
        .nth(position)
        .ok_or_else(|| std::io::Error::other(format!("missing {name} argument")))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = argument(1, "path")?;
    let query = argument(2, "query")?;
    let mut response = kronika_report_wasm::request(
        SEGMENT_ID,
        ZMS.to_vec(),
        IDX.to_vec(),
        SOURCE_OS | SOURCE_POSTGRESQL,
        u64::try_from(ZMS.len())?,
        &path,
        &query,
    );
    if response.status() != 200 {
        return Err(std::io::Error::other(format!(
            "{}: {}",
            response.code().as_deref().unwrap_or("unknown"),
            response.message().as_deref().unwrap_or("request refused")
        ))
        .into());
    }
    std::io::stdout().lock().write_all(&response.take_body())?;
    Ok(())
}
