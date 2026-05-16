# Implementation Handoff

This note captures the current implementation direction so a future agent can
continue without rediscovering why the repository moved away from the original
proof-of-concept harness.

## Goal

Piers should be a non-TUI, Pi-style coding agent shell in Rust. The useful
shape is not a standalone TUI and not a tiny "piers harness" demo. It is a
line-oriented agent worker that can be embedded by another process, tested by
JSONL fixtures, and eventually driven by a real provider loop with host-owned
tools, sessions, cancellation, and reloadable behavior.

The current implementation keeps Pi's broad entry shape:

1. `main.rs` handles process setup only.
1. `app.rs` chooses REPL, print, JSON, or JSONL RPC mode.
1. Every mode submits typed commands to one shared harness core.
1. The harness owns turns, tools, sessions, reloads, and provider event
   reduction.

## Steering Captured

The major steering decisions were:

- Remove the proof-of-concept banner and command surface. This is `piers`, not
  a PoC named "piers harness".
- Think from `main.rs` inward and make the application look like a non-TUI
  version of Pi: one reusable session core with multiple client modes.
- Use `../pi-mono` as a reference for shape, not as code to copy directly.
- Keep the TUI out of scope for now. The useful rewrite is a worker/REPL
  agent tool that can later grow UI clients on top.
- Preserve the reloadable Rust/Wasm guest idea, but put it behind a stronger
  host-owned command, event, session, provider, and tool boundary.
- Document enough state that a future agent can continue without spending the
  next context window reconstructing the same design history.

## Implemented

The current code has these concrete pieces:

- `src/main.rs`: thin process entry point.
- `src/app.rs`: CLI parsing, REPL mode, print mode, JSON output mode, and JSONL
  RPC mode.
- `src/harness.rs`: typed `HarnessCommand`, `HarnessEvent`, turn lifecycle,
  reload lifecycle, guest routing, provider loop, manifest, status, session
  listing, and abort data.
- `src/session.rs`: append-only JSONL session entries plus reducer validation
  for turn, tool, and reload ordering.
- `src/tools.rs`: host-owned `read`, `write`, `edit`, and policy-gated `exec`
  tools.
- `src/provider.rs`: normalized provider event protocol, stream reducer,
  deterministic scripted provider, OpenAI-compatible chat-completions adapter,
  retry policy, and cooperative cancellation token.
- `guest/src/lib.rs`: the reloadable guest behavior behind the WIT boundary.
- `wit/piers.wit`: typed host/guest protocol with guest manifest, commands,
  capabilities, lifecycle calls, and tool requests.

The RPC worker accepts ordinary prompts, provider streams, deterministic
provider scripts, configured provider prompts, session discovery, session entry
reads, session switching, status, provider status, manifest, reload, evolve,
abort, and quit. Mutating source commands can be blocked by `--read-only` or
gated by `--require-approval`.

Provider work now has three levels:

- `provider_stream` accepts one already-normalized response stream.
- `provider_script` runs a deterministic multi-step provider/tool loop.
- `provider_prompt` runs the same loop through an OpenAI-compatible
  chat-completions endpoint configured by environment variables.

The background RPC job path is available for long-running provider work:

- `provider_script_start` starts deterministic provider work in a worker
  thread.
- `provider_prompt_start` starts configured provider work in a worker thread.
- `job_status` polls the job and emits final harness events/results when it
  completes.
- `abort` with `job_id` flips the job's cancellation token.

This is deliberately simple and line-oriented. It proves the host-owned worker
contract before introducing a Tokio runtime or streaming transport.

## Gaps From Pi

This is still not Pi. The important remaining gaps are:

- No full `AgentSession` equivalent that owns provider selection, prompt
  assembly, context compaction, queued input, and concurrent task orchestration
  as one cohesive runtime object.
- No streaming model transport yet. The OpenAI-compatible adapter is
  non-streaming chat completions normalized back into provider events.
- Background jobs are per-RPC-process and in-memory. They are not persisted,
  resumed, or coordinated across multiple worker processes.
- Cancellation is cooperative. It can interrupt retry sleeps and checks around
  provider/tool loop boundaries, but it cannot forcibly stop a blocking HTTP
  call already inside `reqwest::blocking`.
- No model capability registry, provider-specific tool schema adaptation, or
  automatic provider selection.
- No compaction entries, safe cut-point enforcement, or replay-oriented context
  packing.
- No mature approval UX. The current policy is a simple RPC field checked
  before reload/evolve.
- No terminal/TUI client, file watcher, task queue, or multi-agent orchestration
  layer.
- Reload startup repair and promotion journals are still future work.
- Tool policy is functional but minimal compared with a production coding
  agent sandbox.

## Next Useful Validation Work

The highest-value follow-up is to validate that this rewrite is a reasonable
agent-tool kernel rather than only a larger demo:

1. Add a process-level RPC fixture that starts a background provider job, polls
   it to completion, and verifies the JSONL envelopes and session entries.
1. Add a process-level cancellation fixture that starts provider work, aborts
   by `job_id`, and proves the final error path records a failed turn cleanly.
1. Run a live local OpenAI-compatible endpoint through `provider_prompt_start`
   and confirm tool-call arguments, tool results, retries, and cancellation are
   all observable through JSONL.
1. Compare the resulting command/event/session API against Pi's entry points
   and name the missing `AgentSession` responsibilities explicitly before
   adding more features.
1. Decide whether the next runtime step is a small thread-based job manager or
   a real async runtime. Do that before adding streaming transport.

## Validation Commands

Use these before handing off:

```sh
cargo fmt
cargo check --workspace
cargo test
cargo check -p piers-guest --target wasm32-wasip2
cargo build
markdownlint-cli2 AGENTS.md AGENTS.override.md "docs/**/*.md"
```
