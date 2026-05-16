# Reload Lifecycle

The current kernel proves a transactional reload loop: generated code is staged,
built, smoke-tested, and only then promoted over `guest/src/lib.rs`.

The next design should keep making reload more auditable and typed.

## Current Flow

The current `:evolve` flow is:

1. Ask the running guest to generate replacement Rust source.
1. Record a `reload_proposed` session entry with the spec and source digest.
1. Write that source into `.piers/staged`.
1. Run `cargo build -p piers-guest --target wasm32-wasip2` in the staging
   workspace.
1. Capture and record a versioned guest snapshot.
1. Instantiate the staged component and restore the old guest snapshot into it.
1. Run a smoke `handle-event` call against the staged component.
1. Promote the staged source and artifact.
1. Swap the new guest into the host.
1. Record `reload_promoted` with the source digest and artifact path.

If staging, build, manifest validation, restore, smoke testing, or promotion
fails, the old guest keeps running and the host records `reload_failed` with
the failed stage and diagnostic message.

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

The current implementation stores the first slice of this model in
`.piers/sessions/default.jsonl`:

- `reload_proposed`
- `guest_snapshot`
- `reload_failed`
- `reload_promoted`

The log uses monotonic entry IDs, parent links, timestamps, and schema version
`1`. The reducer rejects a reload promotion unless a matching proposal and
nonzero-version guest snapshot have already been recorded.

The host also calls `manifest()` during load and reload acceptance. The current
manifest validator requires schema version `1` and a nonempty guest name.

Ordinary user input now reaches the guest as a typed `host-event.user-input`.
The guest returns typed `guest-event` values, and the host records assistant
messages, diagnostics, tool-call requests, and source-update proposals as
session entries.
