//! The Recovery Kit (§13d) — the key-loss decision, made explicit.
//!
//! *"At setup, the Robot generates a one-page Recovery Kit: a recovery
//! code the owner prints or stores offline. If the passphrase and the Kit
//! are both lost, the data is gone — by design, and the product says so in
//! plain words."*
//!
//! Everything in this robot is reachable from one 32-byte KEK in
//! `data/kek.key`: core.db's key derives from it, every cell DEK is
//! wrapped under it, backups seal under keys derived from it. Lose that
//! file — dead disk, deleted directory — and every encrypted byte,
//! including the off-site backups, is noise. The Kit is that key, encoded
//! for paper: grouped, checksummed against transcription errors, printed
//! to the terminal and NEVER written to disk by us — a recovery code on
//! the same disk as the key it recovers protects against nothing.
//!
//! There is no backdoor to be found here, because there is nowhere to put
//! one: `recover` reconstructs `kek.key` from the code, and nothing else
//! can.

use anyhow::{anyhow, bail};
use std::path::Path;

/// Encode 32 key bytes as 8 groups of 8 hex chars plus a 4-char checksum
/// group: `XXXXXXXX-XXXXXXXX-...-CCCC`. Hex, not a fancier alphabet: it is
/// transcribable in every locale, unambiguous in every font that
/// distinguishes 0/O (and the checksum catches it where the font does not).
pub fn encode(kek: &[u8; 32]) -> String {
    let hex = hex::encode(kek);
    let mut groups: Vec<String> = hex
        .as_bytes()
        .chunks(8)
        .map(|c| String::from_utf8_lossy(c).to_uppercase())
        .collect();
    let check = &trust::ids::sha256_hex(kek)[..4];
    groups.push(check.to_uppercase());
    groups.join("-")
}

/// Parse a recovery code back into key bytes, tolerantly: case, spaces and
/// dashes are transcription noise, not content. The checksum is not.
pub fn decode(code: &str) -> anyhow::Result<[u8; 32]> {
    let cleaned: String = code
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    if cleaned.len() != 68 {
        bail!(
            "a recovery code is 64 key characters plus a 4-character check \
             (got {} after removing separators)",
            cleaned.len()
        );
    }
    let (key_hex, check) = cleaned.split_at(64);
    let bytes = hex::decode(key_hex).map_err(|e| anyhow!("not a recovery code: {e}"))?;
    let kek: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow!("not a recovery code: wrong length"))?;
    if &trust::ids::sha256_hex(&kek)[..4] != check {
        bail!(
            "the checksum does not match -- one or more characters were \
             mistranscribed. compare the code against the printed kit \
             character by character."
        );
    }
    Ok(kek)
}

/// Print the Kit. Terminal only, deliberately: the whole point is that
/// this leaves the machine on paper.
pub fn print_kit(data_dir: &Path, robot_name: &str, slug: &str) -> anyhow::Result<()> {
    let kek_path = data_dir.join("kek.key");
    if !kek_path.exists() {
        bail!("no kek.key at {} -- has this robot booted once?", kek_path.display());
    }
    let text = std::fs::read_to_string(&kek_path)?;
    let bytes = hex::decode(text.trim()).map_err(|e| anyhow!("kek file: {e}"))?;
    let kek: [u8; 32] = bytes.try_into().map_err(|_| anyhow!("kek must be 32 bytes"))?;

    println!("==================== RECOVERY KIT ====================");
    println!();
    println!("robot: {robot_name}");
    println!("chat:  {slug}");
    println!();
    println!("recovery code -- print this page or copy the code onto");
    println!("paper, and store it away from this machine:");
    println!();
    println!("    {}", encode(&kek));
    println!();
    println!("what it is: the master key. every cell, every backup, and");
    println!("every exported package is reachable from it and ONLY from");
    println!("it. anyone holding this code and your files holds your");
    println!("data -- store it like a bearer bond, not like a password.");
    println!();
    println!("to recover on a new machine:");
    println!("  1. restore the data directory from any backup");
    println!("     (robotd backup-restore, or copy data/ wholesale)");
    println!("  2. robotd recover --code <the code above>");
    println!("  3. start the robot normally");
    println!();
    println!("if this kit and the data directory's kek.key are BOTH");
    println!("lost, the data is unrecoverable. there is no backdoor,");
    println!("no reset, and nobody you can call -- by design. the same");
    println!("property that lets deletion be real makes loss real.");
    println!("======================================================");
    Ok(())
}

/// Rebuild `kek.key` from a transcribed code.
pub fn recover(data_dir: &Path, code: &str) -> anyhow::Result<()> {
    let kek = decode(code)?;
    let kek_path = data_dir.join("kek.key");
    if kek_path.exists() {
        let existing = std::fs::read_to_string(&kek_path)?;
        if existing.trim() == hex::encode(kek) {
            println!("kek.key already present and identical -- nothing to do.");
            return Ok(());
        }
        bail!(
            "{} already exists and holds a DIFFERENT key. refusing to \
             overwrite -- if you mean to replace it, move it aside first.",
            kek_path.display()
        );
    }
    if let Some(dir) = kek_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&kek_path, hex::encode(kek))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&kek_path, std::fs::Permissions::from_mode(0o600))?;
    }
    println!("kek.key restored at {}.", kek_path.display());
    println!("start the robot normally; every cell and backup unlocks from here.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kit round-trips through the abuse paper suffers: case changes,
    /// spaces for dashes, line breaks.
    #[test]
    fn a_code_survives_transcription_noise_but_not_errors() {
        let kek = [7u8; 32];
        let code = encode(&kek);
        assert_eq!(code.split('-').count(), 9, "8 key groups + checksum");

        assert_eq!(decode(&code).unwrap(), kek);
        assert_eq!(decode(&code.to_lowercase()).unwrap(), kek);
        assert_eq!(decode(&code.replace('-', " ")).unwrap(), kek);
        assert_eq!(decode(&format!("  {}  ", code.replace('-', "\n"))).unwrap(), kek);

        // one flipped character must be CAUGHT, not silently accepted
        let mut bad: Vec<char> = code.chars().collect();
        let i = bad.iter().position(|c| *c == '0').unwrap_or(0);
        bad[i] = if bad[i] == '0' { '1' } else { '0' };
        let bad: String = bad.into_iter().collect();
        let e = decode(&bad).unwrap_err().to_string();
        assert!(e.contains("checksum") || e.contains("recovery code"), "{e}");

        assert!(decode("too-short").is_err());
    }

    /// Recover writes exactly the file the keychain reads, and refuses to
    /// clobber a different key -- overwriting a live KEK with a mistyped
    /// one would be the tool of loss, not recovery.
    #[test]
    fn recover_writes_the_keychain_file_and_never_clobbers() {
        let dir = std::env::temp_dir().join(format!("kit-{}", trust::ids::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();

        let kek = [42u8; 32];
        recover(&dir, &encode(&kek)).unwrap();
        let chain = trust::keys::KeyChain::load_or_create(&dir.join("kek.key")).unwrap();
        // the keychain accepted it; deriving works
        let _ = chain.core_db_key();

        // idempotent for the same key
        recover(&dir, &encode(&kek)).unwrap();
        // refused for a different one
        let other = [43u8; 32];
        assert!(recover(&dir, &encode(&other)).is_err());

        let _ = std::fs::remove_dir_all(dir);
    }
}
