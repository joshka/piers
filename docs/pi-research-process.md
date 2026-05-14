# Pi Research Process

This document captures the process used to turn Pi's history into Piers design
guidance. It is intentionally about method, not just conclusions.

The project question is:

> What should a Rust-first, self-rewriting coding harness build first if Pi is
> the best available warning label?

The answer should not come from copying Pi feature-for-feature. Pi is a mature
TypeScript product surface. Piers is a Rust/Wasm kernel experiment. The useful
work is translating Pi's accumulated pressure into smaller Rust contracts.

## Sources

The research pass used several kinds of source material:

- Pi issue titles and dates
- Pi pull request titles, states, and dates
- local Pi commit history
- local `../pi` source tree shape
- historical `pi-mono` context
- related Rust agent and assistant-harness repositories
- the current Piers PoC and docs

Each source answers a different question:

- issues show user pain and operational failures
- pull requests show explored, abandoned, or revised design paths
- commits show accepted implementation direction
- related repos show how the same harness pressure appears in other shapes
- Piers source shows what the current kernel can actually support

## Why Titles Were Enough For The First Pass

The goal was not to adjudicate every Pi bug in detail. The goal was to detect
recurring pressure. Titles are useful for that because issue and pull request
titles are the compressed record of what maintainers and users thought was
worth naming.

The pass looked for repetition across time:

- provider stream corruption
- tool-call and tool-result pairing bugs
- compaction races
- extension registration and lifecycle problems
- terminal input and rendering failures
- package, install, and update fragility
- replay, export, RPC, and SDK pressure

When a theme appeared in issues, pull requests, and commits, it was treated as
kernel-level signal rather than incidental feature work.

## Time-Based Reading

The research was not just a flat count of topics. It also asked how Pi changed
over time:

1. Early commits established provider and tool primitives.
1. Initial issues exposed terminal, session, queue, and compaction pressure.
1. Extensions turned plugin convenience into runtime design.
1. Provider drift and session corruption forced stronger protocol boundaries.
1. SDK, RPC, worker, browser, and UI modes made the core API contractual.
1. Late refactors moved state back into explicit lifecycle and resource types.

That arc matters for Piers. It suggests that "simple" features such as reload,
tools, compaction, and UI are only simple before real sessions exist.

## Translation Rule

The translation rule for Piers is:

> Convert Pi product pressure into small Rust kernel contracts.

Examples:

- Pi provider bugs become a host-owned provider stream protocol.
- Pi session corruption becomes an append-only validated state machine.
- Pi extension churn becomes a declarative guest manifest boundary.
- Pi compaction bugs become explicit cut-point and replay invariants.
- Pi rollback/checkpoint PRs become staged reload and promotion rules.
- Pi terminal bugs become a late, event-consuming UI layer with fixtures.

The Rust answer is not "make everything native." Rust gives Piers strong local
types, single-binary distribution, and a clear host process. The Wasm guest
gives Piers a reload boundary. The hard part is deciding what state is allowed
to cross that boundary.

## Main Synthesis

The emerging Piers shape is:

- stable Rust host
- reloadable Rust guest compiled to a WebAssembly component
- WIT boundary for typed host/guest messages
- host-owned session log, provider adapters, tools, permissions, and reload
  promotion
- guest-owned prompts, policies, planning behavior, tool selection, and
  proposed patches
- deterministic replay fixtures as the acceptance test for self-change

This keeps self-rewrite auditable. A guest can propose new behavior, but the
host records, stages, builds, validates, replays, and promotes it.

## What To Do When More Context Is Needed

If this thread is compacted or future work needs to rebuild the context, read
these docs in this order:

1. [How Piers Works](how-it-works.md)
1. [Pi History Timeline Notes](pi-history-timeline.md)
1. [Pi PR History Notes](pi-pr-history.md)
1. [Related Project Notes](related-projects.md)
1. [Host And Guest Boundary](host-guest-boundary.md)
1. [Reload Lifecycle](reload-lifecycle.md)
1. [Session State Machine](session-state-machine.md)
1. [Provider Stream Protocol](provider-stream-protocol.md)

Then continue by asking whether the proposed feature strengthens or weakens
these contracts:

- transactional reload
- validated session state
- host-owned provider normalization
- deterministic replay
- manifest-driven extension behavior
- UI as a client of events

## What Not To Do

Do not treat the Pi history as a feature backlog. Piers should not immediately
build every provider, extension hook, terminal protocol, or SDK surface Pi
grew.

Do not let the reloadable guest own durable resources. That makes failed reload
and rollback too hard.

Do not make the TUI the first serious product surface. The event core needs to
be stable before multiple UIs consume it.

Do not call a build-and-restart loop "self-improvement" unless the session log
can explain what changed, why it was accepted, and how to replay or roll it
back.
