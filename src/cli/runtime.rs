use std::fs;
use std::path::PathBuf;

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
  pub(crate) fn new() -> anyhow::Result<Self> {
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
  fn load_work_dir() -> anyhow::Result<String> {
    std::env::current_dir()
      .map(|p| p.to_string_lossy().to_string())
      .map_err(|e| anyhow::anyhow!("Failed to get current directory: {}", e))
  }

  /// Get directory listing of working directory
  fn load_work_dir_ls() -> anyhow::Result<String> {
    let work_dir = std::env::current_dir()?;
    let mut entries = Vec::new();

    for entry in fs::read_dir(&work_dir)? {
      let entry = entry?;
      let name = entry.file_name().to_string_lossy().to_string();
      let metadata = entry.metadata()?;
      let size = metadata.len();
      let is_dir = metadata.is_dir();
      let permissions = metadata.permissions();
      let mode = if permissions.readonly() { "r--" } else { "rw-" };

      let perms = if is_dir {
        "drwxr-xr-x".to_string()
      } else {
        format!("-{}r--r--", mode)
      };
      let size_str = if is_dir { String::new() } else { format!("{}", size) };
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
  fn load_skills() -> anyhow::Result<String> {
    // TODO: Implement skills discovery
    // For now, return empty or load from a skills directory
    Ok(String::new())
  }
}



/// Runtime holds the system prompt template and arguments for rendering
#[derive(Debug, Clone)]
pub(crate) struct Runtime {
  /// Template arguments for substitution
  pub args: RuntimeArgs,
  /// The raw system prompt template (before substitution)
  pub system_prompt_template: String,
}

impl Runtime {
  /// Create a new Runtime instance by loading all environment data
  pub(crate) fn new() -> anyhow::Result<Self> {
    let system_prompt_template = Self::load_system_prompt_template()?;
    let args = RuntimeArgs::new()?;

    Ok(Self {
      args,
      system_prompt_template,
    })
  }

  /// Load the system prompt template from prompts/system.md
  fn load_system_prompt_template() -> anyhow::Result<String> {
    let prompt_path = PathBuf::from("prompts/system.md");
    let content = fs::read_to_string(&prompt_path)
      .map_err(|e| anyhow::anyhow!("Failed to read system prompt from {:?}: {}", prompt_path, e))?;
    Ok(content)
  }

  /// Render the system prompt with all template variables substituted
  pub fn render_system_prompt(&self) -> String {
    self
      .system_prompt_template
      .replace("${IRONCODE_NOW}", &self.args.now)
      .replace("${IRONCODE_WORK_DIR}", &self.args.work_dir)
      .replace("${IRONCODE_WORK_DIR_LS}", &self.args.work_dir_ls)
      .replace("${IRONCODE_ADDITIONAL_DIRS_INFO}", &self.args.additional_dirs_info)
      .replace("${IRONCODE_AGENTS_MD}", &self.args.agents_md)
      .replace("${IRONCODE_SKILLS}", &self.args.skills)
      .replace("${ROLE_ADDITIONAL}", &self.args.role_additional)
  }
}


