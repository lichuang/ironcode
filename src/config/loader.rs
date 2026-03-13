//! Configuration file loader

use super::Config;
use crate::error::{ConfigError, Result};
use std::path::PathBuf;

/// Default configuration directory name (in home directory)
const CONFIG_DIR: &str = ".ironcode";

/// Default configuration file name
const CONFIG_FILE: &str = "config.toml";

/// Load configuration from standard locations
///
/// Configuration is loaded from (in order of precedence):
/// 1. `./ironcode.toml` (project-local)
/// 2. `~/.ironcode/config.toml` (user-global)
///
/// Later files override earlier ones.
pub fn load_config() -> Result<Config> {
  let config_path = user_config_path().ok_or(ConfigError::HomeDirNotFound)?;
  load_config_from(&config_path)
}

/// Load configuration from a specific file path
///
/// Also merges with project-local `./ironcode.toml` if it exists.
pub fn load_config_from(path: &PathBuf) -> Result<Config> {
  let mut config = Config::default();

  // Load from specified config file (lower priority)
  if path.exists() {
    let file_config = load_from_file(path)?;
    config = merge_configs(config, file_config);
  }

  // Load from project-local config (higher priority)
  let local_config_path = PathBuf::from("ironcode.toml");
  if local_config_path.exists() {
    let local_config = load_from_file(&local_config_path)?;
    config = merge_configs(config, local_config);
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

/// Get the user configuration directory path (~/.ironcode/config.toml)
fn user_config_path() -> Option<PathBuf> {
  dirs::home_dir().map(|dir| dir.join(CONFIG_DIR).join(CONFIG_FILE))
}

/// Merge two configurations (second overrides first)
fn merge_configs(base: Config, override_: Config) -> Config {
  Config {
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
    logging: override_.logging,
  }
}

/// Validate configuration
fn validate_config(config: &Config) -> Result<()> {
  if config.default_model.is_empty() {
    return Err(ConfigError::MissingDefaultModel.into());
  }

  // Check that default_model exists in models
  if !config.models.contains_key(&config.default_model) {
    return Err(ConfigError::ModelNotFound {
      model: config.default_model.clone(),
    }
    .into());
  }

  Ok(())
}

/// Ensure configuration directory exists
pub fn ensure_config_dir() -> Result<PathBuf> {
  let config_dir = dirs::config_dir()
    .ok_or(ConfigError::ConfigDirNotFound)?
    .join(CONFIG_DIR);

  if !config_dir.exists() {
    std::fs::create_dir_all(&config_dir)
      .map_err(|e| ConfigError::create_dir(&config_dir, e))?;
  }

  Ok(config_dir)
}

/// Create a default configuration file if it doesn't exist
pub fn create_default_config() -> Result<PathBuf> {
  let config_dir = ensure_config_dir()?;
  let config_path = config_dir.join(CONFIG_FILE);

  if !config_path.exists() {
    let default_config = r#"# IronCode Configuration File
# Location: ~/.config/ironcode/config.toml

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
  use super::*;
  use super::super::{Config, LoggingConfig};
  use std::collections::HashMap;
  use std::env;

  fn fixtures_dir() -> PathBuf {
    PathBuf::from(file!())
      .parent()
      .unwrap()
      .join("fixtures")
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
  fn test_provider_type_requires_api_key() {
    use super::super::ProviderType;

    // OpenAI-compatible providers typically require an API key
    assert!(ProviderType::OpenaiCompatible.requires_api_key());
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
      default_model: "nonexistent/model".to_string(),
      providers: HashMap::new(),
      models: HashMap::new(),
      logging: LoggingConfig::default(),
    };
    let result = validate_config(&config);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("not found in [models] section"));
  }

  #[test]
  fn test_invalid_provider_type_rejected() {
    // Test that deprecated provider types are rejected
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
    assert!(result.is_err(), "Deprecated 'openai' provider type should be rejected");
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("openai") || err_msg.contains("unknown variant"), 
            "Error message should indicate invalid provider type: {}", err_msg);
  }

  #[test]
  fn test_valid_provider_type_accepted() {
    // Test that 'openai-compatible' provider type is accepted
    let toml = r#"
default_model = "openai/gpt-4o"

[providers.openai]
type = "openai-compatible"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"

[models."openai/gpt-4o"]
provider = "openai"
model = "gpt-4o"
"#;

    let result: std::result::Result<Config, _> = toml::from_str(toml);
    assert!(result.is_ok(), "Valid 'openai-compatible' provider type should be accepted");
    let config = result.unwrap();
    let provider = config.providers.get("openai").unwrap();
    assert!(matches!(provider.provider_type, crate::config::ProviderType::OpenaiCompatible));
  }
}
