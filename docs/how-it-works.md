# How Piers Works

Piers is intended to be a non-TUI, Pi-style coding agent shell in Rust.
The current implementation is the reloadable runtime underneath that agent:
it splits the program into a stable native host and a reloadable Rust guest
compiled to a WebAssembly component. Ordinary input still runs through that
guest, while provider-backed turns now run through a host-owned provider/tool
loop that can be driven by deterministic scripts or an OpenAI-compatible
chat-completions endpoint.

The implemented reload behavior is this loop:

1. Run a native Rust host.
1. Load a Rust guest compiled as Wasm.
1. Let the guest handle input.
1. Ask the guest to write replacement Rust code for itself.
1. Rebuild the guest Wasm artifact.
1. Swap the new guest into the still-running host.
1. Restore guest state into the new instance.

## Pieces

The host core is [src/harness.rs](../src/harness.rs). It owns lifecycle phases,
turn snapshots, session appends, tool execution, guest rebuilds, the Wasmtime
engine, component linker, and reload promotion. [src/app.rs](../src/app.rs)
owns the non-TUI application modes: a line REPL, single-shot text print mode,
JSON event output, and JSONL RPC. [src/main.rs](../src/main.rs) only handles
process setup and delegates into the app layer.

The guest is [guest/src/lib.rs](../guest/src/lib.rs). It is normal Rust code,
but it is compiled to `wasm32-wasip2` instead of the native machine target. The
guest owns the behavior that can be rewritten.

The interface is [wit/piers.wit](../wit/piers.wit). WIT defines the small
contract between host and guest:

```wit
export manifest: func() -> guest-manifest;
export handle-event: func(event: host-event) -> list<guest-event>;
export propose-update: func(spec: string) -> string;
export snapshot: func() -> string;
export restore: func(snapshot: string);
```

The host and guest both generate bindings from this WIT file. This is the
stable reload boundary.

The manifest currently declares the guest name, requested capabilities, and
guest commands. The host validates those declarations into a `GuestRegistry`
before using the guest generation.

## Pi Shape

Pi's CLI entry point creates one `AgentSession` and then runs it through
interactive, print, JSON, or RPC modes. That session owns the model selection,
provider authentication, prompt loop, tool registry, queued input, aborts,
compaction, and session switching. The UI layer is only one client of that
core session.

Piers follows that entry shape from `main.rs` inward: process setup is thin,
`app.rs` chooses REPL, print, JSON, or RPC mode, and every mode submits typed
commands to one shared core. The difference is what sits behind that core.
Pi sends prompts to a provider-backed agent loop. Piers currently sends
ordinary input to the reloadable Wasm guest, and provider commands use the same
session and tool machinery through a normalized provider event protocol.

## Startup

When you run:

```sh
cargo run
```

the host checks for this artifact:

```text
target/wasm32-wasip2/debug/piers_guest.wasm
```

If the artifact does not exist, the host runs:

```sh
cargo build -p piers-guest --target wasm32-wasip2
```

Then the host creates a Wasmtime engine with component model support, adds WASI
Preview 2 imports to the linker, creates a store, and instantiates the guest
component.

## Normal Input

For ordinary input:

```text
hello
```

the REPL submits `HarnessCommand::UserInput` to the core. The core records a
durable `turn_started` entry, captures a `TurnSnapshot`, records the user
message, and calls the guest's `handle-event(host-event.user-input)` export.

The current guest implementation increments a thread-local call counter and
emits a `guest-event.assistant-message`:

```text
<behavior> | call #<n>: <input>
```

The `<behavior>` value comes from this guest constant:

```rust
const BEHAVIOR: &str = "...";
```

That line is the intentionally tiny rewrite target in the current kernel.

## Agent Tool Mode

For another agent or process, Piers can run as a JSONL command worker:

```sh
cargo run -- --mode rpc
```

Use `--session <path>` to isolate the append-only session log for a caller:

```sh
cargo run -- --mode rpc --session .piers/sessions/worker-a.jsonl
```

Use `--read-only` when an external agent should be allowed to inspect and run
ordinary prompts but must not self-rewrite or reload the guest:

```sh
cargo run -- --mode rpc --read-only --session .piers/sessions/worker-a.jsonl
```

Use `--require-approval` when reload/evolve should remain available but only
after an RPC caller marks that individual request as approved:

```sh
cargo run -- --mode rpc --require-approval --session .piers/sessions/worker-a.jsonl
```

Each stdin line is one command:

```json
{"type":"user_input","id":"turn-1","text":"read Cargo.toml"}
{"type":"provider_stream","id":"provider-turn-1","input":"hello","events":[{"type":"response_started","response_id":"r1"},{"type":"text_started","block_id":"t1"},{"type":"text_delta","block_id":"t1","text":"hello"},{"type":"text_finished","block_id":"t1"},{"type":"response_finished"}]}
{"type":"provider_prompt","id":"provider-live-1","input":"read Cargo.toml","max_iterations":8}
{"type":"provider_prompt_start","id":"provider-live-2","job_id":"provider-job-1","input":"read Cargo.toml","max_iterations":8}
{"type":"job_status","id":"provider-status-2","job_id":"provider-job-1"}
{"type":"status","id":"status-1"}
{"type":"provider_status","id":"provider-1"}
{"type":"manifest","id":"manifest-1"}
{"type":"list_sessions","id":"sessions-1"}
{"type":"session_entries","id":"entries-1","limit":20}
{"type":"switch_session","id":"switch-1","path":".piers/sessions/other.jsonl"}
{"type":"abort","id":"abort-1","job_id":"provider-job-1"}
{"type":"reload","id":"reload-1","approved":true}
{"type":"evolve","id":"evolve-1","spec":"answer more tersely","approved":true}
{"type":"quit","id":"stop"}
```

Each stdout line is an event, result, or error envelope. Event envelopes contain
the same `HarnessEvent` values that the REPL renders:

```json
{"type":"event","schema_version":1,"id":"turn-1","sequence":0,"event":{"type":"turn_started","turn_id":0,"generation":0,"phase_before_turn":{"phase":"idle"}}}
{"type":"result","schema_version":1,"id":"turn-1","sequence":5,"display":"..."}
```

The `id` is caller-chosen and is echoed back. `sequence` is stable within one
request so a caller can order streamed events before the terminal `result`.
`manifest` and `status` results include structured `data` for tool discovery
and the active session path. Manifest command entries include JSON input
schemas, mutation flags, and whether an `approved` field is supported. RPC
requests reject unknown fields, matching the manifest schemas'
`additionalProperties: false` contract. In `--read-only` mode, structured
results also report policy data showing that `reload` and `evolve` are
disabled. In `--require-approval` mode, unapproved `reload` and `evolve`
requests fail before the harness runs them.
`provider_status` reports that the current worker is in `guest_harness` mode
when no provider environment is configured. With complete OpenAI-compatible
environment variables, it reports the configured transport, retry policy, and
remaining production gaps.
Provider configuration is read from environment variables:

- `PIERS_PROVIDER`: provider adapter name; use `openai_compatible`
- `PIERS_MODEL`: model id to use once a network adapter is available
- `PIERS_BASE_URL`: OpenAI-compatible or local-provider base URL
- `PIERS_API_KEY`: credential presence check; the value is never printed in
  status output
- `PIERS_PROVIDER_TIMEOUT_MS`: optional per-request timeout in milliseconds

`provider_stream` is the adapter-facing bridge for that future transport: a
caller can submit one normalized provider response stream, and the harness will
record the user turn, persist assistant text, execute requested host tools, and
record provider failures as turn diagnostics. It does not perform network
transport itself; adapters normalize provider-specific responses into this
protocol.
`provider_script` runs that repeated loop with deterministic scripted provider
responses. It exists so local tests and external shims can prove the
tool-result handoff before a network provider adapter is configured.
`provider_prompt` runs the same loop against a configured OpenAI-compatible
chat-completions provider. It is intentionally non-streaming in this first
slice, but its responses are normalized into the same provider event protocol.
Provider HTTP requests use a bounded retry policy for transient HTTP statuses
and transport failures, plus a configured request timeout.
Provider adapters receive a cooperative cancellation token, and retry sleeps
check that token before continuing. `provider_script_start` and
`provider_prompt_start` move provider work into a background worker thread so
the line-oriented RPC process can accept `abort` with a `job_id` while provider
work is still running. Cancellation is still cooperative and cannot forcibly
stop a blocking HTTP call that is already inside the synchronous transport.
`list_sessions` returns known `.piers/sessions/*.jsonl` files with paths, entry
counts, and modification timestamps. `session_entries` returns parsed entries
from the active session log, optionally limited to the last `limit` entries.
`switch_session` changes the active session for the running RPC worker without
restarting the process. `abort` without a `job_id` targets the active
synchronous harness command and reports `accepted: false` when no active
operation can be cancelled. `abort` with a `job_id` targets an in-memory
background provider job.

Error envelopes include a stable `code` so callers do not need to scrape
messages:

```json
{
  "type": "error",
  "schema_version": 1,
  "id": "reload-1",
  "sequence": 0,
  "code": "approval_required",
  "message": "command requires approval"
}
```

Current error codes are `decode_request`, `invalid_command`, `read_only`,
`approval_required`, and `harness`.

## Evolution

For an evolve command:

```text
:evolve answer like a tiny rewritten Rust guest
```

the REPL submits `HarnessCommand::Evolve`. The host calls the guest's
`propose-update(spec)` export. The guest generates a new version of its own
source by reading its current source with:

```rust
include_str!("lib.rs")
```

Then it replaces only the line beginning with:

```rust
const BEHAVIOR: &str =
```

with a new Rust string literal built from the spec.

The host receives the generated source and writes it to a staging workspace
under:

```text
.piers/staged/
```

Before writing, the host does two minimal checks:

1. The generated source must still contain `wit_bindgen::generate!`.
1. The generated source must still contain `export!(PiersGuest);`.

This is not a serious security model. It is just enough guardrail for the kernel
to avoid accidentally writing something that is obviously not the guest
component.

## Reload

After staging generated guest source, the host reloads in this order:

1. Record `reload_proposed` with the spec and source digest.
1. Build the staged guest with Cargo for `wasm32-wasip2`.
1. Capture and record a versioned guest snapshot.
1. Instantiate the staged Wasm component.
1. Validate the staged guest manifest.
1. Call `restore(snapshot)` on the staged guest.
1. Run a smoke `handle-event` call against the staged guest.
1. Promote the staged source and artifact.
1. Replace the old guest instance in the host.
1. Record `reload_promoted` and notify the new guest with
   `host-event.reload-accepted`.

This is why you can see output like:

```text
piers> hello
echo inputs and count calls | call #1: hello
piers> :evolve say evolved things
evolved and reloaded generation 1
piers> world
say evolved things | call #2: world
```

The behavior changed, but the call count continued from `#1` to `#2`. That is
the central proof: code changed while state moved across the reload boundary.

## State

The current guest state is only the call counter:

```rust
thread_local! {
    static CALLS: RefCell<u64> = const { RefCell::new(0) };
}
```

The guest serializes it as versioned JSON-shaped text:

```json
{"version":1,"calls":1}
```

The host treats this as an opaque string. It does not understand the state
schema. It only moves the snapshot from the old guest into the new guest.

That ownership split matters. In a larger version, the host could own durable
resources such as files, tool registries, model clients, queues, or databases,
while the guest owns reloadable behavior.

## Failure Behavior

If a generated update does not compile, reload returns an error and the host
records a structured `reload_failed` session entry with the failed stage and
diagnostic message.

The intended model is that a bad guest update should not corrupt host state or
require rewriting the host binary. The previous guest remains active until the
staged guest builds, restores, smoke-tests, and promotes successfully.

## What This Proves

This proves a small but important Rust-first loop:

1. Runtime behavior is written in Rust.
1. Runtime behavior can generate replacement Rust code.
1. The host can compile that generated Rust to a reloadable artifact.
1. The host can swap that artifact without restarting itself.
1. Guest state can survive the swap through explicit snapshot and restore.

This is not a Rust VM, a Rust REPL, or native hotpatching. It is a reloadable
host/guest architecture that gives Rust code a narrow, explicit place where it
can rewrite and reload itself.

## Current Limits

The rewrite surface is intentionally tiny. The guest only rewrites its
`BEHAVIOR` constant. That keeps the first loop understandable and easy to
debug.

Generated code is staged before promotion. The remaining hardening work is a
promotion journal, replay fixtures, richer manifest validation, and startup
repair for interrupted promotion.

The host has no approval flow yet. Future host self-updates should be proposed
as patches, reviewed, and then applied through a controlled restart path.

The guest uses `wasm32-wasip2`, so not every Rust crate will work unchanged.
That is the cost of getting a clean reload boundary through Wasm.
