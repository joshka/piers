# Tool Policy

Tools are the harness boundary with the operating system. Pi's history shows
that tool bugs become security, correctness, and usability problems quickly:
argument injection, unescapable interactive commands, Unicode path failures,
truncated writes, duplicate abort output, and missing permission checks.

Piers should treat tools as host-owned capabilities. The reloadable guest may
request a tool call only through a declared, validated schema.

## Tool Identity

Every tool should have stable metadata:

- `id`: stable internal identifier
- `name`: model-facing name
- `source`: built-in, guest manifest, extension, MCP server, or local config
- `version`: tool schema version
- `description`: model-facing description
- `input_schema`: structured input schema
- `policy`: default permission and execution policy

Tool identity should be recorded in session entries. A future replay should be
able to tell which version of which tool produced a result.

## Input Validation

The host validates tool input before execution. The guest does not pass raw
shell strings or file paths directly to host internals.

Validation should cover:

- required and unknown fields
- path normalization
- workspace containment
- glob and regex limits
- string length and output limits
- environment variable access
- shell metacharacter handling
- binary versus text mode

Validation failure should produce a structured `tool_call_failed` entry. It
should not be reported as a provider failure or guest crash.

## Permissions

Initial permission categories:

- `read`: inspect files or resources
- `write`: create or modify files
- `exec`: run subprocesses
- `network`: connect outside the local machine
- `dangerous`: destructive or externally visible action

Policies should be explicit about:

- whether confirmation is required
- whether a guest can request the capability
- whether the capability is allowed in replay tests
- whether the capability is allowed during reload validation
- what telemetry must be recorded

The default for generated or guest-declared tools should be conservative.

## File Tools

File tools need careful text and path semantics. The policy should distinguish:

- bytes on disk
- decoded text
- displayed columns
- platform path encoding
- line endings
- exact occurrence counts

Edit operations should support dry-run diagnostics:

- matched occurrence count
- expected occurrence count
- before and after digests
- line-ending mode
- encoding assumptions
- reason for mismatch

This is the Rust advantage to use: make the states and conversions explicit.

## Exec Tools

Shell execution is high risk. The first exec tool should be deliberately plain:

- noninteractive by default
- timeout required
- working directory explicit
- environment explicit or inherited through policy
- stdout and stderr streamed as events
- process group cleanup on abort
- platform-specific shell selection in the host

The tool should reject commands likely to open a full-screen interactive UI
unless the user explicitly approves that mode.

## Cancellation

Every tool call needs a terminal state:

- `completed`
- `failed`
- `aborted`
- `timed_out`
- `policy_denied`

Abort should be idempotent. A repeated abort should not duplicate output or
leave the session with a pending tool call.

## Observability

Tool execution should emit structured events:

- `tool_call_requested`
- `tool_call_validated`
- `tool_call_started`
- `tool_output_chunk`
- `tool_call_completed`
- `tool_call_failed`
- `tool_call_aborted`

The UI can render these compactly, but the session log should preserve the
facts needed for replay and audit.

## Guest-Declared Tools

A guest may propose tools through a manifest. The host should validate:

- name conflicts with built-ins
- schema validity
- requested capabilities
- provenance
- whether the tool survives reload
- whether stale handles need revocation

Guest-declared tools should not silently shadow built-ins. Conflict resolution
should be explicit and recorded.

## Test Fixtures

Tool policy fixtures should include:

- argument injection attempts
- Unicode and Windows paths
- CRLF edits
- long writes and truncation cases
- missing expected occurrences
- aborted exec with child processes
- timeout behavior
- permission denial
- stale guest-declared tool after reload

These fixtures should run without network access and without real destructive
effects.
