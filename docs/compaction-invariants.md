# Compaction Invariants

Compaction is the process of reducing context while preserving the information
needed to continue a session. Pi's history shows that compaction bugs are not
minor prompt-quality issues. They can lose queued messages, orphan tool calls,
delete too much history, break retries, and prevent tasks from resuming.

Piers should model compaction as a session transition with invariants, not as a
text cleanup pass.

## Ownership

The host owns the canonical session log. The guest may suggest compaction
strategy or summary prompts, but it must not mutate durable history directly.

Compaction output is derived state:

- summaries
- retained context windows
- provider replay payloads
- token budget estimates

The append-only session log remains authoritative.

## Cut Points

Compaction can only cut at safe boundaries.

Safe cut points:

- after an assistant turn with all required tool results recorded
- after a failed turn has a terminal failure entry
- after an aborted turn has been marked aborted
- after reload has either failed or promoted

Unsafe cut points:

- between tool call and tool result
- during a provider stream
- during a tool execution
- while compaction is already running
- while reload promotion is pending

The host should reject compaction if the session is not at a safe cut point.

## Required Invariants

Compaction must preserve:

- tool-call and tool-result pairing
- user intent for the active task
- current model and thinking-level choices
- provider compatibility metadata needed for replay
- pending queued user messages
- branch and parent links
- reload proposals and promotion outcomes
- diagnostics for failed tools or reloads

Compaction must not:

- create provider-invalid transcript order
- summarize away required tool results
- lose source provenance for resources or tools
- change the canonical session log
- silently exceed the target model context window

## Repeated Compaction

Repeated compaction should be idempotent enough to test. The second compaction
should not progressively erase required facts just because the first compaction
already summarized them.

The host should track:

- original source range
- previous summary entry
- retained entries
- summary model and budget
- token estimate before and after

If a summary is summarized again, the session should record that chain.

## Budgeting

Compaction needs preflight budget estimates:

- provider model context window
- response reserve
- tool schema budget
- resource budget
- system and policy budget
- user-visible transcript budget

The host should clamp compaction targets to model limits. It should not ask a
model for a summary larger than the model can return.

## Interaction With Reload

Reload and compaction should both be explicit session phases. They should not
run over each other.

Safe rule:

- compaction may prepare context for a future reload proposal
- reload may happen after compaction completes
- reload validation may use compacted replay fixtures
- neither transition should mutate the other's in-progress state

This avoids the Pi class of overlapping compaction and state replacement bugs.

## Test Fixtures

Compaction fixtures should cover:

- tool call followed by tool result
- missing tool result
- aborted tool execution
- queued user message during compaction
- repeated compaction over the same range
- compaction near model token limit
- model handoff after compaction
- branch summary preservation
- reload proposal inside compacted history

The key assertion is that replay after compaction produces legal host events
and legal provider payloads.
