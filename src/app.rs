//! Non-TUI application modes for Piers.
//!
//! Pi's CLI entry point chooses a mode, creates one shared session runtime, and
//! lets each mode add only its own I/O behavior. Piers is smaller, but follows
//! the same shape here: `main` initializes process concerns, then this module
//! drives the stable harness command/event boundary as either a line REPL or a
//! single-shot print/json command.

use std::collections::HashMap;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::harness::{CommandOutcome, Harness, HarnessCommand, HarnessEvent};
use crate::provider::{
    CancellationToken, ProviderEvent, ScriptedProvider, openai_compatible_provider_from_env,
};

/// Parsed top-level application mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppMode {
    Repl,
    Print { output: PrintOutput },
    Rpc,
    Help,
    Version,
}

/// Single-shot output format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PrintOutput {
    Text,
    Json,
}

/// Small CLI surface for the current Piers runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppArgs {
    pub mode: AppMode,
    pub session_path: Option<PathBuf>,
    pub read_only: bool,
    pub require_approval: bool,
    pub messages: Vec<String>,
}

/// Run Piers from process arguments.
pub fn run(args: impl IntoIterator<Item = String>, root: PathBuf) -> Result<()> {
    let args = AppArgs::parse(args)?;
    let stdin_is_terminal = io::stdin().is_terminal();
    let mode = match args.mode {
        AppMode::Repl if !stdin_is_terminal => AppMode::Print {
            output: PrintOutput::Text,
        },
        mode => mode,
    };
    let session_path = args.session_path.clone();
    let policy = CommandPolicy {
        read_only: args.read_only,
        require_approval: args.require_approval,
    };

    match mode {
        AppMode::Help => {
            print_help();
            Ok(())
        }
        AppMode::Version => {
            println!("piers {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        AppMode::Repl => run_repl(load_harness(root, session_path)?, policy),
        AppMode::Rpc => run_rpc(load_harness(root.clone(), session_path)?, root, policy),
        AppMode::Print { output } => {
            let mut messages = args.messages;
            if !stdin_is_terminal {
                let mut piped = String::new();
                io::stdin().read_to_string(&mut piped)?;
                let piped = piped.trim_end();
                if !piped.is_empty() {
                    messages.insert(0, piped.to_string());
                }
            }
            run_print(load_harness(root, session_path)?, policy, output, messages)
        }
    }
}

impl AppArgs {
    /// Parse the intentionally small Piers CLI.
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut mode = AppMode::Repl;
        let mut session_path = None;
        let mut read_only = false;
        let mut require_approval = false;
        let mut messages = Vec::new();
        let mut args = args.into_iter().peekable();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--help" | "-h" => mode = AppMode::Help,
                "--version" | "-V" => mode = AppMode::Version,
                "--read-only" => read_only = true,
                "--require-approval" => require_approval = true,
                "--session" => {
                    let value = args
                        .next()
                        .ok_or_else(|| anyhow!("--session requires a path"))?;
                    session_path = Some(PathBuf::from(value));
                }
                "--print" | "-p" => {
                    mode = AppMode::Print {
                        output: PrintOutput::Text,
                    };
                    if let Some(next) = args.peek() {
                        if !next.starts_with('-') {
                            messages.push(args.next().expect("peeked arg exists"));
                        }
                    }
                }
                "--mode" => {
                    let value = args
                        .next()
                        .ok_or_else(|| anyhow!("--mode requires text or json"))?;
                    mode = match value.as_str() {
                        "text" => AppMode::Print {
                            output: PrintOutput::Text,
                        },
                        "json" => AppMode::Print {
                            output: PrintOutput::Json,
                        },
                        "rpc" => AppMode::Rpc,
                        _ => bail!("unknown mode {value}; expected text, json, or rpc"),
                    };
                }
                _ if arg.starts_with('-') => bail!("unknown option {arg}"),
                _ => messages.push(arg),
            }
        }

        Ok(Self {
            mode,
            session_path,
            read_only,
            require_approval,
            messages,
        })
    }
}

/// App-layer command policy applied before the harness mutates state or source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CommandPolicy {
    read_only: bool,
    require_approval: bool,
}

fn load_harness(root: PathBuf, session_path: Option<PathBuf>) -> Result<Harness> {
    match session_path {
        Some(path) if path.is_absolute() => Harness::load_with_session(root, path),
        Some(path) => Harness::load_with_session(root.clone(), root.join(path)),
        None => Harness::load(root),
    }
}

fn run_repl(mut harness: Harness, policy: CommandPolicy) -> Result<()> {
    println!("piers");
    println!(
        "commands: :status, :provider, :manifest, :sessions, :session, :abort, :reload, :evolve <spec>, :quit"
    );

    let stdin = io::stdin();
    loop {
        print!("piers> ");
        io::stdout().flush()?;

        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            break;
        }

        let line = line.trim_end();
        if line == ":quit" {
            break;
        }
        if line.is_empty() {
            continue;
        }

        match parse_repl_command(line) {
            Ok(command) => {
                let approved = command_is_mutating(&command);
                let outcome = execute_checked(&mut harness, policy, command, approved)?;
                render_repl_outcome(&outcome);
            }
            Err(error) => eprintln!("{error:#}"),
        }
    }

    Ok(())
}

fn run_print(
    mut harness: Harness,
    policy: CommandPolicy,
    output: PrintOutput,
    messages: Vec<String>,
) -> Result<()> {
    if messages.is_empty() {
        bail!("print mode requires a prompt on stdin or as an argument");
    }

    for message in messages {
        let outcome = execute_checked(
            &mut harness,
            policy,
            HarnessCommand::UserInput(message),
            false,
        )?;
        match output {
            PrintOutput::Text => {
                if let Some(display) = outcome.display {
                    println!("{display}");
                }
            }
            PrintOutput::Json => render_json_outcome(&outcome)?,
        }
    }
    Ok(())
}

fn run_rpc(mut harness: Harness, root: PathBuf, policy: CommandPolicy) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let mut jobs = RpcJobs::default();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<RpcRequest>(&line) {
            Ok(request) => request,
            Err(error) => {
                write_rpc_error(
                    &mut stdout,
                    None,
                    RpcErrorCode::DecodeRequest,
                    format!("decode request: {error}"),
                )?;
                continue;
            }
        };

        let id = request.id().cloned();
        if matches!(request, RpcRequest::Quit { .. }) {
            write_rpc_result(&mut stdout, id.as_ref(), 0, None, None)?;
            break;
        }
        if let RpcRequest::ProviderScriptStart {
            job_id,
            input,
            responses,
            max_iterations,
            ..
        } = request
        {
            match jobs.start_provider_script(
                root.clone(),
                harness.session_path().to_path_buf(),
                job_id,
                input,
                responses,
                max_iterations.unwrap_or(16),
            ) {
                Ok(data) => write_rpc_result(&mut stdout, id.as_ref(), 0, None, Some(&data))?,
                Err(error) => write_rpc_error(
                    &mut stdout,
                    id.as_ref(),
                    RpcErrorCode::Harness,
                    format!("{error:#}"),
                )?,
            }
            continue;
        }
        if let RpcRequest::ProviderPromptStart {
            job_id,
            input,
            max_iterations,
            ..
        } = request
        {
            match jobs.start_provider_prompt(
                root.clone(),
                harness.session_path().to_path_buf(),
                job_id,
                input,
                max_iterations.unwrap_or(16),
            ) {
                Ok(data) => write_rpc_result(&mut stdout, id.as_ref(), 0, None, Some(&data))?,
                Err(error) => write_rpc_error(
                    &mut stdout,
                    id.as_ref(),
                    RpcErrorCode::Harness,
                    format!("{error:#}"),
                )?,
            }
            continue;
        }
        if let RpcRequest::JobStatus { job_id, .. } = request {
            match jobs.poll(&job_id) {
                JobPoll::Running(data) => {
                    write_rpc_result(&mut stdout, id.as_ref(), 0, None, Some(&data))?
                }
                JobPoll::Completed(mut outcome) => {
                    attach_policy_data(&mut outcome, policy);
                    write_rpc_outcome(&mut stdout, id.as_ref(), &outcome)?;
                }
                JobPoll::Failed(error) => {
                    write_rpc_error(&mut stdout, id.as_ref(), RpcErrorCode::Harness, error)?
                }
            }
            continue;
        }
        if let RpcRequest::Abort { job_id, .. } = request {
            if let Some(job_id) = job_id {
                let data = jobs.abort(&job_id);
                write_rpc_result(&mut stdout, id.as_ref(), 0, None, Some(&data))?;
            } else {
                match execute_checked(&mut harness, policy, HarnessCommand::Abort, false) {
                    Ok(outcome) => write_rpc_outcome(&mut stdout, id.as_ref(), &outcome)?,
                    Err(error) => write_rpc_error(
                        &mut stdout,
                        id.as_ref(),
                        classify_error(&error),
                        format!("{error:#}"),
                    )?,
                }
            }
            continue;
        }
        if let RpcRequest::SwitchSession { path, .. } = request {
            match load_harness(root.clone(), Some(PathBuf::from(path))) {
                Ok(next_harness) => {
                    harness = next_harness;
                    let session = harness.session_path().display().to_string();
                    let mut outcome = CommandOutcome {
                        display: Some(format!("switched session {session}")),
                        data: Some(serde_json::json!({ "session": session })),
                        events: Vec::new(),
                    };
                    attach_policy_data(&mut outcome, policy);
                    write_rpc_outcome(&mut stdout, id.as_ref(), &outcome)?;
                }
                Err(error) => write_rpc_error(
                    &mut stdout,
                    id.as_ref(),
                    RpcErrorCode::Harness,
                    format!("{error:#}"),
                )?,
            }
            continue;
        }

        let approved = request.approved();
        match request.into_command() {
            Ok(command) => match execute_checked(&mut harness, policy, command, approved) {
                Ok(outcome) => write_rpc_outcome(&mut stdout, id.as_ref(), &outcome)?,
                Err(error) => write_rpc_error(
                    &mut stdout,
                    id.as_ref(),
                    classify_error(&error),
                    format!("{error:#}"),
                )?,
            },
            Err(error) => write_rpc_error(
                &mut stdout,
                id.as_ref(),
                RpcErrorCode::InvalidCommand,
                format!("{error:#}"),
            )?,
        }
    }
    Ok(())
}

fn execute_checked(
    harness: &mut Harness,
    policy: CommandPolicy,
    command: HarnessCommand,
    approved: bool,
) -> Result<CommandOutcome> {
    validate_command(policy, &command, approved)?;
    let mut outcome = harness.execute(command)?;
    attach_policy_data(&mut outcome, policy);
    Ok(outcome)
}

fn validate_command(policy: CommandPolicy, command: &HarnessCommand, approved: bool) -> Result<()> {
    if policy.read_only && matches!(command, HarnessCommand::Evolve(_) | HarnessCommand::Reload) {
        bail!("command is disabled by --read-only");
    }
    if policy.require_approval && command_is_mutating(command) && !approved {
        bail!("command requires approval");
    }
    Ok(())
}

fn command_is_mutating(command: &HarnessCommand) -> bool {
    matches!(command, HarnessCommand::Evolve(_) | HarnessCommand::Reload)
}

fn attach_policy_data(outcome: &mut CommandOutcome, policy: CommandPolicy) {
    let Some(Value::Object(data)) = &mut outcome.data else {
        return;
    };
    data.insert(
        "policy".to_string(),
        serde_json::json!({
            "read_only": policy.read_only,
            "reload_allowed": !policy.read_only,
            "evolve_allowed": !policy.read_only,
            "approval_required": policy.require_approval,
        }),
    );
}

fn parse_repl_command(line: &str) -> Result<HarnessCommand> {
    if line == ":status" {
        Ok(HarnessCommand::Status)
    } else if line == ":provider" {
        Ok(HarnessCommand::ProviderStatus)
    } else if line == ":abort" {
        Ok(HarnessCommand::Abort)
    } else if line == ":reload" {
        Ok(HarnessCommand::Reload)
    } else if line == ":manifest" {
        Ok(HarnessCommand::Manifest)
    } else if line == ":sessions" {
        Ok(HarnessCommand::ListSessions)
    } else if line == ":session" {
        Ok(HarnessCommand::SessionEntries { limit: Some(20) })
    } else if let Some(spec) = line.strip_prefix(":evolve") {
        Ok(HarnessCommand::Evolve(spec.trim().to_string()))
    } else if line.starts_with(':') {
        bail!("unknown command {line}")
    } else {
        Ok(HarnessCommand::UserInput(line.to_string()))
    }
}

fn render_repl_outcome(outcome: &CommandOutcome) {
    for event in &outcome.events {
        if let Some(line) = render_repl_event(event) {
            println!("{line}");
        }
    }
    if outcome
        .events
        .iter()
        .all(|event| !matches!(event, HarnessEvent::AssistantMessage { .. }))
    {
        if let Some(display) = &outcome.display {
            println!("{display}");
        }
    }
}

fn render_repl_event(event: &HarnessEvent) -> Option<String> {
    match event {
        HarnessEvent::AssistantMessage { text } => Some(text.clone()),
        HarnessEvent::ToolCallRequested { call_id, tool_name } => {
            Some(format!("tool {tool_name} requested ({call_id})"))
        }
        HarnessEvent::ToolResultRecorded { call_id, success } => {
            Some(format!("tool result {call_id} success={success}"))
        }
        HarnessEvent::Diagnostic { level, message } => Some(format!("{level}: {message}")),
        HarnessEvent::ReloadStarted { target_generation } => {
            Some(format!("reloading generation {target_generation}"))
        }
        HarnessEvent::ReloadPromoted { generation } => {
            Some(format!("reloaded generation {generation}"))
        }
        HarnessEvent::ReloadFailed { stage, message } => {
            Some(format!("reload failed at {stage:?}: {message}"))
        }
        _ => None,
    }
}

fn render_json_outcome(outcome: &CommandOutcome) -> Result<()> {
    for event in &outcome.events {
        println!("{}", serde_json::to_string(event)?);
    }
    if outcome.events.is_empty() {
        if let Some(display) = &outcome.display {
            println!(
                "{}",
                serde_json::to_string(&DisplayEvent {
                    event_type: "command_display",
                    text: display,
                })?
            );
        }
    }
    Ok(())
}

fn write_rpc_outcome(
    writer: &mut impl Write,
    id: Option<&Value>,
    outcome: &CommandOutcome,
) -> Result<()> {
    let mut sequence = 0;
    for event in &outcome.events {
        writeln!(
            writer,
            "{}",
            serde_json::to_string(&RpcEventLine {
                line_type: "event",
                schema_version: 1,
                id,
                sequence,
                event,
            })?
        )?;
        sequence += 1;
    }
    write_rpc_result(
        writer,
        id,
        sequence,
        outcome.display.as_deref(),
        outcome.data.as_ref(),
    )
}

fn write_rpc_result(
    writer: &mut impl Write,
    id: Option<&Value>,
    sequence: u64,
    display: Option<&str>,
    data: Option<&Value>,
) -> Result<()> {
    writeln!(
        writer,
        "{}",
        serde_json::to_string(&RpcResultLine {
            line_type: "result",
            schema_version: 1,
            id,
            sequence,
            display,
            data,
        })?
    )?;
    writer.flush()?;
    Ok(())
}

fn write_rpc_error(
    writer: &mut impl Write,
    id: Option<&Value>,
    code: RpcErrorCode,
    message: String,
) -> Result<()> {
    writeln!(
        writer,
        "{}",
        serde_json::to_string(&RpcErrorLine {
            line_type: "error",
            schema_version: 1,
            id,
            sequence: 0,
            code,
            message: &message,
        })?
    )?;
    writer.flush()?;
    Ok(())
}

fn classify_error(error: &anyhow::Error) -> RpcErrorCode {
    let message = format!("{error:#}");
    if message.contains("disabled by --read-only") {
        RpcErrorCode::ReadOnly
    } else if message.contains("requires approval") {
        RpcErrorCode::ApprovalRequired
    } else {
        RpcErrorCode::Harness
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RpcErrorCode {
    DecodeRequest,
    InvalidCommand,
    ReadOnly,
    ApprovalRequired,
    Harness,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RpcRequest {
    UserInput {
        id: Option<Value>,
        text: String,
    },
    ProviderStream {
        id: Option<Value>,
        input: String,
        events: Vec<ProviderEvent>,
    },
    ProviderScript {
        id: Option<Value>,
        input: String,
        responses: Vec<Vec<ProviderEvent>>,
        max_iterations: Option<usize>,
    },
    ProviderScriptStart {
        id: Option<Value>,
        job_id: Option<String>,
        input: String,
        responses: Vec<Vec<ProviderEvent>>,
        max_iterations: Option<usize>,
    },
    ProviderPrompt {
        id: Option<Value>,
        input: String,
        max_iterations: Option<usize>,
    },
    ProviderPromptStart {
        id: Option<Value>,
        job_id: Option<String>,
        input: String,
        max_iterations: Option<usize>,
    },
    Evolve {
        id: Option<Value>,
        spec: String,
        approved: Option<bool>,
    },
    Reload {
        id: Option<Value>,
        approved: Option<bool>,
    },
    Abort {
        id: Option<Value>,
        job_id: Option<String>,
    },
    JobStatus {
        id: Option<Value>,
        job_id: String,
    },
    Status {
        id: Option<Value>,
    },
    ProviderStatus {
        id: Option<Value>,
    },
    Manifest {
        id: Option<Value>,
    },
    ListSessions {
        id: Option<Value>,
    },
    SessionEntries {
        id: Option<Value>,
        limit: Option<usize>,
    },
    SwitchSession {
        id: Option<Value>,
        path: String,
    },
    Quit {
        id: Option<Value>,
    },
}

impl RpcRequest {
    fn id(&self) -> Option<&Value> {
        match self {
            Self::UserInput { id, .. }
            | Self::ProviderStream { id, .. }
            | Self::ProviderScript { id, .. }
            | Self::ProviderScriptStart { id, .. }
            | Self::ProviderPrompt { id, .. }
            | Self::ProviderPromptStart { id, .. }
            | Self::Evolve { id, .. }
            | Self::Reload { id, .. }
            | Self::Abort { id, .. }
            | Self::JobStatus { id, .. }
            | Self::Status { id }
            | Self::ProviderStatus { id }
            | Self::Manifest { id }
            | Self::ListSessions { id }
            | Self::SessionEntries { id, .. }
            | Self::SwitchSession { id, .. }
            | Self::Quit { id } => id.as_ref(),
        }
    }

    fn approved(&self) -> bool {
        match self {
            Self::Evolve { approved, .. } | Self::Reload { approved, .. } => {
                approved.unwrap_or(false)
            }
            _ => false,
        }
    }

    fn into_command(self) -> Result<HarnessCommand> {
        match self {
            Self::UserInput { text, .. } => Ok(HarnessCommand::UserInput(text)),
            Self::ProviderStream { input, events, .. } => {
                Ok(HarnessCommand::ProviderStream { input, events })
            }
            Self::ProviderScript {
                input,
                responses,
                max_iterations,
                ..
            } => Ok(HarnessCommand::ProviderScript {
                input,
                responses,
                max_iterations: max_iterations.unwrap_or(16),
            }),
            Self::ProviderScriptStart { .. } => {
                bail!("provider_script_start does not map to a harness command")
            }
            Self::ProviderPrompt {
                input,
                max_iterations,
                ..
            } => Ok(HarnessCommand::ProviderPrompt {
                input,
                max_iterations: max_iterations.unwrap_or(16),
            }),
            Self::ProviderPromptStart { .. } => {
                bail!("provider_prompt_start does not map to a harness command")
            }
            Self::Evolve { spec, .. } => Ok(HarnessCommand::Evolve(spec)),
            Self::Reload { .. } => Ok(HarnessCommand::Reload),
            Self::Abort { .. } => Ok(HarnessCommand::Abort),
            Self::JobStatus { .. } => bail!("job_status does not map to a harness command"),
            Self::Status { .. } => Ok(HarnessCommand::Status),
            Self::ProviderStatus { .. } => Ok(HarnessCommand::ProviderStatus),
            Self::Manifest { .. } => Ok(HarnessCommand::Manifest),
            Self::ListSessions { .. } => Ok(HarnessCommand::ListSessions),
            Self::SessionEntries { limit, .. } => Ok(HarnessCommand::SessionEntries { limit }),
            Self::SwitchSession { .. } => bail!("switch_session does not map to a harness command"),
            Self::Quit { .. } => bail!("quit does not map to a harness command"),
        }
    }
}

#[derive(Serialize)]
struct RpcEventLine<'a> {
    #[serde(rename = "type")]
    line_type: &'static str,
    schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a Value>,
    sequence: u64,
    event: &'a HarnessEvent,
}

#[derive(Serialize)]
struct RpcResultLine<'a> {
    #[serde(rename = "type")]
    line_type: &'static str,
    schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a Value>,
    sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a Value>,
}

#[derive(Serialize)]
struct RpcErrorLine<'a> {
    #[serde(rename = "type")]
    line_type: &'static str,
    schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a Value>,
    sequence: u64,
    code: RpcErrorCode,
    message: &'a str,
}

#[derive(Default)]
struct RpcJobs {
    next_id: u64,
    jobs: HashMap<String, RpcJob>,
}

struct RpcJob {
    cancellation: CancellationToken,
    receiver: Receiver<Result<CommandOutcome>>,
}

enum JobPoll {
    Running(Value),
    Completed(CommandOutcome),
    Failed(String),
}

impl RpcJobs {
    fn start_provider_script(
        &mut self,
        root: PathBuf,
        session_path: PathBuf,
        requested_job_id: Option<String>,
        input: String,
        responses: Vec<Vec<ProviderEvent>>,
        max_iterations: usize,
    ) -> Result<Value> {
        let job_id = self.reserve_job_id(requested_job_id)?;
        let cancellation = CancellationToken::default();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = (|| -> Result<CommandOutcome> {
                let mut harness = load_harness(root, Some(session_path))?;
                let mut provider = ScriptedProvider::new(responses);
                let display = harness.handle_provider_loop_with_cancellation(
                    &input,
                    &mut provider,
                    max_iterations,
                    Some(worker_cancellation),
                )?;
                Ok(CommandOutcome {
                    display: Some(display),
                    data: Some(serde_json::json!({
                        "provider_requests": provider.requests().len(),
                    })),
                    events: harness.drain_events(),
                })
            })();
            let _ = sender.send(result);
        });
        self.jobs.insert(
            job_id.clone(),
            RpcJob {
                cancellation,
                receiver,
            },
        );
        Ok(serde_json::json!({
            "job_id": job_id,
            "status": "running",
            "kind": "provider_script",
        }))
    }

    fn start_provider_prompt(
        &mut self,
        root: PathBuf,
        session_path: PathBuf,
        requested_job_id: Option<String>,
        input: String,
        max_iterations: usize,
    ) -> Result<Value> {
        let job_id = self.reserve_job_id(requested_job_id)?;
        let cancellation = CancellationToken::default();
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = (|| -> Result<CommandOutcome> {
                let mut harness = load_harness(root, Some(session_path))?;
                let mut provider = openai_compatible_provider_from_env()?;
                let display = harness.handle_provider_loop_with_cancellation(
                    &input,
                    &mut provider,
                    max_iterations,
                    Some(worker_cancellation),
                )?;
                Ok(CommandOutcome {
                    display: Some(display),
                    data: None,
                    events: harness.drain_events(),
                })
            })();
            let _ = sender.send(result);
        });
        self.jobs.insert(
            job_id.clone(),
            RpcJob {
                cancellation,
                receiver,
            },
        );
        Ok(serde_json::json!({
            "job_id": job_id,
            "status": "running",
            "kind": "provider_prompt",
        }))
    }

    fn abort(&mut self, job_id: &str) -> Value {
        let accepted = if let Some(job) = self.jobs.get(job_id) {
            job.cancellation.cancel();
            true
        } else {
            false
        };
        serde_json::json!({
            "accepted": accepted,
            "job_id": job_id,
            "status": if accepted { "cancelling" } else { "missing" },
        })
    }

    fn poll(&mut self, job_id: &str) -> JobPoll {
        let Some(job) = self.jobs.get(job_id) else {
            return JobPoll::Failed(format!("unknown job {job_id}"));
        };
        match job.receiver.try_recv() {
            Ok(Ok(mut outcome)) => {
                self.jobs.remove(job_id);
                attach_job_data(&mut outcome, job_id, "completed");
                JobPoll::Completed(outcome)
            }
            Ok(Err(error)) => {
                self.jobs.remove(job_id);
                JobPoll::Failed(format!("{error:#}"))
            }
            Err(TryRecvError::Empty) => JobPoll::Running(serde_json::json!({
                "job_id": job_id,
                "status": "running",
            })),
            Err(TryRecvError::Disconnected) => {
                self.jobs.remove(job_id);
                JobPoll::Failed(format!("job {job_id} worker disconnected"))
            }
        }
    }

    fn reserve_job_id(&mut self, requested_job_id: Option<String>) -> Result<String> {
        let job_id = requested_job_id.unwrap_or_else(|| {
            self.next_id += 1;
            format!("job-{}", self.next_id)
        });
        if job_id.trim().is_empty() {
            bail!("job_id must not be empty");
        }
        if self.jobs.contains_key(&job_id) {
            bail!("job {job_id} already exists");
        }
        Ok(job_id)
    }
}

fn attach_job_data(outcome: &mut CommandOutcome, job_id: &str, status: &str) {
    match &mut outcome.data {
        Some(Value::Object(data)) => {
            data.insert("job_id".to_string(), Value::String(job_id.to_string()));
            data.insert("status".to_string(), Value::String(status.to_string()));
        }
        _ => {
            outcome.data = Some(serde_json::json!({
                "job_id": job_id,
                "status": status,
            }));
        }
    }
}

#[derive(Serialize)]
struct DisplayEvent<'a> {
    #[serde(rename = "type")]
    event_type: &'static str,
    text: &'a str,
}

fn print_help() {
    println!(
        "piers - non-TUI Pi-style agent shell\n\n\
Usage:\n  piers [options] [prompt]\n\n\
Options:\n  --session <path>          Use a specific session JSONL file\n  \
--read-only               Disable reload and evolve commands\n  \
--require-approval        Require approved RPC reload/evolve requests\n  \
--print, -p <prompt>      Run one prompt and print final text\n  \
--mode text|json         Run one prompt as text or JSON events\n  \
--mode rpc               Read JSONL commands and write JSONL events\n  \
--help, -h               Show help\n  \
--version, -V            Show version\n\n\
REPL commands:\n  :status\n  :provider\n  :manifest\n  :sessions\n  :session\n  :abort\n  :reload\n  :evolve <spec>\n  :quit"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use crate::staging::build_guest_in;

    #[test]
    fn parse_defaults_to_repl() -> Result<()> {
        let args = AppArgs::parse(Vec::<String>::new())?;
        assert_eq!(args.mode, AppMode::Repl);
        assert_eq!(args.session_path, None);
        assert!(!args.read_only);
        assert!(!args.require_approval);
        assert!(args.messages.is_empty());
        Ok(())
    }

    #[test]
    fn parse_session_path() -> Result<()> {
        let args = AppArgs::parse(["--session", ".piers/sessions/demo.jsonl"].map(str::to_string))?;
        assert_eq!(
            args.session_path,
            Some(PathBuf::from(".piers/sessions/demo.jsonl"))
        );
        assert_eq!(args.mode, AppMode::Repl);
        Ok(())
    }

    #[test]
    fn parse_read_only() -> Result<()> {
        let args = AppArgs::parse(["--read-only", "--mode", "rpc"].map(str::to_string))?;
        assert!(args.read_only);
        assert_eq!(args.mode, AppMode::Rpc);
        Ok(())
    }

    #[test]
    fn parse_require_approval() -> Result<()> {
        let args = AppArgs::parse(["--require-approval", "--mode", "rpc"].map(str::to_string))?;
        assert!(args.require_approval);
        assert_eq!(args.mode, AppMode::Rpc);
        Ok(())
    }

    #[test]
    fn parse_print_consumes_optional_prompt() -> Result<()> {
        let args = AppArgs::parse(["--print", "hello"].map(str::to_string))?;
        assert_eq!(
            args.mode,
            AppMode::Print {
                output: PrintOutput::Text
            }
        );
        assert_eq!(args.messages, ["hello"]);
        Ok(())
    }

    #[test]
    fn parse_json_mode_selects_print_json() -> Result<()> {
        let args = AppArgs::parse(["--mode", "json", "hello"].map(str::to_string))?;
        assert_eq!(
            args.mode,
            AppMode::Print {
                output: PrintOutput::Json
            }
        );
        assert_eq!(args.messages, ["hello"]);
        Ok(())
    }

    #[test]
    fn parse_rpc_mode_selects_rpc() -> Result<()> {
        let args = AppArgs::parse(["--mode", "rpc"].map(str::to_string))?;
        assert_eq!(args.mode, AppMode::Rpc);
        assert_eq!(args.session_path, None);
        assert!(!args.read_only);
        assert!(!args.require_approval);
        assert!(args.messages.is_empty());
        Ok(())
    }

    #[test]
    fn read_only_rejects_reload_and_evolve() {
        let policy = CommandPolicy {
            read_only: true,
            require_approval: false,
        };
        assert!(validate_command(policy, &HarnessCommand::Reload, true).is_err());
        assert!(validate_command(policy, &HarnessCommand::Evolve("x".to_string()), true).is_err());
        assert!(validate_command(policy, &HarnessCommand::Abort, false).is_ok());
        assert!(validate_command(policy, &HarnessCommand::Status, false).is_ok());
        assert!(validate_command(policy, &HarnessCommand::ProviderStatus, false).is_ok());
        assert!(
            validate_command(
                policy,
                &HarnessCommand::ProviderStream {
                    input: "x".to_string(),
                    events: Vec::new()
                },
                false
            )
            .is_ok()
        );
        assert!(
            validate_command(
                policy,
                &HarnessCommand::ProviderScript {
                    input: "x".to_string(),
                    responses: Vec::new(),
                    max_iterations: 1,
                },
                false
            )
            .is_ok()
        );
        assert!(
            validate_command(
                policy,
                &HarnessCommand::ProviderPrompt {
                    input: "x".to_string(),
                    max_iterations: 1,
                },
                false
            )
            .is_ok()
        );
        assert!(validate_command(policy, &HarnessCommand::Manifest, false).is_ok());
        assert!(validate_command(policy, &HarnessCommand::ListSessions, false).is_ok());
        assert!(
            validate_command(
                policy,
                &HarnessCommand::SessionEntries { limit: Some(1) },
                false
            )
            .is_ok()
        );
        assert!(
            validate_command(policy, &HarnessCommand::UserInput("x".to_string()), false).is_ok()
        );
    }

    #[test]
    fn approval_policy_rejects_unapproved_mutations() {
        let policy = CommandPolicy {
            read_only: false,
            require_approval: true,
        };
        assert!(validate_command(policy, &HarnessCommand::Reload, false).is_err());
        assert!(validate_command(policy, &HarnessCommand::Evolve("x".to_string()), false).is_err());
        assert!(validate_command(policy, &HarnessCommand::Reload, true).is_ok());
        assert!(validate_command(policy, &HarnessCommand::Evolve("x".to_string()), true).is_ok());
        assert!(validate_command(policy, &HarnessCommand::Status, false).is_ok());
    }

    #[test]
    fn policy_data_is_attached_to_structured_outcomes() {
        let mut outcome = CommandOutcome {
            display: None,
            data: Some(serde_json::json!({ "kind": "manifest" })),
            events: Vec::new(),
        };
        attach_policy_data(
            &mut outcome,
            CommandPolicy {
                read_only: true,
                require_approval: true,
            },
        );
        assert_eq!(
            outcome.data.unwrap()["policy"],
            serde_json::json!({
                "read_only": true,
                "reload_allowed": false,
                "evolve_allowed": false,
                "approval_required": true
            })
        );
    }

    #[test]
    fn rpc_provider_script_job_completes_with_harness_events() -> Result<()> {
        let root = std::env::current_dir()?;
        build_guest_in(&root)?;
        let session_path = std::env::temp_dir().join(format!(
            "piers-app-provider-script-job-{}.jsonl",
            std::process::id()
        ));
        let _ = fs_err::remove_file(&session_path);

        let mut jobs = RpcJobs::default();
        let data = jobs.start_provider_script(
            root,
            session_path.clone(),
            Some("job-a".to_string()),
            "hello".to_string(),
            vec![vec![
                ProviderEvent::ResponseStarted {
                    response_id: "r1".to_string(),
                },
                ProviderEvent::TextStarted {
                    block_id: "t1".to_string(),
                },
                ProviderEvent::TextDelta {
                    block_id: "t1".to_string(),
                    text: "done".to_string(),
                },
                ProviderEvent::TextFinished {
                    block_id: "t1".to_string(),
                },
                ProviderEvent::ResponseFinished,
            ]],
            1,
        )?;
        assert_eq!(data["status"], "running");

        let outcome = poll_until_completed(&mut jobs, "job-a")?;
        assert_eq!(outcome.display.as_deref(), Some("done"));
        let data = outcome.data.as_ref().expect("job data");
        assert_eq!(data["job_id"], "job-a");
        assert_eq!(data["status"], "completed");
        assert_eq!(data["provider_requests"], 1);
        assert!(matches!(
            outcome.events.as_slice(),
            [
                HarnessEvent::TurnStarted(_),
                HarnessEvent::UserMessageRecorded { .. },
                HarnessEvent::AssistantMessage { .. },
                HarnessEvent::TurnCompleted { success: true, .. },
            ]
        ));

        let _ = fs_err::remove_file(session_path);
        Ok(())
    }

    #[test]
    fn rpc_abort_flips_background_job_token() {
        let cancellation = CancellationToken::default();
        let (_sender, receiver) = mpsc::channel();
        let mut jobs = RpcJobs::default();
        jobs.jobs.insert(
            "job-a".to_string(),
            RpcJob {
                cancellation: cancellation.clone(),
                receiver,
            },
        );

        let data = jobs.abort("job-a");

        assert_eq!(data["accepted"], true);
        assert_eq!(data["status"], "cancelling");
        assert!(cancellation.is_cancelled());
    }

    #[test]
    fn rpc_request_maps_to_harness_command() -> Result<()> {
        let request: RpcRequest =
            serde_json::from_str(r#"{"type":"user_input","id":1,"text":"hello"}"#)?;
        assert_eq!(request.id(), Some(&Value::from(1)));
        assert_eq!(
            request.into_command()?,
            HarnessCommand::UserInput("hello".to_string())
        );
        Ok(())
    }

    #[test]
    fn rpc_provider_stream_maps_to_harness_command() -> Result<()> {
        let request: RpcRequest = serde_json::from_str(
            r#"{"type":"provider_stream","id":"p1","input":"hello","events":[{"type":"response_started","response_id":"r1"},{"type":"response_finished"}]}"#,
        )?;
        assert_eq!(request.id(), Some(&Value::from("p1")));
        assert!(matches!(
            request.into_command()?,
            HarnessCommand::ProviderStream { input, events }
                if input == "hello" && events.len() == 2
        ));
        Ok(())
    }

    #[test]
    fn rpc_provider_script_maps_to_harness_command() -> Result<()> {
        let request: RpcRequest = serde_json::from_str(
            r#"{"type":"provider_script","id":"p1","input":"hello","responses":[[{"type":"response_started","response_id":"r1"},{"type":"response_finished"}]],"max_iterations":3}"#,
        )?;
        assert_eq!(request.id(), Some(&Value::from("p1")));
        assert!(matches!(
            request.into_command()?,
            HarnessCommand::ProviderScript {
                input,
                responses,
                max_iterations,
            } if input == "hello" && responses.len() == 1 && max_iterations == 3
        ));
        Ok(())
    }

    #[test]
    fn rpc_provider_script_start_keeps_job_fields() -> Result<()> {
        let request: RpcRequest = serde_json::from_str(
            r#"{"type":"provider_script_start","id":"p1","job_id":"job-a","input":"hello","responses":[[{"type":"response_started","response_id":"r1"},{"type":"response_finished"}]],"max_iterations":3}"#,
        )?;
        assert_eq!(request.id(), Some(&Value::from("p1")));
        assert!(matches!(
            request,
            RpcRequest::ProviderScriptStart {
                job_id,
                input,
                responses,
                max_iterations,
                ..
            } if job_id.as_deref() == Some("job-a")
                && input == "hello"
                && responses.len() == 1
                && max_iterations == Some(3)
        ));
        Ok(())
    }

    #[test]
    fn rpc_provider_prompt_maps_to_harness_command() -> Result<()> {
        let request: RpcRequest = serde_json::from_str(
            r#"{"type":"provider_prompt","id":"p1","input":"hello","max_iterations":3}"#,
        )?;
        assert_eq!(request.id(), Some(&Value::from("p1")));
        assert!(matches!(
            request.into_command()?,
            HarnessCommand::ProviderPrompt {
                input,
                max_iterations,
            } if input == "hello" && max_iterations == 3
        ));
        Ok(())
    }

    #[test]
    fn rpc_provider_prompt_start_keeps_job_fields() -> Result<()> {
        let request: RpcRequest = serde_json::from_str(
            r#"{"type":"provider_prompt_start","id":"p1","job_id":"job-a","input":"hello","max_iterations":3}"#,
        )?;
        assert_eq!(request.id(), Some(&Value::from("p1")));
        assert!(matches!(
            request,
            RpcRequest::ProviderPromptStart {
                job_id,
                input,
                max_iterations,
                ..
            } if job_id.as_deref() == Some("job-a")
                && input == "hello"
                && max_iterations == Some(3)
        ));
        Ok(())
    }

    #[test]
    fn rpc_abort_maps_to_harness_command() -> Result<()> {
        let request: RpcRequest = serde_json::from_str(r#"{"type":"abort","id":"a1"}"#)?;
        assert_eq!(request.id(), Some(&Value::from("a1")));
        assert_eq!(request.into_command()?, HarnessCommand::Abort);
        Ok(())
    }

    #[test]
    fn rpc_abort_can_target_background_job() -> Result<()> {
        let request: RpcRequest =
            serde_json::from_str(r#"{"type":"abort","id":"a1","job_id":"job-a"}"#)?;
        assert_eq!(request.id(), Some(&Value::from("a1")));
        assert!(matches!(
            request,
            RpcRequest::Abort { job_id, .. } if job_id.as_deref() == Some("job-a")
        ));
        Ok(())
    }

    #[test]
    fn rpc_job_status_keeps_job_id() -> Result<()> {
        let request: RpcRequest =
            serde_json::from_str(r#"{"type":"job_status","id":"j1","job_id":"job-a"}"#)?;
        assert_eq!(request.id(), Some(&Value::from("j1")));
        assert!(matches!(
            request,
            RpcRequest::JobStatus { job_id, .. } if job_id == "job-a"
        ));
        Ok(())
    }

    #[test]
    fn rpc_provider_status_maps_to_harness_command() -> Result<()> {
        let request: RpcRequest = serde_json::from_str(r#"{"type":"provider_status","id":"p1"}"#)?;
        assert_eq!(request.id(), Some(&Value::from("p1")));
        assert_eq!(request.into_command()?, HarnessCommand::ProviderStatus);
        Ok(())
    }

    #[test]
    fn rpc_manifest_maps_to_harness_command() -> Result<()> {
        let request: RpcRequest = serde_json::from_str(r#"{"type":"manifest","id":"m1"}"#)?;
        assert_eq!(request.id(), Some(&Value::from("m1")));
        assert_eq!(request.into_command()?, HarnessCommand::Manifest);
        Ok(())
    }

    #[test]
    fn rpc_list_sessions_maps_to_harness_command() -> Result<()> {
        let request: RpcRequest = serde_json::from_str(r#"{"type":"list_sessions","id":"s1"}"#)?;
        assert_eq!(request.id(), Some(&Value::from("s1")));
        assert_eq!(request.into_command()?, HarnessCommand::ListSessions);
        Ok(())
    }

    #[test]
    fn rpc_session_entries_maps_to_harness_command() -> Result<()> {
        let request: RpcRequest =
            serde_json::from_str(r#"{"type":"session_entries","id":"s1","limit":5}"#)?;
        assert_eq!(request.id(), Some(&Value::from("s1")));
        assert_eq!(
            request.into_command()?,
            HarnessCommand::SessionEntries { limit: Some(5) }
        );
        Ok(())
    }

    #[test]
    fn rpc_switch_session_keeps_request_id_and_path() -> Result<()> {
        let request: RpcRequest =
            serde_json::from_str(r#"{"type":"switch_session","id":"s1","path":"x.jsonl"}"#)?;
        assert_eq!(request.id(), Some(&Value::from("s1")));
        assert!(matches!(
            request,
            RpcRequest::SwitchSession { path, .. } if path == "x.jsonl"
        ));
        Ok(())
    }

    #[test]
    fn rpc_rejects_unknown_fields() {
        let error =
            serde_json::from_str::<RpcRequest>(r#"{"type":"status","id":"s1","extra":true}"#)
                .expect_err("unknown request fields must be rejected");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn rpc_outcome_writes_events_then_result() -> Result<()> {
        let outcome = CommandOutcome {
            display: Some("done".to_string()),
            data: Some(serde_json::json!({ "ok": true })),
            events: vec![HarnessEvent::AssistantMessage {
                text: "done".to_string(),
            }],
        };
        let mut output = Vec::new();
        write_rpc_outcome(&mut output, Some(&Value::from("a")), &outcome)?;
        let output = String::from_utf8(output)?;

        assert!(output.contains(r#""type":"event""#));
        assert!(output.contains(r#""assistant_message""#));
        assert!(output.contains(r#""type":"result""#));
        assert!(output.contains(r#""display":"done""#));
        assert!(output.contains(r#""data":{"ok":true}"#));
        Ok(())
    }

    #[test]
    fn rpc_error_writes_stable_code() -> Result<()> {
        let mut output = Vec::new();
        write_rpc_error(
            &mut output,
            Some(&Value::from("r1")),
            RpcErrorCode::ApprovalRequired,
            "command requires approval".to_string(),
        )?;
        let output = String::from_utf8(output)?;

        assert!(output.contains(r#""type":"error""#));
        assert!(output.contains(r#""id":"r1""#));
        assert!(output.contains(r#""code":"approval_required""#));
        assert!(output.contains(r#""message":"command requires approval""#));
        Ok(())
    }

    #[test]
    fn classify_policy_errors() {
        assert_eq!(
            classify_error(&anyhow!("command is disabled by --read-only")),
            RpcErrorCode::ReadOnly
        );
        assert_eq!(
            classify_error(&anyhow!("command requires approval")),
            RpcErrorCode::ApprovalRequired
        );
        assert_eq!(classify_error(&anyhow!("boom")), RpcErrorCode::Harness);
    }

    #[test]
    fn render_repl_event_surfaces_tool_progress() {
        let line = render_repl_event(&HarnessEvent::ToolCallRequested {
            call_id: "read-1".to_string(),
            tool_name: "read".to_string(),
        });
        assert_eq!(line, Some("tool read requested (read-1)".to_string()));
    }

    fn poll_until_completed(jobs: &mut RpcJobs, job_id: &str) -> Result<CommandOutcome> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match jobs.poll(job_id) {
                JobPoll::Completed(outcome) => return Ok(outcome),
                JobPoll::Failed(error) => bail!("{error}"),
                JobPoll::Running(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                JobPoll::Running(_) => bail!("job {job_id} did not complete before deadline"),
            }
        }
    }
}
