//! Configuration file loader

use super::{Config, LoggingConfig};
use crate::error::{ConfigError, Result};
use std::path::PathBuf;

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
      let expanded = shellexpand::tilde(&dir_str);
      PathBuf::from(expanded.as_ref())
    })
    .or_else(default_data_dir)
    .unwrap_or_else(|| {
      // Fallback to current directory if home dir is not available
      PathBuf::from(DEFAULT_DIR)
    })
}

/// Get the prompts directory path
pub fn prompts_dir(config: &Config) -> PathBuf {
  data_dir(config).join(PROMPTS_DIR)
}

/// Get the logs directory path
pub fn logs_dir(config: &Config) -> PathBuf {
  data_dir(config).join("logs")
}

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
pub fn load_config_from_dir(config_dir: &PathBuf) -> Result<Config> {
  let config_path = config_dir.join(CONFIG_FILE);
  load_config_from(&config_path)
}

/// Load configuration from a specific file path
pub fn load_config_from(path: &PathBuf) -> Result<Config> {
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
pub fn load_from_file(path: &PathBuf) -> Result<Config> {
  let content = std::fs::read_to_string(path).map_err(|e| ConfigError::read_file(path, e))?;

  let config: Config = toml::from_str(&content).map_err(|e| ConfigError::parse_toml(path, e))?;

  Ok(config)
}

/// Get the user configuration file path (~/.ironcode/config.toml)
fn user_config_path() -> Option<PathBuf> {
  default_data_dir().map(|dir| dir.join(CONFIG_FILE))
}

/// Get the system prompt file path in the config directory
///
/// Returns: config_dir/prompts/system.md
pub fn system_prompt_path(config_dir: &PathBuf) -> PathBuf {
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

/// Ensure data directory exists
pub fn ensure_data_dir(config: &Config) -> Result<PathBuf> {
  let data_dir_path = data_dir(config);

  if !data_dir_path.exists() {
    std::fs::create_dir_all(&data_dir_path)
      .map_err(|e| ConfigError::create_dir(&data_dir_path, e))?;
  }

  Ok(data_dir_path)
}

/// Create a default configuration file if it doesn't exist
///
/// Creates the config file in the default location (~/.ironcode/)
pub fn create_default_config() -> Result<PathBuf> {
  let config_dir = default_data_dir().ok_or(ConfigError::HomeDirNotFound)?;

  // Ensure the config directory exists
  if !config_dir.exists() {
    std::fs::create_dir_all(&config_dir).map_err(|e| ConfigError::create_dir(&config_dir, e))?;
  }

  let config_path = config_dir.join(CONFIG_FILE);

  if !config_path.exists() {
    let default_config = r#"# IronCode Configuration File
# Location: ~/.ironcode/config.toml

# Data directory for ironcode files (logs, prompts, etc.)
# Defaults to ~/.ironcode/ if not specified
# dir = "~/.ironcode"

# Default model to use (required)
default_model = "openai/gpt-4o"

# Provider definitions
[providers.openai]
type = "openai-compatible"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"

# Model definitions
[models."openai/gpt-4o"]
provider = "openai"
model = "gpt-4o"
max_context_size = 128000
supports_streaming = true
supports_vision = true

# Logging
[logging]
level = "info"
"#;

    std::fs::write(&config_path, default_config)
      .map_err(|e| ConfigError::write_file(&config_path, e))?;
  }

  Ok(config_path)
}

#[cfg(test)]
mod tests {
  use super::super::{Config, LoggingConfig};
  use super::*;
  use std::collections::HashMap;
  use std::env;

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

    let config: Config = toml::from_str(toml).expect("Failed to parse TOML");
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

    let result: std::result::Result<Config, _> = toml::from_str(toml);
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

    let result: std::result::Result<Config, _> = toml::from_str(toml);
    assert!(result.is_ok(), "Valid provider type should be accepted");
    let config = result.unwrap();
    let provider = config.providers.get("kimi").unwrap();
    assert_eq!(provider.provider_type, "kimi");
  }
}
