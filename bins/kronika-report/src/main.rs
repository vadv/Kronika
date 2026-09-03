//! Standalone Kronika HTML report generator.

#[cfg(test)]
use serde_json as _;

use kronika_index as _;
use kronika_layout as _;
use kronika_query as _;
use kronika_report as _;
use kronika_store as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    let _input = arguments
        .next()
        .ok_or_else(|| std::io::Error::other("missing standalone ZMS input"))?;
    let _output = arguments
        .next()
        .ok_or_else(|| std::io::Error::other("missing HTML output"))?;
    if arguments.next().is_some() {
        return Err(std::io::Error::other("expected one ZMS input and one HTML output").into());
    }
    Err(std::io::Error::other("HTML generation is not available in this intermediate build").into())
}
