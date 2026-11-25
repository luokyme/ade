use anyhow::{Result, anyhow};
use gpui::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
use i18n::t;
use serde_json::Value;
use std::{
    hash::{Hash as _, Hasher as _},
    sync::Arc,
    time::Duration,
};
use t3_client::{
    ConnectError, ConnectOptions, Connection,
    local::{LocalServer, TokenFile},
    protocol::{ProviderInfo, ShellStreamItem},
    rpc::RpcError,
    state::{Applied, ShellState},
};

const RECONNECT_DELAYS: &[Duration] = &[
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(10),
    Duration::from_secs(30),
];

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Connecting,
    StartingServer,
    Connected,
    NotRunning,
    NotInstalled,
    Failed(SharedString),
}

struct GlobalStore(Entity<T3Store>);

impl Global for GlobalStore {}

/// The connection to the local T3 Code server, shared by every window.
pub struct T3Store {
    status: Status,
    server: LocalServer,
    connection: Option<Arc<Connection>>,
    /// Bumped on every new connection, so views know to subscribe again.
    generation: usize,
    shell: ShellState,
    /// A server Ade started, which stops when Ade quits.
    started_server: Option<tokio::process::Child>,
    reconnect_attempt: usize,
    connect_task: Option<Task<()>>,
    shell_task: Option<Task<()>>,
    reconnect_task: Option<Task<()>>,
}

impl T3Store {
    pub fn global(cx: &mut App) -> Entity<Self> {
        if let Some(store) = cx.try_global::<GlobalStore>() {
            return store.0.clone();
        }
        let store = cx.new(|_| Self::new(LocalServer::discover()));
        cx.set_global(GlobalStore(store.clone()));
        store
    }

    fn new(server: LocalServer) -> Self {
        Self {
            status: Status::Idle,
            server,
            connection: None,
            generation: 0,
            shell: ShellState::default(),
            started_server: None,
            reconnect_attempt: 0,
            connect_task: None,
            shell_task: None,
            reconnect_task: None,
        }
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn connection(&self) -> Option<Arc<Connection>> {
        self.connection.clone()
    }

    pub fn generation(&self) -> usize {
        self.generation
    }

    pub fn shell(&self) -> &ShellState {
        &self.shell
    }

    pub fn providers(&self) -> &[ProviderInfo] {
        self.connection
            .as_ref()
            .map(|connection| connection.config.providers.as_slice())
            .unwrap_or_default()
    }

    pub fn environment_label(&self) -> Option<SharedString> {
        self.connection.as_ref().map(|connection| {
            let environment = &connection.config.environment;
            SharedString::from(format!(
                "{} · T3 Code {}",
                environment.label, environment.server_version
            ))
        })
    }

    /// Connects when nothing is connected or connecting yet.
    pub fn ensure_connected(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, Status::Idle) {
            self.connect(false, cx);
        }
    }

    /// Connects to the running server, starting one first when `start_server` is set.
    pub fn connect(&mut self, start_server: bool, cx: &mut Context<Self>) {
        if matches!(self.status, Status::Connecting | Status::StartingServer) {
            return;
        }
        self.reconnect_task = None;
        self.status = if start_server {
            Status::StartingServer
        } else {
            Status::Connecting
        };
        cx.notify();

        self.server = LocalServer::discover();
        let options = ConnectOptions {
            server: self.server.clone(),
            token_file: TokenFile::new(token_path(&self.server)),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        };
        let start_server = start_server && self.started_server.is_none();
        let work = gpui_tokio::Tokio::spawn(cx, async move {
            let mut child = None;
            if start_server && options.server.running().await?.is_none() {
                if options.server.executable.is_none() {
                    return Err(ConnectError::NotInstalled);
                }
                let (started, _) = options.server.start().await?;
                child = Some(started);
            }
            let connection = Connection::connect_local(&options).await;
            match (connection, child) {
                (Ok(connection), child) => Ok((connection, child)),
                (Err(error), _) => Err(error),
            }
        });
        self.connect_task = Some(cx.spawn(async move |this, cx| {
            let result = match work.await {
                Ok(result) => result,
                Err(error) => Err(ConnectError::Other(anyhow!(error))),
            };
            this.update(cx, |this, cx| this.finish_connect(result, cx))
                .ok();
        }));
    }

    fn finish_connect(
        &mut self,
        result: Result<(Connection, Option<tokio::process::Child>), ConnectError>,
        cx: &mut Context<Self>,
    ) {
        self.connect_task = None;
        let (connection, child) = match result {
            Ok(connected) => connected,
            Err(error) => {
                self.status = match error {
                    ConnectError::NotRunning => Status::NotRunning,
                    ConnectError::NotInstalled => Status::NotInstalled,
                    other => {
                        log::warn!("connecting to T3 Code failed: {other}");
                        Status::Failed(other.to_string().into())
                    }
                };
                cx.notify();
                return;
            }
        };
        if child.is_some() {
            self.started_server = child;
        }
        let connection = Arc::new(connection);
        let mut shell_stream = match connection.subscribe_shell() {
            Ok(stream) => stream,
            Err(error) => {
                self.status = Status::Failed(error.to_string().into());
                cx.notify();
                return;
            }
        };
        self.connection = Some(connection);
        self.generation += 1;
        self.shell = ShellState::default();
        self.status = Status::Connected;
        self.reconnect_attempt = 0;
        cx.notify();

        self.shell_task = Some(cx.spawn(async move |this, cx| {
            let ended = loop {
                match shell_stream.next().await {
                    Some(Ok(value)) => {
                        let applied = this.update(cx, |this, cx| this.apply_shell(&value, cx));
                        match applied {
                            Ok(Applied::NeedsResubscribe) => break None,
                            Ok(_) => {}
                            Err(_) => return,
                        }
                    }
                    Some(Err(error)) => break Some(error),
                    None => break None,
                }
            };
            this.update(cx, |this, cx| this.connection_lost(ended, cx))
                .ok();
        }));
    }

    fn apply_shell(&mut self, value: &Value, cx: &mut Context<Self>) -> Applied {
        match ShellStreamItem::parse(value) {
            Ok(item) => {
                let applied = self.shell.apply(item);
                if applied == Applied::Changed {
                    cx.notify();
                }
                applied
            }
            Err(error) => {
                log::warn!("skipping an unreadable T3 Code shell item: {error:#}");
                Applied::Unchanged
            }
        }
    }

    fn connection_lost(&mut self, error: Option<RpcError>, cx: &mut Context<Self>) {
        if let Some(error) = &error {
            log::info!("T3 Code connection ended: {error}");
        }
        self.connection = None;
        self.shell_task = None;
        self.status = Status::Failed(t!("Disconnected from T3 Code"));
        cx.notify();
        let delay = RECONNECT_DELAYS
            .get(self.reconnect_attempt)
            .or(RECONNECT_DELAYS.last())
            .copied()
            .unwrap_or(Duration::from_secs(30));
        self.reconnect_attempt += 1;
        self.reconnect_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                if this.connection.is_none() {
                    this.connect(false, cx);
                }
            })
            .ok();
        }));
    }

    pub fn dispatch(&self, command: Value, cx: &App) -> Task<Result<u64>> {
        let Some(connection) = self.connection.clone() else {
            return Task::ready(Err(anyhow!(t!("Not connected to T3 Code"))));
        };
        cx.background_spawn(async move { Ok(connection.dispatch(command).await?) })
    }

    pub fn launch_thread(&self, input: Value, cx: &App) -> Task<Result<String>> {
        let Some(connection) = self.connection.clone() else {
            return Task::ready(Err(anyhow!(t!("Not connected to T3 Code"))));
        };
        cx.background_spawn(async move { Ok(connection.launch_thread(input).await?) })
    }

    pub fn create_project(&self, title: String, root: String, cx: &App) -> Task<Result<String>> {
        let Some(connection) = self.connection.clone() else {
            return Task::ready(Err(anyhow!(t!("Not connected to T3 Code"))));
        };
        cx.background_spawn(async move { Ok(connection.create_project(&title, &root).await?) })
    }
}

/// One token per T3 Code home, so pointing Ade at another server signs in there separately.
fn token_path(server: &LocalServer) -> std::path::PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    server.home.hash(&mut hasher);
    paths::data_dir()
        .join("t3")
        .join(format!("token-{:016x}", hasher.finish()))
}
