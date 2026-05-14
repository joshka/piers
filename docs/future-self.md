# Notes To Future Self

The Pi history pass points to a clear order of operations for Piers. Resist
building the rich TUI or broad self-modification surface before the kernel
contracts exist.

## Highest Priority

1. Make reload transactional.
1. Define the session state machine.
1. Define the provider stream protocol.
1. Version guest snapshots.
1. Add a deterministic test harness with fake providers and fake tools.

These five pieces protect the core self-rewriting idea.

## Next Docs To Write

- `snapshot-format.md`: versioned guest state, migration hooks, restore errors
- `tool-policy.md`: path rules, edit semantics, exec cancellation, telemetry
- `extension-runtime.md`: manifests, provenance, conflicts, stale handles
- `compaction-invariants.md`: cut points, repeated compaction, tool pairing
- `platform-terminal-risk.md`: TUI fixtures and platform assumptions
- `deterministic-replay.md`: event fixtures, fake providers, patch replay
- `rollback-testing.md`: staged reload failures and promotion checks
- `core-ui-boundary.md`: REPL, JSON, RPC, TUI, browser, and worker clients

## Design Biases

- Host owns durability, lifecycle, permissions, providers, and promotion.
- Guest owns reloadable behavior behind a narrow interface.
- Every edge emits structured diagnostics.
- Every self-change is replayable and auditable.
- UI is a client of events, not the source of truth.
- Startup should not require network access.
- Broken generated code should be inspectable without breaking the active
  guest.

## Critical Anti-Patterns

- Writing generated code directly over the live source.
- Storing provider wire messages as the canonical session format.
- Letting guests mutate host registries directly.
- Treating compaction as a text cleanup pass.
- Allowing `exec` to run interactive commands without guardrails.
- Hiding tool/reload failures in UI-only logs.
- Adding terminal features before the event core is stable.
- Letting a convenient UI or extension API backfill the core data model.
