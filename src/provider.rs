//! Host-owned provider stream normalization.
//!
//! Provider adapters eventually translate OpenAI, Anthropic, Gemini, local
//! servers, or fake test streams into these events. The reducer here owns the
//! invariants that durable session replay needs: content ordering, final tool
//! arguments, and exactly one terminal state.

#![allow(dead_code)]

use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::tools::builtin_tool_definitions;

const DEFAULT_PROVIDER_RETRY_ATTEMPTS: usize = 3;
const DEFAULT_PROVIDER_TIMEOUT_MS: u64 = 120_000;
const RETRYABLE_HTTP_STATUSES: [u16; 6] = [408, 429, 500, 502, 503, 504];

/// A normalized event from a provider response stream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderEvent {
    ResponseStarted { response_id: String },
    TextStarted { block_id: String },
    TextDelta { block_id: String, text: String },
    TextFinished { block_id: String },
    ThinkingStarted { block_id: String },
    ThinkingDelta { block_id: String, text: String },
    ThinkingFinished { block_id: String },
    ToolCallStarted { call_id: String, tool_name: String },
    ToolCallArgumentsDelta { call_id: String, json_delta: String },
    ToolCallFinished { call_id: String },
    ResponseFinished,
    ResponseFailed { message: String },
    ResponseAborted { reason: String },
    StreamIncomplete { message: String },
}

/// Terminal state for a normalized provider response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderTerminal {
    Finished,
    Failed { message: String },
    Aborted { reason: String },
    Incomplete { message: String },
}

/// Completed host-facing provider output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderResponse {
    pub response_id: String,
    pub text: String,
    pub thinking: Vec<ThinkingBlock>,
    pub tool_calls: Vec<CompletedToolCall>,
    pub terminal: ProviderTerminal,
}

/// Normalized thinking content retained for provider-compatible replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThinkingBlock {
    pub block_id: String,
    pub text: String,
}

/// Final, validated tool call emitted by a provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletedToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments_json: String,
}

/// Tool result sent from the host back into a provider loop.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderToolResult {
    pub call_id: String,
    pub output_json: Value,
    pub success: bool,
}

/// One provider request context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderTurnInput {
    pub user_input: String,
    pub tool_results: Vec<ProviderToolResult>,
}

/// Host-visible provider capability and configuration status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderRuntimeStatus {
    pub mode: String,
    pub provider_adapter: Option<String>,
    pub model: Option<String>,
    pub transport: String,
    pub llm_loop: bool,
    pub config: ProviderConfigStatus,
    pub retry_policy: RetryPolicyStatus,
    pub cancellation: CancellationStatus,
    pub scripted_loop: ScriptedLoopStatus,
    pub stream_reducer: StreamReducerStatus,
    pub missing: Vec<String>,
}

/// Sanitized provider configuration status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderConfigStatus {
    pub source: &'static str,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key_present: bool,
    pub timeout_ms: u64,
    pub complete: bool,
    pub missing: Vec<String>,
}

/// Status for the deterministic provider/tool loop.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScriptedLoopStatus {
    pub available: bool,
    pub default_max_iterations: usize,
    pub network_provider: bool,
}

/// Status for normalized provider stream reduction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StreamReducerStatus {
    pub available: bool,
    pub terminal_states: [&'static str; 4],
}

/// Status for retry behavior around provider transport.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RetryPolicyStatus {
    pub available: bool,
    pub max_attempts: usize,
    pub retryable_http_statuses: [u16; 6],
}

/// Status for cancellation support.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CancellationStatus {
    pub cooperative: bool,
    pub async_abort: bool,
}

/// Cooperative cancellation token shared with provider adapters.
#[derive(Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Requests cancellation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Returns whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Fails if cancellation has been requested.
    pub fn ensure_not_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            bail!("operation cancelled");
        }
        Ok(())
    }
}

/// Returns the current provider capability and configuration status.
pub fn provider_runtime_status() -> ProviderRuntimeStatus {
    provider_runtime_status_from_env(env::vars())
}

/// Returns provider capability and configuration status from env-like values.
pub fn provider_runtime_status_from_env(
    vars: impl IntoIterator<Item = (String, String)>,
) -> ProviderRuntimeStatus {
    let config = ProviderConfig::from_env(vars);
    let network = config.network_config();
    let config = config.status();
    let network_provider = network.is_ok();
    let provider_adapter = config
        .complete
        .then(|| config.provider.clone())
        .flatten()
        .or_else(|| config.provider.clone());
    let model = config.complete.then(|| config.model.clone()).flatten();
    let mut missing = vec!["blocking_transport_cancellation".to_string()];
    if !network_provider {
        missing.push("network_provider_adapter".to_string());
    }
    missing.extend(config.missing.iter().cloned());

    ProviderRuntimeStatus {
        mode: "guest_harness".to_string(),
        provider_adapter,
        model,
        transport: if network_provider {
            "configured"
        } else {
            "not_configured"
        }
        .to_string(),
        llm_loop: network_provider,
        config,
        retry_policy: RetryPolicyStatus {
            available: true,
            max_attempts: DEFAULT_PROVIDER_RETRY_ATTEMPTS,
            retryable_http_statuses: RETRYABLE_HTTP_STATUSES,
        },
        cancellation: CancellationStatus {
            cooperative: true,
            async_abort: true,
        },
        scripted_loop: ScriptedLoopStatus {
            available: true,
            default_max_iterations: 16,
            network_provider,
        },
        stream_reducer: StreamReducerStatus {
            available: true,
            terminal_states: ["finished", "failed", "aborted", "incomplete"],
        },
        missing,
    }
}

/// Converts provider status into stable JSON for RPC and manifests.
pub fn provider_runtime_status_json() -> Value {
    serde_json::to_value(provider_runtime_status())
        .unwrap_or_else(|error| json!({ "error": error.to_string() }))
}

#[derive(Default)]
struct ProviderConfig {
    provider: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    timeout_ms: Option<u64>,
    timeout_invalid: bool,
}

impl ProviderConfig {
    fn from_env(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut config = Self::default();
        for (key, value) in vars {
            if value.trim().is_empty() {
                continue;
            }
            match key.as_str() {
                "PIERS_PROVIDER" => config.provider = Some(value),
                "PIERS_MODEL" => config.model = Some(value),
                "PIERS_BASE_URL" => config.base_url = Some(value),
                "PIERS_API_KEY" => config.api_key = Some(value),
                "PIERS_PROVIDER_TIMEOUT_MS" => match value.parse::<u64>() {
                    Ok(timeout) if timeout > 0 => config.timeout_ms = Some(timeout),
                    _ => config.timeout_invalid = true,
                },
                _ => {}
            }
        }
        config
    }

    fn from_process_env() -> Self {
        Self::from_env(env::vars())
    }

    fn status(&self) -> ProviderConfigStatus {
        let mut missing = Vec::new();
        if self.provider.is_none() {
            missing.push("provider".to_string());
        }
        if self.model.is_none() {
            missing.push("model".to_string());
        }
        if self.base_url.is_none() {
            missing.push("base_url".to_string());
        }
        if self.api_key.is_none() {
            missing.push("api_key".to_string());
        }
        if self.timeout_invalid {
            missing.push("timeout_ms".to_string());
        }
        ProviderConfigStatus {
            source: "environment",
            provider: self.provider.clone(),
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            api_key_present: self.api_key.is_some(),
            timeout_ms: self.timeout_ms.unwrap_or(DEFAULT_PROVIDER_TIMEOUT_MS),
            complete: missing.is_empty(),
            missing,
        }
    }

    fn network_config(&self) -> Result<OpenAiCompatibleConfig> {
        let provider = self
            .provider
            .as_deref()
            .ok_or_else(|| anyhow!("PIERS_PROVIDER is required"))?;
        if provider != "openai_compatible" {
            bail!("unsupported provider adapter {provider}; expected openai_compatible");
        }
        let model = self
            .model
            .clone()
            .ok_or_else(|| anyhow!("PIERS_MODEL is required"))?;
        let base_url = self
            .base_url
            .clone()
            .ok_or_else(|| anyhow!("PIERS_BASE_URL is required"))?;
        let api_key = self
            .api_key
            .clone()
            .ok_or_else(|| anyhow!("PIERS_API_KEY is required"))?;
        if self.timeout_invalid {
            bail!("PIERS_PROVIDER_TIMEOUT_MS must be a positive integer");
        }
        OpenAiCompatibleConfig::new(
            model,
            base_url,
            api_key,
            self.timeout_ms.unwrap_or(DEFAULT_PROVIDER_TIMEOUT_MS),
        )
    }
}

/// Creates an OpenAI-compatible provider adapter from process environment.
pub fn openai_compatible_provider_from_env() -> Result<OpenAiCompatibleProvider> {
    OpenAiCompatibleProvider::new(ProviderConfig::from_process_env().network_config()?)
}

/// Host-owned provider adapter boundary.
pub trait ProviderAdapter {
    /// Returns normalized provider events for the next model response.
    fn next_response(
        &mut self,
        input: ProviderTurnInput,
        cancellation: &CancellationToken,
    ) -> Result<Vec<ProviderEvent>>;
}

/// Deterministic provider adapter for fixtures, tests, and local RPC shims.
pub struct ScriptedProvider {
    responses: VecDeque<Vec<ProviderEvent>>,
    requests: Vec<ProviderTurnInput>,
}

impl ScriptedProvider {
    /// Creates a provider that returns each response script in order.
    pub fn new(responses: Vec<Vec<ProviderEvent>>) -> Self {
        Self {
            responses: responses.into(),
            requests: Vec::new(),
        }
    }

    /// Returns every request context observed by this scripted provider.
    pub fn requests(&self) -> &[ProviderTurnInput] {
        &self.requests
    }
}

impl ProviderAdapter for ScriptedProvider {
    fn next_response(
        &mut self,
        input: ProviderTurnInput,
        cancellation: &CancellationToken,
    ) -> Result<Vec<ProviderEvent>> {
        cancellation.ensure_not_cancelled()?;
        self.requests.push(input);
        self.responses
            .pop_front()
            .context("scripted provider has no response left")
    }
}

/// Configuration for OpenAI-compatible chat-completions providers.
pub struct OpenAiCompatibleConfig {
    model: String,
    endpoint: String,
    api_key: String,
    retry_attempts: usize,
    timeout_ms: u64,
}

impl OpenAiCompatibleConfig {
    fn new(model: String, base_url: String, api_key: String, timeout_ms: u64) -> Result<Self> {
        let base_url = base_url.trim_end_matches('/');
        if base_url.is_empty() {
            bail!("PIERS_BASE_URL must not be empty");
        }
        if timeout_ms == 0 {
            bail!("provider timeout must be greater than zero");
        }
        Ok(Self {
            model,
            endpoint: format!("{base_url}/chat/completions"),
            api_key,
            retry_attempts: DEFAULT_PROVIDER_RETRY_ATTEMPTS,
            timeout_ms,
        })
    }
}

/// Minimal OpenAI-compatible non-streaming provider adapter.
pub struct OpenAiCompatibleProvider {
    config: OpenAiCompatibleConfig,
    client: reqwest::blocking::Client,
    messages: Vec<Value>,
    user_recorded: bool,
}

impl OpenAiCompatibleProvider {
    fn new(config: OpenAiCompatibleConfig) -> Result<Self> {
        Ok(Self {
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_millis(config.timeout_ms))
                .build()
                .context("build OpenAI-compatible HTTP client")?,
            config,
            messages: Vec::new(),
            user_recorded: false,
        })
    }
}

impl ProviderAdapter for OpenAiCompatibleProvider {
    fn next_response(
        &mut self,
        input: ProviderTurnInput,
        cancellation: &CancellationToken,
    ) -> Result<Vec<ProviderEvent>> {
        cancellation.ensure_not_cancelled()?;
        if !self.user_recorded {
            self.messages
                .push(json!({ "role": "user", "content": input.user_input }));
            self.user_recorded = true;
        }
        for result in input.tool_results {
            self.messages.push(json!({
                "role": "tool",
                "tool_call_id": result.call_id,
                "content": result.output_json.to_string(),
            }));
        }

        let body = json!({
            "model": self.config.model.clone(),
            "messages": self.messages.clone(),
            "tools": openai_tool_definitions(),
        });
        let response = self.send_chat_completion(&body, cancellation)?;
        cancellation.ensure_not_cancelled()?;
        let message = response
            .pointer("/choices/0/message")
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow!("OpenAI-compatible response missing choices[0].message"))?;
        self.messages.push(Value::Object(message.clone()));
        normalize_openai_chat_response(&response)
    }
}

impl OpenAiCompatibleProvider {
    fn send_chat_completion(
        &self,
        body: &Value,
        cancellation: &CancellationToken,
    ) -> Result<Value> {
        for attempt in 1..=self.config.retry_attempts {
            cancellation.ensure_not_cancelled()?;
            let response = self
                .client
                .post(&self.config.endpoint)
                .bearer_auth(&self.config.api_key)
                .json(body)
                .send();

            match response {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        cancellation.ensure_not_cancelled()?;
                        return response
                            .json::<Value>()
                            .context("decode OpenAI-compatible chat completion");
                    }
                    if should_retry_status(status.as_u16()) && attempt < self.config.retry_attempts
                    {
                        sleep_with_cancellation(provider_retry_delay(attempt), cancellation)?;
                        continue;
                    }
                    return response
                        .error_for_status()
                        .context("OpenAI-compatible chat completion failed")?
                        .json::<Value>()
                        .context("decode OpenAI-compatible chat completion");
                }
                Err(error)
                    if should_retry_transport_error(&error)
                        && attempt < self.config.retry_attempts =>
                {
                    sleep_with_cancellation(provider_retry_delay(attempt), cancellation)?;
                }
                Err(error) => {
                    return Err(error).context("send OpenAI-compatible chat completion");
                }
            }
        }
        bail!("OpenAI-compatible chat completion retry loop exhausted")
    }
}

fn openai_tool_definitions() -> Vec<Value> {
    builtin_tool_definitions()
        .into_iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                }
            })
        })
        .collect()
}

fn normalize_openai_chat_response(response: &Value) -> Result<Vec<ProviderEvent>> {
    let response_id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("openai-compatible-response")
        .to_string();
    let message = response
        .pointer("/choices/0/message")
        .ok_or_else(|| anyhow!("OpenAI-compatible response missing choices[0].message"))?;
    let mut events = vec![ProviderEvent::ResponseStarted { response_id }];

    if let Some(text) = message.get("content").and_then(Value::as_str)
        && !text.is_empty()
    {
        events.push(ProviderEvent::TextStarted {
            block_id: "text-1".to_string(),
        });
        events.push(ProviderEvent::TextDelta {
            block_id: "text-1".to_string(),
            text: text.to_string(),
        });
        events.push(ProviderEvent::TextFinished {
            block_id: "text-1".to_string(),
        });
    }

    for tool_call in message
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let call_id = tool_call
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("OpenAI-compatible tool call missing id"))?
            .to_string();
        let tool_name = tool_call
            .pointer("/function/name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("OpenAI-compatible tool call missing function.name"))?
            .to_string();
        let arguments = tool_call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}")
            .to_string();
        events.push(ProviderEvent::ToolCallStarted {
            call_id: call_id.clone(),
            tool_name,
        });
        events.push(ProviderEvent::ToolCallArgumentsDelta {
            call_id: call_id.clone(),
            json_delta: arguments,
        });
        events.push(ProviderEvent::ToolCallFinished { call_id });
    }

    events.push(ProviderEvent::ResponseFinished);
    Ok(events)
}

fn should_retry_status(status: u16) -> bool {
    RETRYABLE_HTTP_STATUSES.contains(&status)
}

fn should_retry_transport_error(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

fn provider_retry_delay(attempt: usize) -> Duration {
    Duration::from_millis((attempt as u64).saturating_mul(100))
}

fn sleep_with_cancellation(duration: Duration, cancellation: &CancellationToken) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < duration {
        cancellation.ensure_not_cancelled()?;
        let remaining = duration.saturating_sub(start.elapsed());
        sleep(remaining.min(Duration::from_millis(25)));
    }
    cancellation.ensure_not_cancelled()
}

/// Reducer for one provider response stream.
#[derive(Default)]
pub struct ProviderStreamReducer {
    response_id: Option<String>,
    text_blocks: BTreeMap<String, OpenTextBlock>,
    thinking_blocks: BTreeMap<String, OpenTextBlock>,
    tool_calls: BTreeMap<String, OpenToolCall>,
    terminal: Option<ProviderTerminal>,
}

impl ProviderStreamReducer {
    /// Applies one normalized event.
    pub fn push(&mut self, event: ProviderEvent) -> Result<()> {
        if self.terminal.is_some() {
            bail!("provider stream already reached a terminal state");
        }
        if !matches!(event, ProviderEvent::ResponseStarted { .. }) && self.response_id.is_none() {
            bail!("provider stream event arrived before response_started");
        }

        match event {
            ProviderEvent::ResponseStarted { response_id } => {
                if self.response_id.replace(response_id).is_some() {
                    bail!("provider stream started twice");
                }
            }
            ProviderEvent::TextStarted { block_id } => {
                insert_open_block(&mut self.text_blocks, block_id)?;
            }
            ProviderEvent::TextDelta { block_id, text } => {
                open_block_mut(&mut self.text_blocks, &block_id)?.push_str(&text);
            }
            ProviderEvent::TextFinished { block_id } => {
                open_block_mut(&mut self.text_blocks, &block_id)?.finish()?;
            }
            ProviderEvent::ThinkingStarted { block_id } => {
                insert_open_block(&mut self.thinking_blocks, block_id)?;
            }
            ProviderEvent::ThinkingDelta { block_id, text } => {
                open_block_mut(&mut self.thinking_blocks, &block_id)?.push_str(&text);
            }
            ProviderEvent::ThinkingFinished { block_id } => {
                open_block_mut(&mut self.thinking_blocks, &block_id)?.finish()?;
            }
            ProviderEvent::ToolCallStarted { call_id, tool_name } => {
                if self
                    .tool_calls
                    .insert(call_id, OpenToolCall::new(tool_name))
                    .is_some()
                {
                    bail!("tool call started twice");
                }
            }
            ProviderEvent::ToolCallArgumentsDelta {
                call_id,
                json_delta,
            } => {
                let call = self
                    .tool_calls
                    .get_mut(&call_id)
                    .ok_or_else(|| anyhow::anyhow!("tool call arguments before start"))?;
                call.arguments_json.push_str(&json_delta);
            }
            ProviderEvent::ToolCallFinished { call_id } => {
                let call = self
                    .tool_calls
                    .get_mut(&call_id)
                    .ok_or_else(|| anyhow::anyhow!("tool call finished before start"))?;
                call.finish()?;
            }
            ProviderEvent::ResponseFinished => {
                self.validate_finish()?;
                self.terminal = Some(ProviderTerminal::Finished);
            }
            ProviderEvent::ResponseFailed { message } => {
                self.terminal = Some(ProviderTerminal::Failed { message });
            }
            ProviderEvent::ResponseAborted { reason } => {
                self.terminal = Some(ProviderTerminal::Aborted { reason });
            }
            ProviderEvent::StreamIncomplete { message } => {
                self.terminal = Some(ProviderTerminal::Incomplete { message });
            }
        }
        Ok(())
    }

    /// Finishes the stream and returns host-facing output.
    pub fn finish(self) -> Result<ProviderResponse> {
        if matches!(self.terminal, Some(ProviderTerminal::Finished)) {
            self.validate_finish()?;
        }
        let terminal = self
            .terminal
            .ok_or_else(|| anyhow::anyhow!("provider stream ended without terminal event"))?;

        let response_id = self
            .response_id
            .ok_or_else(|| anyhow::anyhow!("provider stream missing response id"))?;
        let text = self
            .text_blocks
            .into_values()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("");
        let thinking = self
            .thinking_blocks
            .into_iter()
            .map(|(block_id, block)| ThinkingBlock {
                block_id,
                text: block.text,
            })
            .collect();
        let tool_calls = self
            .tool_calls
            .into_iter()
            .map(|(call_id, call)| CompletedToolCall {
                call_id,
                tool_name: call.tool_name,
                arguments_json: call.arguments_json,
            })
            .collect();

        Ok(ProviderResponse {
            response_id,
            text,
            thinking,
            tool_calls,
            terminal,
        })
    }

    fn validate_finish(&self) -> Result<()> {
        for (block_id, block) in &self.text_blocks {
            if !block.finished {
                bail!("text block {block_id} was not finished");
            }
        }
        for (block_id, block) in &self.thinking_blocks {
            if !block.finished {
                bail!("thinking block {block_id} was not finished");
            }
        }
        for (call_id, call) in &self.tool_calls {
            if !call.finished {
                bail!("tool call {call_id} was not finished");
            }
            serde_json::from_str::<serde_json::Value>(&call.arguments_json).map_err(|error| {
                anyhow::anyhow!("tool call {call_id} has invalid JSON: {error}")
            })?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct OpenTextBlock {
    text: String,
    finished: bool,
}

impl OpenTextBlock {
    fn push_str(&mut self, text: &str) {
        self.text.push_str(text);
    }

    fn finish(&mut self) -> Result<()> {
        if self.finished {
            bail!("content block finished twice");
        }
        self.finished = true;
        Ok(())
    }
}

struct OpenToolCall {
    tool_name: String,
    arguments_json: String,
    finished: bool,
}

impl OpenToolCall {
    fn new(tool_name: String) -> Self {
        Self {
            tool_name,
            arguments_json: String::new(),
            finished: false,
        }
    }

    fn finish(&mut self) -> Result<()> {
        if self.finished {
            bail!("tool call finished twice");
        }
        self.finished = true;
        Ok(())
    }
}

fn insert_open_block(blocks: &mut BTreeMap<String, OpenTextBlock>, block_id: String) -> Result<()> {
    if blocks.insert(block_id, OpenTextBlock::default()).is_some() {
        bail!("content block started twice");
    }
    Ok(())
}

fn open_block_mut<'a>(
    blocks: &'a mut BTreeMap<String, OpenTextBlock>,
    block_id: &str,
) -> Result<&'a mut OpenTextBlock> {
    blocks
        .get_mut(block_id)
        .ok_or_else(|| anyhow::anyhow!("content delta before start"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_status(vars: &[(&str, &str)]) -> ProviderRuntimeStatus {
        provider_runtime_status_from_env(
            vars.iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string())),
        )
    }

    fn reduce(events: Vec<ProviderEvent>) -> Result<ProviderResponse> {
        let mut reducer = ProviderStreamReducer::default();
        for event in events {
            reducer.push(event)?;
        }
        reducer.finish()
    }

    #[test]
    fn reduces_text_and_tool_call_stream() -> Result<()> {
        let response = reduce(vec![
            ProviderEvent::ResponseStarted {
                response_id: "r1".to_string(),
            },
            ProviderEvent::TextStarted {
                block_id: "text-1".to_string(),
            },
            ProviderEvent::TextDelta {
                block_id: "text-1".to_string(),
                text: "hello ".to_string(),
            },
            ProviderEvent::TextDelta {
                block_id: "text-1".to_string(),
                text: "world".to_string(),
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
        ])?;

        assert_eq!(response.text, "hello world");
        assert_eq!(response.tool_calls[0].tool_name, "read");
        assert_eq!(
            response.tool_calls[0].arguments_json,
            r#"{"path":"Cargo.toml"}"#
        );
        assert_eq!(response.terminal, ProviderTerminal::Finished);
        Ok(())
    }

    #[test]
    fn provider_status_reports_missing_env_config_without_secret_values() {
        let status = env_status(&[]);

        assert_eq!(status.transport, "not_configured");
        assert_eq!(status.provider_adapter, None);
        assert_eq!(status.model, None);
        assert!(!status.config.complete);
        assert!(!status.config.api_key_present);
        assert!(status.config.missing.contains(&"provider".to_string()));
        assert!(status.config.missing.contains(&"model".to_string()));
        assert!(status.config.missing.contains(&"base_url".to_string()));
        assert!(status.config.missing.contains(&"api_key".to_string()));
        assert_eq!(status.config.timeout_ms, DEFAULT_PROVIDER_TIMEOUT_MS);
    }

    #[test]
    fn provider_status_reports_complete_env_config_without_leaking_key() {
        let status = env_status(&[
            ("PIERS_PROVIDER", "openai_compatible"),
            ("PIERS_MODEL", "test-model"),
            ("PIERS_BASE_URL", "https://example.invalid/v1"),
            ("PIERS_API_KEY", "secret-value"),
        ]);
        let json = serde_json::to_string(&status).expect("serialize provider status");

        assert_eq!(status.transport, "configured");
        assert_eq!(
            status.provider_adapter,
            Some("openai_compatible".to_string())
        );
        assert_eq!(status.model, Some("test-model".to_string()));
        assert!(status.llm_loop);
        assert!(status.retry_policy.available);
        assert!(status.cancellation.cooperative);
        assert!(status.cancellation.async_abort);
        assert_eq!(
            status.retry_policy.max_attempts,
            DEFAULT_PROVIDER_RETRY_ATTEMPTS
        );
        assert!(status.config.complete);
        assert!(status.config.api_key_present);
        assert_eq!(status.config.timeout_ms, DEFAULT_PROVIDER_TIMEOUT_MS);
        assert!(!json.contains("secret-value"));
        assert!(
            !status
                .missing
                .contains(&"network_provider_adapter".to_string())
        );
        assert!(
            status
                .missing
                .contains(&"blocking_transport_cancellation".to_string())
        );
        assert!(!status.missing.contains(&"retry_policy".to_string()));
    }

    #[test]
    fn provider_status_accepts_custom_timeout() {
        let status = env_status(&[
            ("PIERS_PROVIDER", "openai_compatible"),
            ("PIERS_MODEL", "test-model"),
            ("PIERS_BASE_URL", "https://example.invalid/v1"),
            ("PIERS_API_KEY", "secret-value"),
            ("PIERS_PROVIDER_TIMEOUT_MS", "2500"),
        ]);

        assert_eq!(status.transport, "configured");
        assert_eq!(status.config.timeout_ms, 2500);
        assert!(status.config.complete);
    }

    #[test]
    fn provider_status_rejects_invalid_timeout_config() {
        let status = env_status(&[
            ("PIERS_PROVIDER", "openai_compatible"),
            ("PIERS_MODEL", "test-model"),
            ("PIERS_BASE_URL", "https://example.invalid/v1"),
            ("PIERS_API_KEY", "secret-value"),
            ("PIERS_PROVIDER_TIMEOUT_MS", "0"),
        ]);

        assert_eq!(status.transport, "not_configured");
        assert!(!status.config.complete);
        assert!(status.config.missing.contains(&"timeout_ms".to_string()));
        assert!(
            status
                .missing
                .contains(&"network_provider_adapter".to_string())
        );
    }

    #[test]
    fn normalizes_openai_chat_text_response() -> Result<()> {
        let events = normalize_openai_chat_response(&json!({
            "id": "chatcmpl-1",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "hello"
                }
            }]
        }))?;

        assert_eq!(
            events,
            vec![
                ProviderEvent::ResponseStarted {
                    response_id: "chatcmpl-1".to_string()
                },
                ProviderEvent::TextStarted {
                    block_id: "text-1".to_string()
                },
                ProviderEvent::TextDelta {
                    block_id: "text-1".to_string(),
                    text: "hello".to_string()
                },
                ProviderEvent::TextFinished {
                    block_id: "text-1".to_string()
                },
                ProviderEvent::ResponseFinished,
            ]
        );
        Ok(())
    }

    #[test]
    fn retry_policy_classifies_transient_statuses() {
        for status in RETRYABLE_HTTP_STATUSES {
            assert!(should_retry_status(status), "{status} should retry");
        }
        for status in [400, 401, 403, 404, 422] {
            assert!(!should_retry_status(status), "{status} should not retry");
        }
    }

    #[test]
    fn retry_delay_scales_with_attempt_number() {
        assert_eq!(provider_retry_delay(1), Duration::from_millis(100));
        assert_eq!(provider_retry_delay(3), Duration::from_millis(300));
    }

    #[test]
    fn cancellation_token_reports_cancelled_operations() {
        let cancellation = CancellationToken::default();
        assert!(!cancellation.is_cancelled());
        assert!(cancellation.ensure_not_cancelled().is_ok());

        cancellation.cancel();

        assert!(cancellation.is_cancelled());
        assert!(cancellation.ensure_not_cancelled().is_err());
    }

    #[test]
    fn scripted_provider_respects_cancellation_before_request() {
        let mut provider = ScriptedProvider::new(vec![vec![ProviderEvent::ResponseStarted {
            response_id: "r1".to_string(),
        }]]);
        let cancellation = CancellationToken::default();
        cancellation.cancel();

        let result = provider.next_response(
            ProviderTurnInput {
                user_input: "hello".to_string(),
                tool_results: Vec::new(),
            },
            &cancellation,
        );

        assert!(result.is_err());
        assert!(provider.requests().is_empty());
    }

    #[test]
    fn cancellable_sleep_returns_when_cancelled() {
        let cancellation = CancellationToken::default();
        cancellation.cancel();

        let result = sleep_with_cancellation(Duration::from_millis(100), &cancellation);

        assert!(result.is_err());
    }

    #[test]
    fn normalizes_openai_chat_tool_calls() -> Result<()> {
        let events = normalize_openai_chat_response(&json!({
            "id": "chatcmpl-1",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {
                            "name": "read",
                            "arguments": "{\"path\":\"Cargo.toml\"}"
                        }
                    }]
                }
            }]
        }))?;

        assert_eq!(
            events,
            vec![
                ProviderEvent::ResponseStarted {
                    response_id: "chatcmpl-1".to_string()
                },
                ProviderEvent::ToolCallStarted {
                    call_id: "call-1".to_string(),
                    tool_name: "read".to_string()
                },
                ProviderEvent::ToolCallArgumentsDelta {
                    call_id: "call-1".to_string(),
                    json_delta: "{\"path\":\"Cargo.toml\"}".to_string()
                },
                ProviderEvent::ToolCallFinished {
                    call_id: "call-1".to_string()
                },
                ProviderEvent::ResponseFinished,
            ]
        );
        Ok(())
    }

    #[test]
    fn rejects_events_before_response_start() {
        let mut reducer = ProviderStreamReducer::default();
        let result = reducer.push(ProviderEvent::ResponseFinished);

        assert!(result.is_err());
    }

    #[test]
    fn rejects_missing_terminal_event() {
        let result = reduce(vec![ProviderEvent::ResponseStarted {
            response_id: "r1".to_string(),
        }]);

        assert!(result.is_err());
    }

    #[test]
    fn rejects_open_text_on_success() {
        let result = reduce(vec![
            ProviderEvent::ResponseStarted {
                response_id: "r1".to_string(),
            },
            ProviderEvent::TextStarted {
                block_id: "text-1".to_string(),
            },
            ProviderEvent::ResponseFinished,
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn incomplete_stream_is_terminal_but_not_success() -> Result<()> {
        let response = reduce(vec![
            ProviderEvent::ResponseStarted {
                response_id: "r1".to_string(),
            },
            ProviderEvent::StreamIncomplete {
                message: "socket closed".to_string(),
            },
        ])?;

        assert_eq!(
            response.terminal,
            ProviderTerminal::Incomplete {
                message: "socket closed".to_string()
            }
        );
        Ok(())
    }

    #[test]
    fn rejects_invalid_final_tool_arguments() {
        let result = reduce(vec![
            ProviderEvent::ResponseStarted {
                response_id: "r1".to_string(),
            },
            ProviderEvent::ToolCallStarted {
                call_id: "call-1".to_string(),
                tool_name: "read".to_string(),
            },
            ProviderEvent::ToolCallArgumentsDelta {
                call_id: "call-1".to_string(),
                json_delta: "{".to_string(),
            },
            ProviderEvent::ToolCallFinished {
                call_id: "call-1".to_string(),
            },
            ProviderEvent::ResponseFinished,
        ]);

        assert!(result.is_err());
    }
}
