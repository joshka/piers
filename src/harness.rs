//! Host harness that owns guest lifecycle and reload promotion.
//!
//! The harness is the stable native side of the experiment. It owns the
//! Wasmtime engine, the active guest instance, the guest state snapshot flow,
//! and the rule that generated code must build and pass a smoke test before it
//! replaces the live source and artifact.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};
use tracing::{debug, error, info};
use wasmtime::{Engine, Store};

use crate::guest_component::{
    Guest,
    piers::harness::protocol::{
        GuestEvent, GuestManifest, HostEvent, ReloadEvent, ToolResultEvent,
    },
};
use crate::provider::{
    CancellationToken, CompletedToolCall, ProviderAdapter, ProviderEvent, ProviderResponse,
    ProviderStreamReducer, ProviderTerminal, ProviderToolResult, ProviderTurnInput,
    ScriptedProvider, openai_compatible_provider_from_env, provider_runtime_status_json,
};
use crate::runtime::{HostState, create_engine, instantiate_artifact};
use crate::session::{ReloadStage, SessionEntryKind, SessionLog, source_digest as digest_source};
use crate::staging::{
    GUEST_ARTIFACT, GUEST_SOURCE, StagedGuest, build_guest_in, ensure_guest_artifact,
    promote_guest_artifact, promote_guest_source,
};
use crate::tools::{HostTools, ToolOutcome};

/// Stable native host for the reloadable Piers guest component.
///
/// A `Harness` keeps host-owned runtime resources alive while guest behavior is
/// rebuilt and replaced. The guest owns only state it can serialize through the
/// WIT `snapshot` export, so reloads can move that state into a new component
/// instance without preserving guest memory.
pub struct Harness {
    root: PathBuf,
    engine: Engine,
    store: Store<HostState>,
    guest: Guest,
    session: SessionLog,
    tools: HostTools,
    registry: GuestRegistry,
    phase: HarnessPhase,
    next_turn_id: u64,
    events: Vec<HarnessEvent>,
    active_cancellation: Option<CancellationToken>,
    generation: u64,
    last_reload: ReloadStatus,
}

/// Host-owned command surface shared by REPL, tests, and future RPC/worker modes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessCommand {
    UserInput(String),
    ProviderStream {
        input: String,
        events: Vec<ProviderEvent>,
    },
    ProviderScript {
        input: String,
        responses: Vec<Vec<ProviderEvent>>,
        max_iterations: usize,
    },
    ProviderPrompt {
        input: String,
        max_iterations: usize,
    },
    Evolve(String),
    Reload,
    Abort,
    Status,
    ProviderStatus,
    Manifest,
    ListSessions,
    SessionEntries {
        limit: Option<usize>,
    },
}

/// Result of applying a core command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutcome {
    pub display: Option<String>,
    pub data: Option<Value>,
    pub events: Vec<HarnessEvent>,
}

/// Observable host event emitted while applying one command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HarnessEvent {
    TurnStarted(TurnSnapshot),
    UserMessageRecorded { entry_id: u64 },
    AssistantMessage { text: String },
    ToolCallRequested { call_id: String, tool_name: String },
    ToolResultRecorded { call_id: String, success: bool },
    Diagnostic { level: String, message: String },
    ReloadStarted { target_generation: u64 },
    ReloadPromoted { generation: u64 },
    ReloadFailed { stage: ReloadStage, message: String },
    AbortRequested { accepted: bool, reason: String },
    TurnCompleted { turn_id: u64, success: bool },
}

/// Immutable host-owned facts captured for one user turn.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TurnSnapshot {
    pub turn_id: u64,
    pub generation: u64,
    pub phase_before_turn: HarnessPhase,
}

/// Current lifecycle phase of the harness core.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum HarnessPhase {
    Idle,
    InTurn { turn_id: u64 },
    ToolExecution { call_id: String },
    Reloading { target_generation: u64 },
}

/// Host-validated declarations from the active guest generation.
#[derive(Clone, Debug)]
pub struct GuestRegistry {
    pub generation: u64,
    pub manifest: GuestManifest,
}

struct ProviderStep {
    assistant_output: Vec<String>,
    tool_results: Vec<ProviderToolResult>,
}

impl Harness {
    /// Loads the current guest artifact and prepares the host runtime.
    ///
    /// If the artifact is missing, this builds `piers-guest` for the
    /// `wasm32-wasip2` target in `root` before instantiating it. The initial
    /// generation is `0`; later successful reloads increment it.
    pub fn load(root: PathBuf) -> Result<Self> {
        Self::load_with_session(root.clone(), root.join(".piers/sessions/default.jsonl"))
    }

    /// Loads the current guest artifact with an explicit session log path.
    pub fn load_with_session(root: PathBuf, session_path: PathBuf) -> Result<Self> {
        ensure_guest_artifact(&root)?;

        let engine = create_engine()?;
        let (mut store, guest) = instantiate_artifact(&engine, &root.join(GUEST_ARTIFACT))?;
        let manifest = validate_guest_manifest(&guest, &mut store)?;
        let session = SessionLog::open(session_path)?;
        let tools = HostTools::new(root.clone())?;

        Ok(Self {
            root,
            engine,
            store,
            guest,
            session,
            tools,
            registry: GuestRegistry {
                generation: 0,
                manifest,
            },
            phase: HarnessPhase::Idle,
            next_turn_id: 0,
            events: Vec::new(),
            active_cancellation: None,
            generation: 0,
            last_reload: ReloadStatus::InitialBuildOk,
        })
    }

    /// Returns the current guest generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the session log file backing this harness.
    pub fn session_path(&self) -> &std::path::Path {
        self.session.path()
    }

    /// Drains currently buffered command events.
    pub fn drain_events(&mut self) -> Vec<HarnessEvent> {
        std::mem::take(&mut self.events)
    }

    /// Applies one UI/RPC/worker command to the harness core.
    ///
    /// This is the Pi-like boundary the line REPL uses. Terminal commands are
    /// parsed outside the core, then submitted as typed commands. The returned
    /// events are the observable facts another client would render, stream, or
    /// record in a test fixture.
    pub fn execute(&mut self, command: HarnessCommand) -> Result<CommandOutcome> {
        self.events.clear();
        let (display, data) = match command {
            HarnessCommand::UserInput(input) => (Some(self.handle(&input)?), None),
            HarnessCommand::ProviderStream { input, events } => {
                (Some(self.handle_provider_stream(&input, events)?), None)
            }
            HarnessCommand::ProviderScript {
                input,
                responses,
                max_iterations,
            } => {
                let mut provider = ScriptedProvider::new(responses);
                (
                    Some(self.handle_provider_loop(&input, &mut provider, max_iterations)?),
                    Some(json!({
                        "provider_requests": provider.requests().len(),
                    })),
                )
            }
            HarnessCommand::ProviderPrompt {
                input,
                max_iterations,
            } => {
                let mut provider = openai_compatible_provider_from_env()?;
                (
                    Some(self.handle_provider_loop(&input, &mut provider, max_iterations)?),
                    None,
                )
            }
            HarnessCommand::Evolve(spec) => {
                self.evolve(&spec)?;
                (
                    Some(format!(
                        "evolved and reloaded generation {}",
                        self.generation()
                    )),
                    Some(json!({ "generation": self.generation() })),
                )
            }
            HarnessCommand::Reload => {
                self.reload()?;
                (
                    Some(format!("reloaded generation {}", self.generation())),
                    Some(json!({ "generation": self.generation() })),
                )
            }
            HarnessCommand::Abort => {
                let data = self.abort_data();
                (Some("nothing to abort".to_string()), Some(data))
            }
            HarnessCommand::Status => (Some(self.status()), Some(self.status_data())),
            HarnessCommand::ProviderStatus => {
                let data = self.provider_status_data();
                (Some(format_provider_status(&data)), Some(data))
            }
            HarnessCommand::ListSessions => {
                let data = self.list_sessions_data()?;
                (Some(format_session_list(&data)), Some(data))
            }
            HarnessCommand::SessionEntries { limit } => {
                let data = self.session_entries_data(limit)?;
                (Some(format_session_entries(&data)), Some(data))
            }
            HarnessCommand::Manifest => {
                let data = self.manifest_data();
                (Some(serde_json::to_string_pretty(&data)?), Some(data))
            }
        };
        Ok(CommandOutcome {
            display,
            data,
            events: std::mem::take(&mut self.events),
        })
    }

    /// Sends ordinary user input to the active guest.
    ///
    /// This call can mutate guest-local state. The current guest increments
    /// a call counter, which lets reload tests verify that state handoff is
    /// working.
    pub fn handle(&mut self, input: &str) -> Result<String> {
        let snapshot = self.begin_turn(input)?;
        let result = self.handle_inner(input);
        self.finish_turn(snapshot.turn_id, result.is_ok())?;
        result
    }

    /// Handles one normalized provider response stream as a host-owned turn.
    ///
    /// This is the bridge from the provider reducer into the same durable
    /// session and tool machinery used by the reloadable guest. Real adapters
    /// can feed normalized provider events here once transport and model
    /// selection are configured.
    pub fn handle_provider_stream(
        &mut self,
        input: &str,
        events: Vec<ProviderEvent>,
    ) -> Result<String> {
        let snapshot = self.begin_turn(input)?;
        let result = self.handle_provider_stream_inner(input, events);
        self.finish_turn(snapshot.turn_id, result.is_ok())?;
        result
    }

    /// Runs a provider-backed tool loop until the provider emits no tool calls.
    pub fn handle_provider_loop(
        &mut self,
        input: &str,
        provider: &mut impl ProviderAdapter,
        max_iterations: usize,
    ) -> Result<String> {
        self.handle_provider_loop_with_cancellation(input, provider, max_iterations, None)
    }

    /// Runs a provider-backed tool loop with an externally-owned cancellation token.
    pub fn handle_provider_loop_with_cancellation(
        &mut self,
        input: &str,
        provider: &mut impl ProviderAdapter,
        max_iterations: usize,
        cancellation: Option<CancellationToken>,
    ) -> Result<String> {
        let snapshot = self.begin_turn(input)?;
        if let Some(cancellation) = cancellation {
            self.active_cancellation = Some(cancellation);
        }
        let result = self.handle_provider_loop_inner(input, provider, max_iterations);
        self.finish_turn(snapshot.turn_id, result.is_ok())?;
        result
    }

    fn handle_inner(&mut self, input: &str) -> Result<String> {
        debug!(
            generation = self.generation,
            "handling input with active guest"
        );
        let entry = self.session.append(SessionEntryKind::UserMessage {
            text: input.to_string(),
        })?;
        self.emit(HarnessEvent::UserMessageRecorded { entry_id: entry.id });
        let events = self
            .guest
            .call_handle_event(&mut self.store, &HostEvent::UserInput(input.to_string()))
            .map_err(|error| anyhow!("call guest handle-event: {error:#}"))?;
        let output = self.record_guest_events(events)?;
        Ok(output)
    }

    fn handle_provider_stream_inner(
        &mut self,
        input: &str,
        events: Vec<ProviderEvent>,
    ) -> Result<String> {
        debug!(
            input_len = input.len(),
            "handling input with provider stream"
        );
        let entry = self.session.append(SessionEntryKind::UserMessage {
            text: input.to_string(),
        })?;
        self.emit(HarnessEvent::UserMessageRecorded { entry_id: entry.id });

        let response = reduce_provider_events(events)?;
        let step = self.record_provider_response(response)?;
        Ok(step.assistant_output.join("\n"))
    }

    fn handle_provider_loop_inner(
        &mut self,
        input: &str,
        provider: &mut impl ProviderAdapter,
        max_iterations: usize,
    ) -> Result<String> {
        if max_iterations == 0 {
            bail!("provider loop max_iterations must be greater than zero");
        }

        debug!(input_len = input.len(), "handling input with provider loop");
        let entry = self.session.append(SessionEntryKind::UserMessage {
            text: input.to_string(),
        })?;
        self.emit(HarnessEvent::UserMessageRecorded { entry_id: entry.id });

        let mut tool_results = Vec::new();
        let mut assistant_output = Vec::new();
        for iteration in 0..max_iterations {
            let cancellation = self.active_cancellation.clone().unwrap_or_default();
            cancellation.ensure_not_cancelled()?;
            let events = provider.next_response(
                ProviderTurnInput {
                    user_input: input.to_string(),
                    tool_results,
                },
                &cancellation,
            )?;
            cancellation.ensure_not_cancelled()?;
            let response = reduce_provider_events(events)?;
            let step = self.record_provider_response(response)?;
            assistant_output.extend(step.assistant_output);
            if step.tool_results.is_empty() {
                return Ok(assistant_output.join("\n"));
            }
            tool_results = step.tool_results;
            debug!(iteration, "provider loop continuing after tool calls");
        }
        bail!("provider loop exceeded {max_iterations} iterations")
    }

    /// Asks the active guest to generate replacement source and reloads it.
    ///
    /// The live source and artifact are not replaced until the generated source
    /// has been staged, built, restored from the active snapshot, and smoke
    /// tested. If any step fails, the active guest instance keeps running and
    /// the session log records a structured reload failure.
    pub fn evolve(&mut self, spec: &str) -> Result<()> {
        self.ensure_idle()?;
        if spec.is_empty() {
            bail!("usage: :evolve <spec>");
        }

        self.phase = HarnessPhase::Reloading {
            target_generation: self.generation + 1,
        };
        let result = self.evolve_inner(spec);
        self.phase = HarnessPhase::Idle;
        result
    }

    fn evolve_inner(&mut self, spec: &str) -> Result<()> {
        info!(generation = self.generation, "requesting guest evolution");
        self.emit(HarnessEvent::ReloadStarted {
            target_generation: self.generation + 1,
        });
        let next_source = self
            .guest
            .call_propose_update(&mut self.store, spec)
            .map_err(|error| anyhow!("ask guest for update: {error:#}"))?;
        let digest = digest_source(&next_source);
        self.session.append(SessionEntryKind::ReloadProposed {
            spec: spec.to_string(),
            source_digest: digest.clone(),
        })?;

        match self.reload_from_source(&next_source, &digest) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.record_reload_failure(Some(digest), &error)?;
                error!(error = %error, "guest evolution failed");
                Err(error.error)
            }
        }
    }

    /// Reloads from the current live guest source and artifact.
    ///
    /// This is a rebuild of the checked-in guest source, not a generated-source
    /// promotion. The current guest is snapshotted before the rebuild and the
    /// snapshot is restored into a fresh instance from the rebuilt artifact.
    pub fn reload(&mut self) -> Result<()> {
        self.ensure_idle()?;
        self.phase = HarnessPhase::Reloading {
            target_generation: self.generation + 1,
        };
        let result = self.reload_inner();
        self.phase = HarnessPhase::Idle;
        result
    }

    fn reload_inner(&mut self) -> Result<()> {
        self.emit(HarnessEvent::ReloadStarted {
            target_generation: self.generation + 1,
        });
        let source_path = self.root.join(GUEST_SOURCE);
        let source = fs_err::read_to_string(&source_path)
            .with_context(|| format!("read live guest source {}", source_path.display()))?;
        let digest = digest_source(&source);
        self.session.append(SessionEntryKind::ReloadProposed {
            spec: "manual reload".to_string(),
            source_digest: digest.clone(),
        })?;

        let snapshot = match self.snapshot_active_guest() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let error = ReloadAttemptError::new(ReloadStage::Snapshot, error);
                self.record_reload_failure(Some(digest), &error)?;
                return Err(error.error);
            }
        };

        if let Err(error) = build_guest_in(&self.root) {
            let error = ReloadAttemptError::new(ReloadStage::Build, error);
            self.record_reload_failure(Some(digest), &error)?;
            return Err(error.error);
        }
        let (mut next_store, next_guest) =
            match instantiate_artifact(&self.engine, &self.root.join(GUEST_ARTIFACT)) {
                Ok(next) => next,
                Err(error) => {
                    let error = ReloadAttemptError::new(ReloadStage::Build, error);
                    self.record_reload_failure(Some(digest), &error)?;
                    return Err(error.error);
                }
            };
        if let Err(error) = validate_guest_manifest(&next_guest, &mut next_store) {
            let error = ReloadAttemptError::new(ReloadStage::Manifest, error);
            self.record_reload_failure(Some(digest), &error)?;
            return Err(error.error);
        }
        if let Err(error) = next_guest
            .call_restore(&mut next_store, &snapshot)
            .map_err(|error| anyhow!("restore guest snapshot: {error:#}"))
        {
            let error = ReloadAttemptError::new(ReloadStage::Restore, error);
            self.record_reload_failure(Some(digest), &error)?;
            return Err(error.error);
        }

        self.store = next_store;
        self.guest = next_guest;
        self.generation += 1;
        self.refresh_guest_registry()?;
        self.record_reload_promoted(digest, self.root.join(GUEST_ARTIFACT))?;
        self.notify_reload_accepted()?;
        info!(generation = self.generation, "reloaded live guest artifact");
        Ok(())
    }

    /// Returns human-readable host status for the line REPL.
    pub fn status(&self) -> String {
        format!(
            "generation: {}\nguest: {} (registry generation {})\nartifact: {}\nlast rebuild: {}",
            self.generation,
            self.registry.manifest.name,
            self.registry.generation,
            GUEST_ARTIFACT,
            self.last_reload
        )
    }

    /// Returns machine-readable host, guest, and reload status.
    pub fn status_data(&self) -> Value {
        json!({
            "session": self.session_path().display().to_string(),
            "generation": self.generation,
            "guest": {
                "name": self.registry.manifest.name,
                "version": self.registry.manifest.version,
                "registry_generation": self.registry.generation,
            },
            "artifact": GUEST_ARTIFACT,
            "last_reload": self.last_reload.to_string(),
        })
    }

    /// Returns machine-readable provider/model status.
    pub fn provider_status_data(&self) -> Value {
        provider_runtime_status_json()
    }

    /// Returns the agent-facing manifest for command and capability discovery.
    pub fn manifest_data(&self) -> Value {
        let guest_commands = self
            .registry
            .manifest
            .commands
            .iter()
            .map(|command| {
                json!({
                    "name": command.name,
                    "description": command.description,
                })
            })
            .collect::<Vec<_>>();
        json!({
            "protocol_version": 1,
            "session": self.session_path().display().to_string(),
            "commands": rpc_command_manifest(),
            "guest": {
                "generation": self.generation,
                "name": self.registry.manifest.name,
                "version": self.registry.manifest.version,
                "capabilities": self.registry.manifest.capabilities,
                "commands": guest_commands,
            },
            "tools": crate::tools::builtin_tool_definitions()
                .into_iter()
                .map(|tool| {
                    let capability = format!("tool:{}", tool.name);
                    let enabled = self
                        .registry
                        .manifest
                        .capabilities
                        .iter()
                        .any(|declared| declared == &capability);
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.input_schema,
                        "capability": capability,
                        "enabled_for_guest": enabled,
                    })
                })
                .collect::<Vec<_>>(),
        })
    }

    /// Returns machine-readable summaries of known session logs.
    pub fn list_sessions_data(&self) -> Result<Value> {
        let sessions_dir = self.root.join(".piers/sessions");
        let mut sessions = Vec::new();
        if sessions_dir.exists() {
            for entry in fs_err::read_dir(&sessions_dir)
                .with_context(|| format!("read session dir {}", sessions_dir.display()))?
            {
                let entry = entry?;
                let path = entry.path();
                if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl")
                {
                    continue;
                }
                sessions.push(session_summary(&path)?);
            }
        }
        sessions.sort_by(|left, right| {
            right["modified_ms"]
                .as_u64()
                .cmp(&left["modified_ms"].as_u64())
                .then_with(|| left["path"].as_str().cmp(&right["path"].as_str()))
        });
        Ok(json!({
            "sessions_dir": sessions_dir.display().to_string(),
            "active_session": self.session_path().display().to_string(),
            "sessions": sessions,
        }))
    }

    /// Returns entries from the active session log.
    pub fn session_entries_data(&self, limit: Option<usize>) -> Result<Value> {
        let text = fs_err::read_to_string(self.session_path())
            .with_context(|| format!("read session {}", self.session_path().display()))?;
        let mut entries = Vec::new();
        for (index, line) in text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .enumerate()
        {
            let entry: Value = serde_json::from_str(line).with_context(|| {
                format!(
                    "parse session entry {} in {}",
                    index + 1,
                    self.session_path().display()
                )
            })?;
            entries.push(entry);
        }
        let total_entries = entries.len();
        let start = limit
            .map(|limit| total_entries.saturating_sub(limit))
            .unwrap_or(0);
        let entries = entries.into_iter().skip(start).collect::<Vec<_>>();
        Ok(json!({
            "session": self.session_path().display().to_string(),
            "total_entries": total_entries,
            "returned_entries": entries.len(),
            "start_index": start,
            "entries": entries,
        }))
    }

    /// Requests cancellation of active work.
    ///
    /// Work is synchronous today, so this reports that no active operation can
    /// be aborted. The command and event shape are present for future provider
    /// and tool cancellation support.
    pub fn abort_data(&mut self) -> Value {
        let accepted = !matches!(self.phase, HarnessPhase::Idle);
        let reason = if accepted {
            if let Some(cancellation) = &self.active_cancellation {
                cancellation.cancel();
            }
            "abort requested".to_string()
        } else {
            "no active operation".to_string()
        };
        self.emit(HarnessEvent::AbortRequested {
            accepted,
            reason: reason.clone(),
        });
        json!({
            "accepted": accepted,
            "reason": reason,
            "phase": self.phase,
        })
    }

    /// Builds and promotes generated guest source as one reload attempt.
    ///
    /// Promotion is intentionally last. The staged artifact is instantiated
    /// twice: once for smoke testing and once for the new live guest. That keeps
    /// smoke-test mutations out of the promoted guest state.
    fn reload_from_source(
        &mut self,
        source: &str,
        digest: &str,
    ) -> std::result::Result<(), ReloadAttemptError> {
        let next_generation = self.generation + 1;
        let attempt = StagedGuest::create(&self.root, source, next_generation)
            .map_err(|error| ReloadAttemptError::new(ReloadStage::Proposal, error))?;
        build_guest_in(&attempt.root)
            .map_err(|error| ReloadAttemptError::new(ReloadStage::Build, error))?;

        let snapshot = self
            .snapshot_active_guest()
            .map_err(|error| ReloadAttemptError::new(ReloadStage::Snapshot, error))?;

        self.smoke_test_staged_guest(&attempt, &snapshot)
            .map_err(|error| ReloadAttemptError::new(ReloadStage::SmokeTest, error))?;
        let (mut next_store, next_guest) = instantiate_artifact(&self.engine, &attempt.artifact)
            .map_err(|error| ReloadAttemptError::new(ReloadStage::Build, error))?;
        next_guest
            .call_restore(&mut next_store, &snapshot)
            .map_err(|error| anyhow!("restore promoted guest snapshot: {error:#}"))
            .map_err(|error| ReloadAttemptError::new(ReloadStage::Restore, error))?;

        promote_guest_source(&self.root, source)
            .map_err(|error| ReloadAttemptError::new(ReloadStage::Promotion, error))?;
        promote_guest_artifact(&self.root, &attempt.artifact)
            .map_err(|error| ReloadAttemptError::new(ReloadStage::Promotion, error))?;

        self.store = next_store;
        self.guest = next_guest;
        self.generation = next_generation;
        self.refresh_guest_registry()
            .map_err(|error| ReloadAttemptError::new(ReloadStage::Manifest, error))?;
        self.record_reload_promoted(digest.to_string(), attempt.artifact)
            .map_err(|error| ReloadAttemptError::new(ReloadStage::Promotion, error))?;
        self.notify_reload_accepted()
            .map_err(|error| ReloadAttemptError::new(ReloadStage::Promotion, error))?;
        info!(generation = self.generation, "promoted staged guest");
        Ok(())
    }

    /// Captures guest-owned state before a reload boundary is crossed.
    fn snapshot_active_guest(&mut self) -> Result<String> {
        let snapshot = self
            .guest
            .call_snapshot(&mut self.store)
            .map_err(|error| anyhow!("snapshot current guest: {error:#}"))?;
        self.session.append(SessionEntryKind::GuestSnapshot {
            format_version: 1,
            payload: snapshot.clone(),
        })?;
        Ok(snapshot)
    }

    /// Verifies that a staged guest can accept the current state and run once.
    fn smoke_test_staged_guest(&self, attempt: &StagedGuest, snapshot: &str) -> Result<()> {
        let (mut smoke_store, smoke_guest) = instantiate_artifact(&self.engine, &attempt.artifact)?;
        validate_guest_manifest(&smoke_guest, &mut smoke_store)
            .map_err(|error| anyhow!("validate staged guest manifest: {error:#}"))?;
        smoke_guest
            .call_restore(&mut smoke_store, snapshot)
            .map_err(|error| anyhow!("restore staged guest snapshot: {error:#}"))?;
        smoke_guest
            .call_handle_event(
                &mut smoke_store,
                &HostEvent::UserInput("__piers_reload_smoke__".to_string()),
            )
            .map_err(|error| anyhow!("smoke test staged guest: {error:#}"))?;
        debug!(artifact = %attempt.artifact.display(), "staged guest smoke test passed");
        Ok(())
    }

    fn record_guest_events(&mut self, events: Vec<GuestEvent>) -> Result<String> {
        let mut assistant_output = Vec::new();
        self.record_guest_events_inner(events, &mut assistant_output, 0)?;

        if assistant_output.is_empty() {
            bail!("guest did not emit an assistant message");
        }
        Ok(assistant_output.join("\n"))
    }

    fn record_guest_events_inner(
        &mut self,
        events: Vec<GuestEvent>,
        assistant_output: &mut Vec<String>,
        depth: usize,
    ) -> Result<()> {
        if depth > 16 {
            bail!("guest event loop exceeded recursion limit");
        }

        for event in events {
            match event {
                GuestEvent::AssistantMessage(text) => {
                    self.session
                        .append(SessionEntryKind::AssistantMessage { text: text.clone() })?;
                    self.emit(HarnessEvent::AssistantMessage { text: text.clone() });
                    assistant_output.push(text);
                }
                GuestEvent::Diagnostic(diagnostic) => {
                    let level = diagnostic.level;
                    let message = diagnostic.message;
                    self.session.append(SessionEntryKind::Diagnostic {
                        level: level.clone().into(),
                        message: message.clone(),
                    })?;
                    self.emit(HarnessEvent::Diagnostic { level, message });
                }
                GuestEvent::ToolCall(request) => {
                    let input_json: serde_json::Value = serde_json::from_str(&request.input_json)
                        .with_context(|| {
                        format!("parse guest tool call {} input", request.call_id)
                    })?;
                    self.session.append(SessionEntryKind::ToolCall {
                        call_id: request.call_id.clone(),
                        tool_name: request.tool_name.clone(),
                        input_json: input_json.clone(),
                    })?;
                    let tool_name = request.tool_name.clone();
                    self.emit(HarnessEvent::ToolCallRequested {
                        call_id: request.call_id.clone(),
                        tool_name: tool_name.clone(),
                    });
                    let previous_phase = std::mem::replace(
                        &mut self.phase,
                        HarnessPhase::ToolExecution {
                            call_id: request.call_id.clone(),
                        },
                    );
                    let outcome = self.execute_guest_tool(&tool_name, input_json);
                    self.phase = previous_phase;
                    let call_id = request.call_id;
                    let output_json = outcome.output_json;
                    let success = outcome.success;
                    self.session.append(SessionEntryKind::ToolResult {
                        call_id: call_id.clone(),
                        output_json: output_json.clone(),
                        success,
                    })?;
                    self.emit(HarnessEvent::ToolResultRecorded {
                        call_id: call_id.clone(),
                        success,
                    });
                    let follow_up_events = self
                        .guest
                        .call_handle_event(
                            &mut self.store,
                            &HostEvent::ToolResult(ToolResultEvent {
                                call_id,
                                output_json: output_json.to_string(),
                                success,
                            }),
                        )
                        .map_err(|error| anyhow!("call guest with tool result: {error:#}"))?;
                    self.record_guest_events_inner(follow_up_events, assistant_output, depth + 1)?;
                }
                GuestEvent::ProposeSourceUpdate(source) => {
                    let source_digest = digest_source(&source);
                    self.session.append(SessionEntryKind::ReloadProposed {
                        spec: "guest event source update".to_string(),
                        source_digest,
                    })?;
                }
            }
        }
        Ok(())
    }

    fn record_provider_response(&mut self, response: ProviderResponse) -> Result<ProviderStep> {
        match response.terminal {
            ProviderTerminal::Finished => {}
            ProviderTerminal::Failed { message } => {
                self.record_provider_terminal_diagnostic("error", &message)?;
                bail!("provider response failed: {message}");
            }
            ProviderTerminal::Aborted { reason } => {
                self.record_provider_terminal_diagnostic("warn", &reason)?;
                bail!("provider response aborted: {reason}");
            }
            ProviderTerminal::Incomplete { message } => {
                self.record_provider_terminal_diagnostic("error", &message)?;
                bail!("provider stream incomplete: {message}");
            }
        }

        if response.text.is_empty() && response.tool_calls.is_empty() {
            bail!("provider response did not emit text or tool calls");
        }

        let mut assistant_output = Vec::new();
        if !response.text.is_empty() {
            self.session.append(SessionEntryKind::AssistantMessage {
                text: response.text.clone(),
            })?;
            self.emit(HarnessEvent::AssistantMessage {
                text: response.text.clone(),
            });
            assistant_output.push(response.text);
        }

        let mut tool_results = Vec::new();
        for call in response.tool_calls {
            tool_results.push(self.record_provider_tool_call(call)?);
        }

        Ok(ProviderStep {
            assistant_output,
            tool_results,
        })
    }

    fn record_provider_tool_call(&mut self, call: CompletedToolCall) -> Result<ProviderToolResult> {
        let input_json: Value = serde_json::from_str(&call.arguments_json)
            .with_context(|| format!("parse provider tool call {} input", call.call_id))?;
        self.session.append(SessionEntryKind::ToolCall {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
            input_json: input_json.clone(),
        })?;
        self.emit(HarnessEvent::ToolCallRequested {
            call_id: call.call_id.clone(),
            tool_name: call.tool_name.clone(),
        });

        let previous_phase = std::mem::replace(
            &mut self.phase,
            HarnessPhase::ToolExecution {
                call_id: call.call_id.clone(),
            },
        );
        let outcome = self.tools.execute(&call.tool_name, input_json);
        self.phase = previous_phase;

        let output_json = outcome.output_json;
        let success = outcome.success;
        self.session.append(SessionEntryKind::ToolResult {
            call_id: call.call_id.clone(),
            output_json: output_json.clone(),
            success,
        })?;
        self.emit(HarnessEvent::ToolResultRecorded {
            call_id: call.call_id.clone(),
            success,
        });
        Ok(ProviderToolResult {
            call_id: call.call_id,
            output_json,
            success,
        })
    }

    fn record_provider_terminal_diagnostic(&mut self, level: &str, message: &str) -> Result<()> {
        self.session.append(SessionEntryKind::Diagnostic {
            level: level.to_string().into(),
            message: message.to_string(),
        })?;
        self.emit(HarnessEvent::Diagnostic {
            level: level.to_string(),
            message: message.to_string(),
        });
        Ok(())
    }

    fn notify_reload_accepted(&mut self) -> Result<()> {
        let events = self
            .guest
            .call_handle_event(
                &mut self.store,
                &HostEvent::ReloadAccepted(ReloadEvent {
                    generation: self.generation,
                }),
            )
            .map_err(|error| anyhow!("notify guest of accepted reload: {error:#}"))?;
        let mut assistant_output = Vec::new();
        self.record_guest_events_inner(events, &mut assistant_output, 0)
    }

    fn execute_guest_tool(&self, tool_name: &str, input_json: Value) -> ToolOutcome {
        let capability = format!("tool:{tool_name}");
        if !self
            .registry
            .manifest
            .capabilities
            .iter()
            .any(|declared| declared == &capability)
        {
            return ToolOutcome {
                success: false,
                output_json: json!({
                    "error": format!("guest capability {capability} is not declared")
                }),
            };
        }
        self.tools.execute(tool_name, input_json)
    }

    fn refresh_guest_registry(&mut self) -> Result<()> {
        let manifest = validate_guest_manifest(&self.guest, &mut self.store)?;
        self.registry = GuestRegistry {
            generation: self.generation,
            manifest,
        };
        Ok(())
    }

    fn record_reload_promoted(&mut self, digest: String, artifact_path: PathBuf) -> Result<()> {
        self.session.append(SessionEntryKind::ReloadPromoted {
            generation: self.generation,
            source_digest: digest,
            artifact_path: artifact_path.display().to_string(),
        })?;
        self.emit(HarnessEvent::ReloadPromoted {
            generation: self.generation,
        });
        self.last_reload = ReloadStatus::Ok {
            generation: self.generation,
        };
        Ok(())
    }

    fn record_reload_failure(
        &mut self,
        digest: Option<String>,
        error: &ReloadAttemptError,
    ) -> Result<()> {
        self.session.append(SessionEntryKind::ReloadFailed {
            source_digest: digest,
            stage: error.stage.clone(),
            message: format!("{:#}", error.error),
        })?;
        self.emit(HarnessEvent::ReloadFailed {
            stage: error.stage.clone(),
            message: format!("{:#}", error.error),
        });
        self.last_reload = ReloadStatus::Failed {
            stage: error.stage.clone(),
            message: format!("{:#}", error.error),
        };
        Ok(())
    }

    fn begin_turn(&mut self, input: &str) -> Result<TurnSnapshot> {
        self.ensure_idle()?;
        let snapshot = TurnSnapshot {
            turn_id: self.next_turn_id,
            generation: self.generation,
            phase_before_turn: self.phase.clone(),
        };
        self.next_turn_id += 1;
        self.phase = HarnessPhase::InTurn {
            turn_id: snapshot.turn_id,
        };
        self.active_cancellation = Some(CancellationToken::default());
        self.session.append(SessionEntryKind::TurnStarted {
            turn_id: snapshot.turn_id,
            generation: snapshot.generation,
        })?;
        debug!(
            turn_id = snapshot.turn_id,
            generation = snapshot.generation,
            input_len = input.len(),
            "starting harness turn"
        );
        self.emit(HarnessEvent::TurnStarted(snapshot.clone()));
        Ok(snapshot)
    }

    fn finish_turn(&mut self, turn_id: u64, success: bool) -> Result<()> {
        self.phase = HarnessPhase::Idle;
        self.active_cancellation = None;
        self.session
            .append(SessionEntryKind::TurnCompleted { turn_id, success })?;
        self.emit(HarnessEvent::TurnCompleted { turn_id, success });
        Ok(())
    }

    fn ensure_idle(&self) -> Result<()> {
        if !matches!(self.phase, HarnessPhase::Idle) {
            bail!("harness is busy: {:?}", self.phase);
        }
        Ok(())
    }

    fn emit(&mut self, event: HarnessEvent) {
        self.events.push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ScriptedProvider;

    #[test]
    fn invalid_generated_guest_does_not_replace_active_guest() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-reload-failure-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let original_source = fs_err::read_to_string(root.join(GUEST_SOURCE))?;
        let mut harness = Harness::load_with_session(root.clone(), session_path.clone())?;
        assert!(harness.handle("before failed reload")?.contains("call #1"));

        let bad_source = "not a guest component\n";
        let digest = digest_source(bad_source);
        harness.session.append(SessionEntryKind::ReloadProposed {
            spec: "invalid source from test".to_string(),
            source_digest: digest.clone(),
        })?;
        let error = harness
            .reload_from_source(bad_source, &digest)
            .expect_err("invalid source should fail before promotion");
        harness.record_reload_failure(Some(digest), &error)?;

        assert_eq!(harness.generation(), 0);
        assert!(harness.handle("after failed reload")?.contains("call #2"));
        assert_eq!(
            fs_err::read_to_string(root.join(GUEST_SOURCE))?,
            original_source
        );

        let session = fs_err::read_to_string(&session_path)?;
        assert!(session.contains(r#""type":"reload_failed""#));
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn guest_tool_call_round_trips_through_host_policy() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-tool-round-trip-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let mut harness = Harness::load_with_session(root, session_path.clone())?;
        let output = harness.handle("read Cargo.toml")?;

        assert!(output.contains("tool read-1 success=true"));
        assert!(output.contains("[package]"));
        let session = fs_err::read_to_string(&session_path)?;
        assert!(session.contains(r#""type":"tool_call""#));
        assert!(session.contains(r#""type":"tool_result""#));
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn command_api_emits_turn_snapshot_and_tool_events() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-command-api-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let mut harness = Harness::load_with_session(root, session_path.clone())?;
        let outcome = harness.execute(HarnessCommand::UserInput("read Cargo.toml".to_string()))?;

        assert!(matches!(
            outcome.events.first(),
            Some(HarnessEvent::TurnStarted(TurnSnapshot {
                turn_id: 0,
                generation: 0,
                phase_before_turn: HarnessPhase::Idle,
            }))
        ));
        assert!(outcome.events.iter().any(|event| matches!(
            event,
            HarnessEvent::ToolCallRequested {
                call_id,
                tool_name
            } if call_id == "read-1" && tool_name == "read"
        )));
        assert!(outcome.events.iter().any(|event| matches!(
            event,
            HarnessEvent::ToolResultRecorded {
                call_id,
                success: true
            } if call_id == "read-1"
        )));
        assert!(matches!(
            outcome.events.last(),
            Some(HarnessEvent::TurnCompleted {
                turn_id: 0,
                success: true,
            })
        ));
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn provider_stream_turn_records_text_and_executes_tools() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-provider-turn-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let mut harness = Harness::load_with_session(root, session_path.clone())?;
        let output = harness.handle_provider_stream(
            "inspect package",
            vec![
                ProviderEvent::ResponseStarted {
                    response_id: "r1".to_string(),
                },
                ProviderEvent::TextStarted {
                    block_id: "text-1".to_string(),
                },
                ProviderEvent::TextDelta {
                    block_id: "text-1".to_string(),
                    text: "I will read the package manifest.".to_string(),
                },
                ProviderEvent::TextFinished {
                    block_id: "text-1".to_string(),
                },
                ProviderEvent::ToolCallStarted {
                    call_id: "call-1".to_string(),
                    tool_name: "read".to_string(),
                },
                ProviderEvent::ToolCallArgumentsDelta {
                    call_id: "call-1".to_string(),
                    json_delta: r#"{"path":"Cargo.toml"}"#.to_string(),
                },
                ProviderEvent::ToolCallFinished {
                    call_id: "call-1".to_string(),
                },
                ProviderEvent::ResponseFinished,
            ],
        )?;

        assert_eq!(output, "I will read the package manifest.");
        assert!(harness.events.iter().any(|event| matches!(
            event,
            HarnessEvent::ToolResultRecorded {
                call_id,
                success: true,
            } if call_id == "call-1"
        )));
        let session = fs_err::read_to_string(&session_path)?;
        assert!(session.contains(r#""type":"assistant_message""#));
        assert!(session.contains(r#""type":"tool_call""#));
        assert!(session.contains(r#""type":"tool_result""#));
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn provider_stream_failure_records_diagnostic_and_completes_turn() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-provider-failure-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let mut harness = Harness::load_with_session(root, session_path.clone())?;
        let error = harness
            .handle_provider_stream(
                "fail",
                vec![
                    ProviderEvent::ResponseStarted {
                        response_id: "r1".to_string(),
                    },
                    ProviderEvent::ResponseFailed {
                        message: "model unavailable".to_string(),
                    },
                ],
            )
            .expect_err("provider failure should fail the turn");

        assert!(error.to_string().contains("model unavailable"));
        assert!(matches!(harness.phase, HarnessPhase::Idle));
        let session = fs_err::read_to_string(&session_path)?;
        assert!(session.contains(r#""type":"diagnostic""#));
        assert!(session.contains(r#""success":false"#));
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn provider_loop_sends_tool_results_back_to_provider() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-provider-loop-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let mut provider = ScriptedProvider::new(vec![
            vec![
                ProviderEvent::ResponseStarted {
                    response_id: "r1".to_string(),
                },
                ProviderEvent::ToolCallStarted {
                    call_id: "call-1".to_string(),
                    tool_name: "read".to_string(),
                },
                ProviderEvent::ToolCallArgumentsDelta {
                    call_id: "call-1".to_string(),
                    json_delta: r#"{"path":"Cargo.toml"}"#.to_string(),
                },
                ProviderEvent::ToolCallFinished {
                    call_id: "call-1".to_string(),
                },
                ProviderEvent::ResponseFinished,
            ],
            vec![
                ProviderEvent::ResponseStarted {
                    response_id: "r2".to_string(),
                },
                ProviderEvent::TextStarted {
                    block_id: "text-1".to_string(),
                },
                ProviderEvent::TextDelta {
                    block_id: "text-1".to_string(),
                    text: "Cargo manifest read.".to_string(),
                },
                ProviderEvent::TextFinished {
                    block_id: "text-1".to_string(),
                },
                ProviderEvent::ResponseFinished,
            ],
        ]);

        let mut harness = Harness::load_with_session(root, session_path.clone())?;
        let output = harness.handle_provider_loop("inspect package", &mut provider, 4)?;

        assert_eq!(output, "Cargo manifest read.");
        assert_eq!(provider.requests().len(), 2);
        assert!(provider.requests()[0].tool_results.is_empty());
        assert_eq!(provider.requests()[1].tool_results[0].call_id, "call-1");
        assert!(provider.requests()[1].tool_results[0].success);
        assert!(
            provider.requests()[1].tool_results[0]
                .output_json
                .to_string()
                .contains("[package]")
        );
        let session = fs_err::read_to_string(&session_path)?;
        assert!(session.contains(r#""type":"tool_result""#));
        assert!(session.contains(r#""type":"assistant_message""#));
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn provider_loop_rejects_unbounded_tool_cycles() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-provider-loop-limit-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let tool_response = || {
            vec![
                ProviderEvent::ResponseStarted {
                    response_id: "r".to_string(),
                },
                ProviderEvent::ToolCallStarted {
                    call_id: "call-1".to_string(),
                    tool_name: "read".to_string(),
                },
                ProviderEvent::ToolCallArgumentsDelta {
                    call_id: "call-1".to_string(),
                    json_delta: r#"{"path":"Cargo.toml"}"#.to_string(),
                },
                ProviderEvent::ToolCallFinished {
                    call_id: "call-1".to_string(),
                },
                ProviderEvent::ResponseFinished,
            ]
        };
        let mut provider = ScriptedProvider::new(vec![tool_response(), tool_response()]);

        let mut harness = Harness::load_with_session(root, session_path.clone())?;
        let error = harness
            .handle_provider_loop("loop", &mut provider, 2)
            .expect_err("loop should stop at the iteration limit");

        assert!(error.to_string().contains("exceeded 2 iterations"));
        assert!(matches!(harness.phase, HarnessPhase::Idle));
        let session = fs_err::read_to_string(&session_path)?;
        assert!(session.contains(r#""success":false"#));
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn provider_loop_accepts_external_cancellation_token() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-provider-loop-cancelled-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let mut provider = ScriptedProvider::new(vec![vec![ProviderEvent::ResponseStarted {
            response_id: "r1".to_string(),
        }]]);
        let cancellation = CancellationToken::default();
        cancellation.cancel();

        let mut harness = Harness::load_with_session(root, session_path.clone())?;
        let error = harness
            .handle_provider_loop_with_cancellation("cancel", &mut provider, 1, Some(cancellation))
            .expect_err("cancelled provider loop should fail");

        assert!(error.to_string().contains("operation cancelled"));
        assert!(provider.requests().is_empty());
        assert!(matches!(harness.phase, HarnessPhase::Idle));
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn guest_manifest_declarations_are_host_validated() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-manifest-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let harness = Harness::load_with_session(root, session_path.clone())?;

        assert_eq!(harness.registry.generation, 0);
        assert_eq!(harness.registry.manifest.version, "1");
        assert!(
            harness
                .registry
                .manifest
                .capabilities
                .contains(&"tool:read".to_string())
        );
        assert!(harness.registry.manifest.commands.iter().any(|command| {
            command.name == "read-file" && command.description.contains("host-owned read")
        }));
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn agent_manifest_exposes_command_schemas_and_mutation_flags() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-agent-manifest-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let harness = Harness::load_with_session(root, session_path.clone())?;
        let manifest = harness.manifest_data();
        let commands = manifest["commands"].as_array().expect("commands array");

        let user_input = commands
            .iter()
            .find(|command| command["type"] == "user_input")
            .expect("user_input command");
        assert_eq!(user_input["input_schema"]["required"], json!(["text"]));
        assert_eq!(user_input["mutates_session"], true);
        assert_eq!(user_input["mutates_source"], false);

        let provider_stream = commands
            .iter()
            .find(|command| command["type"] == "provider_stream")
            .expect("provider_stream command");
        assert_eq!(
            provider_stream["input_schema"]["required"],
            json!(["input", "events"])
        );
        assert_eq!(provider_stream["mutates_session"], true);
        assert_eq!(provider_stream["mutates_source"], false);

        let provider_script = commands
            .iter()
            .find(|command| command["type"] == "provider_script")
            .expect("provider_script command");
        assert_eq!(
            provider_script["input_schema"]["required"],
            json!(["input", "responses"])
        );
        assert_eq!(provider_script["mutates_session"], true);
        assert_eq!(provider_script["mutates_source"], false);

        let provider_script_start = commands
            .iter()
            .find(|command| command["type"] == "provider_script_start")
            .expect("provider_script_start command");
        assert_eq!(
            provider_script_start["input_schema"]["required"],
            json!(["input", "responses"])
        );
        assert_eq!(provider_script_start["mutates_session"], true);
        assert_eq!(provider_script_start["mutates_source"], false);

        let provider_prompt = commands
            .iter()
            .find(|command| command["type"] == "provider_prompt")
            .expect("provider_prompt command");
        assert_eq!(
            provider_prompt["input_schema"]["required"],
            json!(["input"])
        );
        assert_eq!(provider_prompt["mutates_session"], true);
        assert_eq!(provider_prompt["mutates_source"], false);

        let provider_prompt_start = commands
            .iter()
            .find(|command| command["type"] == "provider_prompt_start")
            .expect("provider_prompt_start command");
        assert_eq!(
            provider_prompt_start["input_schema"]["required"],
            json!(["input"])
        );
        assert_eq!(provider_prompt_start["mutates_session"], true);
        assert_eq!(provider_prompt_start["mutates_source"], false);

        let job_status = commands
            .iter()
            .find(|command| command["type"] == "job_status")
            .expect("job_status command");
        assert_eq!(job_status["input_schema"]["required"], json!(["job_id"]));
        assert_eq!(job_status["mutates_session"], false);
        assert_eq!(job_status["mutates_source"], false);

        let provider_status = commands
            .iter()
            .find(|command| command["type"] == "provider_status")
            .expect("provider_status command");
        assert_eq!(provider_status["mutates_session"], false);
        assert_eq!(provider_status["mutates_source"], false);

        let evolve = commands
            .iter()
            .find(|command| command["type"] == "evolve")
            .expect("evolve command");
        assert_eq!(evolve["input_schema"]["required"], json!(["spec"]));
        assert_eq!(evolve["mutates_session"], true);
        assert_eq!(evolve["mutates_source"], true);
        assert_eq!(evolve["approval_field_supported"], true);

        let abort = commands
            .iter()
            .find(|command| command["type"] == "abort")
            .expect("abort command");
        assert_eq!(abort["mutates_session"], false);
        assert_eq!(abort["mutates_source"], false);

        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn provider_status_reports_not_configured_llm_loop() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-provider-status-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let mut harness = Harness::load_with_session(root, session_path.clone())?;
        let outcome = harness.execute(HarnessCommand::ProviderStatus)?;
        let data = outcome.data.as_ref().expect("provider status data");

        assert_eq!(data["mode"], "guest_harness");
        assert_eq!(data["transport"], "not_configured");
        assert_eq!(data["llm_loop"], false);
        assert_eq!(data["scripted_loop"]["available"], true);
        assert_eq!(data["scripted_loop"]["network_provider"], false);
        assert_eq!(data["cancellation"]["cooperative"], true);
        assert_eq!(data["cancellation"]["async_abort"], true);
        assert_eq!(data["stream_reducer"]["available"], true);
        assert!(
            data["missing"]
                .as_array()
                .expect("missing array")
                .contains(&json!("network_provider_adapter"))
        );
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn abort_reports_no_active_operation_when_idle() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path =
            std::env::temp_dir().join(format!("piers-harness-abort-{}.jsonl", std::process::id()));
        let _ = fs_err::remove_file(&session_path);

        let mut harness = Harness::load_with_session(root, session_path.clone())?;
        let outcome = harness.execute(HarnessCommand::Abort)?;

        assert_eq!(outcome.data.as_ref().unwrap()["accepted"], false);
        assert!(matches!(
            outcome.events.as_slice(),
            [HarnessEvent::AbortRequested {
                accepted: false,
                reason
            }] if reason == "no active operation"
        ));
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn abort_cancels_active_token() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-abort-active-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let mut harness = Harness::load_with_session(root, session_path.clone())?;
        let cancellation = CancellationToken::default();
        harness.active_cancellation = Some(cancellation.clone());
        harness.phase = HarnessPhase::InTurn { turn_id: 7 };

        let data = harness.abort_data();

        assert_eq!(data["accepted"], true);
        assert!(cancellation.is_cancelled());
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn guest_tool_execution_requires_declared_capability() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-tool-capability-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let harness = Harness::load_with_session(root, session_path.clone())?;

        let read = harness.execute_guest_tool("read", json!({ "path": "Cargo.toml" }));
        assert!(read.success);

        let write =
            harness.execute_guest_tool("write", json!({ "path": "notes/a.txt", "text": "no" }));
        assert!(!write.success);
        assert!(
            write
                .output_json
                .to_string()
                .contains("guest capability tool:write is not declared")
        );
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn successful_reload_notifies_new_guest() -> Result<()> {
        let root = std::env::current_dir().context("get test current dir")?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-harness-reload-accepted-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let mut harness = Harness::load_with_session(root, session_path.clone())?;
        harness.reload()?;

        let session = fs_err::read_to_string(&session_path)?;
        assert!(session.contains(r#""type":"reload_promoted""#));
        assert!(session.contains("reload accepted generation 1"));
        let _ = fs_err::remove_file(session_path);
        Ok(())
    }
}

struct ReloadAttemptError {
    stage: ReloadStage,
    error: anyhow::Error,
}

fn validate_guest_manifest(guest: &Guest, store: &mut Store<HostState>) -> Result<GuestManifest> {
    let manifest = guest
        .call_manifest(store)
        .map_err(|error| anyhow!("call guest manifest: {error:#}"))?;
    if manifest.version != "1" {
        bail!("unsupported guest manifest version {}", manifest.version);
    }
    if manifest.name.trim().is_empty() {
        bail!("guest manifest name must not be empty");
    }
    let mut command_names = BTreeSet::new();
    for command in &manifest.commands {
        if command.name.trim().is_empty() {
            bail!("guest manifest command name must not be empty");
        }
        if command.description.trim().is_empty() {
            bail!(
                "guest manifest command {} description must not be empty",
                command.name
            );
        }
        if matches!(
            command.name.as_str(),
            "status" | "reload" | "evolve" | "quit"
        ) {
            bail!(
                "guest manifest command {} conflicts with host command",
                command.name
            );
        }
        if !command_names.insert(command.name.clone()) {
            bail!("guest manifest command {} is declared twice", command.name);
        }
    }
    let mut capabilities = BTreeSet::new();
    for capability in &manifest.capabilities {
        if capability.trim().is_empty() {
            bail!("guest manifest capability must not be empty");
        }
        if !capabilities.insert(capability.clone()) {
            bail!("guest manifest capability {capability} is declared twice");
        }
    }
    Ok(manifest)
}

impl ReloadAttemptError {
    fn new(stage: ReloadStage, error: anyhow::Error) -> Self {
        Self { stage, error }
    }
}

impl std::fmt::Display for ReloadAttemptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {:#}", self.stage, self.error)
    }
}

enum ReloadStatus {
    InitialBuildOk,
    Ok { generation: u64 },
    Failed { stage: ReloadStage, message: String },
}

impl std::fmt::Display for ReloadStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InitialBuildOk => formatter.write_str("initial build ok"),
            Self::Ok { generation } => write!(formatter, "ok: generation {generation}"),
            Self::Failed { stage, message } => {
                write!(formatter, "failed at {stage:?}: {message}")
            }
        }
    }
}

fn session_summary(path: &Path) -> Result<Value> {
    let text = fs_err::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let entry_count = text.lines().filter(|line| !line.trim().is_empty()).count();
    let metadata = fs_err::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    Ok(json!({
        "path": path.display().to_string(),
        "entry_count": entry_count,
        "modified_ms": modified_ms,
    }))
}

fn format_session_list(data: &Value) -> String {
    let Some(sessions) = data.get("sessions").and_then(Value::as_array) else {
        return "sessions: 0".to_string();
    };
    if sessions.is_empty() {
        return "sessions: 0".to_string();
    }
    let mut lines = vec![format!("sessions: {}", sessions.len())];
    for session in sessions {
        let path = session
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let entries = session
            .get("entry_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        lines.push(format!("{path} ({entries} entries)"));
    }
    lines.join("\n")
}

fn format_session_entries(data: &Value) -> String {
    let total = data
        .get("total_entries")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let returned = data
        .get("returned_entries")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let session = data
        .get("session")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    format!("session: {session}\nentries: {returned}/{total}")
}

fn format_provider_status(data: &Value) -> String {
    let mode = data
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let transport = data
        .get("transport")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let llm_loop = data
        .get("llm_loop")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let scripted_loop = data
        .pointer("/scripted_loop/available")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    format!(
        "provider mode: {mode}\ntransport: {transport}\nllm loop: {llm_loop}\nscripted loop: {scripted_loop}"
    )
}

fn reduce_provider_events(events: Vec<ProviderEvent>) -> Result<ProviderResponse> {
    let mut reducer = ProviderStreamReducer::default();
    for event in events {
        reducer.push(event)?;
    }
    reducer.finish()
}

fn rpc_command_manifest() -> Vec<Value> {
    vec![
        rpc_command(
            "user_input",
            "Send one user prompt to the active guest.",
            json_schema(&[("text", "string")]),
            true,
            false,
            false,
        ),
        rpc_command(
            "provider_stream",
            "Submit normalized provider events for one host-owned turn.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["input", "events"],
                "properties": {
                    "input": { "type": "string" },
                    "events": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "additionalProperties": true
                        }
                    }
                }
            }),
            true,
            false,
            false,
        ),
        rpc_command(
            "provider_script",
            "Run a deterministic multi-step provider/tool loop.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["input", "responses"],
                "properties": {
                    "input": { "type": "string" },
                    "responses": {
                        "type": "array",
                        "items": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": true
                            }
                        }
                    },
                    "max_iterations": { "type": "integer", "minimum": 1 }
                }
            }),
            true,
            false,
            false,
        ),
        rpc_command(
            "provider_script_start",
            "Start a deterministic provider/tool loop as a background job.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["input", "responses"],
                "properties": {
                    "job_id": { "type": "string" },
                    "input": { "type": "string" },
                    "responses": {
                        "type": "array",
                        "items": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": true
                            }
                        }
                    },
                    "max_iterations": { "type": "integer", "minimum": 1 }
                }
            }),
            true,
            false,
            false,
        ),
        rpc_command(
            "provider_prompt",
            "Run a configured OpenAI-compatible provider/tool loop.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["input"],
                "properties": {
                    "input": { "type": "string" },
                    "max_iterations": { "type": "integer", "minimum": 1 }
                }
            }),
            true,
            false,
            false,
        ),
        rpc_command(
            "provider_prompt_start",
            "Start a configured OpenAI-compatible provider/tool loop as a background job.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["input"],
                "properties": {
                    "job_id": { "type": "string" },
                    "input": { "type": "string" },
                    "max_iterations": { "type": "integer", "minimum": 1 }
                }
            }),
            true,
            false,
            false,
        ),
        rpc_command(
            "status",
            "Return host and active guest status.",
            json_schema(&[]),
            false,
            false,
            false,
        ),
        rpc_command(
            "provider_status",
            "Return provider, model, and LLM-loop status.",
            json_schema(&[]),
            false,
            false,
            false,
        ),
        rpc_command(
            "manifest",
            "Return this command and capability manifest.",
            json_schema(&[]),
            false,
            false,
            false,
        ),
        rpc_command(
            "list_sessions",
            "List known session JSONL files.",
            json_schema(&[]),
            false,
            false,
            false,
        ),
        rpc_command(
            "session_entries",
            "Return entries from the active session log.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "limit": { "type": "integer", "minimum": 0 }
                }
            }),
            false,
            false,
            false,
        ),
        rpc_command(
            "switch_session",
            "Switch this RPC worker to another session JSONL file.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": {
                    "path": { "type": "string" }
                }
            }),
            false,
            false,
            false,
        ),
        rpc_command(
            "job_status",
            "Poll a background provider job.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["job_id"],
                "properties": {
                    "job_id": { "type": "string" }
                }
            }),
            false,
            false,
            false,
        ),
        rpc_command(
            "abort",
            "Request cancellation of active work or a background job.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "job_id": { "type": "string" }
                }
            }),
            false,
            false,
            false,
        ),
        rpc_command(
            "reload",
            "Rebuild and reload the live guest source.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "approved": { "type": "boolean" }
                }
            }),
            true,
            false,
            true,
        ),
        rpc_command(
            "evolve",
            "Ask the guest to propose replacement source, then reload it.",
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["spec"],
                "properties": {
                    "spec": { "type": "string" },
                    "approved": { "type": "boolean" }
                }
            }),
            true,
            true,
            true,
        ),
        rpc_command(
            "quit",
            "Stop the JSONL RPC worker.",
            json_schema(&[]),
            false,
            false,
            false,
        ),
    ]
}

fn rpc_command(
    command_type: &'static str,
    description: &'static str,
    input_schema: Value,
    mutates_session: bool,
    mutates_source: bool,
    approval_field_supported: bool,
) -> Value {
    json!({
        "type": command_type,
        "description": description,
        "input_schema": input_schema,
        "mutates_session": mutates_session,
        "mutates_source": mutates_source,
        "approval_field_supported": approval_field_supported,
    })
}

fn json_schema(required_fields: &[(&str, &str)]) -> Value {
    let properties = required_fields
        .iter()
        .map(|(name, kind)| (name.to_string(), json!({ "type": kind })))
        .collect::<serde_json::Map<_, _>>();
    let required = required_fields
        .iter()
        .map(|(name, _)| Value::String(name.to_string()))
        .collect::<Vec<_>>();
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": required,
        "properties": properties,
    })
}
