# Pi History Timeline Notes

These notes look at Pi's issue, pull request, and commit history over time
rather than as a flat list. The source data was:

- 2,584 GitHub issues from `earendil-works/pi`
- 1,709 GitHub pull requests from `earendil-works/pi`
- 4,076 commits from the local `../pi` checkout

The issue and pull request exports span November 2025 through May 2026. The
commit history starts in August 2025, so the first implementation phases are
visible before the issue tracker becomes active.

## Phase 1: Repository And Provider Foundations

Approximate period: August to October 2025.

Commit history starts with monorepo setup, npm packaging, provider plumbing,
and early agent/tool shape. The early work is mostly constructive rather than
reactive: establish package boundaries, AI provider abstractions, CLI
installation, and the first agent loop.

Representative commits:

- `a74c5da1`: initial monorepo setup with npm workspaces
- `d304f377`: fix Pi agent CLI execution when installed globally
- `004de3c9`: add streaming generate API with `AsyncIterable`
- `39c626b6`: add partial JSON parsing for streaming tool calls
- `e2f29b05`: add `defineTool` helper

Piers lesson: get the core types right early. The first version should have
explicit Rust types for provider events, tool calls, tool results, session
entries, and reload events. Retrofitting these after a UI and extension
surface exist is much harder.

## Phase 2: First User-Facing Harness Pressure

Approximate period: November to December 2025.

Issues begin around terminal behavior, model/provider configuration, aborted
state, session export, tool output verbosity, MCP, read-only tools, compaction,
and RPC mode. The product is no longer just an agent loop; it is becoming a
coding harness that must persist sessions, expose history, handle queues, and
fit in terminals.

Representative issues:

- #12: context usage is wrong after abort
- #17: branch, commit, and stash in agent sessions
- #31: expand truncated tool output
- #44: tool result streaming
- #67: MCP support
- #74: read-only exploration tools and restricted subagents
- #83: session management breaks in RPC mode
- #92: context compaction for long sessions
- #115: rework context management and overflow handling
- #179: queued messages get lost on compaction
- #198: cross-model handoff fails from tool-call ID incompatibilities

Representative pull requests:

- #1: Windows Git Bash support for the bash tool
- #5: OpenRouter Auto Router model support
- #23: `--thinking` CLI option
- #79: watch `.git/HEAD` while waiting for user input
- #90: per-package CI checks
- #171: skills system
- #248: custom tools wrapped with hooks

Piers lesson: session, queue, compaction, and provider compatibility are not
advanced features. They show up as soon as a tool is used for real work. Piers
should make turn snapshots and append-only session entries foundational.

## Phase 3: Extensions Become The Product Boundary

Approximate period: January to February 2026.

The issue stream shifts toward extensions, custom tools, server/RPC access,
dynamic model control, installable plugins, UI customizations, and provider
quirks. The key change is that users want to change the harness while it is
running.

Representative issues:

- #454: merge hooks and custom tools into unified extensions
- #460: server mode with WebSocket session access
- #474: abort-signal support for extension UI confirmations
- #475: queue messages during compaction
- #481: extension footer components
- #483: `pi.sendUserMessage`
- #509: model and thinking-level APIs for extensions
- #516: install extensions from Claude Code plugins
- #520: single-file config for a session

Representative pull requests:

- #431: event bus for agent and extension events
- #451: OpenAI Codex OAuth and Responses provider support
- #512: `AbortSignal` support for extension UI confirmations
- #513: async extension factory functions
- #737: OpenAI Codex compatibility work
- #785: session ID resolution and fork support
- #1191: `switchSession` extension API

Piers lesson: a plugin boundary quickly turns into a runtime boundary. Piers
should not expose raw host mutation to reloadable code. The Rust/Wasm guest
should declare tools, commands, hooks, and resources through a manifest that
the host validates and installs.

## Phase 4: Scale, Protocol Drift, And State Corruption

Approximate period: March 2026.

March is where the issue volume jumps and the themes get sharper:
cross-provider cache behavior, session corruption, dynamic tool registration,
subprocess sandbox leaks, provider-native tools, compaction ordering, and
unpaired tool messages.

Representative issues:

- #1717: session corruption from `tool_result` before assistant message
- #1720: dynamic tool registration after session initialization
- #1725: expose provider rate-limit headers
- #1740: provider-native server-tool abstraction
- #1744: sandbox domain allowlist not enforced for subprocess fetches
- #1748: serializing conversation before compaction is a bad idea
- #1763: auto-compaction never fires on retryable provider errors
- #1764: compaction creates unpaired tool messages
- #1766: theme hot reloading fails
- #1767: migrate Mistral to native SDK

Representative pull requests:

- #1909 and #1912: harden RPC JSONL framing
- #1995: remove per-keypress render cost proportional to session size
- #2130: custom session IDs in `newSession`
- #2356: JSONL export/import for sessions
- #2465: reuse current Pi invocation for child agents
- #2703: avoid replaying reasoning before tool-call turns

Piers lesson: the session log must be a validated state machine. Tool calls
and tool results need pairing invariants. Compaction must preserve those
invariants. Provider streams need terminal states that distinguish success,
retryable failure, abort, and incomplete stream.

## Phase 5: Extension Runtime And Provider Matrix Expansion

Approximate period: April 2026.

April shows a mature harness under ecosystem pressure: OpenAI Responses,
Anthropic OAuth, custom autocomplete providers, extension-rendered tool
results, skill collision precedence, stale settings during reload, memory
retention, CRLF edits, and terminal compositor drift.

Representative issues:

- #2743: Anthropic OAuth refresh encoding mismatch
- #2744: edit tool on CRLF files
- #2745: streaming tool-call args missing with OpenAI Responses
- #2746: type-safe tool definition helper
- #2750: callback throw hangs stream permanently
- #2752: session manager grows unbounded
- #2753: `/reload` uses stale settings
- #2757: custom `@` autocomplete providers
- #2773: extension tool-result rendering
- #2781: skill collision precedence
- #2783: composed line exceeds terminal width

Representative pull requests:

- #3024: settle all parallel tool executions
- #3075: handle overlapping compactions
- #3345: per-tool execution mode override
- #3412: strip JSON Schema metadata keys before provider calls
- #3474: migrate schema definitions to TypeBox v1
- #3650: omit `tools` instead of sending an empty array

Piers lesson: every extension-facing API needs lifecycle semantics. Callback
errors must terminate cleanly. Reload must invalidate stale contexts and use
fresh resource state. File editing should be conservative across line endings,
Unicode, and path encodings from the start.

## Phase 6: Refactor-Era Lessons

Approximate period: May 2026.

Recent issues and commits show an active refactor phase. The work is about
making implicit behavior explicit: harness stream configuration, resource
configuration, turn snapshots, provider hooks, SDK examples, terminal restore,
and package/update reliability.

Representative issues:

- #4038: guaranteed post-payload, pre-SDK-call hook
- #4045: compaction streaming
- #4046: compaction deletes too much
- #4054: new session blocked by previous response
- #4056: invalid provider drops all custom providers
- #4071: Gemini-to-Anthropic replay fails after tool use
- #4075: model reasoning map ignored
- #4076: managed binary install mode
- #4276: abort during confirmation duplicates output
- #4493: bash tool can trigger unescapable interactive UIs

Representative commits:

- `322759a3`: snapshot harness turn state
- `79db9d62`: make harness resources explicit
- `e25415dd`: finalize harness resource config
- `c0f416aa`: add harness stream configuration
- `f348a062`: cover harness stream configuration
- `3d9e14d7`: clamp compaction summary output tokens
- `9d84e286`: restore terminal on uncaught exception

Representative pull requests:

- #4165: stream bash output incrementally
- #4259: complete rollback architecture with a large test suite
- #4329: worker-loop mode for bus-driven task dispatch
- #4388: split browser-safe core entry from harness exports
- #4453: remove unused dependencies
- #4467 and #4468: remove or replace small dependencies

Piers lesson: the refactor direction is a signal. The stable architecture is a
small kernel with explicit snapshots, resources, stream options, diagnostics,
and lifecycle phases. Piers should begin there instead of rediscovering it
after building the UI.

## Cross-Time Pattern

Across the full history, the same shape repeats:

1. A useful abstraction starts informal.
1. Real sessions expose ordering and lifecycle edge cases.
1. Provider quirks force the abstraction to become typed.
1. Extensions make mutation and reload semantics visible.
1. UI and platform differences turn hidden assumptions into bugs.
1. Refactors move state back into explicit snapshots and events.

For Piers, this suggests a kernel-first roadmap:

1. Define typed event streams for providers, tools, sessions, and reloads.
1. Store append-only, validated session entries.
1. Use turn snapshots for every model request.
1. Make compaction and reload first-class state transitions.
1. Keep provider compatibility and auth in the host.
1. Let Wasm guests declare behavior; do not let them mutate host state
   directly.
1. Add terminal UI only after the event and lifecycle contracts are stable.
