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

The current Rust kernel has a host-owned reducer in `src/provider.rs` and an
RPC bridge named `provider_stream`. It is not a network adapter yet; it is the
contract real adapters and fake providers must satisfy before their output can
become a durable assistant turn.

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

Implemented reducer checks:

- no stream event is accepted before `response_started`
- no event is accepted after a terminal event
- `finish()` rejects streams with no terminal event
- successful streams require all text, thinking, and tool-call blocks to close
- final tool-call arguments must parse as JSON
- `stream_incomplete`, `response_failed`, and `response_aborted` are terminal
  states, but they are not successful assistant turns

## Harness Bridge

`provider_stream` lets an adapter submit one normalized provider response to
the same host-owned turn machinery used by ordinary prompts. `provider_script`
runs several normalized responses as a deterministic provider/tool loop. The
harness:

- records `turn_started`, `user_message`, and `turn_completed`
- persists assistant text as `assistant_message`
- validates final tool-call JSON before execution
- executes host tools and records `tool_call` / `tool_result`
- records provider failure, abort, and incomplete stream terminals as
  diagnostics and completes the turn with `success: false`

`provider_script` sends tool results from one response back into the next
scripted provider request and stops when the provider emits no more tool calls.
`provider_prompt` uses the first network adapter, `openai_compatible`, to send
non-streaming chat-completions requests and then normalize the response back
into provider events. It retries transient HTTP statuses and transport
failures with a bounded host-owned retry policy.
`provider_script_start` and `provider_prompt_start` run those loops in a
background worker thread. `job_status` polls the worker, and `abort` with a
`job_id` flips the provider loop's cooperative cancellation token. Streaming
transport remains future work, and cancellation cannot forcibly interrupt a
blocking synchronous HTTP call once it is inside the transport.
`provider_status` exposes those missing pieces as structured data instead of
requiring callers to infer them from display text.
It reads provider configuration from `PIERS_PROVIDER`, `PIERS_MODEL`,
`PIERS_BASE_URL`, `PIERS_API_KEY`, and `PIERS_PROVIDER_TIMEOUT_MS`, but reports
only whether an API key is present.

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
