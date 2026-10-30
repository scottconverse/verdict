//! The ledger seal: a keyed hash of the newest record, made with a secret key kept outside the
//! project.
//!
//! The hash chain alone catches a changed record, but not a full rewrite: anyone who recomputes
//! every hash can forge a valid chain, including fake human decisions. The seal closes that gap.
//! Every write through [`crate::Ledger::append`] stores `blake3::keyed_hash(key, seq, hash)` of
//! the new head in the `seal` table, in the same transaction as the record. [`crate::Ledger::verify`]
//! recomputes it. A rewritten, unsealed or extended-by-hand ledger then reads BROKEN.
//!
//! The key is 32 random bytes at `~/.verdict/seal.key`, made on the first write. It is never in
//! the project folder, so a program that can only write the project cannot make a valid seal.
//!
//! Limits, stated plainly: a program running as the same user can read the key and forge a seal.
//! A ledger copied to another user or machine reads BROKEN there, because that key differs. A
//! ledger from before sealing is sealed by its next write, which also accepts what is in it then.

use std::io::Write;
use std::path::{Path, PathBuf};

/// The seal table. One row: the sealed head and its keyed hash.
pub(crate) const SQL_SEAL_TABLE: &str = "CREATE TABLE IF NOT EXISTS seal (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    seq INTEGER NOT NULL,
    hash TEXT NOT NULL,
    mac TEXT NOT NULL
)";

/// The key file under a home folder.
pub fn key_path(home: &Path) -> PathBuf {
    home.join(".verdict").join("seal.key")
}

/// Read the key. `Ok(None)` when there is no key file yet.
pub(crate) fn read_key(home: &Path) -> Result<Option<[u8; 32]>, String> {
    let path = key_path(home);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read the seal key {}: {e}", path.display())),
    };
    parse_key(text.trim())
        .map(Some)
        .ok_or_else(|| format!("the seal key {} is damaged", path.display()))
}

/// Read the key, or make one if there is none. Two processes that race both end up with the file
/// that was written first.
pub(crate) fn key_or_create(home: &Path) -> Result<[u8; 32], String> {
    if let Some(k) = read_key(home)? {
        return Ok(k);
    }
    let path = key_path(home);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot make the folder {}: {e}", dir.display()))?;
    }
    let key = new_key();
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    // Write the whole key to a private temp file, then link it into place. The link fails when a
    // key already exists, and a reader never sees a half-written key file.
    let tmp = path.with_file_name(format!(
        "seal.key.{}.{}.tmp",
        std::process::id(),
        &hex[..16]
    ));
    let linked =
        std::fs::write(&tmp, format!("{hex}\n")).and_then(|()| std::fs::hard_link(&tmp, &path));
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(key),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            read_key(home)?.ok_or_else(|| format!("the seal key {} vanished", path.display()))
        }
        Err(e) => Err(format!("cannot write the seal key {}: {e}", path.display())),
    }
}

/// The list of ledgers this user has sealed, one database path per line, next to the key. It
/// tells a ledger whose seal was dropped apart from a ledger made before sealing.
fn sealed_list(home: &Path) -> PathBuf {
    home.join(".verdict").join("sealed-ledgers")
}

fn ledger_id(ledger: &Path) -> String {
    std::fs::canonicalize(ledger)
        .unwrap_or_else(|_| ledger.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// True when this ledger was sealed before on this machine.
pub(crate) fn was_sealed(home: &Path, ledger: &Path) -> bool {
    let id = ledger_id(ledger);
    std::fs::read_to_string(sealed_list(home)).is_ok_and(|t| t.lines().any(|l| l == id))
}

/// Add this ledger to the list. Best effort: a missing line only weakens the dropped-seal check.
pub(crate) fn remember_sealed(home: &Path, ledger: &Path) {
    if was_sealed(home, ledger) {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(sealed_list(home))
    {
        let _ = f.write_all(format!("{}\n", ledger_id(ledger)).as_bytes());
    }
}

/// The keyed hash for a head.
pub(crate) fn mac(key: &[u8; 32], seq: u64, hash: &str) -> String {
    blake3::keyed_hash(
        key,
        format!("verdict-ledger-seal-v1 {seq} {hash}").as_bytes(),
    )
    .to_hex()
    .to_string()
}

fn parse_key(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, b) in key.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(key)
}

/// 32 bytes from the OS-seeded hasher keys, the clock and the process id.
fn new_key() -> [u8; 32] {
    use std::hash::{BuildHasher, Hasher};
    let mut h = blake3::Hasher::new();
    for i in 0..8u64 {
        let mut s = std::collections::hash_map::RandomState::new().build_hasher();
        s.write_u64(i);
        h.update(&s.finish().to_le_bytes());
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    h.update(&nanos.to_le_bytes());
    h.update(&u64::from(std::process::id()).to_le_bytes());
    *h.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_made_once_and_read_back() {
        let home = std::env::temp_dir().join(format!("vdt-seal-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(read_key(&home).unwrap(), None);
        let a = key_or_create(&home).unwrap();
        let b = key_or_create(&home).unwrap();
        assert_eq!(a, b);
        assert_eq!(read_key(&home).unwrap(), Some(a));
        assert_ne!(mac(&a, 1, "x"), mac(&a, 2, "x"));
        assert_ne!(mac(&a, 1, "x"), mac(&[0u8; 32], 1, "x"));
        std::fs::write(key_path(&home), "not a key\n").unwrap();
        assert!(read_key(&home).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn many_writers_making_the_key_at_once_all_get_the_same_key() {
        for round in 0..20 {
            let home =
                std::env::temp_dir().join(format!("vdt-seal-race-{}-{round}", std::process::id()));
            let _ = std::fs::remove_dir_all(&home);
            let keys: Vec<[u8; 32]> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..8)
                    .map(|_| s.spawn(|| key_or_create(&home).unwrap()))
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            assert!(keys.iter().all(|k| *k == keys[0]), "round {round}");
            let _ = std::fs::remove_dir_all(&home);
        }
    }
}
