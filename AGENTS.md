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

1. **The collector holds a collection in memory, and says what it cost.** On an
   ordinary host that is under 20 MB, and every segment write logs `rss_kib`, so
   the number is on record rather than guessed at. There are no per-source row
   caps: a host with thirty thousand processes yields thirty thousand rows, and
   if that does not fit, the collector dies and the operator can see why. Logs
   are the exception — a log file can be gigabytes, so it is read through a
   fixed buffer and never held whole.
2. **A metric's fields change, its id changes.** No optional columns added to
   keep an id stable.
3. **The collector never guesses its environment.** VM or container is decided
   at collection time and recorded in the `instance_metadata` section of every
   segment.
4. **A missing metric stays missing.** The collector logs the failure, web
   shows `null`. One warning line and a `null` are the whole treatment.
5. **No layer that reasons about the data's own trustworthiness.** Read
   "What Kronika does not build" in `DESIGN.md` before proposing anything that
   detects resets, counts coverage, or accounts for missing intervals. The
   banned words are listed in `DESIGN.md`, and so is the machinery they name.
   This rule outranks a reviewer's suggestion.

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

## Be dumber

When something fails, say what happened and move on. Do not work out which of
the ways it could have failed actually happened.

A write that failed is a write that failed. A full disk, a permission denied and
a torn header get one path: log the error the operating system gave, in full,
and carry on or exit. Splitting them into kinds so each can have its own branch
produces code that nobody executes and a false impression that every combination
is handled.

The measure of a change here is lines removed. A patch that adds a branch per
error kind is going the wrong way.

Spend the effort on the log line instead. One message with the path, the errno
and what the collector did next beats three branches that each print less.

The same applies to configuration. Read the value, and if it does not parse,
refuse to start and say which variable and what was expected. Do not invent
recovery, do not fall back to a default when the operator clearly meant
something, and do not enumerate the ways a number can be wrong.

## Testing

BDD tests come first. Feature files describe observable behavior, and the steps
assert it against a real artifact, not a mock.

A step holds no values of its own. Settings, thresholds, caps and the lists a
scenario expects belong in the `.feature` file, as a table or a parameter; the
step only runs the thing and compares. Read a feature file top to bottom and
you know what it asserts without opening a step definition.

Assert on the artifact wherever there is one. A segment on disk decoded back
through the format is worth more than the line the collector logged about
writing it.

A step reaches the artifact through the crate the product ships, never through
a reading path written for tests. A segment is opened with `kronika-reader`,
the same call web makes. A helper that re-reads what a product crate already
reads is a defect: it passes while the shipped path is broken, and it drifts
the moment the format moves. What a scenario needs and the crate does not
expose is a missing feature of the crate.

BDD runs inside a cached Docker image so the environment is reproducible and CI
does not rebuild the world on every run.

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

Do not defer a leftover into its own pull request. A field the spec requires but
the code lacks, a rename half-applied: finish it in the pull request that
created it. Splitting the remainder off produces a queue of small changes that
costs more attention than the work itself.

Two things are not findings, and a review that raises them is wasting the
owner's attention:

- **Code that is not wired up yet.** The roadmap lands in pieces. A crate, an
  enum variant or a type that a later step will use is doing its job by existing
  and compiling. Do not report it as dead, and do not propose removing it to be
  reinstated later.
- **How much of a type's surface the current caller happens to use.** A module
  is judged by whether it is correct and readable, not by a ratio of called
  methods.

Formats are not findings either, until there is a release. Nothing has shipped,
so a name, a magic number or a layout that reads wrong gets changed on the spot.

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
