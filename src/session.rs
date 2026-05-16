//! Durable host-owned session log.
//!
//! The session log is append-only JSONL owned by the native host. Reloadable
//! guest code can cause host facts to be recorded only through explicit host
//! operations; it never receives direct write access to session state.

use std::collections::{BTreeSet, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Schema version for new session entries.
pub const SESSION_SCHEMA_VERSION: u32 = 1;

/// Append-only durable log of host-owned session facts.
pub struct SessionLog {
    path: PathBuf,
    state: SessionState,
}

impl SessionLog {
    /// Opens or creates a session log and validates all existing entries.
    pub fn open(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs_err::create_dir_all(parent)
                .with_context(|| format!("create session directory {}", parent.display()))?;
        }

        let entries = if path.exists() {
            read_entries(&path)?
        } else {
            Vec::new()
        };
        let state = SessionState::from_entries(entries)?;

        Ok(Self { path, state })
    }

    /// Appends a host fact after validating it against replay state.
    pub fn append(&mut self, kind: SessionEntryKind) -> Result<SessionEntry> {
        let entry = SessionEntry::new(self.state.next_id(), kind)?;
        let mut next_state = self.state.clone();
        next_state.apply(&entry)?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("open session log {}", self.path.display()))?;
        serde_json::to_writer(&mut file, &entry).context("encode session entry")?;
        file.write_all(b"\n").context("write session newline")?;
        self.state = next_state;

        Ok(entry)
    }

    /// Returns the file backing this append-only session log.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// One durable host fact in replay order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionEntry {
    /// Stable monotonic id within this session file.
    pub id: u64,

    /// Previous entry id, if this is not the first entry.
    pub parent_id: Option<u64>,

    /// Session schema used to encode this entry.
    pub schema_version: u32,

    /// Milliseconds since the Unix epoch for coarse audit ordering.
    pub timestamp_ms: u128,

    /// Typed host fact.
    pub kind: SessionEntryKind,
}

impl SessionEntry {
    fn new(id: u64, kind: SessionEntryKind) -> Result<Self> {
        Ok(Self {
            id,
            parent_id: id.checked_sub(1),
            schema_version: SESSION_SCHEMA_VERSION,
            timestamp_ms: now_millis()?,
            kind,
        })
    }
}

/// Host-owned session facts persisted by the minimal harness.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEntryKind {
    TurnStarted {
        turn_id: u64,
        generation: u64,
    },
    TurnCompleted {
        turn_id: u64,
        success: bool,
    },
    UserMessage {
        text: String,
    },
    AssistantMessage {
        text: String,
    },
    ToolCall {
        call_id: String,
        tool_name: String,
        input_json: serde_json::Value,
    },
    ToolResult {
        call_id: String,
        output_json: serde_json::Value,
        success: bool,
    },
    ReloadProposed {
        spec: String,
        source_digest: String,
    },
    ReloadFailed {
        source_digest: Option<String>,
        stage: ReloadStage,
        message: String,
    },
    ReloadPromoted {
        generation: u64,
        source_digest: String,
        artifact_path: String,
    },
    GuestSnapshot {
        format_version: u32,
        payload: String,
    },
    Diagnostic {
        level: DiagnosticLevel,
        message: String,
    },
}

/// Reload phase attached to structured failure diagnostics.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReloadStage {
    Proposal,
    Build,
    Manifest,
    Snapshot,
    Restore,
    SmokeTest,
    Promotion,
}

/// Diagnostic severity for replayable host events.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
    Info,
    Warn,
    Error,
}

impl From<String> for DiagnosticLevel {
    fn from(value: String) -> Self {
        match value.as_str() {
            "error" => Self::Error,
            "warn" | "warning" => Self::Warn,
            _ => Self::Info,
        }
    }
}

#[derive(Clone, Default)]
struct SessionState {
    next_id: u64,
    pending_tool_calls: BTreeSet<String>,
    seen_entry_ids: HashSet<u64>,
    pending_reload_digest: Option<String>,
    latest_snapshot_id: Option<u64>,
    active_turn: Option<u64>,
}

impl SessionState {
    fn from_entries(entries: Vec<SessionEntry>) -> Result<Self> {
        let mut state = Self::default();
        for entry in entries {
            state.apply(&entry)?;
        }
        Ok(state)
    }

    fn next_id(&self) -> u64 {
        self.next_id
    }

    fn apply(&mut self, entry: &SessionEntry) -> Result<()> {
        self.validate_entry_shape(entry)?;
        self.apply_kind(entry)?;
        self.seen_entry_ids.insert(entry.id);
        self.next_id = entry
            .id
            .checked_add(1)
            .ok_or_else(|| anyhow!("session entry id overflow"))?;
        Ok(())
    }

    fn validate_entry_shape(&self, entry: &SessionEntry) -> Result<()> {
        if entry.schema_version != SESSION_SCHEMA_VERSION {
            bail!(
                "unsupported session schema version {} for entry {}",
                entry.schema_version,
                entry.id
            );
        }
        if self.seen_entry_ids.contains(&entry.id) {
            bail!("duplicate session entry id {}", entry.id);
        }
        if entry.id != self.next_id {
            bail!(
                "expected session entry id {}, found {}",
                self.next_id,
                entry.id
            );
        }
        let expected_parent = entry.id.checked_sub(1);
        if entry.parent_id != expected_parent {
            bail!(
                "entry {} parent mismatch: expected {:?}, found {:?}",
                entry.id,
                expected_parent,
                entry.parent_id
            );
        }
        Ok(())
    }

    fn apply_kind(&mut self, entry: &SessionEntry) -> Result<()> {
        match &entry.kind {
            SessionEntryKind::TurnStarted { turn_id, .. } => {
                if self.active_turn.replace(*turn_id).is_some() {
                    bail!("turn {turn_id} started while another turn is active");
                }
            }
            SessionEntryKind::TurnCompleted { turn_id, .. } => {
                if self.active_turn != Some(*turn_id) {
                    bail!("turn {turn_id} completed without matching active turn");
                }
                if !self.pending_tool_calls.is_empty() {
                    bail!("turn {turn_id} completed with pending tool calls");
                }
                self.active_turn = None;
            }
            SessionEntryKind::ToolCall { call_id, .. } => {
                if self.active_turn.is_none() {
                    bail!("tool call {call_id} was recorded outside an active turn");
                }
                if !self.pending_tool_calls.insert(call_id.clone()) {
                    bail!("tool call {call_id} is already pending");
                }
            }
            SessionEntryKind::ToolResult { call_id, .. } => {
                if self.active_turn.is_none() {
                    bail!("tool result {call_id} was recorded outside an active turn");
                }
                if !self.pending_tool_calls.remove(call_id) {
                    bail!("tool result {call_id} has no pending tool call");
                }
            }
            SessionEntryKind::ReloadProposed { source_digest, .. } => {
                if self.active_turn.is_some() {
                    bail!("reload proposal recorded during an active turn");
                }
                self.pending_reload_digest = Some(source_digest.clone());
                self.latest_snapshot_id = None;
            }
            SessionEntryKind::ReloadPromoted { source_digest, .. } => {
                if self.pending_reload_digest.as_ref() != Some(source_digest) {
                    bail!("reload promotion has no matching pending proposal");
                }
                if self.latest_snapshot_id.is_none() {
                    bail!("reload promotion requires a recorded guest snapshot");
                }
                self.pending_reload_digest = None;
            }
            SessionEntryKind::GuestSnapshot { format_version, .. } => {
                if *format_version == 0 {
                    bail!("guest snapshot format version must be nonzero");
                }
                self.latest_snapshot_id = Some(entry.id);
            }
            SessionEntryKind::ReloadFailed { source_digest, .. } => {
                if self.pending_reload_digest.as_ref() == source_digest.as_ref() {
                    self.pending_reload_digest = None;
                }
            }
            SessionEntryKind::UserMessage { .. }
            | SessionEntryKind::AssistantMessage { .. }
            | SessionEntryKind::Diagnostic { .. } => {}
        }
        Ok(())
    }
}

fn read_entries(path: &Path) -> Result<Vec<SessionEntry>> {
    let raw = fs_err::read_to_string(path)
        .with_context(|| format!("read session log {}", path.display()))?;
    raw.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line)
                .with_context(|| format!("parse session entry {} in {}", index + 1, path.display()))
        })
        .collect()
}

fn now_millis() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_millis())
}

/// Returns a stable SHA-256 digest for session entries that refer to source.
pub fn source_digest(source: &str) -> String {
    let digest = Sha256::digest(source.as_bytes());
    format!("{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64, kind: SessionEntryKind) -> SessionEntry {
        SessionEntry {
            id,
            parent_id: id.checked_sub(1),
            schema_version: SESSION_SCHEMA_VERSION,
            timestamp_ms: 1,
            kind,
        }
    }

    #[test]
    fn rejects_tool_result_without_pending_call() {
        let mut state = SessionState::default();
        let result = state.apply(&entry(
            0,
            SessionEntryKind::ToolResult {
                call_id: "call-1".to_string(),
                output_json: serde_json::json!({}),
                success: true,
            },
        ));

        assert!(result.is_err());
    }

    #[test]
    fn pairs_tool_call_and_result() -> Result<()> {
        let mut state = SessionState::default();
        state.apply(&entry(
            0,
            SessionEntryKind::TurnStarted {
                turn_id: 0,
                generation: 0,
            },
        ))?;
        state.apply(&entry(
            1,
            SessionEntryKind::ToolCall {
                call_id: "call-1".to_string(),
                tool_name: "read".to_string(),
                input_json: serde_json::json!({"path":"README.md"}),
            },
        ))?;
        state.apply(&entry(
            2,
            SessionEntryKind::ToolResult {
                call_id: "call-1".to_string(),
                output_json: serde_json::json!({"text":"ok"}),
                success: true,
            },
        ))?;
        state.apply(&entry(
            3,
            SessionEntryKind::TurnCompleted {
                turn_id: 0,
                success: true,
            },
        ))?;

        assert!(state.pending_tool_calls.is_empty());
        Ok(())
    }

    #[test]
    fn rejects_nested_turns() -> Result<()> {
        let mut state = SessionState::default();
        state.apply(&entry(
            0,
            SessionEntryKind::TurnStarted {
                turn_id: 0,
                generation: 0,
            },
        ))?;

        let result = state.apply(&entry(
            1,
            SessionEntryKind::TurnStarted {
                turn_id: 1,
                generation: 0,
            },
        ));

        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn rejects_tool_call_outside_turn() {
        let mut state = SessionState::default();

        let result = state.apply(&entry(
            0,
            SessionEntryKind::ToolCall {
                call_id: "call-1".to_string(),
                tool_name: "read".to_string(),
                input_json: serde_json::json!({"path":"README.md"}),
            },
        ));

        assert!(result.is_err());
    }

    #[test]
    fn rejects_turn_completion_with_pending_tool_call() -> Result<()> {
        let mut state = SessionState::default();
        state.apply(&entry(
            0,
            SessionEntryKind::TurnStarted {
                turn_id: 0,
                generation: 0,
            },
        ))?;
        state.apply(&entry(
            1,
            SessionEntryKind::ToolCall {
                call_id: "call-1".to_string(),
                tool_name: "read".to_string(),
                input_json: serde_json::json!({"path":"README.md"}),
            },
        ))?;

        let result = state.apply(&entry(
            2,
            SessionEntryKind::TurnCompleted {
                turn_id: 0,
                success: true,
            },
        ));

        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn rejects_reload_promotion_without_snapshot() {
        let mut state = SessionState::default();
        state
            .apply(&entry(
                0,
                SessionEntryKind::ReloadProposed {
                    spec: "try".to_string(),
                    source_digest: "digest".to_string(),
                },
            ))
            .unwrap();

        let result = state.apply(&entry(
            1,
            SessionEntryKind::ReloadPromoted {
                generation: 1,
                source_digest: "digest".to_string(),
                artifact_path: "guest.wasm".to_string(),
            },
        ));

        assert!(result.is_err());
    }

    #[test]
    fn rejects_reload_promotion_with_stale_snapshot() -> Result<()> {
        let mut state = SessionState::default();
        state.apply(&entry(
            0,
            SessionEntryKind::GuestSnapshot {
                format_version: 1,
                payload: "{}".to_string(),
            },
        ))?;
        state.apply(&entry(
            1,
            SessionEntryKind::ReloadProposed {
                spec: "try".to_string(),
                source_digest: "digest".to_string(),
            },
        ))?;

        let result = state.apply(&entry(
            2,
            SessionEntryKind::ReloadPromoted {
                generation: 1,
                source_digest: "digest".to_string(),
                artifact_path: "guest.wasm".to_string(),
            },
        ));

        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn allows_reload_after_proposal_and_snapshot() -> Result<()> {
        let mut state = SessionState::default();
        state.apply(&entry(
            0,
            SessionEntryKind::ReloadProposed {
                spec: "try".to_string(),
                source_digest: "digest".to_string(),
            },
        ))?;
        state.apply(&entry(
            1,
            SessionEntryKind::GuestSnapshot {
                format_version: 1,
                payload: "{}".to_string(),
            },
        ))?;
        state.apply(&entry(
            2,
            SessionEntryKind::ReloadPromoted {
                generation: 1,
                source_digest: "digest".to_string(),
                artifact_path: "guest.wasm".to_string(),
            },
        ))?;

        assert_eq!(state.pending_reload_digest, None);
        Ok(())
    }

    #[test]
    fn persists_and_reloads_entries() -> Result<()> {
        let temp = std::env::temp_dir().join(format!("piers-session-test-{}.jsonl", now_millis()?));
        let mut log = SessionLog::open(temp.clone())?;
        log.append(SessionEntryKind::UserMessage {
            text: "hello".to_string(),
        })?;
        log.append(SessionEntryKind::AssistantMessage {
            text: "hi".to_string(),
        })?;

        let reopened = SessionLog::open(temp.clone())?;
        assert_eq!(reopened.state.next_id(), 2);

        fs_err::remove_file(temp)?;
        Ok(())
    }
}
