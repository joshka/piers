# Pi PR History Notes

These notes look at Pi's pull request history as a separate signal from issues
and commits. Issues show pain. Commits show accepted changes. Pull requests
also show abandoned, deferred, or heavily revised architecture.

The export covered 1,709 pull requests from `earendil-works/pi`, dated
November 13, 2025 through May 13, 2026:

- 544 merged
- 1,161 closed without merge
- 4 open

The high closed-to-merged ratio matters. For Piers, the failed or deferred pull
requests are often the best clues about where a harness abstraction was
resisting change.

## Phase Notes

### Bootstrap Agent CLI

Approximate period: November 2025.

The earliest pull requests focused on shell portability, model selection,
thinking controls, Unicode, and watching repository state.

Representative pull requests:

- #1: Windows Git Bash support for the bash tool
- #5: OpenRouter Auto Router model support
- #23: `--thinking` CLI option
- #79: watch `.git/HEAD` while waiting for user input

Piers lesson: even a minimal coding harness immediately becomes responsible
for shell choice, repository state, model options, and terminal text behavior.
Treat those as host responsibilities, not guest conveniences.

### Agent Surface Forms

Approximate period: December 2025.

The surface expanded into CI, skills, hooks, image rendering, markdown
correctness, custom tools, and shell behavior.

Representative pull requests:

- #90: per-package CI checks
- #171: skills system
- #177: TUI inline images
- #206: markdown table rendering fixes
- #248: custom tools wrapped with hooks
- #328: use `bash` instead of `sh`

Piers lesson: "just add hooks" quickly becomes a runtime API. Hooks need
ordering, provenance, cancellation, permissions, and observability.

### Extensibility Explosion

Approximate period: January 2026.

January had the highest pull request volume in the export. The repeated themes
were event buses, extension APIs, provider compatibility, model routing,
session picking, and TUI input/rendering behavior.

Representative pull requests:

- #431: event bus for agent and extension events
- #451: OpenAI Codex OAuth and Responses provider support
- #512: `AbortSignal` support for extension UI confirmations
- #513: async extension factory functions
- #737: OpenAI Codex compatibility work
- #785: session ID resolution and fork support

Piers lesson: the extension boundary should be declarative before it is
dynamic. Let guests propose manifests and behavior. Let the host validate,
install, revoke, and persist those registrations.

### Productization And Headless Use

Approximate period: February 2026.

The harness was pushed beyond a local TUI into RPC, SDK-like operations,
subagents, config hardening, Windows, Termux, and extension lifecycle fixes.

Representative pull requests:

- #1191: `switchSession` extension API
- #1196: filter commands that conflict with built-ins
- #1230: better bash detection
- #1375: forward message and tool events to extensions
- #1495: enable VT input mode on Windows
- #1603: restore Termux support

Piers lesson: if the core is useful, it will be embedded. Keep the harness
core independent from any one UI, terminal model, or process-launch shape.

### Replay And Session Hardening

Approximate period: March 2026.

March exposed replay, framing, session identity, cached-token accounting,
subagent invocation, and render performance as core harness problems.

Representative pull requests:

- #1909 and #1912: harden RPC JSONL framing
- #1995: remove per-keypress render cost proportional to session size
- #2130: custom session IDs in `newSession`
- #2356: JSONL export/import for sessions
- #2465: reuse current Pi invocation for child agents
- #2703: avoid replaying reasoning before tool-call turns

Piers lesson: replay is not a debugging feature. It is the mechanism that makes
self-modification auditable and recoverable.

### Architecture Stress And Provider Churn

Approximate period: April 2026.

The April pull requests show compaction races, schema sanitization, provider
proliferation, model-registry pressure, and tool execution policy.

Representative pull requests:

- #3024: settle all parallel tool executions
- #3075: handle overlapping compactions
- #3345: per-tool execution mode override
- #3412: strip JSON Schema metadata keys before provider calls
- #3474: migrate schema definitions to TypeBox v1
- #3650: omit `tools` instead of sending an empty array

Piers lesson: provider-neutral does not mean provider-naive. The host needs a
typed compatibility layer with explicit lossy conversions and golden fixtures.

### Late Stabilization And Dependency Slimming

Approximate period: May 2026.

The late pull requests show architectural stabilization, worker-mode
experiments, browser-safe boundaries, terminal robustness, dependency removal,
and platform packaging.

Representative pull requests:

- #4165: stream bash output incrementally
- #4259: complete rollback architecture with a large test suite
- #4329: worker-loop mode for bus-driven task dispatch
- #4388: split browser-safe core entry from harness exports
- #4453: remove unused dependencies
- #4467 and #4468: remove or replace small dependencies

Piers lesson: browser-safe core exports and worker-loop dispatch both point to
the same design pressure: the agent core must be smaller and less coupled than
the product shell around it.

## Design Paths To Revisit

Some pull requests are useful because they name design directions Piers should
consider directly, regardless of how those lines ended in Pi:

- #135: checkpointing
- #383: context envelopes, patch operations, and deterministic replay
- #4259: rollback architecture
- #4329: worker-loop dispatch
- #4388: browser-safe core split

These are not polish features. They are the skeleton of a robust self-rewriting
harness:

1. A proposed change becomes a patch operation.
1. The host applies it in a staged workspace.
1. The staged guest builds and runs against replay fixtures.
1. The host checkpoints the current live session.
1. The new guest is promoted only after restore and smoke checks pass.
1. Rollback remains a normal state transition, not an emergency path.

## Recurring Clusters

### Provider And Model Compatibility

Representative pull requests:

- #451: OpenAI Codex OAuth and Responses provider support
- #536: align OpenAI Codex models with expected metadata
- #654: avoid cross-provider thought signatures
- #727: Bedrock thought signature support
- #917: handle call-arguments-done stream events
- #3650: omit empty `tools` field
- #4256: multi-turn reasoning with `store: false` on Azure

Piers implication: host-owned provider adapters need to normalize tool calls,
reasoning blocks, stream termination, retries, cache semantics, and model
capabilities before anything enters durable session state.

### Terminal Correctness

Representative pull requests:

- #177: TUI inline images
- #225: Kitty keyboard protocol
- #382: editor word wrapping
- #718: shortcuts on non-Latin keyboard layouts
- #924: word wrap rewrite
- #1495: VT input mode on Windows
- #2082: wide characters at wrap boundaries
- #4347: CJK text extraction

Piers implication: keep the first Piers UI line-oriented. When a TUI arrives,
test it as a protocol implementation, not as a cosmetic layer.

### Sessions, Replay, Compaction, And Context

Representative pull requests:

- #94: auto-compaction
- #314: structured compaction
- #383: context envelopes and deterministic replay
- #386: save initial model and thinking level
- #785: session ID resolution and fork support
- #2356: JSONL export/import
- #2703: avoid replaying reasoning before tool-call turns
- #4202: reject re-entry into session compaction

Piers implication: append-only session entries and legal transition checks
should exist before rich tool calling, compaction, or self-rewrite.

### Extensions, Hooks, And APIs

Representative pull requests:

- #171: skills system
- #219: symlinked tools and hooks
- #248: custom tools wrapped with hooks
- #431: event bus
- #513: async extension factories
- #600: footer data providers
- #1375: message and tool events forwarded to extensions
- #3099: inline extension factories in `main`

Piers implication: guest-provided behavior should be capability-scoped and
manifest-driven. Dynamic convenience should not define the host's internal
model.

### Harness, RPC, Workers, And Subagents

Representative pull requests:

- #215: subagent orchestration examples
- #679: session header in JSON print mode
- #995: `get_commands` RPC
- #1522: session mutation and enriched RPC commands
- #1909: RPC JSONL framing
- #2465: reuse current Pi invocation for child agents
- #4329: worker-loop task dispatch
- #4388: browser-safe core exports

Piers implication: design one core command/event API. Let REPL, JSON mode, RPC,
TUI, browser, and subagent workers be clients of that API.

### Cross-Platform Shell, Build, And Dependency Drag

Representative pull requests:

- #1: Windows Git Bash support
- #328: use `bash` instead of `sh`
- #360: CRLF edit-tool failure on Windows
- #922: Bun compatibility
- #1603: Termux support
- #4013: Windows `pwsh.exe` shell path
- #4458: Windows ARM64 binaries
- #4467 and #4468: remove or replace small dependencies

Piers implication: Rust's single-binary story helps, but tool execution and
guest compilation still cross platform boundaries. Put platform facts in the
host and surface them as diagnostics.

## Piers Defaults From PR History

- Make replay/session state the core product.
- Keep provider compatibility behind typed host adapters.
- Separate core, UI, terminal, browser, extension, and worker surfaces early.
- Model self-rewrite as staged patch, build, replay, promote, rollback.
- Prefer schema-stable Rust boundaries, then generate adapters outward.
- Build hostile replay fixtures for provider, tool, session, terminal, and
  path edge cases.
