# Related Project Notes

These notes compare Piers with nearby projects and repositories that inform
the design. The goal is not to clone any of them. It is to notice which
responsibilities keep reappearing in agent harnesses.

## Repositories Checked

### `earendil-works/pi`

This is the current Pi codebase and the best reference for operational
complexity. The local checkout at `../pi` exposes the mature TypeScript
monorepo shape: agent packages, provider packages, coding-agent packages,
terminal UI, extensions, tools, SDK/RPC surfaces, and tests.

What Piers should copy:

- explicit harness lifecycle state
- turn snapshots
- resource configuration
- fake-provider characterization tests
- session export/import pressure
- provider stream hardening

What Piers should avoid copying too early:

- a broad UI surface before the event core is stable
- extension mutability without a typed manifest boundary
- provider wire formats as durable session state

### `badlogic/pi-mono`

This resolves to the same active Pi lineage but is useful as the historical
pointer from the original Pi/OpenClaw discussion. It reinforces that Pi should
be read as a product harness, not just a coding-agent loop.

Piers lesson: preserve the small kernel idea. Product integration should live
outside the reload boundary until the state model is durable.

### `Dicklesworthstone/pi_agent_rust`

This appears to be the closest Rust-flavored Pi port or reimplementation found
during the related-repo pass. Its themes include startup behavior, memory,
extension safety, ledgers, host-call lanes, SQLite-backed state, and session
performance.

Piers lesson: a Rust port can still inherit the same harness surface area.
Rust helps with types and distribution, but it does not remove the need for
ledgers, safety boundaries, session indexes, and extension threat modeling.

### `dollspace-gay/OpenClaudia`

This is a Rust universal agent harness with a broad feature inventory: LSP,
web search, hooks, permissions, plan mode, subagents, MCP, cron, and worktree
support.

Piers lesson: it is easy for a Rust agent harness to become broad before the
kernel is sharp. The feature list is useful as a future checklist, but Piers
should earn each surface through the core command/event API.

### `Blushyes/coro-code`

This is a Rust CLI coding agent with a core/CLI split, rich TUI, and provider
roadmap. It looks closer to a focused Rust CLI shape than to a full Pi clone.

Piers lesson: a core/CLI split is the right instinct. Piers should take that
further by making REPL, JSON mode, TUI, and worker mode clients of the same
core API.

### `amrit110/oli`

This project uses a Rust backend with a React/Ink terminal frontend.

Piers lesson: a separate UI layer can be healthy, but only if the backend
event stream is authoritative. UI state should derive from harness events, not
become a second session model.

### `openclaw/openclaw` And `badlogic/openclaw`

OpenClaw is relevant as an embedding and integration target around Pi. The
active lineage points toward assistant gateway concerns: daemon mode,
multi-channel input, plugin SDKs, and sandboxing.

Piers lesson: remote inputs require provenance, permissions, channel identity,
and audit trails. These belong in the host-owned session and policy layers.

### `badlogic/pi-share-hf`

This repository is about collecting, redacting, and uploading Pi sessions.

Piers lesson: logs and sessions should be exportable, redaction-friendly, and
replayable from the beginning. Self-rewrite history should be part of that
record.

### `earendil-works/pi-chat`

This repository points at Slack/chat automation around Pi.

Piers lesson: chat integration turns session identity, channel provenance,
authorization, and background task handling into first-class concerns.

## Cross-Project Pattern

The same boundary keeps appearing:

- host owns durability, providers, auth, subprocesses, permissions, reload
  promotion, session logs, diagnostics, and platform facts
- reloadable code owns behavior, prompts, policies, planning, tool selection,
  and declarative extension manifests
- UI and remote clients observe and command the host through a stable event
  stream

That matches the current Piers PoC. The next step is not a bigger guest. The
next step is a stricter host contract around staged reload, session state, and
provider normalization.

## Positioning

Piers should not try to be "Pi rewritten in Rust" at this stage. A better
position is:

> A Rust-first self-modifying harness with a recoverable, auditable reload
> boundary.

That makes the project narrower and more defensible. Pi remains the reference
for product pressure. Rust/Wasm remains the experiment: can the reloadable
part stay in Rust while the host keeps the dangerous state stable?
