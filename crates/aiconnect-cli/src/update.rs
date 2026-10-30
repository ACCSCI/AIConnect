//! Signed static releases; no update server, Node runtime, or trust-on-first-use.
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use clap::Subcommand;
use ed25519_dalek::{Signature, VerifyingKey};
use fs2::FileExt;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    time::Duration,
};

const MAX_BINARY: usize = 100 * 1024 * 1024;
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Subcommand)]
pub enum Action {
    /// Check the signed release without changing the executable.
    Check,
    /// Pin an independently trusted release key and static manifest URL.
    Configure {
        #[arg(long)]
        manifest: String,
        /// Base64 SPKI DER Ed25519 public key, obtained from a trusted source.
        #[arg(long)]
        public_key: String,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        automatic: bool,
    },
    /// Restore the executable saved before the most recent update.
    Rollback,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    manifest_url: String,
    public_key: String,
    #[serde(default)]
    automatic: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Artifact {
    url: String,
    sha256: String,
    size: usize,
    format: String,
    file_name: String,
    chunks: Option<Chunks>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Chunks {
    size: usize,
    sha256: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    schema_version: u64,
    version: String,
    artifacts: BTreeMap<String, Artifact>,
}

fn url(raw: &str) -> Result<Url> {
    let u = Url::parse(raw)?;
    let local_test = cfg!(test)
        && u.scheme() == "http"
        && matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    ensure!(
        (u.scheme() == "https" || local_test) && u.username().is_empty() && u.password().is_none(),
        "update URL must use HTTPS without credentials"
    );
    Ok(u)
}
fn key(raw: &str) -> Result<VerifyingKey> {
    // RFC 8410 Ed25519 SubjectPublicKeyInfo; require the exact algorithm identifier.
    let der = STANDARD.decode(raw)?;
    ensure!(
        der.len() == 44
            && der[..12]
                == [
                    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00
                ],
        "invalid Ed25519 SPKI key"
    );
    Ok(VerifyingKey::from_bytes(der[12..].try_into()?)?)
}
fn verified(raw: &[u8], trusted: &str) -> Result<Payload> {
    let envelope: Value = serde_json::from_slice(raw)?;
    ensure!(
        envelope["signature"]["algorithm"] == "ed25519",
        "signed Ed25519 release required"
    );
    let signature = Signature::from_slice(
        &STANDARD.decode(
            envelope["signature"]["value"]
                .as_str()
                .context("missing signature")?,
        )?,
    )?;
    let canonical = canonical(&envelope["payload"])?;
    key(trusted)?
        .verify_strict(&canonical, &signature)
        .context("release signature verification failed")?;
    let p: Payload = serde_json::from_value(envelope["payload"].clone())?;
    ensure!(p.schema_version == 1, "unsupported release schema");
    semver::Version::parse(&p.version)?;
    Ok(p)
}
fn canonical(v: &Value) -> Result<Vec<u8>> {
    fn string(v: &Value) -> Result<String> {
        Ok(match v {
            Value::Object(map) => {
                let ordered: BTreeMap<_, _> = map.iter().collect();
                let parts: Result<Vec<_>> = ordered
                    .into_iter()
                    .map(|(k, v)| Ok(format!("{}:{}", serde_json::to_string(k)?, string(v)?)))
                    .collect();
                format!("{{{}}}", parts?.join(","))
            }
            Value::Array(items) => {
                let parts: Result<Vec<_>> = items.iter().map(string).collect();
                format!("[{}]", parts?.join(","))
            }
            _ => serde_json::to_string(v)?,
        })
    }
    Ok(string(v)?.into_bytes())
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn validate_artifact(a: &Artifact, target: &str) -> Result<()> {
    ensure!(
        (1..=MAX_BINARY).contains(&a.size) && a.format == "binary",
        "invalid binary artifact"
    );
    ensure!(
        a.file_name
            == if target.starts_with("windows-") {
                "aiconnect.exe"
            } else {
                "aiconnect"
            },
        "unexpected executable name"
    );
    ensure!(
        a.sha256.len() == 64
            && a.sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid SHA256"
    );
    if let Some(c) = &a.chunks {
        ensure!(
            (65536..=4 * 1024 * 1024).contains(&c.size)
                && c.sha256.len() == a.size.div_ceil(c.size),
            "invalid chunk map"
        );
        ensure!(
            c.sha256.iter().all(|h| h.len() == 64
                && h.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
            "invalid chunk digest"
        );
    }
    Ok(())
}
fn target() -> Result<String> {
    let os = match std::env::consts::OS {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "macos",
        _ => bail!("unsupported OS"),
    };
    ensure!(
        matches!(std::env::consts::ARCH, "x86_64" | "aarch64"),
        "unsupported architecture"
    );
    Ok(format!("{os}-{}", std::env::consts::ARCH))
}
fn config(directory: &Path) -> Result<Config> {
    let path = directory.join("updater.json");
    if path.exists() {
        return Ok(serde_json::from_slice(&std::fs::read(path)?)?);
    }
    let receipt = std::env::current_exe()?.with_file_name("aiconnect-install.json");
    let v: Value = serde_json::from_slice(&std::fs::read(receipt).context("No update source configured. Run aiconnect update configure --manifest HTTPS_URL --public-key TRUSTED_BASE64_DER")?)?;
    ensure!(
        v["signatureVerified"] == true,
        "unsigned installation cannot configure trusted updates"
    );
    Ok(Config {
        manifest_url: v["manifestUrl"]
            .as_str()
            .context("missing manifest URL")?
            .into(),
        public_key: v["publicKey"]
            .as_str()
            .context("installation receipt has no pinned key; configure updates explicitly")?
            .into(),
        automatic: v["automaticUpdates"].as_bool().unwrap_or(false),
    })
}
fn save(directory: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(directory.join(name)).map_err(|e| e.error)?;
    Ok(())
}
fn client() -> Result<Client> {
    Ok(Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(60))
        .build()?)
}
async fn response(
    client: &Client,
    raw: &str,
    range: Option<(usize, usize)>,
) -> Result<reqwest::Response> {
    let mut current = url(raw)?;
    for _ in 0..6 {
        let mut request = client.get(current.clone());
        if let Some((start, end)) = range {
            request = request.header("Range", format!("bytes={start}-{end}"));
        }
        let r = request.send().await?;
        if r.status().is_redirection() {
            let next = current.join(
                r.headers()
                    .get("location")
                    .context("missing redirect location")?
                    .to_str()?,
            )?;
            current = url(next.as_str())?;
            continue;
        }
        return Ok(r.error_for_status()?);
    }
    bail!("too many release redirects")
}
async fn bounded(mut r: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    ensure!(
        r.content_length().unwrap_or(0) <= limit as u64,
        "release download exceeds limit"
    );
    let mut data = Vec::new();
    while let Some(chunk) = r.chunk().await? {
        ensure!(
            data.len() + chunk.len() <= limit,
            "release download exceeds limit"
        );
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}
async fn release(c: &Config, client: &Client) -> Result<Payload> {
    verified(
        &bounded(response(client, &c.manifest_url, None).await?, 1024 * 1024).await?,
        &c.public_key,
    )
}

/// Reuse matching old blocks. Range-ignorant static hosts fall back to one full response.
async fn download(
    client: &Client,
    location: &str,
    a: &Artifact,
    old: &Path,
) -> Result<(Vec<u8>, usize)> {
    let mut downloaded = 0;
    let mut bytes = Vec::with_capacity(a.size);
    if let Some(c) = &a.chunks {
        let mut local = std::fs::File::open(old).ok();
        for (i, expected) in c.sha256.iter().enumerate() {
            let start = i * c.size;
            let length = (a.size - start).min(c.size);
            let mut block = vec![0; length];
            let reused = local.as_mut().is_some_and(|f| {
                f.seek(SeekFrom::Start(start as u64)).is_ok() && f.read_exact(&mut block).is_ok()
            }) && hash(&block) == *expected;
            if !reused {
                let r = response(client, location, Some((start, start + length - 1))).await?;
                if r.status() == reqwest::StatusCode::OK {
                    let full = bounded(r, a.size).await?;
                    downloaded += full.len();
                    ensure!(
                        full.len() == a.size && hash(&full) == a.sha256,
                        "full artifact integrity verification failed"
                    );
                    return Ok((full, downloaded));
                }
                ensure!(
                    r.status() == reqwest::StatusCode::PARTIAL_CONTENT,
                    "invalid range response"
                );
                let expected_range = format!("bytes {start}-{}/{}", start + length - 1, a.size);
                ensure!(
                    r.headers()
                        .get("content-range")
                        .and_then(|v| v.to_str().ok())
                        == Some(expected_range.as_str()),
                    "invalid Content-Range"
                );
                block = bounded(r, length).await?;
                downloaded += block.len();
                ensure!(
                    block.len() == length && hash(&block) == *expected,
                    "chunk integrity verification failed"
                );
            }
            bytes.extend_from_slice(&block);
        }
    } else {
        bytes = bounded(response(client, location, None).await?, a.size).await?;
        downloaded = bytes.len();
    }
    ensure!(
        bytes.len() == a.size && hash(&bytes) == a.sha256,
        "artifact integrity verification failed"
    );
    Ok((bytes, downloaded))
}

fn lock(exe: &Path) -> Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(exe.with_extension("update.lock"))?;
    file.try_lock_exclusive()
        .context("another update is in progress")?;
    Ok(file)
}
async fn smoke(file: &Path, expected: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut command = tokio::process::Command::new(file);
    command.arg("--version").kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .context("new executable version check timed out")??;
    ensure!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim() == format!("aiconnect {expected}"),
        "new executable version check failed"
    );
    Ok(())
}
async fn apply(directory: &Path, c: &Config, check: bool) -> Result<Value> {
    url(&c.manifest_url)?;
    key(&c.public_key)?;
    let client = client()?;
    let p = release(c, &client).await?;
    let available = semver::Version::parse(&p.version)? > semver::Version::parse(VERSION)?;
    let target = target()?;
    let a = p
        .artifacts
        .get(&target)
        .context("release has no artifact for this platform")?;
    validate_artifact(a, &target)?;
    if check || !available {
        return Ok(
            json!({"currentVersion":VERSION,"latestVersion":p.version,"updateAvailable":available,"signatureVerified":true,"automatic":c.automatic,"installed":false}),
        );
    }
    let exe = std::env::current_exe()?;
    let _lock = lock(&exe)?;
    let location = url(&c.manifest_url)?.join(&a.url)?;
    let (bytes, downloaded) = download(&client, location.as_str(), a, &exe).await?;
    let staged = tempfile::Builder::new()
        .prefix("aiconnect-update-")
        .tempdir_in(exe.parent().context("executable has no parent")?)?;
    let candidate = staged.path().join(&a.file_name);
    std::fs::write(&candidate, &bytes)?;
    smoke(&candidate, &p.version).await?;
    let old = std::fs::read(&exe)?;
    // Persist rollback metadata before touching the installed file.
    save(
        exe.parent().unwrap(),
        "aiconnect-rollback.json",
        &serde_json::to_vec(&json!({"version":VERSION,"sha256":hash(&old)}))?,
    )?;
    save(exe.parent().unwrap(), "aiconnect-rollback.bin", &old)?;
    self_replace::self_replace(&candidate)?;
    let report = json!({"installed":true,"version":p.version,"downloadedBytes":downloaded,"reusedBytes":a.size.saturating_sub(downloaded),"artifactBytes":a.size,"signatureVerified":true,"restartRequired":true,"dataPreserved":true});
    // A receipt failure cannot undo a successful replacement or misreport it as failed.
    let _ = save(directory, "last-update.json", &serde_json::to_vec(&report)?);
    Ok(report)
}
pub async fn run(directory: &Path, action: Option<Action>) -> Result<Value> {
    match action {
        Some(Action::Configure {
            manifest,
            public_key,
            automatic,
        }) => {
            url(&manifest)?;
            key(&public_key)?;
            let c = Config {
                manifest_url: manifest,
                public_key,
                automatic,
            };
            save(directory, "updater.json", &serde_json::to_vec_pretty(&c)?)?;
            Ok(json!({"configured":true,"automatic":automatic,"checkOnDaemonStart":true}))
        }
        Some(Action::Rollback) => {
            let exe = std::env::current_exe()?;
            let _lock = lock(&exe)?;
            let meta: Value = serde_json::from_slice(&std::fs::read(
                exe.with_file_name("aiconnect-rollback.json"),
            )?)?;
            let bytes = std::fs::read(exe.with_file_name("aiconnect-rollback.bin"))?;
            ensure!(
                Some(hash(&bytes).as_str()) == meta["sha256"].as_str(),
                "rollback backup is corrupted"
            );
            let staged = tempfile::tempdir_in(exe.parent().context("executable has no parent")?)?;
            let file = staged.path().join(if cfg!(windows) {
                "aiconnect.exe"
            } else {
                "aiconnect"
            });
            std::fs::write(&file, bytes)?;
            smoke(
                &file,
                meta["version"]
                    .as_str()
                    .context("missing rollback version")?,
            )
            .await?;
            self_replace::self_replace(file)?;
            Ok(
                json!({"rolledBack":true,"version":meta["version"],"restartRequired":true,"dataPreserved":true}),
            )
        }
        other => {
            apply(
                directory,
                &config(directory)?,
                matches!(other, Some(Action::Check)),
            )
            .await
        }
    }
}
/// Check on every daemon launch. Network failure cannot prevent offline messaging.
/// Automatic replacement leaves the loaded daemon running; the next launch uses the new binary.
pub async fn startup(directory: &Path) -> bool {
    let Ok(c) = config(directory) else {
        return false;
    };
    // Timeout only a read-only check. Never cancel a replacement halfway through.
    match tokio::time::timeout(Duration::from_secs(5), apply(directory, &c, true)).await {
        Ok(Ok(report)) => {
            let _ = save(
                directory,
                "last-update-check.json",
                &serde_json::to_vec(&report).unwrap_or_default(),
            );
            if report["updateAvailable"] == true {
                eprintln!("{}", json!({"update":report,"command":"aiconnect update"}));
                if c.automatic {
                    match apply(directory, &c, false).await {
                        Ok(r) => return r["installed"] == true,
                        Err(e) => eprintln!("{}", json!({"updateError":e.to_string()})),
                    }
                }
            }
        }
        result => {
            let _ = save(
                directory,
                "last-update-check.json",
                &serde_json::to_vec(&json!({"checked":false,"reason":format!("{result:?}")}))
                    .unwrap_or_default(),
            );
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    fn signed() -> (Vec<u8>, String) {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let mut der = vec![
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
        ];
        der.extend(signing.verifying_key().as_bytes());
        let payload = json!({"schemaVersion":1,"version":"0.4.0","artifacts":{}});
        let sig = signing.sign(&serde_json::to_vec(&payload).unwrap());
        (serde_json::to_vec(&json!({"payload":payload,"signature":{"algorithm":"ed25519","value":STANDARD.encode(sig.to_bytes())}})).unwrap(),STANDARD.encode(der))
    }
    #[test]
    fn rejects_tampered_manifest_and_wrong_key() {
        let (raw, key) = signed();
        assert_eq!(verified(&raw, &key).unwrap().version, "0.4.0");
        let mut tampered: Value = serde_json::from_slice(&raw).unwrap();
        tampered["payload"]["version"] = json!("9.9.9");
        assert!(verified(&serde_json::to_vec(&tampered).unwrap(), &key).is_err());
        let mut wrong = STANDARD.decode(&key).unwrap();
        wrong[43] ^= 1;
        assert!(verified(&raw, &STANDARD.encode(wrong)).is_err());
    }
    #[tokio::test]
    async fn identical_chunks_require_no_network() {
        let data = vec![42; 131072];
        let temp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(temp.path(), &data).unwrap();
        let a = Artifact {
            url: "https://invalid.example/no-network".into(),
            sha256: hash(&data),
            size: data.len(),
            format: "binary".into(),
            file_name: "aiconnect".into(),
            chunks: Some(Chunks {
                size: 65536,
                sha256: vec![hash(&data[..65536]), hash(&data[65536..])],
            }),
        };
        validate_artifact(&a, "linux-x86_64").unwrap();
        let (bytes, count) = download(&client().unwrap(), &a.url, &a, temp.path())
            .await
            .unwrap();
        assert_eq!(bytes, data);
        assert_eq!(count, 0);
    }
    #[test]
    fn rejects_untrusted_transport_and_malformed_chunks() {
        assert!(url("http://example.com/x").is_err());
        assert!(url("https://user:pass@example.com/x").is_err());
        let mut a = Artifact {
            url: "https://example.com/x".into(),
            sha256: "0".repeat(64),
            size: 100,
            format: "binary".into(),
            file_name: "aiconnect".into(),
            chunks: Some(Chunks {
                size: 1,
                sha256: vec![],
            }),
        };
        assert!(validate_artifact(&a, "linux-x86_64").is_err());
        a.chunks = None;
        assert!(validate_artifact(&a, "linux-x86_64").is_ok());
    }
}
