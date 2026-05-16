# Pi-Shaped Kernel Audit

This note records the deeper pass from the Pi-derived docs into the current
Piers kernel. It is a review artifact: the goal is to show which Pi lessons are
now represented in code and which remain deliberate follow-up work.

## Docs Read

This pass covered every Markdown document under `docs/`, including:

- the Pi research notes: `pi-research-process.md`, `pi-harness-notes.md`,
  `pi-lessons.md`, `pi-history-timeline.md`, and `pi-pr-history.md`
- the kernel design notes: host/guest boundary, reload, snapshots, sessions,
  provider streams, deterministic replay, rollback, tools, extensions,
  compaction, core/UI, platform risk, and future notes
- the copied development guidance under `docs/development/`

## Pi Lessons Applied

Pi's recurring failure modes point to a harness core rather than a command
demo. The current kernel now represents these lessons directly:

- Core command/event boundary: `HarnessCommand`, `CommandOutcome`, and
  `HarnessEvent` let REPL, RPC, TUI, JSON, and worker clients share one model.
- Lifecycle state: `HarnessPhase` distinguishes idle, turn, tool execution, and
  reload phases so structural operations require safe points.
- Turn snapshots: `TurnSnapshot` captures host-owned facts before user input
  reaches guest behavior.
- Durable turns: `turn_started` and `turn_completed` entries make save points
  replayable.
- Session validation: the reducer rejects nested turns, orphan tool results,
  tool calls outside turns, turns completing with pending tools, stale reload
  snapshots, and reload promotion without a matching proposal.
- Host-owned tools: `read`, `write`, `edit`, and policy-gated `exec` stay in
  the host and are exposed only through typed guest requests.
- Capability-gated tools: guest tool requests must match `tool:<name>`
  capabilities declared by the active manifest before the host executes them.
- Guest declarations: the WIT manifest now carries capabilities and commands;
  the host validates declarations before accepting a guest generation.
- Transactional reload: generated source is staged, built, restored,
  smoke-tested, and only then promoted.
- Provider state: provider streams reduce through a host-owned state machine
  with explicit terminal states.
- Provider loop: deterministic provider scripts and configured
  OpenAI-compatible prompts now run through the same tool-result loop.
- Background provider jobs: RPC clients can start provider work, poll
  completion, and request cooperative cancellation by `job_id`.
- App/runtime separation: `src/main.rs` handles only process setup, while
  `src/app.rs` owns non-TUI modes and JSONL RPC over the shared harness
  command/event API.
- Read-only worker policy: the app layer can reject reload and evolve commands
  before they mutate source or session state.
- Approval policy: RPC reload and evolve requests can require an explicit
  `approved: true` field before the harness runs them.
- Stable RPC errors: error envelopes include machine-readable codes instead of
  requiring callers to parse display text.
- Session discovery: RPC and REPL clients can list known session JSONL files
  with paths, entry counts, and modification timestamps.
- Transcript access: RPC and REPL clients can fetch parsed entries from the
  active session log without reading `.piers` internals directly.
- Session switching: RPC clients can move a running worker to another session
  file without restarting the process.
- Abort contract: RPC and REPL clients have an explicit abort command and event
  shape, even though current work is still synchronous.
- Provider visibility: RPC and REPL clients can ask whether this worker is a
  model-backed agent or the current guest harness.
- Tool-ready manifest: command declarations include JSON input schemas,
  mutation flags, and approval-field support.
- Strict RPC decoding: requests reject unknown fields to match advertised
  schemas.

## Evidence

The relevant code surfaces are:

- `src/harness.rs`: core command/event API, lifecycle phases, turn snapshots,
  guest registry validation, reload promotion, and Wasm guest routing
- `src/session.rs`: append-only JSONL entries and replay validation
- `src/tools.rs`: host-owned coding tools and policy checks
- `src/provider.rs`: provider stream reducer
- `src/staging.rs`: stale artifact detection and staged guest builds
- `wit/piers.wit`: typed host/guest protocol and manifest declarations
- `guest/src/lib.rs`: reloadable guest behavior behind the WIT boundary
- `src/app.rs`: Pi-like non-TUI app modes over the core command API
- `src/main.rs`: thin process entry point

The regression suite covers the new Pi-shaped boundaries with tests for:

- failed generated source preserving the active guest and live source
- typed tool calls round-tripping through host policy and back to the guest
- undeclared guest tool capabilities being rejected before host execution
- command events carrying turn snapshots and tool lifecycle events
- guest manifest declarations being host-validated
- reload acceptance notifying the promoted guest
- CLI parsing and REPL event rendering for the non-TUI app layer
- RPC request mapping and event/result envelope rendering
- read-only command policy for external worker mode
- approval-gated reload/evolve RPC requests
- RPC error codes for decode, command, policy, approval, and harness failures
- session listing over the same command/result path
- active session entry retrieval with optional limits
- in-process RPC session switching
- abort command and `abort_requested` event shape
- provider status command for mode/model visibility
- manifest command schemas and mutation metadata
- strict request decoding for schema parity
- session reducer rejection of invalid turn, tool, and reload transitions
- provider reducer rejection of malformed stream ordering and terminal states
- cooperative cancellation token handling in provider loops

## Remaining Follow-Up

This is still a small kernel, not a mature Pi clone. The next meaningful
follow-ups are:

- promotion journals and startup repair for interrupted reloads
- replay fixtures that build staged guests against recorded sessions
- richer guest manifest declarations for tools, hooks, resources, and
  provenance
- compaction entries and safe cut-point enforcement
- persisted job state and startup repair for interrupted background work
- streaming provider transport and stronger cancellation around blocking calls
- provider model capability metadata behind the provider reducer
