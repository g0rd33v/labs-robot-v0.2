//! Updates & release channels (§13e), scaled to a self-hosted instance.
//!
//! *"The binary verifies signatures before applying — unsigned or tampered
//! updates do not install, anywhere. Self-update with self-rollback: new
//! version alongside, switch on success; a failed post-upgrade health check
//! rolls back automatically."*
//!
//! What exists here and what deliberately does not: signature-gated
//! install, the health check, atomic switch, rollback, version pinning —
//! all local properties — exist. Staged fleet rollout and canary waves are
//! Control-Plane work and are not pretended at.
//!
//! Two lessons already paid for are load-bearing:
//! * the binary is replaced by **rename, never by copy-over** — copying
//!   over a running binary killed it (exit 137) in an earlier session;
//! * the manifest names the version the signature covers, so a signature
//!   cannot be replayed onto different bytes or a different version.

use anyhow::{anyhow, bail, Context};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The release verify key, embedded at build time. The signing half lives
/// in the operator's keychain (`RELEASE_SIGNING_KEY`) and never ships.
/// Rotating it is a release: embed the new key, sign with the old one.
pub const RELEASE_PUBKEY_HEX: &str =
    env_or_default_pubkey();

const fn env_or_default_pubkey() -> &'static str {
    match option_env!("BENDER_RELEASE_PUBKEY") {
        Some(k) => k,
        // the pair generated 2026-08-01 for this instance's channel; the
        // private half is in the owner's macOS keychain, never on disk
        None => "ce9f11c7318e77c71a2f47fc85ca35fb43c1192eae72add8dad8be005bed6408",
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReleaseManifest {
    pub version: String,
    pub channel: String,
    /// sha256 of the release binary
    pub sha256: String,
    /// ed25519 over [`signed_payload`], hex
    pub signature: String,
    /// where the binary is (a path or file:// URL for self-hosted; the
    /// Control Plane serves https later)
    pub binary: String,
    #[serde(default)]
    pub changelog: String,
    /// flagged per §13e so a pinned owner can see what pinning is costing
    #[serde(default)]
    pub security: bool,
}

/// The exact bytes the signature covers. Version and channel are INSIDE
/// the signed payload: a signature over the hash alone could be replayed
/// with a manifest claiming any version on any channel.
pub fn signed_payload(m: &ReleaseManifest) -> String {
    format!("bender-release\n{}\n{}\n{}", m.version, m.channel, m.sha256)
}

pub fn verify_manifest(m: &ReleaseManifest, pubkey_hex: &str) -> anyhow::Result<()> {
    let pk_bytes: [u8; 32] = hex::decode(pubkey_hex)
        .map_err(|e| anyhow!("release pubkey: {e}"))?
        .try_into()
        .map_err(|_| anyhow!("release pubkey must be 32 bytes"))?;
    let pk = VerifyingKey::from_bytes(&pk_bytes).map_err(|e| anyhow!("release pubkey: {e}"))?;
    let sig_bytes: [u8; 64] = hex::decode(&m.signature)
        .map_err(|e| anyhow!("signature: {e}"))?
        .try_into()
        .map_err(|_| anyhow!("signature must be 64 bytes"))?;
    let sig = Signature::from_bytes(&sig_bytes);
    pk.verify(signed_payload(m).as_bytes(), &sig)
        .map_err(|_| anyhow!("SIGNATURE INVALID -- this release was not signed by the \
                              channel key. it does not install, anywhere."))?;
    Ok(())
}

fn read_manifest(channel_url: &str) -> anyhow::Result<ReleaseManifest> {
    let raw = if let Some(path) = channel_url.strip_prefix("file://") {
        std::fs::read_to_string(path)?
    } else if channel_url.starts_with("http") {
        ureq::get(channel_url)
            .timeout(std::time::Duration::from_secs(20))
            .call()
            .with_context(|| format!("fetching {channel_url}"))?
            .into_string()?
    } else {
        std::fs::read_to_string(channel_url)?
    };
    Ok(serde_json::from_str(&raw)?)
}

fn read_binary(src: &str) -> anyhow::Result<Vec<u8>> {
    if let Some(path) = src.strip_prefix("file://") {
        Ok(std::fs::read(path)?)
    } else if src.starts_with("http") {
        let resp = ureq::get(src)
            .timeout(std::time::Duration::from_secs(300))
            .call()
            .with_context(|| format!("downloading {src}"))?;
        let mut buf = vec![];
        use std::io::Read;
        resp.into_reader()
            .take(512 * 1024 * 1024)
            .read_to_end(&mut buf)?;
        Ok(buf)
    } else {
        Ok(std::fs::read(src)?)
    }
}

/// `robotd update --check`: is there anything newer, and is it real?
pub fn check(channel_url: &str, pinned: Option<&str>) -> anyhow::Result<i32> {
    let m = read_manifest(channel_url)?;
    verify_manifest(&m, RELEASE_PUBKEY_HEX)?;
    let current = env!("CARGO_PKG_VERSION");
    println!(
        "channel {}: version {} (running {}){}",
        m.channel,
        m.version,
        current,
        if m.security { "  [SECURITY]" } else { "" }
    );
    if !m.changelog.is_empty() {
        println!("changelog: {}", m.changelog);
    }
    if let Some(pin) = pinned {
        println!(
            "version PINNED to {pin} -- updates wait for you. pinning means \
             owning your own patch latency{}",
            if m.security { ", AND THIS ONE IS FLAGGED SECURITY" } else { "" }
        );
        return Ok(0);
    }
    if m.version == current {
        println!("up to date.");
    } else {
        println!("update available -- apply with: robotd update --apply");
    }
    Ok(0)
}

/// `robotd update --apply`: verify, stage, health-check, switch, keep the
/// previous binary for rollback. The switch is rename-only.
pub fn apply(channel_url: &str, pinned: Option<&str>) -> anyhow::Result<i32> {
    if let Some(pin) = pinned {
        bail!("version is pinned to {pin}; unpin in robot.toml to update");
    }
    let m = read_manifest(channel_url)?;
    verify_manifest(&m, RELEASE_PUBKEY_HEX)?;
    let current_exe = std::env::current_exe()?;
    let bytes = read_binary(&m.binary)?;
    let got = trust::ids::sha256_hex(&bytes);
    if got != m.sha256 {
        bail!(
            "the binary's hash does not match the signed manifest \
             (manifest {}, got {got}) -- refusing to install tampered bytes",
            m.sha256
        );
    }

    // stage beside the live binary (same filesystem, so rename is atomic)
    let staged = current_exe.with_extension("staged");
    let prev = current_exe.with_extension("prev");
    std::fs::write(&staged, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }

    // sec 13e: boot + journal replay + one synthetic turn, in the NEW
    // binary, before it becomes the binary
    println!("health-checking the staged binary...");
    let health = std::process::Command::new(&staged).arg("health").status()?;
    if !health.success() {
        let _ = std::fs::remove_file(&staged);
        bail!(
            "the staged {} FAILED its health check -- nothing was changed, \
             {} still runs",
            m.version,
            env!("CARGO_PKG_VERSION")
        );
    }

    // the switch: rename current aside, rename staged in. Never copy-over:
    // copying over a running binary kills it (exit 137, learned live).
    let _ = std::fs::remove_file(&prev);
    std::fs::rename(&current_exe, &prev)?;
    std::fs::rename(&staged, &current_exe)?;
    println!(
        "installed {} (was {}). previous binary kept at {} -- \
         `robotd update --rollback` restores it. restart the robot to run it.",
        m.version,
        env!("CARGO_PKG_VERSION"),
        prev.display()
    );
    Ok(0)
}

/// `robotd update --rollback`: the previous binary comes back, by rename.
pub fn rollback() -> anyhow::Result<i32> {
    let current_exe = std::env::current_exe()?;
    let prev = current_exe.with_extension("prev");
    if !prev.exists() {
        bail!("no previous binary at {} -- nothing to roll back to", prev.display());
    }
    let undone = current_exe.with_extension("undone");
    let _ = std::fs::remove_file(&undone);
    std::fs::rename(&current_exe, &undone)?;
    std::fs::rename(&prev, &current_exe)?;
    println!(
        "rolled back. the replaced binary is kept at {} in case this was a \
         mistake. restart the robot.",
        undone.display()
    );
    Ok(0)
}

/// `robotd update --sign <binary>`: the operator-side half. The signing key
/// comes from the environment (keychain-injected, like every secret) and
/// is never written anywhere.
pub fn sign(binary: &Path, version: &str, channel: &str, changelog: &str) -> anyhow::Result<i32> {
    use ed25519_dalek::{Signer, SigningKey};
    let key_hex = std::env::var("RELEASE_SIGNING_KEY")
        .map_err(|_| anyhow!("RELEASE_SIGNING_KEY not set (it lives in the keychain)"))?;
    let key_bytes: [u8; 32] = hex::decode(key_hex.trim())?
        .try_into()
        .map_err(|_| anyhow!("signing key must be 32 bytes"))?;
    let sk = SigningKey::from_bytes(&key_bytes);

    let bytes = std::fs::read(binary)?;
    let mut m = ReleaseManifest {
        version: version.into(),
        channel: channel.into(),
        sha256: trust::ids::sha256_hex(&bytes),
        signature: String::new(),
        binary: format!("file://{}", binary.display()),
        changelog: changelog.into(),
        security: false,
    };
    let sig = sk.sign(signed_payload(&m).as_bytes());
    m.signature = hex::encode(sig.to_bytes());

    // sanity: what we just signed must verify with the embedded pubkey,
    // or the channel key and the signing key have drifted apart
    let embedded_pk = hex::encode(sk.verifying_key().to_bytes());
    if embedded_pk != RELEASE_PUBKEY_HEX {
        eprintln!(
            "warning: this signing key's public half ({embedded_pk}) is NOT \
             the embedded release pubkey -- binaries built without \
             BENDER_RELEASE_PUBKEY={embedded_pk} will refuse this release"
        );
    }
    println!("{}", serde_json::to_string_pretty(&m)?);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signed_release(sk: &SigningKey, version: &str, sha: &str) -> ReleaseManifest {
        let mut m = ReleaseManifest {
            version: version.into(),
            channel: "stable".into(),
            sha256: sha.into(),
            signature: String::new(),
            binary: "file:///tmp/none".into(),
            changelog: String::new(),
            security: false,
        };
        m.signature = hex::encode(sk.sign(signed_payload(&m).as_bytes()).to_bytes());
        m
    }

    /// §13e's sentence, as a test: unsigned or tampered does not install.
    #[test]
    fn only_the_channel_key_signs_and_nothing_survives_tampering() {
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let pk = hex::encode(sk.verifying_key().to_bytes());
        let m = signed_release(&sk, "0.3.0", &"a".repeat(64));
        assert!(verify_manifest(&m, &pk).is_ok());

        // tampered hash: the signature no longer covers the bytes
        let mut bad = signed_release(&sk, "0.3.0", &"a".repeat(64));
        bad.sha256 = "b".repeat(64);
        assert!(verify_manifest(&bad, &pk).is_err());

        // replayed signature onto a different version
        let mut replay = signed_release(&sk, "0.3.0", &"a".repeat(64));
        replay.version = "9.9.9".into();
        assert!(verify_manifest(&replay, &pk).is_err());

        // a different key entirely
        let other = SigningKey::from_bytes(&[10u8; 32]);
        let forged = signed_release(&other, "0.3.0", &"a".repeat(64));
        assert!(verify_manifest(&forged, &pk).is_err());

        // garbage signature is an error, not a panic
        let mut junk = signed_release(&sk, "0.3.0", &"a".repeat(64));
        junk.signature = "zz".into();
        assert!(verify_manifest(&junk, &pk).is_err());
    }
}
