//! Reloadable Rust guest used by the Piers host kernel.
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

use crate::piers::harness::protocol::{CommandDecl, DiagnosticEvent, ToolCallRequest};

/// Model-visible behavior string rewritten by `propose_update`.
///
/// Keeping the generated edit to one constant makes the reload path auditable:
/// any behavior change in this kernel should appear as a small Rust source diff.
const BEHAVIOR: &str = "answer like a tiny rewritten Rust guest";

thread_local! {
    /// Guest-owned state that must cross reloads through `snapshot` and `restore`.
    static CALLS: RefCell<u64> = const { RefCell::new(0) };
}

struct PiersGuest;

impl Guest for PiersGuest {
    /// Exposes stable metadata for host validation.
    fn manifest() -> GuestManifest {
        GuestManifest {
            version: "1".to_string(),
            name: "piers-default-guest".to_string(),
            capabilities: vec!["tool:read".to_string()],
            commands: vec![CommandDecl {
                name: "read-file".to_string(),
                description: "Request a host-owned read tool call.".to_string(),
            }],
        }
    }

    /// Handles typed host events and returns proposed guest events.
    fn handle_event(event: HostEvent) -> Vec<GuestEvent> {
        match event {
            HostEvent::UserInput(input) => handle_user_event(&input),
            HostEvent::ToolResult(result) => vec![GuestEvent::AssistantMessage(format!(
                "tool {} success={} output={}",
                result.call_id, result.success, result.output_json
            ))],
            HostEvent::ReloadAccepted(event) => vec![GuestEvent::Diagnostic(DiagnosticEvent {
                level: "info".to_string(),
                message: format!("reload accepted generation {}", event.generation),
            })],
        }
    }

    /// Generates replacement guest source by rewriting the behavior constant.
    ///
    /// The returned source is compiled in a staged workspace before it can
    /// replace the live guest.
    fn propose_update(spec: String) -> String {
        render_source(spec.trim())
    }

    /// Serializes the guest-local call counter.
    fn snapshot() -> String {
        let calls = CALLS.with(|calls| *calls.borrow());
        format!(r#"{{"version":1,"calls":{calls}}}"#)
    }

    /// Restores the guest-local call counter from a previous snapshot.
    ///
    /// Invalid snapshots are ignored in this kernel so an old or malformed
    /// guest state cannot crash reload. A production host would likely surface
    /// this as an explicit migration error instead.
    fn restore(snapshot: String) {
        if let Some(calls) = parse_calls(&snapshot) {
            CALLS.with(|state| *state.borrow_mut() = calls);
        }
    }
}

export!(PiersGuest);

/// Handles ordinary host input or emits a host-owned tool request.
fn handle_user_event(input: &str) -> Vec<GuestEvent> {
    let calls = increment_calls();
    if let Some(path) = input.strip_prefix("read ") {
        return vec![GuestEvent::ToolCall(ToolCallRequest {
            call_id: format!("read-{calls}"),
            tool_name: "read".to_string(),
            input_json: format!(r#"{{"path":{}}}"#, json_string(path.trim())),
        })];
    }

    vec![GuestEvent::AssistantMessage(format!(
        "{BEHAVIOR} | call #{calls}: {input}"
    ))]
}

/// Handles ordinary host input and increments guest-local state.
fn increment_calls() -> u64 {
    CALLS.with(|calls| {
        let mut calls = calls.borrow_mut();
        *calls += 1;
        *calls
    })
}

/// Encodes enough JSON string syntax for the guest's narrow tool request demo.
fn json_string(value: &str) -> String {
    let mut encoded = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => encoded.push_str("\\\""),
            '\\' => encoded.push_str("\\\\"),
            '\n' => encoded.push_str("\\n"),
            '\r' => encoded.push_str("\\r"),
            '\t' => encoded.push_str("\\t"),
            character => encoded.push(character),
        }
    }
    encoded.push('"');
    encoded
}

/// Parses the narrow snapshot format emitted by `snapshot`.
fn parse_calls(snapshot: &str) -> Option<u64> {
    let snapshot = snapshot.trim();
    let calls = snapshot.split(r#""calls":"#).nth(1)?.strip_suffix('}')?;
    calls.parse().ok()
}

/// Renders the next version of this guest source.
///
/// `include_str!("lib.rs")` embeds the source that was present when the live
/// guest artifact was built. After editing this file by hand, rebuild the guest
/// artifact before asking the running harness to evolve; otherwise the old
/// artifact will render its older embedded source.
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
