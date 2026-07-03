//! Subagent / Multi-Agent System.
//!
//! Mirrors kimi-cli's `src/kimi_cli/subagents/`: a `LaborMarket` registry of
//! built-in agent types, a `SubagentStore` for per-agent persistence, and
//! helpers for building child agents from YAML specs.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::utils::time::Timestamp;

pub mod store;

/// Mode for a subagent's tool policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicyMode {
  /// Inherit the parent agent's full toolset.
  #[default]
  Inherit,
  /// Restrict to an explicit allowlist of tool names.
  Allowlist,
}

/// Tool policy controlling which tools a subagent may use.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ToolPolicy {
  /// Policy mode.
  #[serde(default)]
  pub mode: ToolPolicyMode,
  /// Tool names allowed when mode is `Allowlist`.
  #[serde(default)]
  pub tools: Vec<String>,
}

impl ToolPolicy {
  /// Returns true if the given tool name is allowed under this policy.
  pub fn allows(&self, tool_name: &str) -> bool {
    match self.mode {
      ToolPolicyMode::Inherit => true,
      ToolPolicyMode::Allowlist => self.tools.iter().any(|t| t == tool_name),
    }
  }
}

/// Definition of a built-in subagent type.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct AgentTypeDefinition {
  /// Subagent type name (e.g. "coder", "explore", "plan").
  pub name: String,
  /// Short description shown in the Agent tool description.
  pub description: String,
  /// Path to the agent YAML spec file.
  pub agent_file: PathBuf,
  /// Guidance on when to use this subagent type.
  #[serde(default)]
  pub when_to_use: String,
  /// Additional role text injected into the system prompt.
  #[serde(default)]
  pub role_additional: String,
  /// Optional default model alias override.
  #[serde(default)]
  pub default_model: Option<String>,
  /// Tool policy for this subagent.
  #[serde(default)]
  pub tool_policy: ToolPolicy,
  /// Whether this subagent type supports background execution.
  #[serde(default = "default_true")]
  pub supports_background: bool,
}

fn default_true() -> bool {
  true
}

/// Specification for a concrete subagent instance launch.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct AgentLaunchSpec {
  /// Stable agent instance id.
  pub agent_id: String,
  /// Subagent type name.
  pub subagent_type: String,
  /// Optional model alias requested by the user/tool call.
  pub model_override: Option<String>,
  /// Effective model alias after resolution.
  pub effective_model: Option<String>,
  /// Creation timestamp (seconds since epoch).
  pub created_at: Timestamp,
}

/// Persistent payload stored in a background task spec for agent-kind tasks.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentTaskPayload {
  /// Stable agent instance id.
  pub agent_id: String,
  /// Subagent type name.
  pub subagent_type: String,
  /// User prompt given to the subagent.
  pub prompt: String,
  /// Optional model alias requested by the user/tool call.
  pub model_override: Option<String>,
  /// Effective model alias after resolution.
  pub effective_model: Option<String>,
  /// Tool policy enforced for the subagent.
  pub tool_policy: ToolPolicy,
  /// Human-readable task description.
  pub description: String,
  /// Whether this task resumed an existing agent instance.
  #[serde(default)]
  pub resumed: bool,
  /// Timeout in seconds for the background task.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub timeout_s: Option<u64>,
}

/// Status of a subagent instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
  /// Running in the foreground.
  RunningForeground,
  /// Running as a background task.
  RunningBackground,
  /// Completed successfully.
  Completed,
  /// Failed due to an error.
  Failed,
  /// Explicitly stopped.
  Killed,
  /// Worker lost (e.g., parent process exited).
  Lost,
}

impl fmt::Display for SubagentStatus {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let s = match self {
      SubagentStatus::RunningForeground => "running_foreground",
      SubagentStatus::RunningBackground => "running_background",
      SubagentStatus::Completed => "completed",
      SubagentStatus::Failed => "failed",
      SubagentStatus::Killed => "killed",
      SubagentStatus::Lost => "lost",
    };
    write!(f, "{}", s)
  }
}

/// Persisted metadata for a subagent instance.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct AgentInstanceRecord {
  /// Stable agent instance id.
  pub agent_id: String,
  /// Subagent type name.
  pub subagent_type: String,
  /// Current status of the subagent instance.
  pub status: SubagentStatus,
  /// Task description.
  pub description: String,
  /// Creation timestamp.
  pub created_at: Timestamp,
  /// Last update timestamp.
  pub updated_at: Timestamp,
  /// Last background task id, if any.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub last_task_id: Option<String>,
  /// Launch spec for this instance.
  pub launch_spec: AgentLaunchSpec,
}

/// Registry of built-in subagent types.
#[derive(Debug, Default)]
pub struct LaborMarket {
  types: HashMap<String, AgentTypeDefinition>,
}

#[allow(dead_code)]
impl LaborMarket {
  /// Create an empty labor market.
  pub fn new() -> Self {
    Self {
      types: HashMap::new(),
    }
  }

  /// Register a built-in subagent type.
  pub fn add_builtin_type(&mut self, type_def: AgentTypeDefinition) {
    self.types.insert(type_def.name.clone(), type_def);
  }

  /// Look up a subagent type by name.
  pub fn get(&self, name: &str) -> Option<&AgentTypeDefinition> {
    self.types.get(name)
  }

  /// Require a subagent type by name, returning an error if missing.
  pub fn require(&self, name: &str) -> anyhow::Result<&AgentTypeDefinition> {
    self
      .types
      .get(name)
      .ok_or_else(|| anyhow::anyhow!("Unknown subagent type: {}", name))
  }

  /// Iterate over all registered types.
  pub fn iter(&self) -> impl Iterator<Item = &AgentTypeDefinition> {
    self.types.values()
  }

  /// Load all built-in agent types shipped with the binary.
  ///
  /// Reads the bundled `src/agents/default/agent.yaml` and registers every
  /// declared subagent.
  pub fn load_builtin() -> anyhow::Result<Self> {
    let mut market = Self::new();
    let root_spec = include_str!("../agents/default/agent.yaml");
    let root: RootAgentSpec = serde_yaml::from_str(root_spec)?;

    for entry in root.subagents {
      let agent_spec: AgentYamlSpec = match entry.name.as_str() {
        "coder" => serde_yaml::from_str(include_str!("../agents/default/coder.yaml"))?,
        "explore" => serde_yaml::from_str(include_str!("../agents/default/explore.yaml"))?,
        "plan" => serde_yaml::from_str(include_str!("../agents/default/plan.yaml"))?,
        other => anyhow::bail!("Unknown built-in subagent type: {}", other),
      };

      let tool_policy = agent_spec.tool_policy.unwrap_or_default();

      market.add_builtin_type(AgentTypeDefinition {
        name: entry.name.clone(),
        description: agent_spec.description.unwrap_or_default(),
        agent_file: PathBuf::from(&entry.path),
        when_to_use: agent_spec.when_to_use.unwrap_or_default(),
        role_additional: agent_spec.role_additional.unwrap_or_default(),
        default_model: agent_spec.model,
        tool_policy,
        supports_background: agent_spec.supports_background.unwrap_or(true),
      });
    }

    Ok(market)
  }
}

/// Root agent spec declaring available subagents.
#[derive(Debug, Deserialize)]
struct RootAgentSpec {
  subagents: Vec<SubagentEntry>,
}

/// Entry in the root agent spec pointing to a subagent YAML file.
#[derive(Debug, Deserialize)]
struct SubagentEntry {
  name: String,
  path: String,
}

/// Parsed subagent YAML spec.
#[derive(Debug, Deserialize)]
struct AgentYamlSpec {
  description: Option<String>,
  #[serde(default)]
  when_to_use: Option<String>,
  #[serde(default)]
  role_additional: Option<String>,
  #[serde(default)]
  model: Option<String>,
  #[serde(default)]
  tool_policy: Option<ToolPolicy>,
  #[serde(default)]
  supports_background: Option<bool>,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_tool_policy_allowlist() {
    let policy = ToolPolicy {
      mode: ToolPolicyMode::Allowlist,
      tools: vec!["ReadFile".to_string(), "Glob".to_string()],
    };
    assert!(policy.allows("ReadFile"));
    assert!(!policy.allows("WriteFile"));
  }

  #[test]
  fn test_tool_policy_inherit() {
    let policy = ToolPolicy {
      mode: ToolPolicyMode::Inherit,
      tools: vec![],
    };
    assert!(policy.allows("Anything"));
  }

  #[test]
  fn test_labor_market_builtin_loads() {
    let market = LaborMarket::load_builtin().expect("builtin agents should load");
    assert!(market.get("coder").is_some());
    assert!(market.get("explore").is_some());
    assert!(market.get("plan").is_some());
  }
}
