//! Minimal host-owned coding tools.
//!
//! Tools run in the native host, behind structured JSON inputs and local
//! policy checks. Reloadable guests can request tools, but this module owns the
//! filesystem and process effects.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Host-owned tool metadata and input schema.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// Returns the built-in host tool definitions.
pub fn builtin_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "read",
            description: "Read UTF-8 text from a workspace-relative path.",
            input_schema: json_schema(&[("path", "string")]),
        },
        ToolDefinition {
            name: "write",
            description: "Write UTF-8 text to a workspace-relative path.",
            input_schema: json_schema(&[("path", "string"), ("text", "string")]),
        },
        ToolDefinition {
            name: "edit",
            description: "Replace exactly one text occurrence in a workspace file.",
            input_schema: json_schema(&[("path", "string"), ("old", "string"), ("new", "string")]),
        },
        ToolDefinition {
            name: "exec",
            description: "Run an allow-listed noninteractive program with a timeout.",
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["program"],
                "properties": {
                    "program": { "type": "string" },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "timeout_ms": { "type": "integer", "minimum": 1 }
                }
            }),
        },
    ]
}

/// Host tool executor scoped to one workspace root.
pub struct HostTools {
    root: PathBuf,
    policy: ToolPolicy,
}

impl HostTools {
    /// Creates tools with filesystem access scoped to `root` and exec disabled.
    pub fn new(root: PathBuf) -> Result<Self> {
        validate_tool_definitions()?;
        let root = root.canonicalize().context("canonicalize tool root")?;
        Ok(Self {
            root,
            policy: ToolPolicy::default(),
        })
    }

    /// Creates tools with an explicit policy.
    #[cfg(test)]
    fn with_policy(root: PathBuf, policy: ToolPolicy) -> Result<Self> {
        validate_tool_definitions()?;
        let root = root.canonicalize().context("canonicalize tool root")?;
        Ok(Self { root, policy })
    }

    /// Executes one structured tool call and returns structured output.
    pub fn execute(&self, tool_name: &str, input: Value) -> ToolOutcome {
        match self.execute_inner(tool_name, input) {
            Ok(output) => ToolOutcome {
                success: true,
                output_json: output,
            },
            Err(error) => ToolOutcome {
                success: false,
                output_json: json!({ "error": format!("{error:#}") }),
            },
        }
    }

    fn execute_inner(&self, tool_name: &str, input: Value) -> Result<Value> {
        match tool_name {
            "read" => self.read(input),
            "write" => self.write(input),
            "edit" => self.edit(input),
            "exec" => self.exec(input),
            _ => bail!("unknown tool {tool_name}"),
        }
    }

    fn read(&self, input: Value) -> Result<Value> {
        let input: PathInput = serde_json::from_value(input).context("decode read input")?;
        let path = self.existing_path(&input.path)?;
        let text =
            fs_err::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        Ok(json!({ "path": input.path, "text": text }))
    }

    fn write(&self, input: Value) -> Result<Value> {
        let input: WriteInput = serde_json::from_value(input).context("decode write input")?;
        let path = self.writable_path(&input.path)?;
        fs_err::write(&path, &input.text).with_context(|| format!("write {}", path.display()))?;
        Ok(json!({ "path": input.path, "bytes": input.text.len() }))
    }

    fn edit(&self, input: Value) -> Result<Value> {
        let input: EditInput = serde_json::from_value(input).context("decode edit input")?;
        if input.old.is_empty() {
            bail!("edit old text must not be empty");
        }

        let path = self.existing_path(&input.path)?;
        let text =
            fs_err::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let matches = text.matches(&input.old).count();
        if matches != 1 {
            bail!("edit expected exactly one match, found {matches}");
        }

        let next = text.replacen(&input.old, &input.new, 1);
        fs_err::write(&path, next).with_context(|| format!("write {}", path.display()))?;
        Ok(json!({ "path": input.path, "replacements": 1 }))
    }

    fn exec(&self, input: Value) -> Result<Value> {
        let input: ExecInput = serde_json::from_value(input).context("decode exec input")?;
        if !self.policy.allow_exec {
            bail!("exec tool is disabled by policy");
        }
        if !self.policy.allowed_programs.contains(&input.program) {
            bail!("program {} is not allowed by policy", input.program);
        }

        let timeout = Duration::from_millis(input.timeout_ms.unwrap_or(self.policy.timeout_ms));
        let mut child = Command::new(&input.program)
            .args(&input.args)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn {}", input.program))?;
        let start = Instant::now();
        loop {
            if child.try_wait().context("poll child process")?.is_some() {
                let output = child.wait_with_output().context("collect child output")?;
                return Ok(json!({
                    "status": output.status.code(),
                    "stdout": String::from_utf8_lossy(&output.stdout),
                    "stderr": String::from_utf8_lossy(&output.stderr),
                }));
            }
            if start.elapsed() >= timeout {
                child.kill().context("kill timed out child")?;
                let output = child
                    .wait_with_output()
                    .context("collect timed out child")?;
                return Ok(json!({
                    "timed_out": true,
                    "stdout": String::from_utf8_lossy(&output.stdout),
                    "stderr": String::from_utf8_lossy(&output.stderr),
                }));
            }
            sleep(Duration::from_millis(10));
        }
    }

    fn existing_path(&self, relative: &str) -> Result<PathBuf> {
        let path = self.workspace_path(relative)?;
        let canonical = path
            .canonicalize()
            .with_context(|| format!("canonicalize {}", path.display()))?;
        if !canonical.starts_with(&self.root) {
            bail!("path escapes workspace: {relative}");
        }
        Ok(canonical)
    }

    fn writable_path(&self, relative: &str) -> Result<PathBuf> {
        let path = self.workspace_path(relative)?;
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("path has no parent: {relative}"))?;
        fs_err::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        let canonical_parent = parent
            .canonicalize()
            .with_context(|| format!("canonicalize {}", parent.display()))?;
        if !canonical_parent.starts_with(&self.root) {
            bail!("path escapes workspace: {relative}");
        }
        Ok(path)
    }

    fn workspace_path(&self, relative: &str) -> Result<PathBuf> {
        let path = Path::new(relative);
        if path.as_os_str().is_empty() || path.is_absolute() {
            bail!("tool path must be relative: {relative}");
        }
        for component in path.components() {
            match component {
                Component::Normal(_) => {}
                _ => bail!("tool path contains unsupported component: {relative}"),
            }
        }
        Ok(self.root.join(path))
    }
}

/// Tool execution result suitable for a session `tool_result` entry.
pub struct ToolOutcome {
    pub success: bool,
    pub output_json: Value,
}

/// Host tool policy.
#[derive(Default)]
pub struct ToolPolicy {
    allow_exec: bool,
    allowed_programs: BTreeSet<String>,
    timeout_ms: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathInput {
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteInput {
    path: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditInput {
    path: String,
    old: String,
    new: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecInput {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    timeout_ms: Option<u64>,
}

fn validate_tool_definitions() -> Result<()> {
    for definition in builtin_tool_definitions() {
        if definition.name.trim().is_empty() {
            bail!("tool definition name must not be empty");
        }
        if definition.description.trim().is_empty() {
            bail!("tool definition description must not be empty");
        }
        if definition.input_schema.get("type") != Some(&Value::String("object".to_string())) {
            bail!("tool {} schema must be an object", definition.name);
        }
    }
    Ok(())
}

fn json_schema(required_strings: &[(&str, &str)]) -> Value {
    let required = required_strings
        .iter()
        .map(|(name, _)| Value::String((*name).to_string()))
        .collect::<Vec<_>>();
    let properties = required_strings
        .iter()
        .map(|(name, value_type)| ((*name).to_string(), json!({ "type": value_type })))
        .collect::<serde_json::Map<_, _>>();
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": required,
        "properties": properties
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> Result<PathBuf> {
        let root = std::env::temp_dir().join(format!(
            "piers-tools-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs_err::create_dir_all(&root)?;
        Ok(root)
    }

    #[test]
    fn reads_and_writes_inside_workspace() -> Result<()> {
        let root = temp_root()?;
        let tools = HostTools::new(root.clone())?;

        let write = tools.execute("write", json!({ "path": "notes/a.txt", "text": "hello" }));
        assert!(write.success);

        let read = tools.execute("read", json!({ "path": "notes/a.txt" }));
        assert!(read.success);
        assert_eq!(read.output_json["text"], "hello");

        fs_err::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn rejects_parent_path_escape() -> Result<()> {
        let root = temp_root()?;
        let tools = HostTools::new(root.clone())?;

        let outcome = tools.execute("write", json!({ "path": "../outside.txt", "text": "no" }));

        assert!(!outcome.success);
        fs_err::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn rejects_ambiguous_edits() -> Result<()> {
        let root = temp_root()?;
        fs_err::write(root.join("a.txt"), "same same")?;
        let tools = HostTools::new(root.clone())?;

        let outcome = tools.execute(
            "edit",
            json!({ "path": "a.txt", "old": "same", "new": "next" }),
        );

        assert!(!outcome.success);
        fs_err::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn denies_exec_by_default() -> Result<()> {
        let root = temp_root()?;
        let tools = HostTools::new(root.clone())?;

        let outcome = tools.execute("exec", json!({ "program": "printf", "args": ["hi"] }));

        assert!(!outcome.success);
        fs_err::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn enforces_exec_timeout() -> Result<()> {
        let root = temp_root()?;
        let mut allowed_programs = BTreeSet::new();
        allowed_programs.insert("sleep".to_string());
        let policy = ToolPolicy {
            allow_exec: true,
            allowed_programs,
            timeout_ms: 50,
        };
        let tools = HostTools::with_policy(root.clone(), policy)?;

        let outcome = tools.execute("exec", json!({ "program": "sleep", "args": ["1"] }));

        assert!(outcome.success);
        assert_eq!(outcome.output_json["timed_out"], true);
        fs_err::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn exposes_builtin_tool_schemas() {
        let definitions = builtin_tool_definitions();
        let names = definitions
            .iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();

        assert_eq!(names, ["read", "write", "edit", "exec"]);
        assert_eq!(definitions[0].input_schema["required"], json!(["path"]));
        assert_eq!(
            definitions[3].input_schema["properties"]["args"]["type"],
            "array"
        );
    }
}
