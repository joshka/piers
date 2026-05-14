# Host And Guest Boundary

This document turns the Pi history and related-project review into a concrete
ownership model for Piers.

The current PoC already has the right coarse shape:

- a stable native Rust host
- a reloadable Rust guest compiled to a WebAssembly component
- a small WIT contract between them
- explicit snapshot and restore

The next version should make that boundary stricter rather than larger.

## Host Responsibilities

The host owns things that must survive bad generated code, failed reloads,
provider drift, platform differences, and UI crashes.

Host-owned responsibilities:

- session log storage
- session state-machine validation
- turn snapshots
- reload staging, build, validation, promotion, and rollback
- provider adapters, auth, retries, and model capability metadata
- provider stream normalization
- tool registry installation and conflict resolution
- filesystem, subprocess, and network policy
- durable resource handles
- terminal and platform capability detection
- event log, diagnostics, and replay fixtures
- redaction and export

The host should treat guest output as a proposal. Generated source, tool
manifests, provider hooks, context rewrites, and state migrations all need host
validation before they become durable facts.

## Guest Responsibilities

The guest owns behavior that benefits from fast iteration and self-rewrite.

Guest-owned responsibilities:

- prompts and agent policies
- planning behavior
- tool selection strategy
- command behavior behind declared manifests
- extension-like declarations
- provider selection policy, not provider wire handling
- compaction policy hints, not transcript mutation
- proposed source patches
- guest-local ephemeral state
- versioned snapshot payloads

The guest should not hold raw host resources. It should receive handles or
capabilities that the host can revoke, migrate, or rebind after reload.

## Boundary Shape

The boundary should move from stringly exports toward versioned typed messages.
A future WIT surface might separate commands, events, and manifests:

```wit
record guest-manifest {
    version: string,
    commands: list<command-decl>,
    tools: list<tool-decl>,
    hooks: list<hook-decl>,
}

variant host-command {
    user-input(string),
    tool-result(tool-result-event),
    reload-accepted(reload-event),
    compact-request(compact-request),
}

variant guest-event {
    assistant-message(string),
    tool-call(tool-call-request),
    propose-patch(source-patch),
    propose-manifest(guest-manifest),
    diagnostics(diagnostic-event),
}
```

This does not need to be implemented immediately. The important direction is
that host and guest exchange typed events, not arbitrary provider messages or
host internals.

## Reload Contract

The guest may propose source changes. The host decides whether and when to
apply them.

Reload should eventually follow this lifecycle:

1. Guest emits a proposed patch or replacement source.
1. Host records the proposal as a session entry.
1. Host applies the proposal in a staging workspace.
1. Host builds the staged guest.
1. Host runs smoke checks and replay fixtures.
1. Host asks the old guest for a versioned snapshot.
1. Host instantiates the new guest.
1. Host restores the snapshot into the new guest.
1. Host validates the new manifest and guest health.
1. Host promotes the staged source and component.
1. Host records the accepted reload as a session entry.

Failed reloads should record structured diagnostics and leave the active guest
unchanged.

## Session Contract

The canonical session format belongs to the host. It should not be provider
wire messages and it should not be UI transcript text.

Session entries should represent harness facts:

- user input
- assistant output
- tool call requested
- tool call started
- tool output chunk
- tool call completed
- tool call failed
- compaction requested
- compaction completed
- reload proposed
- reload failed
- reload accepted
- guest snapshot captured
- guest snapshot restored
- abort requested
- retry scheduled

Every append should pass transition validation. Loading an old session should
either validate cleanly or produce a repair diagnostic.

## Provider Contract

Provider adapters are host-owned. The guest should not store provider-native
messages directly or replay them across providers.

The normalized provider stream should distinguish:

- message deltas
- reasoning deltas
- partial tool-call arguments
- completed tool calls
- usage and cache accounting
- retryable transport failures
- provider protocol failures
- clean stream termination
- incomplete stream termination

Provider quirks can still be exposed as diagnostics or capability metadata,
but durable session state should stay provider-neutral.

## Tool Contract

Tools are host-owned capabilities. The guest may request tool calls only
through declared, validated schemas.

Each tool call should have:

- a stable call ID
- a declared tool identity and source
- validated structured input
- permission policy
- execution mode
- cancellation behavior
- streamed output events
- terminal success, failure, or abort state

This keeps self-rewrite from turning into arbitrary host mutation.

## UI Contract

The UI is a client of host events. It is not the source of truth for session
state, tool state, reload state, or provider state.

This makes it possible to add:

- line REPL
- JSON mode
- RPC server
- terminal UI
- browser UI
- worker-loop mode
- subagent workers

without inventing a new harness model for each surface.
