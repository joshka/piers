# Core And UI Boundary

Piers should keep the agent core independent from any one interface. Pi's
history shows pressure from line mode, TUI, JSON print mode, RPC, SDK, browser
clients, worker loops, subagents, and chat integrations. Those should not each
invent their own harness model.

The core should expose one command and event API. Interfaces are clients.

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

Rules:

- every event has a schema version
- every event has a stable ID
- every event has provenance
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
