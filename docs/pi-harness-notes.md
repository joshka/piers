# Pi Harness Notes

These notes summarize ideas from the local `../pi` checkout and Mario
Zechner's Pi blog post that are useful for shaping Piers. The `../pi`
`AGENTS.md` file belongs to that repository only; it is not an instruction
source for this repo.

## Useful Pi Shape

Pi separates the system into layers:

- model/provider API
- low-level agent loop
- harness/session/runtime layer
- UI and extension-facing application layer

The important lesson for Piers is that the reloadable part should not own
everything. A stable host should own session state, runtime config, tool
registration, persistence, and lifecycle. Reloadable code should provide
behavior behind explicit interfaces.

Pi's `AgentHarness` is especially relevant. It keeps separate concepts for:

- latest harness config
- per-turn snapshots
- persisted session state
- pending writes queued during active turns
- save points between turns
- structural operations that require idle state

Piers should mirror that split before it grows a real coding agent. Reloading
guest behavior is much easier if a turn snapshot is already explicit and state
handoff only happens at known save points.

## Extension Lessons

Pi extensions are TypeScript modules that can register tools, commands,
shortcuts, UI, hooks, providers, and session entries. The extension loader
discovers project and global files, loads them through `jiti`, and invalidates
old contexts after reload or session replacement.

The Rust/Wasm equivalent should avoid trying to make every guest arbitrary at
first. A better shape is:

- host owns extension discovery and reload policy
- guest exports a small registration function or manifest
- host validates registrations before making them model-visible
- stale guest handles become unusable after reload
- generated code is staged and promoted after a successful build

The current Piers PoC only rewrites `guest/src/lib.rs` directly. That proves
the compile/reload loop, but a Pi-like harness should add a staging directory
before expanding the rewrite surface.

## Minimal Core

Mario's blog argues for a small, observable core: minimal prompt, minimal tool
set, explicit context, visible events, and no hidden orchestration. The local
Pi docs show the same bias: the built-in tool surface stays small, while
extensions, skills, prompt templates, themes, and packages carry workflow
specific behavior.

For Piers, the analogous default should be:

- `read`
- `write`
- `edit`
- `exec`

Everything else should start life as reloadable guest behavior, a skill, or a
host capability that the guest explicitly registers. This keeps the model
context small and makes self-improvement inspectable.

## Session Tree

Pi stores sessions as JSONL trees. Entries have parent links, and the active
position is a leaf. This lets the user branch from earlier turns, review
alternate attempts, and preserve abandoned work with summaries.

Piers should eventually use the same underlying idea:

- append-only session log
- parent-linked entries
- explicit active leaf
- model and thinking-level changes as entries
- guest reloads as entries
- generated-code proposals as entries
- accepted generated-code promotions as entries

This matters for a self-rewriting harness because generated code should be
auditable. A session should answer: what code proposed this change, what prompt
caused it, what artifact was built, and what state snapshot was restored.

## Reload Model

Pi's `/reload` reloads extensions, skills, prompts, themes, and keybindings
without replacing the whole process. Extension APIs guard against stale
contexts after reload.

Piers already has the lowest-level version of this:

1. snapshot old guest
1. rebuild guest Wasm
1. instantiate new guest
1. restore snapshot
1. swap guest handle

The next step is to make reload a first-class lifecycle event:

- `before_reload`
- `reload_build_started`
- `reload_build_failed`
- `reload_promoted`
- `after_reload`

The host should keep old guest state alive until the new guest is built,
instantiated, validated, and restored.

## Proposed Next Architecture

Turn the current PoC into a small harness with these pieces:

- `HostRuntime`: owns Wasmtime engine, current guest, and host capabilities.
- `Session`: append-only JSONL tree with messages and reload events.
- `TurnState`: immutable snapshot passed to one model request.
- `GuestRegistry`: tools, commands, hooks, and resources exported by the guest.
- `ReloadManager`: stages generated guest code, builds it, validates exports,
  restores state, and promotes only successful artifacts.

The guest should eventually export something like:

```wit
export manifest: func() -> string;
export handle-event: func(event: string) -> string;
export propose-update: func(spec: string) -> string;
export snapshot: func() -> string;
export restore: func(snapshot: string);
```

`manifest()` can return JSON that describes tools, commands, prompts, and hook
interests. The host remains responsible for validating and enforcing those
registrations.

## Sources

- Local Pi harness notes:
  `../pi/packages/agent/docs/agent-harness.md`
- Local Pi extension docs:
  `../pi/packages/coding-agent/docs/extensions.md`
- Local Pi session docs:
  `../pi/packages/coding-agent/docs/sessions.md`
- Mario Zechner:
  <https://mariozechner.at/posts/2025-11-30-pi-coding-agent/>
- Armin Ronacher:
  <https://lucumr.pocoo.org/2026/1/31/pi/>
