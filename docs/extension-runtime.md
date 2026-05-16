# Extension Runtime

Pi's history shows that extensions become a runtime, not a side feature. Hooks,
custom tools, skills, resources, autocomplete providers, footer renderers,
model filters, and cross-extension calls all need lifecycle semantics once real
users depend on them.

For Piers, the extension idea maps to reloadable guest declarations. The guest
can propose behavior. The host validates and installs it.

## Principle

Extensions should be declarative before they are executable.

The host should receive a manifest that says what the guest wants to expose:

- commands
- tools
- hooks
- resources
- prompts or policies
- event subscriptions
- UI contributions
- required capabilities

The host decides what becomes active.

## Manifest

An initial manifest should include:

- `id`: stable extension or guest identity
- `version`: manifest version
- `source`: guest artifact digest or installed package source
- `capabilities`: requested host capabilities
- `commands`: slash or REPL commands
- `tools`: model-callable tools
- `hooks`: lifecycle hooks
- `resources`: model-visible context providers
- `conflicts`: declared override behavior

The manifest should be part of reload validation. If a new guest cannot produce
a valid manifest, it should not be promoted.

The current kernel implements the first manifest slice in WIT: guest
generations declare capabilities and commands, and the host installs them into
a `GuestRegistry` only after validating schema version, nonempty fields,
duplicate names, and conflicts with built-in host commands.

## Provenance

Every registered item should carry source metadata:

- which guest or extension declared it
- which artifact digest produced it
- which reload accepted it
- which permissions it requested
- whether the user approved it

This prevents a future session from showing a tool or command without knowing
where it came from.

## Conflicts

Conflicts should be explicit:

- a guest tool should not silently replace a built-in tool
- command names should not shadow host commands by default
- resource names should have stable namespaces
- multiple hooks should have deterministic ordering

If overriding is supported, it should require a manifest field and a recorded
approval or policy decision.

## Lifecycle

Extension lifecycle states:

- `declared`
- `validated`
- `active`
- `stale`
- `revoked`
- `failed`

Reload can make previously active declarations stale. The host should revoke or
rebind handles when the guest changes.

Stale handles are a real bug class. A command registered by generation 3 should
not accidentally call generation 4 behavior unless the host intentionally
rebinds it.

## Hooks

Hooks should have narrow inputs and outputs. Examples:

- before provider request
- after provider response
- before tool execution
- after tool execution
- before compaction
- after compaction
- before reload promotion
- after reload promotion

Hook failures need policy:

- fail closed for security-sensitive hooks
- fail soft for display-only hooks
- record diagnostics either way

Hooks should not mutate host registries directly. They should return proposed
events or changes for the host to validate.

## Resources

Resources are model-visible context. The host should track:

- source path or provider
- freshness
- load diagnostics
- permissions
- token budget
- whether the resource was included in a turn snapshot

If reload changes resource policy, the next turn should use a fresh turn
snapshot rather than retroactively changing an in-flight request.

## Cross-Extension Calls

Cross-extension calls are useful but risky. They should go through the host:

1. caller requests a named service
1. host validates caller permission
1. host resolves the callee by stable identity
1. host records the call
1. callee returns a typed result or diagnostic

Avoid direct guest-to-guest mutable references. They make reload and rollback
hard to reason about.

## Test Fixtures

Extension runtime tests should cover:

- manifest parse failures
- name conflicts
- stale handles after reload
- hook failure policy
- revoked capabilities
- resource load diagnostics
- extension command provenance
- cross-extension call denial

The goal is not to build a large plugin ecosystem immediately. The goal is to
avoid painting the reload boundary into a corner.
