//! trust: the substrate (arch sec 7) -- keys, encrypted cells, the boundary
//! log, core schema, and identifiers. Everything the other organs stand on.

pub mod boundary;
pub mod cells;
pub mod ids;
pub mod keys;
pub mod schema;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TrustError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sql: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("crypto: {0}")]
    Crypto(String),
    #[error("bad key material: {0}")]
    KeyMaterial(String),
}
