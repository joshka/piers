# Rollback Testing

Rollback is the other half of self-rewrite. A harness that can generate and
build new code but cannot reject or roll back bad code is only a build loop.

Piers should treat rollback as a normal lifecycle path with tests.

## What Can Roll Back

Different facts roll back differently:

- staged source can be discarded
- staged artifacts can be deleted or retained for inspection
- active guest handles can remain unchanged
- guest snapshots can be restored into an older guest
- session entries cannot be deleted from history
- host-owned tools and resources can be rebound or revoked

The session log is append-only. Rollback adds entries that explain what
happened; it does not erase the failed attempt.

## Failure Points

Rollback tests should cover each reload step:

- generated source rejected by static checks
- staged source fails to write
- staged guest fails to build
- staged component fails WIT validation
- staged manifest is invalid
- old guest snapshot fails
- new guest restore fails
- smoke event fails
- replay fixture fails
- promotion copy fails
- post-promotion health check fails

Each failure point should leave the active guest and host-owned session state
in a known state.

## Test Shape

A rollback test should assert:

- active generation before the attempt
- staged source or artifact path
- failure diagnostic
- active generation after the attempt
- whether staged files were retained
- session entries appended
- old guest can still handle input

This is more valuable than only asserting an error return.

## Promotion Boundary

Promotion is the danger line. Before promotion, failures should be cheap. After
promotion, failures need explicit recovery.

The host should minimize the post-promotion failure window:

1. validate staged source
1. build staged artifact
1. validate WIT exports
1. validate manifest
1. restore snapshot
1. run smoke and replay checks
1. atomically promote source and artifact
1. swap active guest handle
1. record promotion

If promotion itself can partially fail, the host needs a promotion journal that
can repair or roll forward on next startup.

## Startup Recovery

Startup should detect interrupted reloads:

- staged source exists without promotion record
- promoted source digest does not match session record
- artifact digest does not match source digest
- previous promotion journal is incomplete

The host should choose a conservative recovery path:

- keep or restore the last known good guest
- quarantine uncertain artifacts
- record a startup repair diagnostic

## Rollback And Snapshots

Rollback depends on snapshots, but snapshots are not enough by themselves.

The host must know:

- which guest produced the snapshot
- which guest schemas can restore it
- whether restore was full or partial
- which host resources were rebound
- whether any guest-declared tools became stale

Failed snapshot or restore should block promotion unless the user explicitly
accepts state loss.

## Minimal Suite

The first rollback suite should include:

- build failure leaves active guest unchanged
- invalid manifest leaves active guest unchanged
- restore failure leaves active guest unchanged
- replay failure leaves active guest unchanged
- successful reload increments generation
- startup repair detects interrupted staged reload

These tests define the safety envelope for future self-modification.
