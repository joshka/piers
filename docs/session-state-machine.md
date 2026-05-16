# Session State Machine

Pi's history shows that corrupted transcripts, orphaned tool results,
compaction regressions, abort races, and retry ordering bugs appear quickly in
real use. Piers should treat session history as a validated state machine, not
as a list of chat messages.

## Core Principle

The session log is authoritative. UI rendering, provider replay, compaction,
reload, and debugging should all be derived from append-only session entries.

The host owns the session log. Reloadable guests can request entries through
host APIs, but they do not write the log directly.

The current kernel writes append-only JSONL to `.piers/sessions/default.jsonl`
by default. Non-TUI modes accept `--session <path>` so a caller can isolate a
worker, test run, or tool invocation in its own session file.
Each entry has a monotonic ID, parent ID, timestamp, schema version, and typed
entry payload.

## Phases

Initial phases:

- `idle`
- `turn`
- `tool_execution`
- `compaction`
- `reload`
- `retry`
- `abort`

Structural mutations require `idle` unless a specific queued-write rule says
otherwise. Examples: tree navigation, compaction, reload promotion, and session
replacement.

## Entry Types

Minimum entries for a coding harness:

- `user_message`
- `assistant_message`
- `tool_call`
- `tool_result`
- `model_change`
- `thinking_level_change`
- `compaction`
- `branch_summary`
- `reload_proposed`
- `reload_failed`
- `reload_promoted`
- `guest_snapshot`
- `custom`

Implemented entries in the current kernel:

- `turn_started`
- `turn_completed`
- `user_message`
- `assistant_message`
- `tool_call`
- `tool_result`
- `reload_proposed`
- `reload_failed`
- `reload_promoted`
- `guest_snapshot`
- `diagnostic`

Each entry should have:

- stable id
- parent id
- timestamp
- source/provenance when applicable
- schema version

## Invariants

The reducer should reject or quarantine invalid transitions:

- no `tool_result` without a known pending `tool_call`
- no completed assistant turn with unpaired required tool calls
- no compaction that separates a tool call from its result
- no reload promotion without a validated build and restore
- no provider replay that violates the target provider transcript grammar
- no branch switch that loses the previous active leaf

Implemented reducer checks:

- entry IDs must be monotonic and parent-linked
- schema versions must be supported
- turns must not nest
- turn completion must match the active turn
- tool calls must be recorded inside an active turn
- turns cannot complete while tool calls remain pending
- tool results require a known pending tool call
- duplicate pending tool call IDs are rejected
- reload proposals cannot be recorded during an active turn
- reload promotion requires a matching pending reload proposal
- reload promotion requires a matching proposal and a guest snapshot recorded
  after that proposal
- guest snapshot format version must be nonzero

Invalid history loaded from disk should not be silently replayed. The loader
should produce diagnostics and a repair plan.

## Save Points

A save point occurs after an assistant response and its tool results are fully
recorded. At a save point, the host can:

- flush queued writes
- refresh resources
- compact context
- apply model or stream configuration changes
- reload guest behavior
- prepare the next turn snapshot

This mirrors the direction Pi moved toward: live config and per-turn snapshots
are distinct.

## Repair

Repair should be explicit. Examples:

- insert synthetic error tool results for orphaned tool calls
- mark a malformed assistant message unreplayable
- split a partial turn into an aborted turn
- quarantine entries produced by a failed guest version

Every repair should itself be a session entry so the history remains auditable.
