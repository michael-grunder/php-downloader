# AGENTS.md

## Scope and purpose

This file applies to the entire repository. `php-downloader` is a small Rust
2024 CLI for discovering official PHP releases, caching their source tarballs,
extracting complete source trees, running optional build hooks, and upgrading
existing trees while preserving locally added files.

Keep changes focused. Preserve existing behavior unless the task explicitly
changes it, and avoid unrelated refactors or dependency churn.

## Repository map

- `src/main.rs`: CLI definitions and operation orchestration. It also enables
  the `clippy::all`, `clippy::nursery`, and `clippy::pedantic` lint groups.
- `src/downloads.rs`: PHP version parsing and ordering, release discovery,
  download metadata, serialization, and atomic tarball downloads.
- `src/extract.rs`: archive extraction, build-root discovery, manifests,
  custom-file backups, and removal of replaced trees.
- `src/config.rs`: paths under `PHPDOWNLOADER_ROOT` or the user's home and the
  cached active-release data from php.net.
- `src/hooks.rs`: discovery and sequential execution of Unix shell hooks.
- `src/view.rs`: human-readable and JSON output.
- `build.rs`: embeds the Git SHA and reproducible/current build date.
- `install.sh`: installs release or nightly binaries on Linux and macOS.
- `.github/workflows/release.yml`: cross-builds the release artifacts.

## Important behavior and safety boundaries

- Treat user paths and cached data as valuable. Never delete or overwrite a
  tree without following the existing explicit checks and confirmation flow.
- Keep downloads and extraction transactional. Temporary files/directories are
  deliberately created on the destination filesystem before an atomic rename;
  do not casually replace this with direct writes or cross-device moves.
- Preserve manifest semantics during upgrades: files from the original archive
  are tracked, while locally added files are backed up before an old tree can be
  removed.
- Keep `--no-hooks` effective. Hooks are user-provided executable scripts named
  `post-extract`, `configure`, and `make`, run in that order. Avoid invoking
  shells with newly interpolated or untrusted data; prefer structured
  `Command` arguments over shell command construction when changing this area.
- Preserve output contracts. Machine-readable JSON belongs on stdout without
  progress or diagnostic noise; warnings, progress, and human operational
  messages belong on stderr.
- Version parsing, prerelease ordering, partial `major.minor` matching, archive
  extensions, php.net URLs, and museum URLs are domain logic. Cover edge cases
  with tests whenever any of these change.
- The implementation and release workflow currently target Unix platforms
  (Linux and macOS) and rely on Unix permissions and Bash. Do not imply Windows
  support without making the code portable and adding corresponding validation.
- Tests must not use the developer's real home, cache, hooks, PHP trees, or the
  live network. Isolate filesystem state with temporary directories and set
  `PHPDOWNLOADER_ROOT` to test-owned storage where configuration is involved.

## Rust design guidelines

- Prefer idiomatic Rust and use the type system to express invariants. Reach for
  suitable enums, newtypes, traits, associated types, generics, and lifetimes
  when they make invalid states harder to represent or make an API clearer.
- Use the least ownership an operation actually needs. Borrow with `&T` or
  `&mut T` when work is temporary; take `T` by value when ownership is consumed,
  transferred, or the value is small/`Copy`. Passing by value is often simpler
  and faster and does not need to be avoided.
- Do not scatter `clone()`, `to_owned()`, `to_string()`, `Arc`, or leaked/static
  data through the code merely to appease the borrow checker. First reconsider
  ownership boundaries, lifetimes, data layout, and the point at which owned
  data is genuinely required. Clone deliberately when duplication is the
  intended semantic operation or is demonstrably the clearest tradeoff.
- Use generics and trait bounds where callers benefit from them, as with path,
  iterator, writer, or decoder abstractions. Do not introduce abstraction for a
  single hypothetical use. Excessive monomorphization can slow compilation,
  increase binary size, and obscure signatures; prefer a concrete type or a
  trait object when dynamic dispatch and a simpler boundary are the better fit.
- Prefer standard conversion and iterator traits (`From`, `TryFrom`, `FromStr`,
  `AsRef`, `IntoIterator`) over ad hoc conversion helpers when their semantics
  fit. Do not use an overly broad generic bound that weakens the contract.
- Keep lifetimes implicit when elision is clear. Add explicit lifetime notation
  when it accurately relates borrowed inputs and outputs; do not use lifetimes
  or generics as decoration.
- Prefer exhaustive `match` expressions and domain-specific types over boolean
  flag combinations. Use iterators when they improve clarity, but a direct loop
  is fine when it better communicates stateful or fallible work.
- This is an application crate, so `anyhow::Result` with useful `Context` is
  appropriate at I/O and orchestration boundaries. Preserve the original error
  source and add actionable path, URL, version, or operation context. Reserve
  `panic!`, `unwrap`, `expect`, and `unreachable!` for invariants that truly
  cannot be violated, and explain non-obvious invariants.
- Keep asynchronous network work non-blocking. Be deliberate before moving
  CPU-heavy or blocking filesystem/archive work onto async executor threads.
- Avoid `unsafe`. If a task truly requires it, minimize the unsafe surface and
  document and test every safety invariant.
- Follow `rustfmt.toml`; do not hand-format around the formatter. Prefer code
  that satisfies the enabled Clippy groups. A narrowly scoped `#[allow]` is
  acceptable only when the lint is consciously inapplicable, with a nearby
  explanation when the reason is not self-evident. Never add broad allowances
  merely to make validation pass.

## Change workflow

1. Read the relevant module and its callers before editing. Search for all uses
   of affected types, CLI options, output fields, and filesystem paths.
2. Make the smallest cohesive change that addresses the task. Do not rewrite
   user changes or generated state, and do not edit `Cargo.lock` unless the
   dependency graph intentionally changes.
3. Add or update tests with the implementation. Existing unit tests live beside
   the version logic in `src/downloads.rs`; keep small pure tests near their
   module and use integration tests for observable CLI behavior when added.
   Include success, boundary, malformed-input, and failure cases as relevant.
4. Update `README.md`, help text, and examples when user-visible CLI behavior,
   environment variables, hooks, supported platforms, or output changes.
5. Run the quality gates below. Fix warnings at their cause; do not suppress or
   ignore them. If a gate cannot run because of the environment, report the
   exact command and reason and do not describe the change as fully verified.

## Required quality gates

Run these from the repository root after every Rust change:

```sh
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
```

For dependency-sensitive or release changes, also run:

```sh
cargo build --release --locked
```

Use focused tests during development, but always finish with the full commands.
Network-backed manual checks may supplement deterministic tests; they do not
replace them. A feature, fix, or refactor is not done until formatting passes,
all current and newly added tests pass, and Clippy completes with zero warnings.

## Final handoff

Summarize the behavioral change, identify any user-visible or safety impact,
list the validation commands actually run and their results, and call out any
remaining limitation. Do not claim a check passed unless it was run successfully.
