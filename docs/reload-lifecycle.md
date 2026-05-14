# Reload Lifecycle

The current PoC proves the reload loop, but it writes generated code directly
to `guest/src/lib.rs`. That is acceptable for the first demonstration and too
fragile for the next version.

The next design should make reload transactional.

## Current Flow

The current `:evolve` flow is:

1. Ask the running guest to generate replacement Rust source.
1. Write that source to `guest/src/lib.rs`.
1. Run `cargo build -p piers-guest --target wasm32-wasip2`.
1. Instantiate the new component.
1. Restore the old guest snapshot into the new guest.
1. Swap the new guest into the host.

If the build fails, the running guest can still handle requests, but the source
tree can be left broken. That is the wrong failure mode for a self-rewriting
harness.

## Transactional Flow

The hardened reload flow should be:

1. Record a `reload_proposed` session entry with the spec and source digest.
1. Write generated source into a staging directory.
1. Build the staged guest into a staged artifact.
1. Validate WIT exports and guest manifest.
1. Snapshot the current guest.
1. Instantiate the staged component.
1. Restore the snapshot into the staged component.
1. Run reload acceptance checks.
1. Promote staged source and artifact atomically.
1. Swap the current guest handle.
1. Record `reload_promoted` with diagnostics and artifact digest.

If any step fails, record `reload_failed`, keep the old guest active, and keep
the failed staged source for inspection.

## Promotion Rules

Promotion should be the only step that mutates the live guest source path. A
failed build, failed manifest validation, or failed restore must not corrupt
the live source.

The host owns promotion. The guest can propose source, but it cannot directly
decide that a generated artifact is safe to run.

## Acceptance Checks

The first acceptance checks can be simple:

- the component exports the expected WIT world
- `manifest()` parses and validates
- `snapshot()` and `restore()` round-trip without error
- a smoke `handle-event` call returns a well-formed response

Later checks should replay known failure fixtures from Pi's history:

- orphaned tool calls
- incomplete provider streams
- malformed tool arguments
- oversized tool outputs
- CRLF edits
- Unicode paths
- interrupted tool execution
- compaction boundaries

## Session Entries

Reload should be visible in the session log. Minimum entries:

- `reload_proposed`: generated source digest, proposer, prompt/spec
- `reload_build_started`: staged path and command
- `reload_build_failed`: stderr, exit code, source digest
- `reload_validated`: WIT and manifest validation details
- `reload_restore_failed`: snapshot version and restore error
- `reload_promoted`: artifact digest and previous artifact digest

This gives future Piers enough history to answer why a self-change happened
and how it was accepted.
