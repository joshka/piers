# Core And UI Boundary

Piers should keep the agent core independent from any one interface. Pi's
history shows pressure from line mode, TUI, JSON print mode, RPC, SDK, browser
clients, worker loops, subagents, and chat integrations. Those should not each
invent their own harness model.

The core should expose one command and event API. Interfaces are clients.

The current kernel exposes this boundary as `HarnessCommand` and
`HarnessEvent` in `src/harness.rs`. The non-TUI app layer in `src/app.rs`
parses REPL, print, and JSON-mode input into typed commands; it does not call
reload, tool, or session internals directly.

## Core Responsibilities

The core owns:

- session state machine
- provider stream normalization
- tool registry and policy
- reload lifecycle
- snapshot lifecycle
- compaction lifecycle
- diagnostics
- event persistence
- replay fixtures

The core should be usable without a terminal UI.

## Interface Types

Likely clients:

- line REPL
- JSON mode
- RPC server
- terminal UI
- browser UI
- worker-loop process
- subagent runner
- chat or Slack gateway

Each client should send host commands and consume host events. None should
write durable session state directly.

## Command API

Initial command categories:

- `user_input`
- `evolve`
- `reload`
- `status`
- `submit_user_message`
- `request_tool_approval`
- `abort_current_turn`
- `request_compaction`
- `propose_reload`
- `approve_reload`
- `switch_session`
- `export_session`
- `inspect_status`

Commands should return either accepted command IDs or structured rejection
diagnostics. Long-running work should emit events.

## Event API

Initial event categories:

- `turn_started`
- `user_message_recorded`
- `assistant_message`
- `tool_call_requested`
- `tool_result_recorded`
- `reload_started`
- `reload_promoted`
- `reload_failed`
- `turn_completed`
- session entry appended
- provider event normalized
- tool event emitted
- compaction state changed
- reload state changed
- guest snapshot captured or restored
- diagnostic emitted
- UI hint emitted

UI hints can exist, but they should not be the only record of important state.

## JSON And RPC

JSON mode and RPC are useful because they force the event model to be explicit.
Piers now has a small JSON print mode over `HarnessEvent` and a JSONL RPC mode
that reads command envelopes from stdin and writes event/result/error envelopes
to stdout.

Rules:

- every event has a schema version
- every event has a stable request ID and request-local sequence
- every event has provenance
- every error has a stable code
- every command exposed to tools has an input schema and mutation flags
- partial events are marked partial
- terminal events are explicit
- shutdown and retry states are visible

If JSON mode cannot represent a state, the core model is probably too tied to
the current UI.

## TUI

The TUI should derive from the event stream:

- render transcript from session entries
- render tool progress from tool events
- render provider progress from normalized stream events
- render reload progress from reload events
- render diagnostics from diagnostic events

The TUI can keep local presentation state such as scroll position, selected
entry, focus, and layout caches.

It should not be required for replay, tests, or headless use.

## Worker Loop

Worker-loop mode is useful for embedding:

1. receive command
1. process through core
1. emit events
1. wait for next command

This shape supports subagents and remote clients without making the harness a
terminal application first.

The current `--mode rpc` implementation is the first worker loop. It supports
`user_input`, `status`, `provider_status`, `manifest`, `list_sessions`,
`session_entries`, `switch_session`, `abort`, `reload`, `evolve`, and `quit`
requests over JSONL. Callers can pass `--session <path>` before or after the
mode flag to choose the initial append-only session log backing the worker,
then use `switch_session` to change sessions without restarting. Callers can
pass `--read-only` to reject source-mutating `reload` and `evolve` commands
before the harness runs them, or `--require-approval` to require
`approved: true` on each mutating RPC request.

## Browser Or GUI Clients

Browser and GUI clients add another constraint: the core API should not depend
on terminal libraries, process-global raw mode, or direct stdout rendering.

If the core has browser-safe or UI-neutral exports later, they should be thin
wrappers over the same command/event model used by the REPL.

## Testing The Boundary

Boundary tests should prove:

- a session can run without a TUI
- JSON mode receives the same events as the TUI
- replay uses the same reducer as live execution
- a UI crash does not corrupt the session log
- a worker process can resume from stored session state
- switching sessions does not leak UI-only state into core state

This lets Piers add richer interfaces without weakening the reload kernel.
