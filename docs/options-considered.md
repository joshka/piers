# Options Considered

This PoC uses a Rust host with a Rust guest compiled as a WebAssembly
component. The goal is to make behavior reloadable while keeping the host
stable enough to supervise rebuilds and recover from broken generated code.

## Wasm Guest

Chosen for the first version. Wasm gives the harness a stable host/guest
boundary without depending on Rust's native ABI. The component interface is
explicit, reload is just a new component instantiation, and the host can keep
the previous guest alive if a generated update does not compile.

The cost is extra boundary plumbing through WIT and Wasmtime, plus some
friction around crates that do not compile cleanly to `wasm32-wasip2`.

## Process Worker

A worker process would be the most robust and probably the fastest way to
build a useful tool. The host could compile and restart a native Rust worker
over JSONL or another local RPC protocol. Crash isolation would be excellent
and dependency compatibility would be closer to normal Rust.

The tradeoff is that it mostly proves restart orchestration. It is less like
loading new code into a running runtime.

## Native Hotpatch or Dylib Reload

This is the closest to the "Rust code changes under a running process"
feeling. Dioxus' hotpatching/Subsecond direction is worth watching for this
reason.

The tradeoff is that native Rust reload has sharp edges around ABI, crate
boundaries, static state, and platform behavior. It is a poor first boundary
for a tiny self-rewriting harness.

## Future Direction

The first PoC only rewrites guest code. The architecture should later allow
gated host patches: the guest or an agent can propose host changes, the host
can show the patch, and a human approval step can rebuild and restart the
harness. Direct host self-editing should wait until recovery, rollback, and
state handoff are explicit.
