# Snapshot Format

Snapshots are the state payloads that let a new guest instance continue after a
reload. In the current PoC, the guest returns an opaque JSON-shaped string:

```json
{"calls":1}
```

That proves the mechanism, but a serious harness needs versioned snapshots with
validation, migration, and restore diagnostics.

## Ownership

The guest owns the schema of guest-local state. The host owns the lifecycle of
capturing, storing, validating, restoring, and recording snapshot outcomes.

This split is important:

- guest-local state can change with guest behavior
- host state must survive bad guest behavior
- session history must be able to say which guest version produced a snapshot
- failed restore must not corrupt the active guest

The host should treat snapshot payloads as opaque for business semantics but
not opaque for envelope validation.

## Envelope

Use a host-validated envelope around guest state:

```json
{
  "format": "piers.guest-snapshot",
  "version": 1,
  "guest_id": "default",
  "guest_version": "sha256:...",
  "schema": "counter-v1",
  "created_at": "2026-05-14T00:00:00Z",
  "payload_encoding": "json",
  "payload": {
    "calls": 1
  }
}
```

Initial fields:

- `format`: constant identifying the envelope
- `version`: host envelope version
- `guest_id`: stable guest identity
- `guest_version`: source or artifact digest for the producing guest
- `schema`: guest-declared payload schema name
- `created_at`: host timestamp
- `payload_encoding`: `json`, `cbor`, or `bytes`
- `payload`: guest-defined state

The host can validate the envelope without understanding the payload.

## Restore Contract

Restore should return a structured result, not silently ignore malformed state.

Initial result categories:

- `restored`: the guest accepted the snapshot
- `migrated`: the guest accepted an older schema after migration
- `rejected`: the snapshot is valid but not usable by this guest
- `invalid`: the envelope or payload is malformed
- `partial`: the guest restored some state and dropped some state

The host should record the restore result as a session entry during reload.

## Migration

Guest migrations should be explicit and local:

1. Host passes the previous snapshot envelope to the new guest.
1. Guest checks `schema` and `guest_version`.
1. Guest either restores directly, migrates to its current schema, or rejects
   the snapshot.
1. Host records the outcome and diagnostics.

The host should not run arbitrary guest migration code against host-owned
resources. If a migration needs host data, the guest should request it through
declared capabilities.

## Save Points

Snapshots should be captured at known save points:

- before staged reload
- after successful reload promotion
- before compaction
- after compaction
- before risky tool execution if rollback support exists
- before shutdown

Capturing snapshots mid-tool or mid-provider-stream should require an explicit
interruption rule. Otherwise the guest may serialize state that cannot be
replayed cleanly.

## Failure Policy

Snapshot failure is a harness event, not just a log line.

If snapshot capture fails before reload:

- abort the reload
- keep the active guest
- record `guest_snapshot_failed`
- keep the proposed source staged for inspection

If restore fails in a staged guest:

- do not promote the staged guest
- record `guest_restore_failed`
- keep the previous guest active
- preserve restore diagnostics

If restore succeeds partially:

- require explicit policy before promotion
- record which fields or capabilities were dropped

## Test Fixtures

Snapshot tests should cover:

- unknown envelope versions
- unknown guest schemas
- missing payload fields
- malformed JSON
- oversized payloads
- partial restore
- migration from old schema to new schema
- restore failure after staged build success
- rollback to previous guest after restore failure

These tests are part of reload safety. A self-rewriting harness is only useful
if bad generated code cannot make previous state unrecoverable.
