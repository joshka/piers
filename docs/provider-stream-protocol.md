# Provider Stream Protocol

Provider drift was the largest recurring theme in Pi's issue history. OpenAI,
Anthropic, Gemini, Bedrock, Copilot, OpenRouter, and OpenAI-compatible servers
all behave differently around streaming, tool calls, thinking blocks, retries,
auth, and replay.

Piers should keep provider integration host-owned and normalize every backend
into a typed event protocol.

## Goals

The protocol should:

- expose partial output without making it authoritative too early
- preserve provider-specific diagnostics
- validate terminal states
- make replay compatibility explicit
- keep guest code away from wire-format compatibility

## Event Shape

Initial event categories:

- `response_started`
- `text_started`
- `text_delta`
- `text_finished`
- `thinking_started`
- `thinking_delta`
- `thinking_finished`
- `tool_call_started`
- `tool_call_arguments_delta`
- `tool_call_finished`
- `response_finished`
- `response_failed`
- `response_aborted`
- `stream_incomplete`

Every provider stream must end with exactly one terminal event:

- `response_finished`
- `response_failed`
- `response_aborted`
- `stream_incomplete`

An incomplete stream is not success. It may be retryable, but it should not be
serialized as a valid assistant turn.

## Partial Tool Arguments

Partial tool arguments are useful for UI and progress reporting, but they are
not validated tool inputs. The host should validate only the final tool-call
arguments emitted by `tool_call_finished`.

If a provider can emit interleaved text and tool-call deltas, the protocol must
preserve content indexes or stable block ids. Consumers must not assume that
all deltas for one content block are contiguous.

## Replay Policy

Replay is provider-specific. A stored session entry can be canonical, but each
provider adapter must declare how that entry maps back to its wire format.

Compatibility policy should cover:

- tool-call id normalization
- unsupported thinking blocks
- signed or unsigned thought signatures
- orphaned tool calls
- empty tool results
- image support
- cache/session affinity fields
- model-specific reasoning levels

If an adapter cannot replay a session safely, it should return a typed
diagnostic instead of sending a malformed request.

## Test Fixtures

Provider adapters should have golden fixtures for:

- incomplete SSE streams
- interleaved content and tool deltas
- provider-native tool calls
- retries after 429 and 5xx responses
- model handoff after tool use
- thinking/signature preservation
- local OpenAI-compatible quirks
- cache-affinity and prompt-cache behavior

These fixtures are more important than a large real-provider smoke suite. They
let Piers test the protocol runtime without paid APIs or unstable networks.
