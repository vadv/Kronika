//! Read a data directory back and print what is in it.
//!
//! Everything here goes through `kronika-reader`, the same path web takes, so
//! what this prints is what a dashboard would be served.

mod args;
mod render;

use std::fmt;
use std::io;
use std::ops::Bound;
use std::process::ExitCode;

use kronika_reader::{Reader, ReaderError};

use crate::args::{USAGE, Want};

fn main() -> ExitCode {
    let parsed = match args::parse(std::env::args().skip(1)) {
        Ok(parsed) => parsed,
        Err(problem) => {
            eprintln!("kronika-dump: {problem}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let stdout = io::stdout();
    let mut output = stdout.lock();
    match run(&parsed, &mut output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(DumpError::Output(problem)) if problem.kind() == io::ErrorKind::BrokenPipe => {
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("kronika-dump: {problem}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
enum DumpError {
    Reader(ReaderError),
    Output(io::Error),
}

impl fmt::Display for DumpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reader(problem) => problem.fmt(f),
            Self::Output(problem) => write!(f, "write output: {problem}"),
        }
    }
}

impl From<ReaderError> for DumpError {
    fn from(problem: ReaderError) -> Self {
        Self::Reader(problem)
    }
}

impl From<io::Error> for DumpError {
    fn from(problem: io::Error) -> Self {
        Self::Output(problem)
    }
}

fn run(args: &args::Args, output: &mut impl io::Write) -> Result<(), DumpError> {
    let reader = Reader::open(&args.root)?;
    let listing = reader.segments((
        args.from.map_or(Bound::Unbounded, Bound::Included),
        args.to.map_or(Bound::Unbounded, Bound::Included),
    ))?;
    for warning in &listing.warnings {
        render::warning(output, args.json, warning)?;
    }
    for reference in &listing.segments {
        let segment = reader.open_segment(reference)?;
        match args.want {
            Want::Sizes => render::sizes(output, args.json, &segment)?,
            Want::Index => render::index(output, args.json, &segment)?,
            Want::Section(type_id) => {
                render::section(output, args.json, &segment, type_id, args.limit)?;
            }
        }
    }
    Ok(())
}
