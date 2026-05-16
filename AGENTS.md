# Repository Guidelines

## Project Structure & Module Organization

Piers is a Rust workspace with a native host and a reloadable Rust guest.
The host lives in `src/`: `main.rs` owns the line REPL, `harness.rs` owns
guest lifecycle, `runtime.rs` owns Wasmtime setup, `staging.rs` owns staged
source/artifact promotion, and `guest_component.rs` contains generated WIT
bindings. The guest is in `guest/src/lib.rs` and compiles to `wasm32-wasip2`.
The component interface is `wit/piers.wit`. Design notes and rule packs live
under `docs/`; the copied reviewed rule pack is in `docs/development/rules/`.

## Build, Test, and Development Commands

Run commands from the repository root:

```bash
cargo +nightly fmt --all -- --check
cargo run
cargo build -p piers-guest --target wasm32-wasip2
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
markdownlint-cli2 "**/*.md"
```

`cargo run` starts the REPL. Use `:evolve <spec>` to exercise staged guest
reload and `:status` to inspect the current generation.
When the local `justfile` is available, `just ci` runs the repo's aggregate
formatting, check, test, clippy, rustdoc, and markdown validation.

## Coding Style & Naming Conventions

Use `cargo fmt` and standard Rust naming. Keep modules concept-owned and small:
filesystem effects belong in `staging.rs`, Wasmtime setup in `runtime.rs`, and
lifecycle transitions in `harness.rs`. Use `fs_err` for filesystem operations,
preserve error context with `anyhow`, and log owned lifecycle boundaries with
`tracing`.

Follow local code, tests, docs, and existing conventions before general
preferences. Keep the Rust host/guest boundary explicit: generated guest
behavior must be staged before promotion.

## Testing Guidelines

There are currently no substantive unit tests, so validation relies on compile,
lint, rustdoc, and smoke checks. For reload work, run a scripted REPL flow that
changes behavior and changes it back, confirming call counts continue across
reloads. Add tests near the owning module when logic becomes testable.

## Commit & Pull Request Guidelines

Keep changes small, atomic, and reviewable. Describe each change with an
imperative summary, for example `Implement transactional reload`. PRs should
include the linked issue, problem, non-goals, and validation commands run. For
terminal or REPL behavior, include the manual smoke command or transcript.
Keep GitHub issues and docs standalone enough for collaborators who did not see
the original chat context.

## Shared Development Preferences

This repo carries a local copy of shared development guidance in
`docs/development/`. Use this repo's local rules first. When local guidance is
silent, use the shared guidance as a fallback.

Entry points:

- `docs/development/snippets/agents/rules.md`: compact reviewed rule pack.
- `docs/development/rules/README.md`: rule domains for targeted loading.
- `docs/development/bootstrap-downstream.md`: how to refresh and merge the
  guidance.
- [Software Practices](https://www.joshka.net/practice/): rendered reference
  with deeper guide, rule, pattern, principle, mechanism, and tag context.

If a shared rule causes friction or seems wrong for most Rust or agent work,
capture that feedback for the upstream `development-preferences`/`practice`
repo instead of only patching around it locally.

## Agent-Specific Instructions

Use this file as the repo-local map. The reviewed rule pack is copied from the
canonical `practice` template into `docs/development/`; refresh it with
`python3 docs/development/update.py` when the shared rule set changes.

Agents are expected to know about and use these repo-local guidance files when
they match the task:

- `AGENTS.override.md`: local checkout-specific overrides, including source
  control preferences.
- `docs/development/README.md`: local map for development guidance.
- `docs/development/bootstrap-downstream.md`: downstream bootstrap and refresh
  process.
- `docs/development/update.py`: helper that refreshes the copied guidance from
  the canonical source.
- `docs/development/snippets/agents/rules.md`: compact reviewed rule pack.
- `docs/development/rules/README.md`: index of reviewed rule domains.
- `docs/development/rules/agent-workflow.md`: agent workflow and handoff rules.
- `docs/development/rules/boundary.md`: ownership, lifecycle, and boundary rules.
- `docs/development/rules/change-shape.md`: reviewable change-shape rules.
- `docs/development/rules/documentation.md`: docs-as-contracts rules.
- `docs/development/rules/observability.md`: diagnostics and failure rules.
- `docs/development/rules/performance.md`: measurement and optimization rules.
- `docs/development/rules/refactoring.md`: local reasoning and refactor rules.
- `docs/development/rules/review.md`: review artifact and private-context rules.
- `docs/development/rules/rust.md`: Rust API and crate-shape rules.
- `docs/development/rules/source.md`: source and context hygiene rules.
- `docs/development/rules/test-failures.md`: useful test-failure output rules.
- `docs/development/rules/testing.md`: testing and verification rules.
- `docs/development/rules/vcs.md`: jj topology and source-control rules.

Preserve unowned human or agent work. Report concrete validation evidence in
handoffs instead of confidence language.
