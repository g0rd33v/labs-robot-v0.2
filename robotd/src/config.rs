//! robot.toml -- the instance configuration. Written with defaults on first
//! boot if absent; never contains secrets (those live in the vault/keychain).

use anyhow::Context;
use serde::Deserialize;
use std::fs;
use std::path::Path;

const DEFAULT_TOML: &str = r#"# bender -- robot v0.2 instance configuration.
# no secrets belong in this file.

[robot]
name = "bender"
data_dir = "./data"

[server]
host = "127.0.0.1"
port = 7777

[mind]
# local embedding seat (bge-m3-class); weights are fetched once through the
# hub gateway (boundary-logged) into model_cache. set false to run without
# the vector door -- recall degrades to FTS + recency.
embeddings = true
model_cache = "./data/models"

[hub]
# the intelligence gateway (arch sec 6). the API keys are NOT here -- they
# come from the environment (OPENROUTER_API_KEY, SERPER_API_KEY), pulled
# from the OS keychain at launch. no key = the deterministic floor still
# works and the robot says so honestly.
base_url = "https://openrouter.ai/api/v1"
hedge_after_ms = 2500
verify_percent = 10
ultra_daily_cap = 20
# the cast (sec 6a / Q28) is overridable per role:
# [hub.cast]
# verdict = "google/gemma-4-26b-a4b-it"
# answer  = "google/gemma-4-31b-it"

[backup]
# off-site backup runs inside the robot (a launchd agent is blocked by macos
# TCC from reading ~/Documents). 0 disables it. failures are reported in chat.
every_hours = 24
script = "./scripts/backup-offsite.sh"

[sync]
# other instances of THIS robot to stay in agreement with -- a usb stick, a
# second machine. two-way: knowledge merges both directions, deletions
# propagate and do not come back. a peer that is not plugged in is skipped
# silently. 0 disables the lane.
peers = []
every_minutes = 10
"#;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct RobotConfig {
    pub robot: RobotSection,
    pub server: ServerSection,
    pub mind: MindSection,
    pub hub: HubSection,
    pub backup: BackupSection,
    pub sync: SyncSection,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct RobotSection {
    pub name: String,
    pub data_dir: String,
}

impl Default for RobotSection {
    fn default() -> Self {
        Self {
            name: "bender".into(),
            data_dir: "./data".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ServerSection {
    pub host: String,
    pub port: u16,
    /// base URL printed in invite links; empty = http://host:port
    pub public_base: String,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 7777,
            public_base: String::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct MindSection {
    pub embeddings: bool,
    pub model_cache: String,
}

impl Default for MindSection {
    fn default() -> Self {
        Self {
            embeddings: true,
            model_cache: "./data/models".into(),
        }
    }
}

/// Other instances of THIS robot to stay in agreement with.
///
/// A peer that is not present is not an error -- a removable disk is absent
/// most of the time -- so the lane simply skips it and says nothing.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct SyncSection {
    /// paths to other instances; `~/` is expanded
    pub peers: Vec<String>,
    /// minutes between sweeps; 0 disables the lane
    pub every_minutes: u64,
}

impl Default for SyncSection {
    fn default() -> Self {
        Self {
            peers: vec![],
            every_minutes: 10,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct BackupSection {
    /// hours between off-site backups; 0 disables the lane
    pub every_hours: u64,
    pub script: String,
}

impl Default for BackupSection {
    fn default() -> Self {
        Self {
            every_hours: 24,
            script: "./scripts/backup-offsite.sh".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct HubSection {
    pub base_url: String,
    pub hedge_after_ms: u64,
    pub ultra_daily_cap: u32,
    /// Q26: percent of ROUTINE turns whose expression is verified on the
    /// evaluator seat. Turns that acted are always verified regardless.
    pub verify_percent: u32,
    pub cast: hub::Cast,
}

impl Default for HubSection {
    fn default() -> Self {
        Self {
            base_url: "https://openrouter.ai/api/v1".into(),
            hedge_after_ms: 2500,
            ultra_daily_cap: 20,
            verify_percent: 10,
            cast: hub::Cast::default(),
        }
    }
}

/// Load config from `path`; write the default template first if absent.
pub fn load(path: &Path) -> anyhow::Result<RobotConfig> {
    if !path.exists() {
        fs::write(path, DEFAULT_TOML)
            .with_context(|| format!("writing default config to {}", path.display()))?;
        tracing::info!("wrote default config to {}", path.display());
    }
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading config {}", path.display()))?;
    let cfg: RobotConfig =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_template_parses_and_matches_defaults() {
        let cfg: RobotConfig = toml::from_str(DEFAULT_TOML).unwrap();
        assert_eq!(cfg.robot.name, "bender");
        assert_eq!(cfg.server.port, 7777);
        assert_eq!(cfg.server.host, "127.0.0.1");
    }

    #[test]
    fn missing_sections_fall_back_to_defaults() {
        let cfg: RobotConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.robot.data_dir, "./data");
        assert_eq!(cfg.server.port, 7777);
    }
}
