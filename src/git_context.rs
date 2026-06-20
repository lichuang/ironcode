//! Git repository context collection for future subagent use.
//!
//! This module mirrors the collection strategy used by kimi-cli's explore
//! subagent (`kimi_cli/subagents/git_context.py`) and formats the result as a
//! markdown block. It is intentionally kept for subagent prompt construction
//! once the subagent system is implemented; kimi-cli does **not** inject git
//! context into the main agent system prompt.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::timeout;

/// Configuration controlling git context injection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitContextConfig {
  /// Whether git context should be collected and injected at all.
  #[serde(default = "default_true")]
  pub enabled: bool,

  /// Per-command timeout in seconds.
  #[serde(default = "default_timeout_seconds")]
  pub timeout_seconds: u64,

  /// Maximum number of dirty files to list before truncating.
  #[serde(default = "default_max_dirty_files")]
  pub max_dirty_files: usize,

  /// Maximum number of recent commits to include.
  #[serde(default = "default_max_log_commits")]
  pub max_log_commits: usize,

  /// Maximum number of branches to list.
  #[serde(default = "default_max_branches")]
  pub max_branches: usize,

  /// Maximum number of lines for each `git diff --stat` block.
  #[serde(default = "default_max_diff_stat_lines")]
  pub max_diff_stat_lines: usize,
}

impl Default for GitContextConfig {
  fn default() -> Self {
    Self {
      enabled: true,
      timeout_seconds: default_timeout_seconds(),
      max_dirty_files: default_max_dirty_files(),
      max_log_commits: default_max_log_commits(),
      max_branches: default_max_branches(),
      max_diff_stat_lines: default_max_diff_stat_lines(),
    }
  }
}

const fn default_true() -> bool {
  true
}

const fn default_timeout_seconds() -> u64 {
  5
}

const fn default_max_dirty_files() -> usize {
  20
}

const fn default_max_log_commits() -> usize {
  20
}

const fn default_max_branches() -> usize {
  20
}

const fn default_max_diff_stat_lines() -> usize {
  50
}

/// Known public git hosts whose remote URLs may be safely displayed.
const ALLOWED_HOSTS: &[&str] = &[
  "github.com",
  "gitlab.com",
  "gitee.com",
  "bitbucket.org",
  "codeberg.org",
  "sr.ht",
];

static SSH_REMOTE_RE: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"^[\w.-]+@([\w.-]+):(.+)$").expect("valid SSH remote regex"));

static SSH_PROJECT_RE: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"^[\w.-]+@[\w.-]+:(.+)$").expect("valid SSH project regex"));

/// Collector that runs a small set of git commands and formats the result.
pub struct GitContext {
  config: GitContextConfig,
  cwd: PathBuf,
}

impl GitContext {
  /// Create a new collector bound to `cwd`.
  pub fn new(config: GitContextConfig, cwd: PathBuf) -> Self {
    Self { config, cwd }
  }

  /// Collect git context for the working directory.
  ///
  /// Returns an empty string when git context is disabled, when `cwd` is not
  /// inside a git repository, or when all git commands fail.
  pub async fn collect(&self) -> String {
    if !self.config.enabled {
      return String::new();
    }

    let work_dir = match run_git(&["rev-parse", "--show-toplevel"], &self.cwd, self.timeout()).await
    {
      Some(dir) => dir,
      None => return String::new(),
    };

    let log_limit = self.config.max_log_commits.to_string();
    let branch_limit = (self.config.max_branches + 1).to_string();
    let log_args = ["log", "--oneline", "-n", log_limit.as_str()];
    let branch_args = [
      "branch",
      "-a",
      "--format",
      "%(refname:short)",
      "-n",
      branch_limit.as_str(),
    ];
    let (inside_work_tree, remote_url, branch, status, unstaged_stat, staged_stat, log, branches) = tokio::join!(
      run_git(
        &["rev-parse", "--is-inside-work-tree"],
        &self.cwd,
        self.timeout()
      ),
      run_git(&["remote", "get-url", "origin"], &self.cwd, self.timeout()),
      run_git(&["branch", "--show-current"], &self.cwd, self.timeout()),
      run_git(&["status", "--porcelain"], &self.cwd, self.timeout()),
      run_git(&["diff", "--stat"], &self.cwd, self.timeout()),
      run_git(&["diff", "--cached", "--stat"], &self.cwd, self.timeout()),
      run_git(&log_args, &self.cwd, self.timeout()),
      run_git(&branch_args, &self.cwd, self.timeout()),
    );

    if inside_work_tree.as_deref() != Some("true") {
      return String::new();
    }

    Some(format_git_context(
      &work_dir,
      remote_url.as_deref(),
      branch.as_deref(),
      status.as_deref(),
      unstaged_stat.as_deref(),
      staged_stat.as_deref(),
      log.as_deref(),
      branches.as_deref(),
      self.config.max_dirty_files,
      self.config.max_branches,
      self.config.max_diff_stat_lines,
    ))
    .filter(|s| !s.is_empty())
    .unwrap_or_default()
  }

  fn timeout(&self) -> Duration {
    Duration::from_secs(self.config.timeout_seconds)
  }
}

/// Run a git command and return its trimmed stdout, or `None` on any failure.
async fn run_git(args: &[&str], cwd: &Path, timeout_d: Duration) -> Option<String> {
  let result = timeout(
    timeout_d,
    Command::new("git")
      .args(args)
      .current_dir(cwd)
      .kill_on_drop(true)
      .output(),
  )
  .await;

  match result {
    Ok(Ok(output)) if output.status.success() => {
      Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
    _ => None,
  }
}

/// Format collected git information as a markdown block.
#[allow(clippy::too_many_arguments)]
fn format_git_context(
  work_dir: &str,
  remote_url: Option<&str>,
  branch: Option<&str>,
  status: Option<&str>,
  unstaged_stat: Option<&str>,
  staged_stat: Option<&str>,
  log: Option<&str>,
  branches: Option<&str>,
  max_dirty_files: usize,
  max_branches: usize,
  max_diff_stat_lines: usize,
) -> String {
  let mut sections: Vec<String> = Vec::new();

  sections.push("## Repository Context".to_string());
  sections.push(String::new());
  sections.push(format!("- **Working directory**: {}", work_dir));

  let remote_display = remote_url.and_then(sanitize_remote_url);
  if let Some(remote) = remote_display {
    sections.push(format!("- **Remote**: {}", remote));
  }

  if let Some(project) = remote_url.and_then(parse_project_name) {
    sections.push(format!("- **Project**: {}", project));
  }

  if let Some(branch) = branch {
    sections.push(format!("- **Branch**: {}", branch));
  }

  sections.push(String::new());

  // Working tree status
  let status_body = {
    let s = status.unwrap_or("");
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
      "No changes.".to_string()
    } else {
      let total = lines.len();
      let shown = lines.len().min(max_dirty_files);
      let mut body: String = lines[..shown]
        .iter()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n");
      if total > max_dirty_files {
        body.push_str(&format!("\n  ... and {} more", total - max_dirty_files));
      }
      body
    }
  };
  sections.push("### Working tree status".to_string());
  sections.push("```text".to_string());
  sections.push(status_body);
  sections.push("```".to_string());
  sections.push(String::new());

  // Diff stats
  if let Some(stat) = truncate_lines(unstaged_stat, max_diff_stat_lines) {
    sections.push("### Unstaged diff stat".to_string());
    sections.push("```text".to_string());
    sections.push(stat);
    sections.push("```".to_string());
    sections.push(String::new());
  }

  if let Some(stat) = truncate_lines(staged_stat, max_diff_stat_lines) {
    sections.push("### Staged diff stat".to_string());
    sections.push("```text".to_string());
    sections.push(stat);
    sections.push("```".to_string());
    sections.push(String::new());
  }

  // Recent commits
  if let Some(log) = log {
    let lines: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    if !lines.is_empty() {
      let total = lines.len();
      let shown = lines.len().min(max_diff_stat_lines);
      let body: String = lines[..shown]
        .iter()
        .map(|l| format!("  {}", &l[..l.len().min(200)]))
        .collect::<Vec<_>>()
        .join("\n");
      let mut body = body;
      if total > max_diff_stat_lines {
        body.push_str(&format!("\n  ... and {} more", total - max_diff_stat_lines));
      }
      sections.push("### Recent commits".to_string());
      sections.push("```text".to_string());
      sections.push(body);
      sections.push("```".to_string());
      sections.push(String::new());
    }
  }

  // All branches
  if let Some(branches) = branches {
    let lines: Vec<&str> = branches.lines().filter(|l| !l.trim().is_empty()).collect();
    if !lines.is_empty() {
      let total = lines.len();
      let shown = lines.len().min(max_branches);
      let mut body: String = lines[..shown]
        .iter()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n");
      if total > max_branches {
        body.push_str(&format!("\n  ... and {} more", total - max_branches));
      }
      sections.push("### Branches".to_string());
      sections.push("```text".to_string());
      sections.push(body);
      sections.push("```".to_string());
    }
  }

  let mut output = sections.join("\n");
  output.truncate(output.trim_end().len());
  output
}

/// Truncate a multi-line string to at most `max_lines` and indent each line.
fn truncate_lines(text: Option<&str>, max_lines: usize) -> Option<String> {
  let text = text?;
  let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
  if lines.is_empty() {
    return None;
  }

  let total = lines.len();
  let shown = lines.len().min(max_lines);
  let mut body: String = lines[..shown]
    .iter()
    .map(|l| format!("  {l}"))
    .collect::<Vec<_>>()
    .join("\n");
  if total > max_lines {
    body.push_str(&format!("\n  ... and {} more", total - max_lines));
  }
  Some(body)
}

/// Sanitize a remote URL so that credentials and non-public hosts are hidden.
fn sanitize_remote_url(url: &str) -> Option<String> {
  // SSH-like: git@host:path
  if let Some(captures) = SSH_REMOTE_RE.captures(url) {
    let host = captures.get(1)?.as_str();
    if is_allowed_host(host) {
      return Some(url.to_string());
    }
    return None;
  }

  // URL-like: https://user:pass@host/path or ssh://git@host/path
  let mut parsed = url::Url::parse(url).ok()?;
  let host = parsed.host_str()?.to_lowercase();
  if !is_allowed_host(&host) {
    return None;
  }

  let _ = parsed.set_username("");
  let _ = parsed.set_password(None);
  let mut display = parsed.to_string();
  // Url::to_string may leave an empty username (e.g. "https://@github.com/..."); clean it up.
  display = display.replace("://@", "://");
  Some(display.trim_end_matches(".git").to_string())
}

/// Extract "owner/repo" from any supported remote URL format.
fn parse_project_name(url: &str) -> Option<String> {
  // SSH-like
  if let Some(captures) = SSH_PROJECT_RE.captures(url) {
    let path = captures.get(1)?.as_str();
    return clean_project_path(path);
  }

  let parsed = url::Url::parse(url).ok()?;
  clean_project_path(parsed.path())
}

fn clean_project_path(path: &str) -> Option<String> {
  let path = path.trim_start_matches('/').trim_end_matches(".git");
  let mut parts = path.split('/').filter(|p| !p.is_empty());
  let owner = parts.next()?;
  let repo = parts.next()?;
  Some(format!("{}/{}", owner, repo))
}

fn is_allowed_host(host: &str) -> bool {
  ALLOWED_HOSTS
    .iter()
    .any(|allowed| host.eq_ignore_ascii_case(allowed))
}

#[cfg(test)]
mod tests {
  use std::process::Command as SyncCommand;

  use super::*;

  #[test]
  fn test_sanitize_remote_url_public_https() {
    assert_eq!(
      sanitize_remote_url("https://user:pass@github.com/owner/repo.git"),
      Some("https://github.com/owner/repo".to_string())
    );
  }

  #[test]
  fn test_sanitize_remote_url_public_ssh() {
    assert_eq!(
      sanitize_remote_url("git@github.com:owner/repo.git"),
      Some("git@github.com:owner/repo.git".to_string())
    );
  }

  #[test]
  fn test_sanitize_remote_url_self_hosted_hidden() {
    assert_eq!(
      sanitize_remote_url("https://git.company.com/owner/repo.git"),
      None
    );
  }

  #[test]
  fn test_parse_project_name_ssh() {
    assert_eq!(
      parse_project_name("git@github.com:moonshot-ai/kimi-cli.git"),
      Some("moonshot-ai/kimi-cli".to_string())
    );
  }

  #[test]
  fn test_parse_project_name_https_with_credentials() {
    assert_eq!(
      parse_project_name("https://user:pass@gitlab.com/org/project.git"),
      Some("org/project".to_string())
    );
  }

  fn init_git_repo(dir: &Path) {
    SyncCommand::new("git")
      .args(["init"])
      .current_dir(dir)
      .output()
      .expect("git init failed");
    SyncCommand::new("git")
      .args(["config", "user.email", "test@example.com"])
      .current_dir(dir)
      .output()
      .unwrap();
    SyncCommand::new("git")
      .args(["config", "user.name", "Test User"])
      .current_dir(dir)
      .output()
      .unwrap();
  }

  #[tokio::test]
  async fn test_collect_empty_outside_git_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = GitContext::new(GitContextConfig::default(), tmp.path().to_path_buf());
    assert!(ctx.collect().await.is_empty());
  }

  #[tokio::test]
  async fn test_collect_clean_repo() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    // Create and commit a file so log/status have content.
    std::fs::write(tmp.path().join("README.md"), "# hello").unwrap();
    SyncCommand::new("git")
      .args(["add", "README.md"])
      .current_dir(tmp.path())
      .output()
      .unwrap();
    SyncCommand::new("git")
      .args(["commit", "-m", "initial"])
      .current_dir(tmp.path())
      .output()
      .unwrap();

    let ctx = GitContext::new(GitContextConfig::default(), tmp.path().to_path_buf());
    let result = ctx.collect().await;
    assert!(result.contains("## Repository Context"));
    assert!(result.contains("Working directory"));
    assert!(result.contains("initial"));
    assert!(result.contains("No changes."));
  }

  #[tokio::test]
  async fn test_collect_dirty_files_cap() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    for i in 0..5 {
      std::fs::write(tmp.path().join(format!("file{}.txt", i)), "x").unwrap();
    }

    let config = GitContextConfig {
      max_dirty_files: 2,
      ..GitContextConfig::default()
    };
    let ctx = GitContext::new(config, tmp.path().to_path_buf());
    let result = ctx.collect().await;
    assert!(result.contains("... and 3 more"));
  }

  #[tokio::test]
  async fn test_disabled_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());

    let config = GitContextConfig {
      enabled: false,
      ..GitContextConfig::default()
    };
    let ctx = GitContext::new(config, tmp.path().to_path_buf());
    assert!(ctx.collect().await.is_empty());
  }
}
