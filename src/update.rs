//! Self-update from this project's GitHub Releases.
//!
//! - The only network traffic the app ever makes. It can be turned off in the settings.
//! - Every URL is built here from the fixed repository name; nothing in a download can redirect
//!   the app to another project.
//! - The new .exe must carry a minisign signature from the release key embedded below, with a
//!   signed comment naming the exact version, so an old or foreign build can't be swapped in.

use crate::{http, info, warn};
use std::path::{Path, PathBuf};

pub const REPO: &str = "MehdiMamas/discord-cursor-stream-switcher";
pub const ASSET: &str = "cursor-stream-switcher.exe";
/// minisign public key of the release signing key (the secret half lives in GitHub Actions).
pub const PUBLIC_KEY: &str = include_str!("../release-key.pub");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('v');
        let mut parts = s.split('.');
        let mut next = || -> Option<u32> {
            let p = parts.next()?;
            if p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            p.parse().ok()
        };
        let v = Self(next()?, next()?, next()?);
        parts.next().is_none().then_some(v)
    }

    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("crate version is x.y.z")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(serde::Deserialize)]
struct Manifest {
    version: String,
}

pub fn parse_manifest(text: &str) -> Result<Version, String> {
    let m: Manifest = toml::from_str(text).map_err(|e| format!("bad latest.toml: {e}"))?;
    Version::parse(&m.version).ok_or_else(|| format!("bad version {:?} in latest.toml", m.version))
}

pub fn trusted_comment(v: Version) -> String {
    format!("cursor-stream-switcher {v}")
}

/// Checks the signature and that it was made for exactly this version.
pub fn verify(public_key: &str, data: &[u8], signature: &str, v: Version) -> Result<(), String> {
    let pk = minisign_verify::PublicKey::decode(public_key.trim())
        .or_else(|_| minisign_verify::PublicKey::from_base64(public_key.trim()))
        .map_err(|e| format!("bad public key: {e}"))?;
    let sig =
        minisign_verify::Signature::decode(signature).map_err(|e| format!("bad signature file: {e}"))?;
    pk.verify(data, &sig, false).map_err(|_| "signature does not match".to_owned())?;
    if sig.trusted_comment() != trusted_comment(v) {
        return Err(format!(
            "signature is for {:?}, expected {:?}",
            sig.trusted_comment(),
            trusted_comment(v)
        ));
    }
    Ok(())
}

/// Returns the newer version on GitHub, if any.
pub fn check() -> Result<Option<Version>, String> {
    let body = http::get("github.com", &format!("/{REPO}/releases/latest/download/latest.toml"), 64 * 1024)?;
    let latest = parse_manifest(&String::from_utf8_lossy(&body))?;
    info!("update check: latest {latest}, running {}", Version::current());
    Ok((latest > Version::current()).then_some(latest))
}

/// Downloads and verifies version `v`, swaps it in for the running .exe and starts it.
/// On success the caller must exit right away.
pub fn install(v: Version) -> Result<(), String> {
    let base = format!("/{REPO}/releases/download/v{v}/{ASSET}");
    let exe = http::get("github.com", &base, 64 * 1024 * 1024)?;
    let sig = http::get("github.com", &format!("{base}.minisig"), 16 * 1024)?;
    verify(PUBLIC_KEY, &exe, &String::from_utf8_lossy(&sig), v)?;
    info!("downloaded and verified v{v} ({} bytes)", exe.len());

    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    replace_exe(&current, &exe)?;
    std::process::Command::new(&current)
        .arg("--after-update")
        .spawn()
        .map_err(|e| format!("updated, but could not restart: {e}"))?;
    Ok(())
}

fn old_path(exe: &Path) -> PathBuf {
    exe.with_extension("exe.old")
}

/// Windows lets a running .exe be renamed but not overwritten, so: write new, move current
/// aside, move new into place. The old file is deleted on the next start.
fn replace_exe(current: &Path, bytes: &[u8]) -> Result<(), String> {
    let new = current.with_extension("exe.new");
    let old = old_path(current);
    let _ = std::fs::remove_file(&old);
    std::fs::write(&new, bytes).map_err(|e| format!("can't write {}: {e}", new.display()))?;
    if let Err(e) = std::fs::rename(current, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(format!(
            "can't replace {} ({e}). Move the app to a folder you can write to.",
            current.display()
        ));
    }
    if let Err(e) = std::fs::rename(&new, current) {
        let _ = std::fs::rename(&old, current);
        return Err(format!("can't move the new version into place: {e}"));
    }
    Ok(())
}

/// Removes the previous version left behind by an update.
pub fn cleanup_old() {
    if let Ok(exe) = std::env::current_exe() {
        let old = old_path(&exe);
        if old.exists() {
            match std::fs::remove_file(&old) {
                Ok(()) => info!("removed previous version {}", old.display()),
                Err(e) => warn!("could not remove {}: {e}", old.display()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(Version::parse("v1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse(" 0.10.0 "), Some(Version(0, 10, 0)));
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        for bad in ["1.2", "1.2.3.4", "1.2.x", "", "1..3", "-1.0.0", "1.2.3-beta", "+1.2.3"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn manifest_needs_a_clean_version() {
        assert_eq!(parse_manifest("version = \"0.2.0\"\n"), Ok(Version(0, 2, 0)));
        assert!(parse_manifest("version = \"latest\"\n").is_err());
        assert!(parse_manifest("garbage").is_err());
    }

    #[test]
    fn embedded_public_key_is_valid() {
        let key = PUBLIC_KEY.trim();
        assert!(
            minisign_verify::PublicKey::decode(key).is_ok()
                || minisign_verify::PublicKey::from_base64(key).is_ok()
        );
    }

    fn sign(kp: &minisign::KeyPair, data: &[u8], comment: &str) -> String {
        minisign::sign(Some(&kp.pk), &kp.sk, std::io::Cursor::new(data), Some(comment), None)
            .unwrap()
            .into_string()
    }

    #[test]
    fn signature_checks_key_data_and_version() {
        let kp = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pk = kp.pk.to_base64();
        let v = Version(1, 2, 3);
        let data = b"new exe bytes";
        let good = sign(&kp, data, &trusted_comment(v));

        assert_eq!(verify(&pk, data, &good, v), Ok(()));
        // Tampered download.
        assert!(verify(&pk, b"evil exe bytes", &good, v).is_err());
        // Valid signature of an older release replayed as a newer one.
        assert!(verify(&pk, data, &good, Version(1, 2, 4)).is_err());
        // Signed by someone else.
        let other = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        assert!(verify(&pk, data, &sign(&other, data, &trusted_comment(v)), v).is_err());
        assert!(verify(&pk, data, "not a signature", v).is_err());
    }

    #[test]
    fn replace_exe_swaps_files() {
        let dir = std::env::temp_dir().join(format!("css-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("app.exe");
        std::fs::write(&exe, b"old").unwrap();
        replace_exe(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(old_path(&exe)).unwrap(), b"old");
        assert!(!exe.with_extension("exe.new").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
