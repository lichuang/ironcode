use crate::config::loader::system_prompt_path;
use crate::error::{Result, RuntimeError};
use crate::tools::handlers::{
  AskUserQuestionHandler, GlobHandler, GrepHandler, ReadFileHandler, ReplaceFileHandler,
  WriteFileHandler,
};
use crate::tools::{ExecutableToolRegistry, ToolRegistry};

// Import platform-specific shell handlers
#[cfg(target_os = "windows")]
use crate::tools::handlers::PowerShellHandler;
#[cfg(not(target_os = "windows"))]
use crate::tools::handlers::BashHandler;
use log::{debug, info, warn};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

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
    chrono::Local::now().to_rfc3339()
  }

  /// Get current working directory
  fn load_work_dir() -> Result<String> {
    std::env::current_dir()
      .map(|p| p.to_string_lossy().to_string())
      .map_err(|e| RuntimeError::GetCurrentDir { source: e }.into())
  }

  /// Get directory listing of working directory
  fn load_work_dir_ls() -> Result<String> {
    let work_dir =
      std::env::current_dir().map_err(|e| RuntimeError::GetCurrentDir { source: e })?;
    let mut entries = Vec::new();

    for entry in fs::read_dir(&work_dir).map_err(|e| RuntimeError::read_dir(&work_dir, e))? {
      let entry = entry.map_err(|e| RuntimeError::read_dir(&work_dir, e))?;
      let name = entry.file_name().to_string_lossy().to_string();
      let metadata = entry
        .metadata()
        .map_err(|e| RuntimeError::read_metadata(&work_dir, e))?;
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
    match fs::read_to_string(&agents_path) {
      Ok(content) => content,
      Err(_) => String::new(),
    }
  }

  /// Load available skills (placeholder for now)
  fn load_skills() -> Result<String> {
    // TODO: Implement skills discovery
    // For now, return empty or load from a skills directory
    Ok(String::new())
  }
}

/// Runtime holds the system prompt template, arguments for rendering, and tool registry
#[derive(Debug, Clone)]
pub(crate) struct Runtime {
  /// Template arguments for substitution
  pub args: RuntimeArgs,
  /// The raw system prompt template (before substitution)
  pub system_prompt_template: String,
  /// Tool registry containing all loaded tools (shared with providers)
  pub tool_registry: Arc<ToolRegistry>,
  /// Executable tool registry for handling tool calls (shared across sessions)
  pub executable_tool_registry: Arc<ExecutableToolRegistry>,
}

impl Runtime {
  /// Create a new Runtime instance by loading all environment data
  ///
  /// Loads system prompt from data_dir/prompts/system.md
  /// Loads tools from data_dir/prompts/tools/
  /// Returns empty string if prompt file doesn't exist
  pub(crate) fn new(data_dir: &PathBuf) -> Result<Self> {
    let system_prompt_template = Self::load_system_prompt_template(data_dir);
    let args = RuntimeArgs::new()?;

    // Load executable tool registry first (handlers must be registered before checking)
    let executable_tool_registry = Arc::new(Self::load_executable_tools());

    // Load tool definitions from Markdown files
    let tool_registry = Arc::new(Self::load_tools(data_dir)?);

    // Check that all defined tools have corresponding handlers
    Self::validate_tool_handlers(&tool_registry, &executable_tool_registry)?;

    Ok(Self {
      args,
      system_prompt_template,
      tool_registry,
      executable_tool_registry,
    })
  }

  /// Load and initialize the executable tool registry with all handlers
  fn load_executable_tools() -> ExecutableToolRegistry {
    let mut registry = ExecutableToolRegistry::new();
    registry.register("ReadFile", Box::new(ReadFileHandler::new()));
    registry.register("WriteFile", Box::new(WriteFileHandler::new()));
    registry.register("ReplaceFile", Box::new(ReplaceFileHandler::new()));
    registry.register("Grep", Box::new(GrepHandler::new()));
    registry.register("Glob", Box::new(GlobHandler::new()));
    registry.register("AskUserQuestion", Box::new(AskUserQuestionHandler::new()));
    
    // Register platform-specific shell handler
    #[cfg(target_os = "windows")]
    registry.register("PowerShell", Box::new(PowerShellHandler::new()));
    #[cfg(not(target_os = "windows"))]
    registry.register("Bash", Box::new(BashHandler::new()));
    
    registry
  }

  /// Load tools from the data directory
  /// Tools are loaded from {data_dir}/prompts/tools/
  fn load_tools(data_dir: &PathBuf) -> Result<ToolRegistry> {
    let tools_dir = data_dir.join("prompts").join("tools");
    debug!("Loading tools from: {:?}", tools_dir);

    match ToolRegistry::load_from_dir(&tools_dir) {
      Ok(registry) => {
        info!("Loaded {} tools from {:?}", registry.len(), tools_dir);
        Ok(registry)
      }
      Err(e) => {
        warn!("Failed to load tools from {:?}: {}", tools_dir, e);
        // If directory doesn't exist or fails to load, return empty registry
        // This is not a fatal error - tools are optional
        Ok(ToolRegistry::new())
      }
    }
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
          RuntimeError::MissingToolHandler {
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
  fn load_system_prompt_template(config_dir: &PathBuf) -> String {
    let prompt_path = system_prompt_path(config_dir);
    debug!("Loading system prompt from: {:?}", prompt_path);

    match fs::read_to_string(&prompt_path) {
      Ok(content) => {
        if content.trim().is_empty() {
          warn!("System prompt file exists but is empty: {:?}", prompt_path);
        } else {
          debug!("Loaded system prompt, length: {} chars", content.len());
        }
        content
      }
      Err(e) => {
        warn!("Failed to load system prompt from {:?}: {}", prompt_path, e);
        String::new()
      }
    }
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
