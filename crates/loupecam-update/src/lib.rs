//! Signed self-update from GitHub Releases.
//!
//! Each release carries, per target, a raw executable named
//! `loupecam-<target-triple>[.exe]` and a detached signature `<asset>.sig`. The
//! signature is a minisign signature in the base64-wrapped form `tauri signer sign`
//! writes, made with the same key the desktop app's updater trusts. We only ever install
//! a binary whose signature verifies against [`PUBLIC_KEY`], which is compiled in, so
//! control of the GitHub account alone is not enough to push code to installs.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `tauri signer` public key (base64 of the minisign public key file).
pub const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDYxMUZENzA2Nzc1MkIxM0UKUldRK3NWSjNCdGNmWVp0NFNuQ2h1d1NjWTFaSHJOODNrd1RmWkNtRzdQaFI2bDF2SWtkRzJYM1oK";

pub const REPOSITORY: &str = "cinderblock/loupecam";

/// The target triple this binary was built for.
pub const TARGET: &str = env!("LOUPECAM_TARGET");

/// Largest binary we are willing to download.
const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Http(#[from] ureq::Error),
    #[error("unexpected release data: {0}")]
    Release(String),
    #[error("signature check failed: {0}")]
    Signature(String),
    #[error("installing update: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A release newer than the running version.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    pub tag: String,
    pub notes: String,
    pub url: String,
    pub published_at: Option<String>,
    /// Download for this platform, if the release has one.
    #[serde(skip)]
    pub asset: Option<Asset>,
    pub installable: bool,
}

#[derive(Debug, Clone)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub signature_url: String,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    published_at: Option<String>,
    draft: bool,
    prerelease: bool,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// Name of this platform's release asset.
pub fn asset_name() -> String {
    let ext = if TARGET.contains("windows") { ".exe" } else { "" };
    format!("loupecam-{TARGET}{ext}")
}

pub fn current_version() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("valid package version")
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent(concat!("loupecam/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Ask GitHub for the latest release. `None` when the running version is current.
pub fn check() -> Result<Option<Release>> {
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/latest");
    let mut resp = match agent().get(&url).header("Accept", "application/vnd.github+json").call() {
        Ok(r) => r,
        // No (non-prerelease) releases published yet.
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let gh: GhRelease = resp.body_mut().read_json()?;
    if gh.draft || gh.prerelease {
        return Ok(None);
    }
    let version = semver::Version::parse(gh.tag_name.trim_start_matches('v'))
        .map_err(|e| Error::Release(format!("tag {:?}: {e}", gh.tag_name)))?;
    if version <= current_version() {
        return Ok(None);
    }
    let name = asset_name();
    let find = |n: &str| gh.assets.iter().find(|a| a.name == n).map(|a| a.browser_download_url.clone());
    let asset = match (find(&name), find(&format!("{name}.sig"))) {
        (Some(url), Some(signature_url)) => Some(Asset { name, url, signature_url }),
        _ => None,
    };
    Ok(Some(Release {
        version: version.to_string(),
        tag: gh.tag_name,
        notes: gh.body.unwrap_or_default(),
        url: gh.html_url,
        published_at: gh.published_at,
        installable: asset.is_some(),
        asset,
    }))
}

fn download(url: &str) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    agent()
        .get(url)
        .call()?
        .into_body()
        .into_reader()
        .take(MAX_DOWNLOAD)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

/// Verify a `tauri signer sign` signature (base64-wrapped minisign) over `data`.
pub fn verify(data: &[u8], signature_b64: &str, public_key_b64: &str) -> Result<()> {
    let b64 = |s: &str| {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s.trim())
            .map_err(|e| Error::Signature(format!("base64: {e}")))?;
        String::from_utf8(bytes).map_err(|e| Error::Signature(e.to_string()))
    };
    let pk_text = b64(public_key_b64)?;
    let pk_line = pk_text.lines().nth(1).ok_or_else(|| Error::Signature("malformed public key".into()))?;
    let pk = minisign_verify::PublicKey::from_base64(pk_line).map_err(|e| Error::Signature(e.to_string()))?;
    let sig = minisign_verify::Signature::decode(&b64(signature_b64)?).map_err(|e| Error::Signature(e.to_string()))?;
    pk.verify(data, &sig, false).map_err(|e| Error::Signature(e.to_string()))
}

/// Download, verify and install `release` over the running executable. Takes effect
/// on the next start; see [`restart`].
pub fn install(release: &Release) -> Result<()> {
    let asset = release
        .asset
        .as_ref()
        .ok_or_else(|| Error::Release(format!("release {} has no build for {TARGET}", release.version)))?;
    tracing::info!(version = %release.version, asset = %asset.name, "downloading update");
    let signature = String::from_utf8(download(&asset.signature_url)?).map_err(|e| Error::Signature(e.to_string()))?;
    let binary = download(&asset.url)?;
    verify(&binary, &signature, PUBLIC_KEY)?;
    let exe = std::env::current_exe()?;
    let staged = staging_path(&exe);
    std::fs::write(&staged, &binary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    let r = self_replace::self_replace(&staged);
    let _ = std::fs::remove_file(&staged);
    r?;
    tracing::info!(version = %release.version, "update installed");
    Ok(())
}

fn staging_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".update");
    exe.with_file_name(name)
}

/// Replace this process with the (updated) executable, keeping the arguments. On Unix
/// this `exec`s in place (same PID, so service managers keep tracking it). On Windows
/// it starts a new process and exits.
pub fn restart() -> std::io::Error {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return e,
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        std::process::Command::new(exe).args(args).exec()
    }
    #[cfg(not(unix))]
    {
        match std::process::Command::new(exe).args(args).spawn() {
            Ok(_) => std::process::exit(0),
            Err(e) => e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names() {
        assert!(asset_name().starts_with("loupecam-"));
        assert!(asset_name().contains(TARGET));
    }

    /// Signature produced by `tauri signer sign` with the release key over the bytes
    /// "loupecam signature test\n" (see tests/fixtures).
    #[test]
    fn verifies_real_signature() {
        let data = include_bytes!("../tests/fixtures/signed.txt");
        let sig = include_str!("../tests/fixtures/signed.txt.sig");
        verify(data, sig, PUBLIC_KEY).unwrap();
        let mut tampered = data.to_vec();
        tampered[0] ^= 1;
        assert!(verify(&tampered, sig, PUBLIC_KEY).is_err());
    }
}
