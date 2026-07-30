//! hub: the sole external gate (arch sec 6).
//!
//! Law #3: every external relationship of the Robot -- model APIs, search,
//! web fetch, Telegram, and model-weight downloads -- is held here and
//! nowhere else. M3 adds the local tier of the intelligence gateway: the
//! embedding seat (bge-m3-class, Q24) running on CPU inside the process.
//! The one external act it performs -- fetching model weights on first init
//! -- is boundary-logged in both directions. M4 fills in OpenRouter, Serper,
//! and the fetch/READ loop.

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;
use thiserror::Error;
use trust::boundary::{self, Crossing, Direction};

#[derive(Debug, Error)]
pub enum HubError {
    #[error("embed: {0}")]
    Embed(String),
    #[error("trust: {0}")]
    Trust(#[from] trust::TrustError),
}

#[derive(Default)]
pub struct Gateway {
    _sealed: (),
}

impl Gateway {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of external endpoints configured. Still zero in M3: the
    /// embedder is local inference, not an external relationship.
    pub fn endpoints(&self) -> usize {
        0
    }
}

/// The local embedding seat (Q24): multilingual, retrieval-tuned, CPU-fast.
/// e5-small fills the bge-m3-class seat for the MVP; swapping the model is a
/// re-index, not a redesign.
pub struct Embedder {
    inner: Mutex<TextEmbedding>,
    pub dim: usize,
}

impl Embedder {
    /// Initialize, downloading weights into `cache_dir` on first run. The
    /// download is external I/O and is boundary-logged (both directions)
    /// when `core` is provided.
    pub fn init(cache_dir: &Path, core: Option<&Connection>) -> Result<Self, HubError> {
        let fresh = !cache_dir.exists()
            || std::fs::read_dir(cache_dir)
                .map(|mut d| d.next().is_none())
                .unwrap_or(true);
        if fresh {
            if let Some(conn) = core {
                boundary::append(
                    conn,
                    &Crossing {
                        direction: Direction::Out,
                        channel: "model-download".into(),
                        counterparty: "huggingface.co".into(),
                        purpose: "fetch embedding model weights (bge-m3-class seat)".into(),
                        categories: "model-request".into(),
                        payload_hash: String::new(),
                        size: 0,
                        trust_tag: "granted".into(),
                    },
                )?;
            }
        }
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::MultilingualE5Small)
                .with_cache_dir(cache_dir.to_path_buf())
                .with_show_download_progress(false),
        )
        .map_err(|e| HubError::Embed(format!("init: {e}")))?;
        if fresh {
            if let Some(conn) = core {
                let bytes = dir_size(cache_dir);
                boundary::append(
                    conn,
                    &Crossing {
                        direction: Direction::In,
                        channel: "model-download".into(),
                        counterparty: "huggingface.co".into(),
                        purpose: "embedding model weights received".into(),
                        categories: "model-weights".into(),
                        payload_hash: String::new(),
                        size: bytes,
                        trust_tag: "untrusted".into(),
                    },
                )?;
            }
            tracing::info!("embedding model cached in {}", cache_dir.display());
        }
        Ok(Self {
            inner: Mutex::new(model),
            dim: 384,
        })
    }

    /// Embed stored content (e5 asymmetric scheme: passage side).
    pub fn embed_passage(&self, text: &str) -> Result<Vec<f32>, HubError> {
        self.embed(format!("passage: {text}"))
    }

    /// Embed a retrieval query (e5 asymmetric scheme: query side).
    pub fn embed_query(&self, text: &str) -> Result<Vec<f32>, HubError> {
        self.embed(format!("query: {text}"))
    }

    fn embed(&self, text: String) -> Result<Vec<f32>, HubError> {
        let mut model = self
            .inner
            .lock()
            .map_err(|_| HubError::Embed("embedder lock poisoned".into()))?;
        let mut out = model
            .embed(vec![text], None)
            .map_err(|e| HubError::Embed(format!("embed: {e}")))?;
        out.pop()
            .ok_or_else(|| HubError::Embed("empty embedding batch".into()))
    }
}

fn dir_size(dir: &Path) -> i64 {
    fn walk(dir: &Path, acc: &mut i64) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, acc);
                } else if let Ok(md) = e.metadata() {
                    *acc += md.len() as i64;
                }
            }
        }
    }
    let mut total = 0;
    walk(dir, &mut total);
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m3_gateway_still_has_zero_external_endpoints() {
        assert_eq!(Gateway::new().endpoints(), 0);
    }
}
