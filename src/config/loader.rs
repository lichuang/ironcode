//! Configuration file loader

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use shellexpand::tilde;
use toml::from_str;

use crate::error::{ConfigError, Result};

use super::{Config, LoggingConfig, McpConfig};

/// Default configuration directory name (in home directory)
const DEFAULT_DIR: &str = ".ironcode";

/// Default configuration file name
const CONFIG_FILE: &str = "config.toml";

/// Default system prompt directory name
const PROMPTS_DIR: &str = "prompts";
/// Default system prompt file name
const SYSTEM_PROMPT_FILE: &str = "system.md";

/// Get the default data directory (~/.ironcode)
pub fn default_data_dir() -> Option<PathBuf> {
  dirs::home_dir().map(|dir| dir.join(DEFAULT_DIR))
}

/// Get the data directory from config or default
///
/// If config.dir is set, use that (with ~ expanded to home directory);
/// otherwise use ~/.ironcode
pub fn data_dir(config: &Config) -> PathBuf {
  config
    .dir
    .as_ref()
    .map(|dir| {
      // Expand ~ to home directory if present
      let dir_str = dir.to_string_lossy();
      let expanded = tilde(&dir_str);
      PathBuf::from(expanded.as_ref())
    })
    .or_else(default_data_dir)
    .unwrap_or_else(|| {
      // Fallback to current directory if home dir is not available
      PathBuf::from(DEFAULT_DIR)
    })
}

#[allow(dead_code)]
/// Load configuration from standard location
///
/// Configuration is loaded from `~/.ironcode/config.toml`.
pub fn load_config() -> Result<Config> {
  let config_dir = default_data_dir().ok_or(ConfigError::HomeDirNotFound)?;
  load_config_from_dir(&config_dir)
}

/// Load configuration from a specific directory
///
/// Reads config.toml from the specified directory.
pub fn load_config_from_dir(config_dir: &Path) -> Result<Config> {
  let config_path = config_dir.join(CONFIG_FILE);
  load_config_from(&config_path)
}

/// Load configuration from a specific file path
pub fn load_config_from(path: &Path) -> Result<Config> {
  let mut config = Config::default();

  // Load from config file
  if path.exists() {
    let file_config = load_from_file(path)?;
    config = merge_configs(config, file_config);
  }

  // Validate configuration
  validate_config(&config)?;

  Ok(config)
}

/// Load configuration from a specific file path
pub fn load_from_file(path: &Path) -> Result<Config> {
  let content = fs::read_to_string(path).map_err(|e| ConfigError::read_file(path, e))?;

  let config: Config = from_str(&content).map_err(|e| ConfigError::parse_toml(path, e))?;

  Ok(config)
}

/// Get the system prompt file path in the config directory
///
/// Returns: config_dir/prompts/system.md
pub fn system_prompt_path(config_dir: &Path) -> PathBuf {
  config_dir.join(PROMPTS_DIR).join(SYSTEM_PROMPT_FILE)
}

/// Merge two configurations (second overrides first)
fn merge_configs(base: Config, override_: Config) -> Config {
  Config {
    dir: override_.dir.or(base.dir),
    default_model: if !override_.default_model.is_empty() {
      override_.default_model
    } else {
      base.default_model
    },
    providers: {
      let mut merged = base.providers;
      merged.extend(override_.providers);
      merged
    },
    models: {
      let mut merged = base.models;
      merged.extend(override_.models);
      merged
    },
    logging: LoggingConfig {
      level: if !override_.logging.level.is_empty() {
        override_.logging.level
      } else {
        base.logging.level
      },
    },
    default_thinking: override_.default_thinking,
    history: override_.history,
    compaction: override_.compaction,
    retry: override_.retry,
    yolo: override_.yolo || base.yolo,
    auto_approve: if override_.auto_approve.is_empty() {
      base.auto_approve
    } else {
      override_.auto_approve
    },
    mcp: merge_mcp_configs(base.mcp, override_.mcp),
  }
}

/// Merge two MCP configurations
fn merge_mcp_configs(base: McpConfig, override_: McpConfig) -> McpConfig {
  McpConfig {
    config_file: override_.config_file.or(base.config_file),
    servers: {
      let mut merged = base.servers;
      merged.extend(override_.servers);
      merged
    },
  }
}

/// Validate configuration
fn validate_config(config: &Config) -> Result<()> {
  if config.default_model.is_empty() {
    return Err(ConfigError::MissingDefaultModel.into());
  }

  // Check that default_model exists in models
  if !config.models.contains_key(&config.default_model) {
    return Err(
      ConfigError::ModelNotFound {
        model: config.default_model.clone(),
      }
      .into(),
    );
  }

  Ok(())
}

/// JSON root structure for external MCP config files (kimi-cli compatible)
#[derive(Debug, serde::Deserialize)]
struct McpJsonRoot {
  #[serde(default, rename = "mcpServers")]
  mcp_servers: HashMap<String, McpJsonServer>,
}

/// JSON structure for a single MCP server in external config files
#[derive(Debug, serde::Deserialize)]
struct McpJsonServer {
  #[serde(skip_serializing_if = "Option::is_none")]
  url: Option<String>,
  #[serde(default)]
  headers: HashMap<String, String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  command: Option<String>,
  #[serde(default)]
  args: Vec<String>,
  #[serde(default)]
  env: HashMap<String, String>,
}

/// Load MCP configuration from an external JSON file.
///
/// The JSON file follows the standard MCP client format:
/// ```json
/// { "mcpServers": { "server-name": { "command": "...", "args": [...] } } }
/// ```
pub fn load_mcp_json_config(path: &Path) -> Result<HashMap<String, super::McpServerConfig>> {
  let content = fs::read_to_string(path).map_err(|e| ConfigError::read_file(path, e))?;
  let json: McpJsonRoot =
    serde_json::from_str(&content).map_err(|e| ConfigError::ParseMcpJson {
      message: format!("Invalid MCP JSON config: {}", e),
    })?;

  let mut servers = HashMap::new();
  for (name, server) in json.mcp_servers {
    let transport = if server.url.is_some() {
      super::McpTransport::Http
    } else {
      super::McpTransport::Stdio
    };

    // Expand environment variables in headers and env values
    let headers = server
      .headers
      .into_iter()
      .map(|(k, v)| {
        (
          k,
          shellexpand::env(&v)
            .unwrap_or_else(|_| Cow::Borrowed(&v))
            .to_string(),
        )
      })
      .collect();
    let env = server
      .env
      .into_iter()
      .map(|(k, v)| {
        (
          k,
          shellexpand::env(&v)
            .unwrap_or(Cow::Borrowed(&v))
            .to_string(),
        )
      })
      .collect();

    servers.insert(
      name,
      super::McpServerConfig {
        transport,
        url: server.url,
        headers,
        command: server.command,
        args: server.args,
        env,
        disabled: false,
        auto_approve: Vec::new(),
      },
    );
  }

  Ok(servers)
}

/// Resolve the final MCP configuration by combining inline TOML, external JSON, and CLI overrides.
///
/// Priority (later overrides earlier):
/// 1. Inline TOML servers from config.toml
/// 2. External JSON file referenced by `mcp.config_file`
/// 3. CLI `--mcp-config-file` override
pub fn resolve_mcp_config(
  mcp_config: &super::McpConfig,
  cli_mcp_config_file: Option<&Path>,
) -> Result<super::McpConfig> {
  let mut resolved = super::McpConfig {
    config_file: mcp_config.config_file.clone(),
    servers: mcp_config.servers.clone(),
  };

  // Load external JSON config file if specified in TOML
  if let Some(ref config_file) = mcp_config.config_file {
    let config_file_str = config_file.to_string_lossy();
    let expanded = tilde(&config_file_str);
    let path = PathBuf::from(expanded.as_ref());
    if path.exists() {
      let external_servers = load_mcp_json_config(&path)?;
      resolved.servers.extend(external_servers);
    }
  }

  // CLI override takes highest priority
  if let Some(cli_file) = cli_mcp_config_file
    && cli_file.exists()
  {
    let cli_servers = load_mcp_json_config(cli_file)?;
    resolved.servers.extend(cli_servers);
    resolved.config_file = Some(cli_file.to_path_buf());
  }

  Ok(resolved)
}

#[allow(dead_code)]
/// Ensure data directory exists
pub fn ensure_data_dir(config: &Config) -> Result<PathBuf> {
  let data_dir_path = data_dir(config);

  if !data_dir_path.exists() {
    fs::create_dir_all(&data_dir_path).map_err(|e| ConfigError::create_dir(&data_dir_path, e))?;
  }

  Ok(data_dir_path)
}

#[cfg(test)]
mod tests {
  use super::super::{CompactionConfig, Config, HistoryConfig, LoggingConfig, RetryConfig};
  use super::*;
  use std::collections::HashMap;
  use std::env;
  use std::result::Result as StdResult;

  fn fixtures_dir() -> PathBuf {
    PathBuf::from(file!()).parent().unwrap().join("fixtures")
  }

  #[test]
  fn test_parse_example_config() {
    let toml = r#"
default_model = "openai/gpt-4o"

[providers.openai]
type = "openai-compatible"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"

[models."openai/gpt-4o"]
provider = "openai"
model = "gpt-4o"
max_context_size = 128000
supports_streaming = true
"#;

    let config: Config = from_str(toml).expect("Failed to parse TOML");
    assert_eq!(config.default_model, "openai/gpt-4o");
    assert!(config.providers.contains_key("openai"));
    assert!(config.models.contains_key("openai/gpt-4o"));
  }

  #[test]
  fn test_load_from_file() {
    let test_config = fixtures_dir().join("test_config.toml");
    let config = load_from_file(&test_config).expect("Failed to load test config");

    // Check default model
    assert_eq!(config.default_model, "openai/gpt-4o");

    // Check providers
    assert_eq!(config.providers.len(), 2);
    assert!(config.providers.contains_key("openai"));
    assert!(config.providers.contains_key("ollama"));

    // Check openai provider details
    let openai = config.providers.get("openai").unwrap();
    assert_eq!(openai.base_url, "https://api.openai.com/v1");
    assert_eq!(openai.api_key, Some("${OPENAI_API_KEY}".to_string()));

    // Check ollama provider (no api_key)
    let ollama = config.providers.get("ollama").unwrap();
    assert_eq!(ollama.base_url, "http://localhost:11434/v1");
    assert!(ollama.api_key.is_none());

    // Check models
    assert_eq!(config.models.len(), 2);
    assert!(config.models.contains_key("openai/gpt-4o"));
    assert!(config.models.contains_key("openai/gpt-4o-mini"));

    // Check model details
    let gpt4o = config.models.get("openai/gpt-4o").unwrap();
    assert_eq!(gpt4o.provider, "openai");
    assert_eq!(gpt4o.model, "gpt-4o");
    assert_eq!(gpt4o.max_context_size, Some(128000));
    assert_eq!(gpt4o.temperature, Some(0.7));
    assert_eq!(gpt4o.max_tokens, Some(4096));
    assert!(gpt4o.supports_streaming);
    assert!(gpt4o.supports_vision);

    // Check logging
    assert_eq!(config.logging.level, "debug");
  }

  #[test]
  fn test_get_provider_and_model() {
    let test_config = fixtures_dir().join("test_config.toml");
    let config = load_from_file(&test_config).unwrap();

    // Test get_provider
    let provider = config.get_provider("openai");
    assert!(provider.is_some());
    assert!(config.get_provider("nonexistent").is_none());

    // Test get_model
    let model = config.get_model("openai/gpt-4o");
    assert!(model.is_some());
    assert!(config.get_model("nonexistent").is_none());

    // Test default_model_config
    let default = config.default_model_config();
    assert!(default.is_some());
    assert_eq!(default.unwrap().model, "gpt-4o");
  }

  #[test]
  fn test_resolve_api_key() {
    let config = Config::default();

    // Set environment variable for testing (unsafe in Rust 2024 edition)
    unsafe {
      env::set_var("TEST_API_KEY", "sk-test-12345");
    }

    // Test environment variable substitution
    let resolved = config.resolve_api_key("${TEST_API_KEY}");
    assert_eq!(resolved, "sk-test-12345");

    // Test plain key (no substitution)
    let resolved = config.resolve_api_key("sk-plain-key");
    assert_eq!(resolved, "sk-plain-key");

    // Test non-existent variable
    let resolved = config.resolve_api_key("${NON_EXISTENT_VAR}");
    assert_eq!(resolved, "");

    // Clean up (unsafe in Rust 2024 edition)
    unsafe {
      env::remove_var("TEST_API_KEY");
    }
  }

  #[test]
  fn test_merge_configs() {
    let base = load_from_file(&fixtures_dir().join("test_config.toml")).unwrap();
    let override_ = load_from_file(&fixtures_dir().join("override_config.toml")).unwrap();

    let merged = merge_configs(base, override_);

    // Default model should be overridden
    assert_eq!(merged.default_model, "ollama/llama3.1");

    // Providers should be merged (2 base + 1 override = 3 total, but ollama exists in both)
    assert!(merged.providers.contains_key("openai")); // from base
    assert!(merged.providers.contains_key("ollama")); // from base
    assert!(merged.providers.contains_key("local")); // from override

    // Models should be merged (2 base + 1 override = 3 total)
    assert!(merged.models.contains_key("openai/gpt-4o")); // from base
    assert!(merged.models.contains_key("openai/gpt-4o-mini")); // from base
    assert!(merged.models.contains_key("ollama/llama3.1")); // from override

    // Logging should be overridden
    assert_eq!(merged.logging.level, "warn");
  }

  #[test]
  fn test_default_config() {
    let config = Config::default();

    // Default config has empty default_model (must be set by user)
    assert!(config.default_model.is_empty());
    assert!(config.providers.is_empty());
    assert!(config.models.is_empty());
    assert_eq!(config.logging.level, "info");
  }

  #[test]
  fn test_validate_config_empty_default_model() {
    let config = Config::default();
    let result = validate_config(&config);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("Missing required field: default_model"));
  }

  #[test]
  fn test_validate_config_missing_model() {
    let config = Config {
      dir: None,
      default_model: "nonexistent/model".to_string(),
      providers: HashMap::new(),
      models: HashMap::new(),
      logging: LoggingConfig::default(),
      default_thinking: true,
      history: HistoryConfig::default(),
      compaction: CompactionConfig::default(),
      retry: RetryConfig::default(),
      yolo: false,
      auto_approve: Vec::new(),
      mcp: McpConfig::default(),
    };
    let result = validate_config(&config);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("not found in [models] section"));
  }

  #[test]
  fn test_provider_type_as_string() {
    // Test that provider type is parsed as string (accepts any value)
    let toml = r#"
default_model = "openai/gpt-4o"

[providers.openai]
type = "openai"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"

[models."openai/gpt-4o"]
provider = "openai"
model = "gpt-4o"
"#;

    let result: StdResult<Config, _> = from_str(toml);
    assert!(
      result.is_ok(),
      "Provider type 'openai' should be accepted as string"
    );
    let config = result.unwrap();
    let provider = config.providers.get("openai").unwrap();
    assert_eq!(provider.provider_type, "openai");
  }

  #[test]
  fn test_valid_provider_type_accepted() {
    // Test that 'openai-compatible' provider type is accepted
    let toml = r#"
default_model = "kimi/kimi-for-coding"

[providers.kimi]
type = "kimi"
base_url = "https://api.moonshot.cn/v1"
api_key = "${KIMI_API_KEY}"

[models."kimi/kimi-for-coding"]
provider = "kimi"
model = "kimi-for-coding"
"#;

    let result: StdResult<Config, _> = from_str(toml);
    assert!(result.is_ok(), "Valid provider type should be accepted");
    let config = result.unwrap();
    let provider = config.providers.get("kimi").unwrap();
    assert_eq!(provider.provider_type, "kimi");
  }

  #[test]
  fn test_parse_inline_mcp_config() {
    let toml = r#"
default_model = "openai/gpt-4o"

[providers.openai]
type = "openai-compatible"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"

[models."openai/gpt-4o"]
provider = "openai"
model = "gpt-4o"

[mcp.servers.context7]
transport = "http"
url = "https://mcp.context7.com/mcp"
headers = { CONTEXT7_API_KEY = "test-key" }

[mcp.servers.devtools]
transport = "stdio"
command = "npx"
args = ["-y", "chrome-devtools-mcp@latest"]
env = { SOME_VAR = "value" }
disabled = true
auto_approve = ["navigate"]
"#;

    let config: Config = from_str(toml).expect("Failed to parse TOML with MCP config");
    assert_eq!(config.mcp.servers.len(), 2);

    let context7 = config.mcp.servers.get("context7").unwrap();
    assert!(matches!(
      context7.transport,
      crate::config::McpTransport::Http
    ));
    assert_eq!(
      context7.url,
      Some("https://mcp.context7.com/mcp".to_string())
    );
    assert_eq!(
      context7.headers.get("CONTEXT7_API_KEY"),
      Some(&"test-key".to_string())
    );
    assert!(!context7.disabled);

    let devtools = config.mcp.servers.get("devtools").unwrap();
    assert!(matches!(
      devtools.transport,
      crate::config::McpTransport::Stdio
    ));
    assert_eq!(devtools.command, Some("npx".to_string()));
    assert_eq!(devtools.args, vec!["-y", "chrome-devtools-mcp@latest"]);
    assert_eq!(devtools.env.get("SOME_VAR"), Some(&"value".to_string()));
    assert!(devtools.disabled);
    assert_eq!(devtools.auto_approve, vec!["navigate"]);
  }

  #[test]
  fn test_load_mcp_json_config() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let mcp_path = dir.path().join("mcp.json");
    let json = r#"{
  "mcpServers": {
    "context7": {
      "url": "https://mcp.context7.com/mcp",
      "headers": {
        "CONTEXT7_API_KEY": "json-key"
      }
    },
    "chrome-devtools": {
      "command": "npx",
      "args": ["-y", "chrome-devtools-mcp@latest"],
      "env": {
        "SOME_VAR": "json-value"
      }
    }
  }
}"#;
    {
      let mut file = fs::File::create(&mcp_path).unwrap();
      file.write_all(json.as_bytes()).unwrap();
    }

    let servers = load_mcp_json_config(&mcp_path).expect("Failed to load MCP JSON");
    assert_eq!(servers.len(), 2);

    let context7 = servers.get("context7").unwrap();
    assert!(matches!(
      context7.transport,
      crate::config::McpTransport::Http
    ));
    assert_eq!(
      context7.url,
      Some("https://mcp.context7.com/mcp".to_string())
    );
    assert_eq!(
      context7.headers.get("CONTEXT7_API_KEY"),
      Some(&"json-key".to_string())
    );

    let devtools = servers.get("chrome-devtools").unwrap();
    assert!(matches!(
      devtools.transport,
      crate::config::McpTransport::Stdio
    ));
    assert_eq!(devtools.command, Some("npx".to_string()));
    assert_eq!(devtools.args, vec!["-y", "chrome-devtools-mcp@latest"]);
    assert_eq!(
      devtools.env.get("SOME_VAR"),
      Some(&"json-value".to_string())
    );
  }

  #[test]
  fn test_resolve_mcp_config_merge() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let mcp_path = dir.path().join("mcp.json");
    let json = r#"{"mcpServers": {"external": {"url": "https://external.com/mcp"}}}"#;
    {
      let mut file = fs::File::create(&mcp_path).unwrap();
      file.write_all(json.as_bytes()).unwrap();
    }

    let mut inline = super::McpConfig::default();
    inline.servers.insert(
      "inline".to_string(),
      crate::config::McpServerConfig {
        transport: crate::config::McpTransport::Stdio,
        command: Some("node".to_string()),
        args: vec!["server.js".to_string()],
        url: None,
        headers: HashMap::new(),
        env: HashMap::new(),
        disabled: false,
        auto_approve: Vec::new(),
      },
    );
    inline.config_file = Some(mcp_path.clone());

    let resolved = resolve_mcp_config(&inline, None).expect("Failed to resolve MCP config");
    assert_eq!(resolved.servers.len(), 2);
    assert!(resolved.servers.contains_key("inline"));
    assert!(resolved.servers.contains_key("external"));
  }

  #[test]
  fn test_mcp_json_env_expansion() {
    use std::io::Write;
    unsafe {
      env::set_var("MCP_TEST_KEY", "expanded-value");
    }

    let dir = tempfile::tempdir().unwrap();
    let mcp_path = dir.path().join("mcp.json");
    let json = r#"{"mcpServers": {"test": {"url": "https://test.com", "headers": {"API_KEY": "${MCP_TEST_KEY}"}, "env": {"SECRET": "${MCP_TEST_KEY}"}}}}"#;
    {
      let mut file = fs::File::create(&mcp_path).unwrap();
      file.write_all(json.as_bytes()).unwrap();
    }

    let servers = load_mcp_json_config(&mcp_path).unwrap();
    let test = servers.get("test").unwrap();
    assert_eq!(
      test.headers.get("API_KEY"),
      Some(&"expanded-value".to_string())
    );
    assert_eq!(test.env.get("SECRET"), Some(&"expanded-value".to_string()));

    unsafe {
      env::remove_var("MCP_TEST_KEY");
    }
  }
}
