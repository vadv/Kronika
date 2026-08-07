# Kronika agent instructions

This file is the contract for every coding agent working in this repository:
Codex, Claude, Kimi. `CLAUDE.md` is a symlink to this file. Read it before
touching code and follow it over your own defaults.

Read `DESIGN.md` first. It describes what Kronika is, how the segment format
works, and what the collector and web must do. This file covers how to work
here, not what is being built.

Ask the owner when something here is missing or contradicts the task. Do not
invent a rule and proceed.

## Rules that are easy to break

These come out of `DESIGN.md` and get violated most often. Check them on every
change.

1. **Memory has a bound.** Every new code path needs an answer to: what is the
   peak memory, and what enforces the bound? A config limit, a format constant,
   or the size of an input the caller already holds are bounds. "Usually small"
   is not. The collector shares a host with a production database.
2. **A metric's fields change, its id changes.** No optional columns added to
   keep an id stable.
3. **A framing change comes with numbers.** Any change to segment framing,
   encoding, or the string dictionary is measured for size on demo data, and
   the PR reports before and after. A framing change without a size benchmark
   is not reviewable.
4. **The collector never guesses its environment.** VM or container is decided
   at collection time and written into the segment header.
5. **A missing metric stays missing.** The collector logs the failure, web
   shows `null`.

## Rust rules

- Write plain Rust a newcomer can read. Simple control flow, simple types, no
  clever generics or macro tricks where a function does the job.
- Do not multiply entities. No trait with one implementation, no factory for
  one product, no config for a value that never changes.
- Split large files into small ones in their own directories. A file that has
  grown past comprehension gets split, not a table of contents comment.
- Keep tests in separate files from the code they test.
- Handle errors explicitly. No `unwrap()` or `expect()` on a path that can fail
  in production.
- Lean on the tooling. Clippy runs strict, warnings are errors, and a lint you
  disagree with gets discussed before it gets an `#[allow]`.

Before proposing anything: `cargo fmt --all --check`, clippy with warnings
denied across the workspace and all targets, and the full test suite.

## Testing

BDD tests come first. Feature files describe observable behavior, and the steps
assert it against a real artifact, not a mock.

BDD runs inside a cached Docker image so the environment is reproducible and CI
does not rebuild the world on every run. Parsing collector log messages is the
reference case: write the feature against the log output an operator would
read.

Pure functions get unit tests in the same change. Put tests in their own files
rather than growing the module they cover.

Do not write `@wip` feature files with no step definitions. A scenario that
asserts nothing is worse than no scenario.

## Pull requests and review

Make PRs large enough to deliver a working piece of behavior, with as many
commits as the work needs. Do not split a change into fragments that nobody can
review on their own.

Before opening a PR or merging one, run a review panel and fix every **high**
finding:

1. **Rust performance** — hot paths, allocations, needless clones, per-row
   work, async overhead.
2. **PostgreSQL DBA** — query cost and locking behavior on the monitored
   instance, correctness across supported PostgreSQL versions, safe SQL.
3. **Rust architect** — module and crate boundaries, public API shape, whether
   the change fits the design or bends it.

When a reviewer proposes something that makes the program more complicated, do
not apply it silently. Ask the owner whether the complexity is worth it. A
review comment can be generated filler, and filler that lands as an abstraction
is expensive to remove later.

## Language

The product is bilingual from the start, `ru` and `en`, with more languages
later. Build user-facing strings and docs so a third language is a translation,
not a rewrite.

Fixed rules:

- **Log messages: English only.** No exceptions, including messages that are
  only ever read during development.
- **Code comments: simple English.** Write what the code cannot say: an
  invariant, a trade-off, a reason. A comment that restates the next line is a
  defect to delete.
- **Commit messages: English.** Say what the change does to the behavior of the
  system and why. Do not list the files you touched.

All three are checked at review. Padding, throat-clearing, and generated prose
that says nothing are blocking findings, in comments and in commit messages
alike.
