//! Reloadable Rust guest used by the Piers host PoC.
//!
//! The guest deliberately keeps its state and rewrite surface tiny: a call
//! counter plus one behavior string. That makes the host reload boundary easy
//! to inspect while still proving that Rust code can generate replacement Rust
//! code for itself.

use std::cell::RefCell;

wit_bindgen::generate!({
    path: "../wit",
    world: "guest",
});

/// Model-visible behavior string rewritten by `propose_update`.
const BEHAVIOR: &str = "answer like a tiny rewritten Rust guest";

thread_local! {
    static CALLS: RefCell<u64> = const { RefCell::new(0) };
}

struct PiersGuest;

impl Guest for PiersGuest {
    /// Handles ordinary host input and increments guest-local state.
    fn handle(input: String) -> String {
        let calls = CALLS.with(|calls| {
            let mut calls = calls.borrow_mut();
            *calls += 1;
            *calls
        });
        format!("{BEHAVIOR} | call #{calls}: {input}")
    }

    /// Generates replacement guest source by rewriting the behavior constant.
    fn propose_update(spec: String) -> String {
        render_source(spec.trim())
    }

    /// Serializes the guest-local call counter.
    fn snapshot() -> String {
        let calls = CALLS.with(|calls| *calls.borrow());
        format!(r#"{{"calls":{calls}}}"#)
    }

    /// Restores the guest-local call counter from a previous snapshot.
    fn restore(snapshot: String) {
        if let Some(calls) = parse_calls(&snapshot) {
            CALLS.with(|state| *state.borrow_mut() = calls);
        }
    }
}

export!(PiersGuest);

fn parse_calls(snapshot: &str) -> Option<u64> {
    let value = snapshot
        .trim()
        .strip_prefix(r#"{"calls":"#)?
        .strip_suffix('}')?;
    value.parse().ok()
}

fn render_source(spec: &str) -> String {
    let behavior = if spec.is_empty() {
        "handle inputs and count calls"
    } else {
        spec
    };
    let behavior_literal = format!("{behavior:?}");
    include_str!("lib.rs")
        .lines()
        .map(|line| {
            if line.starts_with("const BEHAVIOR: &str = ") {
                format!("const BEHAVIOR: &str = {behavior_literal};")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}
