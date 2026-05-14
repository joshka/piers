# Deterministic Replay

Deterministic replay is the test strategy that makes self-rewrite credible.
If Piers cannot replay important sessions and failures, it cannot safely accept
generated code changes.

Replay does not mean calling real providers again and hoping for the same
answer. It means feeding recorded host events, fake provider streams, fake tool
results, and staged guest artifacts through the same reducers and validators.

## Goals

Replay should prove:

- session entries form a legal state machine
- provider streams normalize into legal events
- tool calls and tool results remain paired
- compaction preserves replay legality
- reload promotion leaves a restorable guest
- UI clients can reconstruct state from events
- bad generated code fails before promotion

This is the harness equivalent of unit tests for self-modification.

## Replay Inputs

Replay fixtures should be plain files that can run offline:

- session event logs
- provider stream fixtures
- tool result fixtures
- guest source patches or replacement source
- guest snapshot envelopes
- expected diagnostics
- expected final session state

The fixtures should avoid network access and destructive file operations.

## Fake Providers

Fake providers should emit normalized or provider-native streams for edge
cases:

- incomplete stream
- retryable transport failure
- malformed tool-call arguments
- interleaved text and tool deltas
- missing terminal event
- unsupported thinking blocks
- provider-specific replay rejection

The same fixture should be usable to test both the provider adapter and the
session reducer.

## Fake Tools

Fake tools should cover:

- successful read/write/edit
- validation failure
- streamed output
- timeout
- abort
- permission denial
- Unicode path handling
- CRLF edit behavior

Tool fixtures should not depend on the user's machine state unless the test is
explicitly about platform behavior.

## Replay Around Reload

The reload replay loop should be:

1. Load a session fixture.
1. Apply guest patch in a staging workspace.
1. Build staged guest.
1. Instantiate staged guest.
1. Restore fixture snapshot.
1. Replay selected host events.
1. Check expected guest events and diagnostics.
1. Promote only if acceptance checks pass.

This makes rollback and promotion testable without a real model.

## Golden Diagnostics

Replay should assert diagnostics, not just success or failure. Examples:

- `provider_stream_incomplete`
- `tool_result_without_call`
- `compaction_unsafe_cut_point`
- `guest_restore_schema_rejected`
- `reload_manifest_invalid`
- `tool_policy_denied`

Stable diagnostics are part of the developer experience. They also let future
agents understand why a self-change failed.

## Fixture Naming

Use names that encode the risk:

```text
fixtures/replay/provider/incomplete-anthropic-message-stop.jsonl
fixtures/replay/session/orphan-tool-result.jsonl
fixtures/replay/reload/restore-schema-rejected.json
fixtures/replay/tool/windows-crlf-edit.json
```

The exact layout can change later. The important property is that fixtures are
small, named, reviewable, and runnable without external services.

## Success Criteria

The first useful replay suite should cover:

- one successful turn
- one tool call and result
- one invalid orphan tool result
- one incomplete provider stream
- one compaction over a safe cut point
- one rejected compaction over an unsafe cut point
- one successful staged reload
- one restore failure that keeps the old guest active

That suite would already validate the core Piers architecture better than a
large manual demo.
