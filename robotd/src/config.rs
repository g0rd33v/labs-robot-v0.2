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
ultra_daily_cap = 20
# the cast (sec 6a / Q28) is overridable per role:
# [hub.cast]
# verdict = "google/gemma-4-26b-a4b-it"
# answer  = "google/gemma-4-31b-it"
"#;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct RobotConfig {
    pub robot: RobotSection,
    pub server: ServerSection,
    pub mind: MindSection,
    pub hub: HubSection,
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

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct HubSection {
    pub base_url: String,
    pub hedge_after_ms: u64,
    pub ultra_daily_cap: u32,
    pub cast: hub::Cast,
}

impl Default for HubSection {
    fn default() -> Self {
        Self {
            base_url: "https://openrouter.ai/api/v1".into(),
            hedge_after_ms: 2500,
            ultra_daily_cap: 20,
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
