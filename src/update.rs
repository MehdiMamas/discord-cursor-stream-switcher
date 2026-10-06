//! Self-update from this project's GitHub Releases.
//!
//! - The only network traffic the app ever makes. It can be turned off in the settings.
//! - Every URL is built here from the fixed repository name; nothing in a download can redirect
//!   the app to another project.
//! - The app lives in Program Files, so it can't replace itself: it downloads the release
//!   installer and runs it, and the installer (after a UAC prompt) does the upgrade.
//! - The installer must carry a minisign signature from the release key embedded below, with a
//!   signed comment naming the exact version, so an old or foreign build can't be swapped in.

use crate::{http, info, paths, system};
use std::path::{Path, PathBuf};

pub const REPO: &str = "MehdiMamas/discord-cursor-stream-switcher";
/// The installer published with each release.
pub const ASSET: &str = "cursor-stream-switcher-setup.exe";
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

/// Arguments for an unattended upgrade. `/relaunch=1` tells the installer to start the new
/// version when it's done (see installer/cursor-stream-switcher.iss).
fn installer_args(log: &Path) -> String {
    format!("/SILENT /SUPPRESSMSGBOXES /NORESTART /SP- /relaunch=1 \"/LOG={}\"", log.display())
}

/// Where downloaded installers wait to run. Emptied on the next start.
fn download_dir() -> PathBuf {
    paths::data_dir().join("update")
}

/// Downloads and verifies the installer for version `v` and runs it. The installer asks for
/// admin rights, closes this app, replaces it in Program Files and starts the new version, so
/// on success this process is usually gone before the function returns.
pub fn install(v: Version) -> Result<(), String> {
    let base = format!("/{REPO}/releases/download/v{v}/{ASSET}");
    let setup = http::get("github.com", &base, 64 * 1024 * 1024)?;
    let sig = http::get("github.com", &format!("{base}.minisig"), 16 * 1024)?;
    verify(PUBLIC_KEY, &setup, &String::from_utf8_lossy(&sig), v)?;
    info!("downloaded and verified v{v} ({} bytes)", setup.len());

    let dir = download_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    let path = dir.join(ASSET);
    let log = dir.join("setup.log");
    let _held = write_and_hold(&path, &setup)?;
    let code = system::run_and_wait(&path, &installer_args(&log))?;
    info!("installer exited with code {code}");
    match code {
        0 => Ok(()),
        // Inno Setup: 2 = cancelled before installing, 5 = cancelled while installing.
        2 | 5 => Err("cancelled".into()),
        _ => Err(format!("The installer stopped with code {code}. Details: {}", log.display())),
    }
}

/// Writes `bytes` to `path`, reopens it with read-only sharing and checks it holds exactly
/// `bytes`. While the returned handle lives, nothing can change the verified installer before
/// Windows runs it.
fn write_and_hold(path: &Path, bytes: &[u8]) -> Result<std::fs::File, String> {
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;

    let _ = std::fs::remove_file(path);
    std::fs::write(path, bytes).map_err(|e| format!("can't write {}: {e}", path.display()))?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(|e| format!("can't open {}: {e}", path.display()))?;
    let mut on_disk = Vec::with_capacity(bytes.len());
    file.read_to_end(&mut on_disk).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    if on_disk != bytes {
        return Err(format!("{} changed after it was written", path.display()));
    }
    Ok(file)
}

/// Removes installers left by an earlier update. One that is still running (it starts the new
/// version just before it exits) stays until the next start.
pub fn cleanup() {
    let dir = download_dir();
    if dir.exists() {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => info!("removed {}", dir.display()),
            Err(e) => info!("could not remove {} yet: {e}", dir.display()),
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
    fn held_installer_cannot_be_swapped() {
        let dir = std::env::temp_dir().join(format!("css-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(ASSET);
        std::fs::write(&path, b"stale").unwrap();

        let held = write_and_hold(&path, b"verified installer").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"verified installer");
        assert!(std::fs::write(&path, b"evil").is_err());
        assert!(std::fs::rename(&path, dir.join("moved.exe")).is_err());
        assert!(std::fs::remove_file(&path).is_err());

        drop(held);
        std::fs::write(&path, b"free again").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installer_runs_silently_and_relaunches() {
        let args = installer_args(Path::new(r"C:\Users\A B\setup.log"));
        for flag in ["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-", "/relaunch=1"] {
            assert!(args.split(' ').any(|a| a == flag), "{flag} missing from {args}");
        }
        assert!(args.ends_with(r#""/LOG=C:\Users\A B\setup.log""#), "{args}");
    }
}
