# How Piers Works

Piers is a minimal proof of concept for a Rust-first, self-rewriting harness.
It is not trying to interpret Rust source directly. Instead, it splits the
program into a stable native host and a reloadable Rust guest compiled to a
WebAssembly component.

The important behavior is this loop:

1. Run a native Rust host.
1. Load a Rust guest compiled as Wasm.
1. Let the guest handle input.
1. Ask the guest to write replacement Rust code for itself.
1. Rebuild the guest Wasm artifact.
1. Swap the new guest into the still-running host.
1. Restore guest state into the new instance.

## Pieces

The host is [src/main.rs](../src/main.rs). It owns the REPL, filesystem writes,
guest rebuilds, Wasmtime engine, component linker, and reload lifecycle.

The guest is [guest/src/lib.rs](../guest/src/lib.rs). It is normal Rust code,
but it is compiled to `wasm32-wasip2` instead of the native machine target. The
guest owns the behavior that can be rewritten.

The interface is [wit/piers.wit](../wit/piers.wit). WIT defines the small
contract between host and guest:

```wit
export handle: func(input: string) -> string;
export propose-update: func(spec: string) -> string;
export snapshot: func() -> string;
export restore: func(snapshot: string);
```

The host and guest both generate bindings from this WIT file. This is the
stable reload boundary.

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

the host calls the guest's `handle(input)` export.

The current guest implementation increments a thread-local call counter and
returns:

```text
<behavior> | call #<n>: <input>
```

The `<behavior>` value comes from this guest constant:

```rust
const BEHAVIOR: &str = "...";
```

That line is the intentionally tiny rewrite target in the current PoC.

## Evolution

For an evolve command:

```text
:evolve answer like a tiny rewritten Rust guest
```

the host calls the guest's `propose-update(spec)` export. The guest generates a
new version of its own source by reading its current source with:

```rust
include_str!("lib.rs")
```

Then it replaces only the line beginning with:

```rust
const BEHAVIOR: &str =
```

with a new Rust string literal built from the spec.

The host receives the generated source and writes it to exactly:

```text
guest/src/lib.rs
```

Before writing, the host does two minimal checks:

1. The generated source must still contain `wit_bindgen::generate!`.
1. The generated source must still contain `export!(PiersGuest);`.

This is not a serious security model. It is just enough guardrail for the PoC
to avoid accidentally writing something that is obviously not the guest
component.

## Reload

After writing the generated guest source, the host reloads in this order:

1. Call `snapshot()` on the currently running guest.
1. Rebuild the guest with Cargo for `wasm32-wasip2`.
1. Instantiate the newly built Wasm component.
1. Call `restore(snapshot)` on the new guest.
1. Replace the old guest instance in the host.
1. Increment the host generation counter.

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

The guest serializes it as JSON-shaped text:

```json
{"calls":1}
```

The host treats this as an opaque string. It does not understand the state
schema. It only moves the snapshot from the old guest into the new guest.

That ownership split matters. In a larger version, the host could own durable
resources such as files, tool registries, model clients, queues, or databases,
while the guest owns reloadable behavior.

## Failure Behavior

If a generated update does not compile, reload returns an error and the host
records the failed rebuild in `last_rebuild`.

The intended model is that a bad guest update should not corrupt host state or
require rewriting the host binary. The previous guest remains the conceptual
fallback boundary. This PoC is still minimal, so the next improvement would be
to write generated code to a staging file first and only promote it after a
successful build.

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

The generated code is written directly to `guest/src/lib.rs`. A more serious
version should stage generated code, build from the staged source, and promote
it only after the new component passes checks.

The host has no approval flow yet. Future host self-updates should be proposed
as patches, reviewed, and then applied through a controlled restart path.

The guest uses `wasm32-wasip2`, so not every Rust crate will work unchanged.
That is the cost of getting a clean reload boundary through Wasm.
