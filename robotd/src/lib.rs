//! robotd as a library: boot, composition, capabilities, and the
//! operational subcommands.
//!
//! The binary (`src/main.rs`) is a thin dispatcher over this. The split
//! exists so integration tests can drive a real Robot -- a `[[bin]]`-only
//! crate cannot be imported, which is why the workspace had no `tests/`
//! directory and nothing covered the HTTP layer against real state.

pub mod archive;
pub mod backup;
pub mod backup_lane;
pub mod boot;
pub mod caps;
pub mod cli;
pub mod config;
pub mod evals;
pub mod maintenance;
pub mod notify;
pub mod package;
pub mod prompts;
pub mod robot;
pub mod scheduler;
pub mod telegram;
