//! RobotCore: the composition of the organs behind the `surfaces::Robot`
//! trait. Owns the connections; enforces the boundary-log law on every turn.

use anyhow::anyhow;
use rusqlite::Connection;
use std::sync::Mutex;
use trust::boundary::{self, Crossing, Direction};

pub struct RobotCore {
    pub owner_principal: i64,
    pub core: Mutex<Connection>,
    pub owner_cell: Mutex<Connection>,
}

fn chat_crossing(direction: Direction, payload_hash: String, size: i64) -> Crossing {
    Crossing {
        direction,
        channel: "chat".into(),
        counterparty: "local-web".into(),
        purpose: "conversation".into(),
        categories: "message".into(),
        payload_hash,
        size,
        // the local owner session; remote/unknown surfaces get `untrusted`
        trust_tag: "owner".into(),
    }
}

impl surfaces::Robot for RobotCore {
    fn handle_message(&self, text: String) -> anyhow::Result<String> {
        // 1. boundary log: the inbound crossing, before anything else (law #3)
        {
            let core = self
                .core
                .lock()
                .map_err(|_| anyhow!("core lock poisoned"))?;
            boundary::append(
                &core,
                &chat_crossing(
                    Direction::In,
                    trust::ids::sha256_hex(text.as_bytes()),
                    text.len() as i64,
                ),
            )?;
        }

        // 2. the turn, inside the owner's encrypted cell
        let reply = {
            let cell = self
                .owner_cell
                .lock()
                .map_err(|_| anyhow!("cell lock poisoned"))?;
            mind::record_message(&cell, "in", "chat", &text)?;
            let env = prism::Envelope {
                surface: "chat".into(),
                principal_id: self.owner_principal,
                modality: "text".into(),
                content: text,
                ts: trust::ids::ts_ms(),
                device_trust: "owner-session".into(),
            };
            let reply = prism::handle_turn(&cell, &env)?;
            mind::record_message(&cell, "out", "chat", &reply)?;
            reply
        };

        // 3. boundary log: the outbound crossing, before the reply leaves
        {
            let core = self
                .core
                .lock()
                .map_err(|_| anyhow!("core lock poisoned"))?;
            boundary::append(
                &core,
                &chat_crossing(
                    Direction::Out,
                    trust::ids::sha256_hex(reply.as_bytes()),
                    reply.len() as i64,
                ),
            )?;
        }

        Ok(reply)
    }
}
