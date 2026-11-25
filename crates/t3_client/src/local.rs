//! Finding, starting and signing in to the T3 Code server on this machine.
//!
//! The server owns its data directory ("T3 home", `~/.t3` by default) and writes
//! `userdata/server-runtime.json` while it runs. Ade signs in with a bearer token from
//! `t3 auth session issue`, and trades it for a short-lived ticket for each socket.

use anyhow::{Context as _, Result};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::{Child, Command};
use url::Url;

const SERVER_START_TIMEOUT: Duration = Duration::from_secs(90);
const TOKEN_LABEL: &str = "Ade";

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeState {
    pub pid: u32,
    pub origin: String,
    #[serde(default)]
    pub server_version: String,
}

#[derive(Clone, Debug)]
pub struct LocalServer {
    pub home: PathBuf,
    pub executable: Option<PathBuf>,
}

impl LocalServer {
    /// The server for `T3CODE_HOME`, or `~/.t3`, run by the `t3` command on `PATH`.
    pub fn discover() -> Self {
        Self {
            home: default_home(),
            executable: find_executable(),
        }
    }

    fn executable(&self) -> Result<&Path> {
        self.executable.as_deref().ok_or_else(|| {
            anyhow::anyhow!("the t3 command was not found; install T3 Code from https://t3.codes")
        })
    }

    pub fn runtime_state_path(&self) -> PathBuf {
        self.home.join("userdata").join("server-runtime.json")
    }

    /// The running server, if its runtime file names a live process.
    pub async fn running(&self) -> Result<Option<RuntimeState>> {
        let contents = match tokio::fs::read_to_string(self.runtime_state_path()).await {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("reading the T3 Code runtime file"),
        };
        let state: RuntimeState =
            serde_json::from_str(&contents).context("parsing the T3 Code runtime file")?;
        Ok(process_is_alive(state.pid).then_some(state))
    }

    /// Starts `t3 serve` for this home. The server stops when the returned child is dropped.
    pub async fn start(&self) -> Result<(Child, RuntimeState)> {
        let mut command = Command::new(self.executable()?);
        command
            .arg("serve")
            .arg("--base-dir")
            .arg(&self.home)
            .arg("--no-browser")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().context("starting the T3 Code server")?;
        let deadline = tokio::time::Instant::now() + SERVER_START_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait()? {
                anyhow::bail!("the T3 Code server exited during startup ({status})");
            }
            if let Some(state) = self.running().await?
                && child.id() == Some(state.pid)
            {
                return Ok((child, state));
            }
            if tokio::time::Instant::now() > deadline {
                anyhow::bail!("the T3 Code server did not start within {SERVER_START_TIMEOUT:?}");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Issues a bearer token through the local auth control plane.
    pub async fn issue_token(&self) -> Result<String> {
        let output = Command::new(self.executable()?)
            .args([
                "auth",
                "session",
                "issue",
                "--token-only",
                "--label",
                TOKEN_LABEL,
            ])
            .arg("--base-dir")
            .arg(&self.home)
            .stdin(Stdio::null())
            .output()
            .await
            .context("running t3 auth session issue")?;
        if !output.status.success() {
            anyhow::bail!(
                "t3 auth session issue failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let token = String::from_utf8(output.stdout)
            .context("t3 auth session issue printed invalid UTF-8")?
            .trim()
            .to_string();
        if token.is_empty() || token.contains(char::is_whitespace) {
            anyhow::bail!("t3 auth session issue did not print a token");
        }
        Ok(token)
    }
}

pub fn default_home() -> PathBuf {
    if let Some(home) = std::env::var_os("T3CODE_HOME").filter(|home| !home.is_empty()) {
        return PathBuf::from(home);
    }
    home_dir().join(".t3")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Searches `PATH`, then the places T3 Code's installers use, since a desktop launch may not
/// have the login shell's `PATH`.
pub fn find_executable() -> Option<PathBuf> {
    let name = if cfg!(windows) { "t3.exe" } else { "t3" };
    let path_dirs = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    let home = home_dir();
    let fallbacks = [
        home.join(".local").join("bin"),
        home.join(".t3").join("bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
    ];
    path_dirs
        .into_iter()
        .chain(fallbacks)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // Signal 0 only checks that the process exists and may be signalled.
    let signalled = unsafe { libc::kill(pid, 0) } == 0;
    signalled || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_is_alive(_pid: u32) -> bool {
    true
}

#[derive(Debug, thiserror::Error)]
pub enum TicketError {
    #[error("the T3 Code server rejected Ade's sign-in")]
    Unauthorized,
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

/// Trades the bearer token for a one-use WebSocket ticket.
pub async fn mint_ticket(origin: &Url, token: &str) -> Result<String, TicketError> {
    #[derive(Deserialize)]
    struct Ticket {
        ticket: String,
    }
    let url = origin
        .join("/api/auth/websocket-ticket/rpc")
        .map_err(anyhow::Error::from)?;
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(anyhow::Error::from)?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(TicketError::Unauthorized);
    }
    if !status.is_success() {
        return Err(anyhow::anyhow!("minting a T3 Code socket ticket failed with {status}").into());
    }
    let body = response.bytes().await.map_err(anyhow::Error::from)?;
    let ticket: Ticket = serde_json::from_slice(&body).map_err(anyhow::Error::from)?;
    Ok(ticket.ticket)
}

/// The socket address for `origin`, with the query the `/ws` route expects.
pub fn socket_url(origin: &Url, ticket: &str, app_version: &str) -> Result<Url> {
    let mut url = origin.join("/ws")?;
    let scheme = match origin.scheme() {
        "https" => "wss",
        _ => "ws",
    };
    url.set_scheme(scheme)
        .map_err(|()| anyhow::anyhow!("cannot use {scheme} for {origin}"))?;
    url.query_pairs_mut()
        .append_pair(
            "orchestrationProtocol",
            &crate::protocol::ORCHESTRATION_PROTOCOL_VERSION.to_string(),
        )
        .append_pair("clientSurface", "desktop")
        .append_pair("clientAppVersion", app_version)
        .append_pair("wsTicket", ticket);
    Ok(url)
}

/// Remembers the token between launches, readable only by this user.
pub struct TokenFile {
    path: PathBuf,
}

impl TokenFile {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub async fn load(&self) -> Option<String> {
        let token = tokio::fs::read_to_string(&self.path).await.ok()?;
        let token = token.trim();
        (!token.is_empty()).then(|| token.to_string())
    }

    pub async fn save(&self, token: &str) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&self.path).await?;
        tokio::io::AsyncWriteExt::write_all(&mut file, token.as_bytes()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_urls_carry_the_protocol_and_ticket() {
        let origin = Url::parse("http://127.0.0.1:37791").expect("url");
        let url = socket_url(&origin, "abc.def", "1.2.3").expect("socket url");
        assert_eq!(url.scheme(), "ws");
        assert_eq!(url.path(), "/ws");
        let query = url.query_pairs().collect::<Vec<_>>();
        assert!(query.contains(&("orchestrationProtocol".into(), "2".into())));
        assert!(query.contains(&("wsTicket".into(), "abc.def".into())));
        assert!(query.contains(&("clientSurface".into(), "desktop".into())));
    }

    #[tokio::test]
    async fn a_missing_runtime_file_means_no_server() {
        let home = std::env::temp_dir().join(format!("ade-t3-test-{}", uuid::Uuid::new_v4()));
        let server = LocalServer {
            home: home.clone(),
            executable: None,
        };
        assert!(server.running().await.expect("reads").is_none());

        tokio::fs::create_dir_all(home.join("userdata"))
            .await
            .expect("mkdir");
        let own_pid = std::process::id();
        tokio::fs::write(
            server.runtime_state_path(),
            format!(r#"{{"version":1,"pid":{own_pid},"origin":"http://127.0.0.1:1","serverVersion":"0.1.12"}}"#),
        )
        .await
        .expect("write");
        let state = server.running().await.expect("reads").expect("running");
        assert_eq!(state.origin, "http://127.0.0.1:1");
        tokio::fs::remove_dir_all(&home).await.ok();
    }

    #[tokio::test]
    async fn token_files_round_trip_privately() {
        let path = std::env::temp_dir()
            .join(format!("ade-t3-token-{}", uuid::Uuid::new_v4()))
            .join("token");
        let file = TokenFile::new(path.clone());
        assert!(file.load().await.is_none());
        file.save("secret").await.expect("saves");
        assert_eq!(file.load().await.as_deref(), Some("secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        if let Some(parent) = path.parent() {
            tokio::fs::remove_dir_all(parent).await.ok();
        }
    }
}
