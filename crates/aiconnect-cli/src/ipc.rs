use aiconnect_core::{Config, Core};
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::Notify,
};
use uuid::Uuid;

const MAX_PACKET: usize = 1024 * 1024;
#[derive(Serialize, Deserialize, Clone)]
struct DaemonInfo {
    address: SocketAddr,
    token: String,
    pid: u32,
    instance: String,
}

/// Metadata contains a bearer token. Never write it to stdout or logs.
pub fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    // Rename replaces the old regular file atomically; never leave a remove/rename gap.
    std::fs::rename(&temporary, path)?;
    Ok(())
}

pub async fn call(directory: &Path, request: Value) -> Result<Value> {
    let bytes = std::fs::read(directory.join("daemon.json"))
        .context("daemon_not_running: run aiconnect setup")?;
    let info: DaemonInfo = serde_json::from_slice(&bytes).context("invalid_daemon_metadata")?;
    if !info.address.ip().is_loopback() {
        bail!("unsafe_daemon_address")
    }
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut stream = TcpStream::connect(info.address)
            .await
            .context("daemon_unreachable: run daemon start")?;
        let mut payload = serde_json::to_vec(&json!({"token":info.token,"request":request}))?;
        if payload.len() > MAX_PACKET {
            bail!("request_too_large")
        }
        payload.push(b'\n');
        stream.write_all(&payload).await?;
        let mut bytes = Vec::new();
        BufReader::new(stream.take((MAX_PACKET + 1) as u64))
            .read_until(b'\n', &mut bytes)
            .await?;
        if bytes.len() > MAX_PACKET {
            bail!("response_too_large")
        }
        let response: Value = serde_json::from_slice(&bytes).context("invalid_daemon_response")?;
        if response["ok"] != true {
            bail!(
                "{}",
                response["error"]
                    .as_str()
                    .unwrap_or("daemon_request_failed")
            )
        }
        Ok(response["result"].clone())
    })
    .await
    .context("daemon_request_timeout")?
}

pub async fn start(
    directory: &Path,
    direct_only: bool,
    relay_only: bool,
    relay: Option<String>,
) -> Result<Value> {
    if let Ok(status) = call(directory, json!({"action":"status"})).await {
        return Ok(status);
    }
    std::fs::create_dir_all(directory)?;
    let mut arguments = vec![
        std::ffi::OsString::from("--data"),
        directory.as_os_str().to_owned(),
        "daemon".into(),
        "run".into(),
        "--background".into(),
    ];
    if direct_only {
        arguments.push("--direct-only".into());
    }
    if !direct_only && relay.is_none() {
        // The CLI has already required an explicit development-only opt-in.
        arguments.push("--dev-public-relay".into());
    }
    if relay_only {
        arguments.push("--relay-only".into());
    }
    if let Some(relay) = relay {
        arguments.extend(["--relay".into(), relay.into()]);
    }
    #[cfg(windows)]
    let mut child = crate::windows_process::spawn(&std::env::current_exe()?, &arguments)
        .context("daemon_spawn_failed")?;
    #[cfg(unix)]
    let mut child = {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join("daemon.log"))?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(&arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        command.process_group(0);
        command.spawn().context("daemon_spawn_failed")?
    };
    for _ in 0..150 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if let Ok(status) = call(directory, json!({"action":"status"})).await {
            return Ok(status);
        }
        if let Some(status) = child.try_wait()? {
            bail!(
                "daemon_start_failed ({status}): inspect {}",
                directory.join("daemon.log").display()
            )
        }
    }
    bail!(
        "daemon_start_timeout: process remains running; inspect {}",
        directory.join("daemon.log").display()
    )
}

pub async fn stop(directory: &Path) -> Result<Value> {
    let result = call(directory, json!({"action":"daemon.stop"})).await?;
    for _ in 0..100 {
        if !directory.join("daemon.json").exists() {
            return Ok(result);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("daemon_shutdown_timeout")
}

pub async fn run(
    directory: PathBuf,
    direct_only: bool,
    relay_only: bool,
    relay_url: Option<String>,
    background: bool,
) -> Result<()> {
    std::fs::create_dir_all(&directory)?;
    #[cfg(windows)]
    let _own_log = if background {
        Some(crate::windows_process::redirect_output(
            &directory.join("daemon.log"),
        )?)
    } else {
        None
    };
    #[cfg(not(windows))]
    let _ = background;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("daemon.lock"))?;
    lock.try_lock_exclusive()
        .context("daemon_already_running_for_data_directory")?;
    let core = Core::open(Config {
        directory: directory.clone(),
        direct_only,
        relay_only,
        relay_url,
    })
    .await?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let token = rand::random::<[u8; 32]>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let info = DaemonInfo {
        address: listener.local_addr()?,
        token,
        pid: std::process::id(),
        instance: Uuid::new_v4().to_string(),
    };
    private_write(&directory.join("daemon.json"), &serde_json::to_vec(&info)?)?;
    let shutdown = Arc::new(Notify::new());
    let permits = Arc::new(tokio::sync::Semaphore::new(32));
    let runtime = tokio::spawn(crate::runtime::run(core.clone(), directory.clone()));
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _=shutdown.notified()=>break,
            _=tokio::signal::ctrl_c()=>break,
            Some(_)=tasks.join_next(),if !tasks.is_empty()=>{},
            accepted=listener.accept()=>{
                let (stream,peer)=accepted?;if !peer.ip().is_loopback(){continue}
                let Ok(permit)=permits.clone().try_acquire_owned() else{continue};
                let core=core.clone();let token=info.token.clone();let shutdown=shutdown.clone();
                tasks.spawn(async move {let _permit=permit;let _=tokio::time::timeout(Duration::from_secs(35),handle(stream,core,token,shutdown)).await;});
            }
        }
    }
    runtime.abort();
    let _ = runtime.await;
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    core.close().await;
    let metadata = directory.join("daemon.json");
    if let Ok(bytes) = std::fs::read(&metadata) {
        if let Ok(current) = serde_json::from_slice::<DaemonInfo>(&bytes) {
            if current.instance == info.instance {
                let _ = std::fs::remove_file(metadata);
            }
        }
    }
    lock.unlock()?;
    Ok(())
}

fn token_equal(a: &str, b: &str) -> bool {
    let mut difference = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        difference |= (a.as_bytes().get(i).copied().unwrap_or(0)
            ^ b.as_bytes().get(i).copied().unwrap_or(0)) as usize;
    }
    difference == 0
}

async fn handle(stream: TcpStream, core: Core, token: String, shutdown: Arc<Notify>) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut bytes = Vec::new();
    BufReader::new(read.take((MAX_PACKET + 1) as u64))
        .read_until(b'\n', &mut bytes)
        .await?;
    let mut stop = false;
    let result: Result<Value> = async {
        if bytes.len() > MAX_PACKET {
            bail!("request_too_large")
        }
        let envelope: Value = serde_json::from_slice(&bytes).context("invalid_json")?;
        if !token_equal(envelope["token"].as_str().unwrap_or(""), &token) {
            bail!("unauthorized")
        }
        let request = envelope
            .get("request")
            .cloned()
            .context("missing_request")?;
        if request["action"] == "daemon.stop" {
            stop = true;
            return Ok(json!({"stopping":true}));
        }
        core.dispatch(request).await
    }
    .await;
    let response = match result {
        Ok(result) => json!({"ok":true,"result":result}),
        Err(error) => json!({"ok":false,"error":error.to_string()}),
    };
    let mut payload = serde_json::to_vec(&response)?;
    if payload.len() > MAX_PACKET {
        payload = serde_json::to_vec(&json!({"ok":false,"error":"response_too_large"}))?;
    }
    payload.push(b'\n');
    write.write_all(&payload).await?;
    write.shutdown().await?;
    if stop {
        shutdown.notify_one();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tokens_require_exact_equality() {
        assert!(token_equal("abcd", "abcd"));
        assert!(!token_equal("abcd", "abc"));
        assert!(!token_equal("abcd", "abce"));
        assert!(!token_equal("", "abcd"));
    }

    #[test]
    fn private_write_replaces_existing_file_without_delete_gap() {
        let directory = std::env::temp_dir().join(format!("aiconnect-atomic-{}", Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("claims.json");
        private_write(&path, b"old claims").unwrap();
        private_write(&path, b"new claims").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new claims");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
