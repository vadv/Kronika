//! Read a data directory back and print what is in it.
//!
//! Everything here goes through `kronika-reader`, the same path web takes, so
//! what this prints is what a dashboard would be served.

mod args;
mod render;

use std::ops::Bound;
use std::process::ExitCode;

use kronika_reader::Reader;

use crate::args::{USAGE, Want};

fn main() -> ExitCode {
    let parsed = match args::parse(std::env::args().skip(1)) {
        Ok(parsed) => parsed,
        Err(problem) => {
            eprintln!("kronika-dump: {problem}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match run(&parsed) {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("kronika-dump: {problem}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &args::Args) -> Result<(), String> {
    let reader = Reader::open(&args.root).map_err(|error| format!("{error:#}"))?;
    let listing = reader
        .segments((
            args.from.map_or(Bound::Unbounded, Bound::Included),
            args.to.map_or(Bound::Unbounded, Bound::Included),
        ))
        .map_err(|error| format!("{error:#}"))?;
    let out = render::Output::new(args.json);
    for warning in &listing.warnings {
        render::warning(&out, warning);
    }
    for reference in &listing.segments {
        let segment = reader
            .open_segment(reference)
            .map_err(|error| format!("{error:#}"))?;
        match args.want {
            Want::Sizes => render::sizes(&out, &segment),
            Want::Index => render::index(&out, &segment).map_err(|error| format!("{error:#}"))?,
            Want::Section(type_id) => {
                render::section(&out, &segment, type_id, args.limit)
                    .map_err(|error| format!("{error:#}"))?;
            }
        }
    }
    Ok(())
}
