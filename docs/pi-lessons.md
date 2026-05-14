# Lessons From Pi's Issue, PR, And Commit History

These notes mine Pi's recent issue titles and commit subjects for lessons that
matter to Piers. The goal is not to copy Pi feature-for-feature. It is to
notice where a coding agent harness accumulates real complexity over time.

The first pass used a recent sample; the later pass expanded to the full
available issue, pull request, and commit exports:

- 2,584 issues from `earendil-works/pi`
- 1,709 pull requests from `earendil-works/pi`
- 4,076 commits from the local `../pi` checkout

Subagents mined the issues, pull requests, commits, and related repositories
independently, then these notes were merged from those findings.

For the time-based view, see [Pi History Timeline Notes](pi-history-timeline.md).
For pull request-specific lessons, see [Pi PR History Notes](pi-pr-history.md).
For nearby Rust and Pi-lineage projects, see
[Related Project Notes](related-projects.md).

## Terminal Behavior Is Product Surface

Representative issues:

- #4487: Backspace unresponsive in Windows Terminal
- #4450: Suppress tmux extended-keys warning
- #4440: Debugging tools for TUI rendering
- #4437: Kitty keyboard protocol breaks IME dead-key composition
- #4435: Extension selector breaks on long option text
- #4415: Image rendering offset in narrow viewports
- #4406: Newline binding missing on GNOME Terminal
- #4373: Undo command-backspace
- #4270: Background color bleeds beyond tool result blocks

Representative commits:

- `9d84e286`: restore terminal on uncaught exception
- `863341fb`: render checkboxes in to-do list items
- `116bffeb`: wrap list items with indent
- `801db80b`: bound kitty image id parsing
- `b8712457`: keep kitty image redraws inside TUI
- `c806dea1`: preserve OSC 8 hyperlink terminators

Lesson for Piers: terminal handling is not a thin UI concern. It is part of
the harness reliability story. If Piers grows a TUI, input protocols, focus,
IME composition, terminal restore, image rendering, scrollback stability, and
layout constraints should be treated as core invariants with tests or captured
terminal fixtures.

For the Rust version, this argues for keeping the initial harness line-oriented
until the runtime model is solid, then using a terminal layer with explicit
protocol knowledge rather than assuming crossterm-level abstractions are
enough.

## Provider Protocols Drift Constantly

Representative issues:

- #4485: `GITHUB_TOKEN` incorrectly triggers Copilot availability
- #4474: OpenRouter reasoning setting breaks tools
- #4464: Anthropic-compatible replay sends unsigned thinking blocks as content
- #4462: surrogate sanitization breaks thought signatures on replay
- #4454: support Anthropic base URL and auth token env vars
- #4439: Harmony response format corrupts tool calls
- #4433: retry missing Anthropic `message_stop`
- #4266: local OpenAI-compatible servers reject non-string `tool_choice`
- #4249: thinking levels do not work with OpenAI GPT-5.5
- #4210: Bedrock empty `end_turn` treated as successful stop

Representative commits:

- `5ac874c8`: retry Anthropic message_stop stream endings
- `83592bb2`: detect incomplete Anthropic streams
- `4b926a30`: own Anthropic SSE parsing
- `46b5800d`: cross-provider message handoff
- `2cfd8ff3`: use API type for message compatibility checks
- `99dc6fce`: session affinity and provider-cache fixes for Fireworks
- `8c2e3edd`: respect proxy envs in Bun websocket
- `783e96a1`: disable OpenAI reasoning where supported
- `9eb126e7`: document interleaved stream events
- `31f5c232`: handle OpenAI Responses reasoning text deltas
- `6b271842`: handle mixed chat completion deltas

Lesson for Piers: providers should sit behind a narrow event protocol owned by
the harness. The harness should not let provider-specific concepts leak into
session storage without normalization. Thinking blocks, tool-call IDs, cache
keys, retry behavior, transport options, and model capabilities need explicit
compatibility policy.

The provider-neutral stream should be a typed state machine with strict
ordering guarantees and terminal states. Partial tool-call arguments can be
observable as non-authoritative structured data, but validation should happen
only when the provider says the tool call is complete.

For a Rust/Wasm self-rewriting harness, provider code should stay host-owned at
first. Reloadable guests can choose policies or hooks, but the host should own
wire compatibility, auth, retries, and replay safety.

## Extensions Become A Runtime

Representative issues:

- #4500: dynamic OpenAPI to Pi tools and tool-selection challenges
- #4491: post-render session replacement hook for extension commands
- #4469: detect subagent subprocess so session-start hooks can self-skip
- #4465: sub-extension `node_modules` removed but not reinstalled
- #4457: allow extension to filter models
- #4451: skip skill-name validation for non-user-invocable directories
- #4443: install GitHub repo at tagged commit
- #4239: expose loaded resource metadata to extensions
- #4216: allow extensions to append branch-local context rewrites
- #4214: add extension actions separate from tools and slash commands
- #4207: typed cross-extension service calls
- #3553: extension tools silently override built-in tools

Representative commits:

- `d62c416c`: allow tool expansion during extension confirms
- `e25415dd`: finalize harness resource config
- `79db9d62`: make harness resources explicit
- `e1647aaa`: make resource invocation explicit
- `3421726e`: disambiguate resource paths
- `dacb7eaa`: detect renamed npm self updates
- `5e1e4c3c`: support renamed self-update package
- `88619669`: strip skill wrapper XML from HTML export user messages

Lesson for Piers: an extension system is not just "load a plugin." It becomes
a runtime with discovery, dependency management, resource provenance, stale
context invalidation, permissions, package updates, and model-visible
registration rules.

For Piers, the Wasm guest should not directly mutate host registries. It should
return a manifest of proposed tools, commands, hooks, and resources. The host
should validate and install that manifest, and every registration should have
source metadata that can be shown to the user and persisted in the session.

Conflict resolution should be explicit. A guest-provided tool should not be
able to silently shadow a built-in tool, and extensions should have stable
identity, declared capabilities, and stale-context invalidation after reload.

## Session Lifecycle Needs Save Points

Representative issues:

- #4189: corrupted session from orphaned tool use without tool result
- #3903: empty tool-call id and name poison session history
- #4497: auto-compaction never triggers for local models
- #4484: compaction bypasses custom stream function
- #4481: auto-compact setting
- #4477: fake context-window usage size
- #4438: resume directly by session id
- #4436: avoid loading symlinked context files twice
- #4431: improve tree actions: copy and branch from selected entry
- #4392: queued slash-command follow-up does not execute
- #4390: compaction max tokens not clamped at model max tokens
- #4325: context grows unbounded during long tool loops
- #4274: task does not resume after compaction
- #4276: abort during tool confirmation can duplicate output
- #4046: compaction deletes too much history
- #3660: auto-compaction triggers after context overflow

Representative commits:

- `a5b27367`: add initial harness foundation
- `cdde2e89`: consolidate harness session abstraction
- `c89b1ec3`: add context compaction
- `eeace797`: preserve kept messages across repeated compaction
- `fd385ecf`: add JSONL export/import for sessions
- `d501b9ca`: attach source info to resources and commands
- `3d9e14d7`: clamp summary output tokens
- `fe6b85b3`: clarify harness lifecycle state
- `322759a3`: snapshot harness turn state
- `29dea9a4`: add session context stats
- `f348a062`: cover harness stream configuration
- `c0f416aa`: add harness stream configuration

Lesson for Piers: session state needs a lifecycle model, not ad hoc vectors of
messages. Pi's split between live config, per-turn snapshot, persisted session
entries, pending writes, and save points is the right direction.

Conversation history should be validated at append time. Assistant tool calls,
tool results, retries, aborts, compactions, branch summaries, and reload
promotions should be legal state transitions, not loosely related message
objects. If an invalid transcript is found on load, the harness should expose a
repair path rather than silently replaying poisoned history.

For Piers, reload should happen only at explicit safe points unless the user
accepts a hard interruption. Generated-code proposals, build attempts,
accepted reloads, failed reloads, compactions, and state snapshots should be
session entries. This makes self-modification auditable.

Compaction should be treated as orchestration, not cleanup. It needs model
specific caps, preflight budget estimates, repeated-compaction invariants, and
tests proving kept messages and tool-result pairings survive.

## Tools Need Policy And Observability

Representative issues:

- #4493: bash tool can trigger unescapable interactive UIs
- #4460: edit tool expected-occurrences proposal
- #4459: native command-level permission system
- #4430: read, edit, and write errors during long sessions
- #4425: edit tool fails on Korean paths on Windows
- #4408: writing long files fails or truncates
- #4198: literal `\uXXXX` and Unicode character edit asymmetry
- #4018: grep tool argument injection enables command execution
- #4226: MCP tool parameters sent as strings instead of native types
- #4165: stream bash output incrementally

Representative commits:

- `bfa11a50`: add per-tool execution-mode override
- `759d5515`: emit parallel tool completion eagerly
- `63ac2df2`: sync tool hooks with agent event processing
- `3d43d2e1`: stop tool argument injection
- `e2f29b05`: add `defineTool` helper
- `39245529`: add read tool stats script
- `6d4d2e92`: add tool stats script
- `6b18cdba`: stream bash output incrementally
- `e355696d`: show compact read line ranges
- `8940c023`: compact resource read rendering
- `324aa1d6`: render compact read calls directly

Lesson for Piers: tools are the agent's real operating system interface. Tool
execution needs typed inputs, validation, streaming output, cancellation,
permission policy, path encoding correctness, and compact but inspectable
rendering.

The initial Piers built-ins should stay small: read, write, edit, exec. Even
those should be designed around policy hooks and structured telemetry from the
start.

Shell tools need noninteractive guards, timeouts, process-group cleanup, and
argument-safe APIs. File tools need path and text handling that distinguishes
bytes, scalar values, grapheme-ish display concerns, and platform path rules.
Patch-style edits should expose occurrence counts, dry-run diagnostics, and
clear mismatch explanations.

## Observability Is Not Optional

Representative issues:

- #4440: debugging tools for TUI rendering
- #4338: agent says working but makes no progress
- #3905: compact JSON log mode for finalized messages
- #3886: print mode does not exit when stdout is piped

Representative commits:

- `883862a3`: add AgentSession test harness with faux provider
- `746f770b`: add session lifecycle characterization suite
- `ddb18640`: return diagnostics from resource loaders
- `f348a062`: cover harness stream configuration
- `29dea9a4`: add session context stats
- `ef6af5eb`: add faux provider and model-registry factories

Lesson for Piers: debugging surfaces should be structured outputs, not only
logs. The core should expose event logs, provider request summaries, token
budgets, resource diagnostics, tool lifecycle events, reload outcomes, and
hang diagnostics.

Tests should be built around fake providers, fake tools, and deterministic
streams. The same event stream used by SDK/RPC users should be usable as a
golden-test fixture.

## Distribution Is Part Of The Harness

Representative issues:

- #4490: compiled Bun binary fails outside repo due missing package
- #4488: exits while downloading `fd` on Windows 11
- #4480: replace `koffi` with vendored Windows console helper
- #4478: brew formula does not pin Node version
- #4465: package update removes but does not reinstall dependencies
- #4456: cannot start without internet
- #4315: lockfile missing resolved or integrity entries
- #4267: package manager permission error while creating directories

Representative commits:

- `206bd085`: restore Linux binary build
- `24dec9fc`: fix Windows shell stdio handling
- `6ba53af8`: restore platform optional packages
- `060c10b8`: skip X11-only native addon for copy on Linux
- `5e1e4c3c`: support renamed self-update package
- `dacb7eaa`: detect renamed npm self updates

Lesson for Piers: a coding agent must start reliably in hostile local
environments. Startup should avoid network requirements, optional platform
helpers should fail soft, and self-update/package-update mechanisms need
transactional behavior.

Rust gives Piers an advantage here. A single native binary plus Wasm guests is
a cleaner distribution story than a bundled Node/Bun runtime, but guest
dependency builds and tool downloads can reintroduce the same failure modes if
they are not staged and cached deliberately.

## SDK, RPC, And UI Modes Add Contract Pressure

Representative issues:

- #4447: create GUI Pi client
- #4386: VS Code extension for RPC mode
- #4384: extension `setModel` and `setThinkingLevel` persist globally
- #4375: SDK docs show outdated tool config API
- #4314: `agent_end` should expose retry-pending state
- #4303: JSON mode never exits when stdin is `/dev/null`
- #4258: web UI hides final assistant message while generating appears active
- #4225: web UI stale after session state mutation

Representative commits:

- `d68011da`: dispose SDK example sessions
- `74739567`: fix SDK tool config docs
- `4eadc8fd`: fix SDK README tool config
- `c3ce1d33`: fix SDK example tool config
- `fe6b85b3`: clarify harness lifecycle state

Lesson for Piers: if the harness is useful, people will want to embed it.
Embedding turns events, shutdown, retries, queue state, and session replacement
into API contracts. Piers should design the event stream before designing a
rich TUI.

The line REPL can remain a debug shell, but the core should expose a stable
event stream and command API that a TUI, JSON mode, RPC server, and future GUI
can all share.

## Design Defaults For Piers

Based on the issue and commit patterns, Piers should bias toward these
defaults:

- Keep the host stable and small; make behavior reloadable behind explicit
  Wasm interfaces.
- Treat provider integration, auth, retries, and replay compatibility as
  host-owned infrastructure.
- Define provider streams as typed state machines with terminal states.
- Use append-only session storage with parent links from the start.
- Validate session transitions at append/load time and provide repair paths.
- Represent reload attempts and generated-code proposals as first-class
  session entries.
- Add save points before adding live reload during arbitrary execution.
- Treat compaction as a first-class transition with budget invariants.
- Stage generated code and promote only after build, validation, and restore.
- Keep built-in tools minimal, typed, cancellable, and policy-gated.
- Make extension or guest registrations declarative and host-validated.
- Reject silent tool/resource shadowing unless explicitly configured.
- Preserve source metadata for every loaded resource, tool, command, and hook.
- Use fake providers/tools and deterministic streams for characterization
  tests.
- Return structured diagnostics for resources, providers, tools, reloads, and
  sessions.
- Treat terminal UI as a later, test-heavy layer over a stable event core.
- Avoid startup network requirements and make optional platform helpers fail
  soft.
- Design SDK/RPC/event contracts before coupling behavior to a specific UI.
- Treat closed or abandoned architectural pull requests as signal, not noise.
- Revisit checkpointing, deterministic replay, rollback, worker-loop dispatch,
  and browser-safe core boundaries as first-class design topics.
