//! The Telegram surface loop (behind the TELEGRAM_BOT_TOKEN flag): long-poll
//! through hub's boundary-logged transport, invite-only by default (Q2) --
//! an unknown chat is told to ask the owner; binding happens with a
//! short-lived code the owner mints in their own chat.

use crate::robot::RobotCore;
use std::sync::Arc;
use std::time::Duration;
use trust::schema;

pub fn spawn(robot: Arc<RobotCore>, tg: Arc<hub::Telegram>) {
    tokio::spawn(async move {
        tracing::info!("telegram surface online (long-polling)");
        let mut offset = 0i64;
        loop {
            let tg_poll = tg.clone();
            let updates =
                tokio::task::spawn_blocking(move || tg_poll.get_updates(offset)).await;
            match updates {
                Ok(Ok(list)) => {
                    for u in list {
                        let robot = robot.clone();
                        let tg = tg.clone();
                        let update_id = u.update_id;
                        let outcome = tokio::task::spawn_blocking(move || {
                            process(&robot, &tg, &u)
                        })
                        .await;
                        // The offset is Telegram's ACK: advancing it makes the
                        // update unrecoverable. Advance only after the turn
                        // actually succeeded, so a failure is retried on the
                        // next poll instead of silently swallowing the
                        // person's message (the Second Law: never drop a
                        // request without saying so).
                        match outcome {
                            Ok(Ok(())) => offset = offset.max(update_id + 1),
                            Ok(Err(e)) => {
                                tracing::error!(
                                    "telegram turn failed for update {update_id}, will retry: {e:#}"
                                );
                                tokio::time::sleep(Duration::from_secs(2)).await;
                                break; // re-poll from the un-acked offset
                            }
                            Err(e) => {
                                tracing::error!("telegram task panicked on {update_id}: {e}");
                                tokio::time::sleep(Duration::from_secs(2)).await;
                                break;
                            }
                        }
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!("telegram poll failed: {e}");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                Err(e) => {
                    tracing::error!("telegram join error: {e}");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });
}

fn process(robot: &RobotCore, tg: &hub::Telegram, u: &hub::TgUpdate) -> anyhow::Result<()> {
    if u.text.is_empty() {
        return Ok(());
    }
    let bound_chat: Option<i64> = {
        let core = robot
            .core
            .lock()
            .map_err(|_| anyhow::anyhow!("core lock poisoned"))?;
        schema::meta_get(&core, "tg_owner_chat")?.and_then(|v| v.parse().ok())
    };

    if bound_chat == Some(u.chat_id) {
        // the owner's bound chat: a normal governed turn on the telegram surface
        let reply = robot.turn(robot.owner_principal, u.text.clone(), "telegram")?;
        tg.send_message(u.chat_id, &reply)?;
        return Ok(());
    }

    // unbound chat: check for a pending bind code (10-minute window)
    let code_ok = {
        let core = robot
            .core
            .lock()
            .map_err(|_| anyhow::anyhow!("core lock poisoned"))?;
        let hash = schema::meta_get(&core, "tg_bind_code_hash")?;
        let expiry: i64 = schema::meta_get(&core, "tg_bind_expiry")?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        match hash {
            Some(h)
                if trust::ids::sha256_hex(u.text.trim().as_bytes()) == h
                    && trust::ids::ts_ms() < expiry =>
            {
                schema::meta_set(&core, "tg_owner_chat", &u.chat_id.to_string())?;
                core.execute("DELETE FROM meta WHERE key IN ('tg_bind_code_hash','tg_bind_expiry')", [])?;
                schema::core_journal(
                    &core,
                    "telegram.bind",
                    &serde_json::json!({ "chat_id": u.chat_id }).to_string(),
                )?;
                true
            }
            _ => false,
        }
    };

    if code_ok {
        tg.send_message(
            u.chat_id,
            "bound -- this chat now talks to your robot. everything here is \
             journaled and boundary-logged like any other surface.",
        )?;
    } else {
        tg.send_message(
            u.chat_id,
            "this robot is invite-only. if it's yours, say \"telegram code\" \
             to it in your own chat and send me the 6-digit code.",
        )?;
    }
    Ok(())
}
