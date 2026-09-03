//! Standalone Kronika HTML report generator.

mod generator;

use kronika_report as _;
#[cfg(test)]
use serde_json as _;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: kronika-report <signed-decimal>.zms <output>.html";

fn arguments(
    values: impl IntoIterator<Item = OsString>,
) -> Result<(PathBuf, PathBuf), &'static str> {
    let mut values = values.into_iter();
    let input = values.next().ok_or("missing standalone ZMS input")?;
    let output = values.next().ok_or("missing HTML output")?;
    if values.next().is_some() {
        return Err("expected one ZMS input and one HTML output");
    }
    Ok((input.into(), output.into()))
}

fn main() -> ExitCode {
    let (input, output) = match arguments(std::env::args_os().skip(1)) {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("kronika-report: {error}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match generator::generate(&input, &output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kronika-report: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
