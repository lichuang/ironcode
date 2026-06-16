use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Local;
use log::{debug, info, warn};

use crate::background::BackgroundTaskManager;
use crate::config::loader::{data_dir, system_prompt_path};
use crate::config::{
  BackgroundConfig, CompactionConfig, Config, DEFAULT_MAX_CONTEXT_SIZE, HistoryConfig, ModelConfig,
  ProviderConfig, RetryConfig,
};
use crate::error::Result;
use crate::notification::NotificationManager;
use crate::tools::handlers::{
  AskUserQuestionHandler, EnterPlanModeHandler, ExitPlanModeHandler, FetchURLHandler, GlobHandler,
  GrepHandler, ReadFileHandler, ReplaceFileHandler, SearchWebHandler, SetTodoListHandler,
  TaskListHandler, TaskOutputHandler, TaskStopHandler, WriteFileHandler,
};
use crate::tools::{ExecutableToolRegistry, ToolRegistry};

// Import platform-specific shell handlers
#[cfg(not(target_os = "windows"))]
use crate::tools::handlers::BashHandler;
#[cfg(target_os = "windows")]
use crate::tools::handlers::PowerShellHandler;

/// Runtime environment errors
#[derive(thiserror::Error, Debug)]
pub enum Error {
  #[error("Failed to get current directory")]
  GetCurrentDir {
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to read directory: {path}")]
  ReadDir {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to read file metadata: {path}")]
  ReadMetadata {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to read system prompt from: {path}")]
  ReadSystemPrompt {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Tool '{tool_name}' is defined in prompts but no handler is implemented")]
  MissingToolHandler { tool_name: String },
}

impl Error {
  /// Create a read directory error with path
  pub fn read_dir(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    Error::ReadDir {
      path: path.into(),
      source,
    }
  }

  /// Create a read metadata error with path
  pub fn read_metadata(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    Error::ReadMetadata {
      path: path.into(),
      source,
    }
  }

  /// Create a read system prompt error with path
  pub fn read_system_prompt(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
    Error::ReadSystemPrompt {
      path: path.into(),
      source,
    }
  }
}

/// Runtime environment arguments for template substitution
///
/// These variables are loaded at startup and used to replace
/// placeholders in the system prompt template.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeArgs {
  /// Current date and time in ISO format (${IRONCODE_NOW})
  pub now: String,
  /// Working directory absolute path (${IRONCODE_WORK_DIR})
  pub work_dir: String,
  /// Working directory listing (${IRONCODE_WORK_DIR_LS})
  pub work_dir_ls: String,
  /// Additional directories info (${IRONCODE_ADDITIONAL_DIRS_INFO})
  pub additional_dirs_info: String,
  /// AGENTS.md file content (${IRONCODE_AGENTS_MD})
  pub agents_md: String,
  /// Available skills list (${IRONCODE_SKILLS})
  pub skills: String,
  /// Role additional info (${ROLE_ADDITIONAL})
  pub role_additional: String,
}

impl RuntimeArgs {
  /// Create a new RuntimeArgs instance by loading all environment data
  pub(crate) fn new() -> Result<Self> {
    Ok(Self {
      now: Self::load_now(),
      work_dir: Self::load_work_dir()?,
      work_dir_ls: Self::load_work_dir_ls()?,
      additional_dirs_info: String::new(), // TODO: Load from config or env
      agents_md: Self::load_agents_md(),
      skills: Self::load_skills()?,
      role_additional: String::new(), // TODO: Load from config or env
    })
  }

  /// Get current timestamp in ISO format
  fn load_now() -> String {
    Local::now().to_rfc3339()
  }

  /// Get current working directory
  fn load_work_dir() -> Result<String> {
    env::current_dir()
      .map(|p| p.to_string_lossy().to_string())
      .map_err(|e| Error::GetCurrentDir { source: e }.into())
  }

  /// Get directory listing of working directory
  fn load_work_dir_ls() -> Result<String> {
    let work_dir = env::current_dir().map_err(|e| Error::GetCurrentDir { source: e })?;
    let mut entries = Vec::new();

    for entry in fs::read_dir(&work_dir).map_err(|e| Error::read_dir(&work_dir, e))? {
      let entry = entry.map_err(|e| Error::read_dir(&work_dir, e))?;
      let name = entry.file_name().to_string_lossy().to_string();
      let metadata = entry
        .metadata()
        .map_err(|e| Error::read_metadata(&work_dir, e))?;
      let size = metadata.len();
      let is_dir = metadata.is_dir();
      let permissions = metadata.permissions();
      let mode = if permissions.readonly() { "r--" } else { "rw-" };

      let perms = if is_dir {
        "drwxr-xr-x".to_string()
      } else {
        format!("-{}r--r--", mode)
      };
      let size_str = if is_dir {
        String::new()
      } else {
        format!("{}", size)
      };
      entries.push(format!("{}{:>10} {}", perms, size_str, name));
    }

    entries.sort();
    Ok(entries.join("\n"))
  }

  /// Load AGENTS.md content if exists
  fn load_agents_md() -> String {
    let agents_path = PathBuf::from("AGENTS.md");
    fs::read_to_string(&agents_path).unwrap_or_default()
  }

  /// Load available skills (placeholder for now)
  fn load_skills() -> Result<String> {
    // TODO: Implement skills discovery
    // For now, return empty or load from a skills directory
    Ok(String::new())
  }
}

/// Runtime holds the effective configuration, system prompt template, arguments,
/// and tool registries. All fields are loaded at startup and are read-only during
/// the session.
#[derive(Debug, Clone)]
pub(crate) struct Runtime {
  /// Effective configuration (user config + CLI overrides applied)
  config: Arc<Config>,
  /// Template arguments for substitution
  pub args: RuntimeArgs,
  /// The raw system prompt template (before substitution)
  pub system_prompt_template: String,
  /// Tool definitions registry (loaded from Markdown files)
  pub tool_registry: Arc<ToolRegistry>,
  /// Executable tool registry for dispatching and previewing tool calls
  pub executable_registry: Arc<ExecutableToolRegistry>,
  /// Background task manager (session is bound after session creation)
  pub background_manager: Arc<BackgroundTaskManager>,
  /// Notification manager (session is bound after session creation)
  pub notification_manager: Arc<NotificationManager>,
}

impl Runtime {
  /// Create a new Runtime instance by loading all environment data
  ///
  /// Loads system prompt from data_dir/prompts/system.md
  /// Loads tools from data_dir/prompts/tools/
  /// Returns empty string if prompt file doesn't exist
  pub(crate) fn new(data_dir: &Path, config: Arc<Config>) -> Result<Self> {
    let system_prompt_template = Self::load_system_prompt_template(data_dir);
    let args = RuntimeArgs::new()?;

    let background_manager = Arc::new(BackgroundTaskManager::new(
      data_dir.to_path_buf(),
      config.background.clone(),
    ));
    let notification_manager = Arc::new(NotificationManager::new(
      data_dir.to_path_buf(),
      config.notifications.clone(),
    ));

    // Load executable tool registry first (handlers must be registered before checking)
    let executable_registry = Arc::new(Self::load_executable_tools(&background_manager));

    // Load tool definitions from Markdown files
    let tool_registry = Arc::new(Self::load_tools(data_dir)?);

    // Check that all defined tools have corresponding handlers
    Self::validate_tool_handlers(&tool_registry, &executable_registry)?;

    Ok(Self {
      config,
      args,
      system_prompt_template,
      tool_registry,
      executable_registry,
      background_manager,
      notification_manager,
    })
  }

  #[cfg(test)]
  pub(crate) fn for_test(config: Config) -> Self {
    Self {
      config: Arc::new(config),
      args: RuntimeArgs {
        now: String::new(),
        work_dir: String::new(),
        work_dir_ls: String::new(),
        additional_dirs_info: String::new(),
        agents_md: String::new(),
        skills: String::new(),
        role_additional: String::new(),
      },
      system_prompt_template: String::new(),
      tool_registry: Arc::new(ToolRegistry::default()),
      executable_registry: Arc::new(ExecutableToolRegistry::new()),
      background_manager: Arc::new(BackgroundTaskManager::new(
        std::path::PathBuf::from("."),
        crate::config::BackgroundConfig::default(),
      )),
      notification_manager: Arc::new(NotificationManager::new(
        std::path::PathBuf::from("."),
        crate::config::NotificationConfig::default(),
      )),
    }
  }

  /// Load and initialize the executable tool registry with all handlers
  fn load_executable_tools(manager: &Arc<BackgroundTaskManager>) -> ExecutableToolRegistry {
    let mut registry = ExecutableToolRegistry::new();
    registry.register("ReadFile", Box::new(ReadFileHandler::new()));
    registry.register("WriteFile", Box::new(WriteFileHandler::new()));
    registry.register("ReplaceFile", Box::new(ReplaceFileHandler::new()));
    registry.register("Grep", Box::new(GrepHandler::new()));
    registry.register("Glob", Box::new(GlobHandler::new()));
    registry.register("AskUserQuestion", Box::new(AskUserQuestionHandler::new()));
    registry.register("SetTodoList", Box::new(SetTodoListHandler::new()));
    registry.register("FetchURL", Box::new(FetchURLHandler::new()));
    registry.register("SearchWeb", Box::new(SearchWebHandler::new()));
    registry.register("EnterPlanMode", Box::new(EnterPlanModeHandler::new()));
    registry.register("ExitPlanMode", Box::new(ExitPlanModeHandler::new()));
    registry.register("TaskList", Box::new(TaskListHandler::new(manager.clone())));
    registry.register(
      "TaskOutput",
      Box::new(TaskOutputHandler::new(manager.clone())),
    );
    registry.register("TaskStop", Box::new(TaskStopHandler::new(manager.clone())));

    // Register platform-specific shell handler
    #[cfg(target_os = "windows")]
    registry.register("PowerShell", Box::new(PowerShellHandler::new()));
    #[cfg(not(target_os = "windows"))]
    registry.register("Bash", Box::new(BashHandler::new(manager.clone())));

    registry
  }

  /// Load tools from the data directory
  /// Tools are loaded from {data_dir}/prompts/tools/
  fn load_tools(data_dir: &Path) -> Result<ToolRegistry> {
    let tools_dir = data_dir.join("prompts").join("tools");
    debug!("Loading tools from: {:?}", tools_dir);

    let mut registry = match ToolRegistry::load_from_dir(&tools_dir) {
      Ok(registry) => registry,
      Err(e) => {
        warn!("Failed to load tools from {:?}: {}", tools_dir, e);
        // If directory doesn't exist or fails to load, return empty registry
        // This is not a fatal error - tools are optional
        return Ok(ToolRegistry::new());
      }
    };

    info!("Loaded {} tools from {:?}", registry.len(), tools_dir);

    // Apply ${SHELL} template replacement for shell tool descriptions
    // (mirrors kimi-cli's load_desc with {"SHELL": "..."})
    let shell_replacement = if cfg!(target_os = "windows") {
      "PowerShell (powershell.exe)"
    } else {
      "bash (/bin/bash)"
    };
    for tool_name in ["Bash", "PowerShell"] {
      if let Some(tool) = registry.get_mut(tool_name) {
        tool.description = tool.description.replace("${SHELL}", shell_replacement);
      }
    }

    Ok(registry)
  }

  /// Validate that all tools defined in registry have corresponding handlers
  /// Tools marked with `no_handler: true` are skipped from validation
  fn validate_tool_handlers(
    tool_registry: &ToolRegistry,
    executable_registry: &ExecutableToolRegistry,
  ) -> Result<()> {
    for tool in tool_registry.all() {
      // Skip tools that are marked as not having a handler
      if tool.no_handler {
        log::debug!(
          "Skipping handler check for tool '{}' (no_handler: true)",
          tool.name
        );
        continue;
      }
      // Skip platform-specific shell tools based on OS
      #[cfg(target_os = "windows")]
      if tool.name == "Bash" {
        log::debug!("Skipping handler check for 'Bash' tool on Windows system");
        continue;
      }
      #[cfg(not(target_os = "windows"))]
      if tool.name == "PowerShell" {
        log::debug!("Skipping handler check for 'PowerShell' tool on non-Windows system");
        continue;
      }
      if !executable_registry.has(&tool.name) {
        return Err(
          Error::MissingToolHandler {
            tool_name: tool.name.clone(),
          }
          .into(),
        );
      }
    }
    Ok(())
  }

  /// Load the system prompt template from config directory
  ///
  /// Reads from config_dir/prompts/system.md
  /// Returns empty string if file doesn't exist
  fn load_system_prompt_template(config_dir: &Path) -> String {
    let prompt_path = system_prompt_path(config_dir);
    debug!("Loading system prompt from: {:?}", prompt_path);

    fs::read_to_string(&prompt_path).unwrap_or_else(|e| {
      warn!("Failed to load system prompt from {:?}: {}", prompt_path, e);
      String::new()
    })
  }

  /// Whether automatic compaction is enabled.
  #[allow(dead_code)]
  pub fn enable_compaction(&self) -> bool {
    self.config.compaction.enabled
  }

  /// Get the compaction configuration.
  pub fn compaction_config(&self) -> &CompactionConfig {
    &self.config.compaction
  }

  /// Get the default model name.
  pub fn default_model(&self) -> String {
    self.config.default_model.clone()
  }

  /// Get the default model configuration.
  pub fn default_model_config(&self) -> Option<&ModelConfig> {
    self.config.default_model_config()
  }

  /// Get the max context size of the default model.
  pub fn default_model_max_context_size(&self) -> usize {
    self
      .config
      .default_model_config()
      .and_then(|m| m.max_context_size)
      .unwrap_or(DEFAULT_MAX_CONTEXT_SIZE)
  }

  /// Get the data directory.
  pub fn data_dir(&self) -> PathBuf {
    data_dir(&self.config)
  }

  /// Get the history configuration.
  pub fn history_config(&self) -> HistoryConfig {
    self.config.history.clone()
  }

  /// Whether YOLO mode is enabled.
  pub fn yolo(&self) -> bool {
    self.config.yolo
  }

  /// Get the list of tools to auto-approve.
  pub fn auto_approve(&self) -> Vec<String> {
    self.config.auto_approve.clone()
  }

  /// Whether default thinking mode is enabled.
  pub fn default_thinking(&self) -> bool {
    self.config.default_thinking
  }

  /// Get the background task configuration.
  #[allow(dead_code)]
  pub fn background_config(&self) -> BackgroundConfig {
    self.config.background.clone()
  }

  /// Get the background task manager.
  pub fn background_manager(&self) -> Arc<BackgroundTaskManager> {
    self.background_manager.clone()
  }

  /// Get the notification manager.
  pub fn notification_manager(&self) -> Arc<NotificationManager> {
    self.notification_manager.clone()
  }

  /// Publish notifications for all terminal background tasks.
  pub fn publish_task_notifications(&self) {
    self
      .background_manager
      .reconcile(&self.notification_manager);
  }

  /// Get a provider by name.
  pub fn get_provider(&self, name: &str) -> Option<&ProviderConfig> {
    self.config.get_provider(name)
  }

  /// Resolve an API key (handles env var substitution like "${OPENAI_API_KEY}").
  pub fn resolve_api_key(&self, key: &str) -> String {
    self.config.resolve_api_key(key)
  }

  /// Get the retry configuration.
  pub fn retry_config(&self) -> RetryConfig {
    self.config.retry.clone()
  }

  /// Render the system prompt with all template variables substituted
  pub fn render_system_prompt(&self) -> String {
    self
      .system_prompt_template
      .replace("${IRONCODE_NOW}", &self.args.now)
      .replace("${IRONCODE_WORK_DIR}", &self.args.work_dir)
      .replace("${IRONCODE_WORK_DIR_LS}", &self.args.work_dir_ls)
      .replace(
        "${IRONCODE_ADDITIONAL_DIRS_INFO}",
        &self.args.additional_dirs_info,
      )
      .replace("${IRONCODE_AGENTS_MD}", &self.args.agents_md)
      .replace("${IRONCODE_SKILLS}", &self.args.skills)
      .replace("${ROLE_ADDITIONAL}", &self.args.role_additional)
  }
}
