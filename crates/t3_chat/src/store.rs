use anyhow::{Result, anyhow};
use db::kvp::KeyValueStore;
use gpui::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
use i18n::t;
use serde_json::Value;
use std::{
    hash::{Hash as _, Hasher as _},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use t3_client::{
    ConnectError, ConnectOptions, Connection, ModelSelection,
    local::{LocalServer, TokenFile},
    protocol::{ChatAttachment, ProviderInfo, ShellStreamItem, TurnItem},
    rpc::RpcError,
    state::{Applied, ShellState},
};
use util::ResultExt as _;
use workspace::AgentActivity;

const LAST_MODEL_SELECTION_KEY: &str = "t3_chat_last_model_selection";
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

fn announce_activity_changes(store: &Entity<T3Store>, cx: &mut App) {
    let Some(changes) = workspace::agent_activity_changes(cx) else {
        return;
    };
    let mut last_seen = Vec::new();
    cx.observe(store, move |store, cx| {
        let activities = store.read(cx).thread_activities();
        if activities != last_seen {
            last_seen = activities;
            changes.update(cx, |_, cx| cx.notify());
        }
    })
    .detach();
}

pub(crate) fn project_agent_activity(roots: &[PathBuf], cx: &App) -> Option<AgentActivity> {
    cx.try_global::<GlobalStore>()?
        .0
        .read(cx)
        .agent_activity_in(roots)
}

/// The connection to the local T3 Code server, shared by every window.
pub struct T3Store {
    status: Status,
    server: LocalServer,
    connection: Option<Arc<Connection>>,
    /// Bumped on every new connection, so views know to subscribe again.
    generation: usize,
    shell: ShellState,
    last_model_selection: Option<ModelSelection>,
    save_model_selection: Option<Task<()>>,
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
        let store = cx.new(|cx| Self::new(LocalServer::discover(), cx));
        cx.set_global(GlobalStore(store.clone()));
        announce_activity_changes(&store, cx);
        store
    }

    /// The most urgent activity among the threads of the projects rooted at `roots`.
    pub fn agent_activity_in(&self, roots: &[PathBuf]) -> Option<AgentActivity> {
        self.thread_activities()
            .into_iter()
            .filter(|(root, _, _)| roots.iter().any(|candidate| candidate == root))
            .map(|(_, _, activity)| activity)
            .max()
    }

    fn thread_activities(&self) -> Vec<(PathBuf, String, AgentActivity)> {
        let mut activities = Vec::new();
        for project in &self.shell.projects {
            let root = PathBuf::from(&project.workspace_root);
            for thread in self.shell.project_threads(&project.id) {
                let activity = if thread.needs_attention() {
                    AgentActivity::NeedsInput
                } else if thread.is_working() {
                    AgentActivity::Working
                } else {
                    continue;
                };
                activities.push((root.clone(), thread.id.clone(), activity));
            }
        }
        activities.sort_by(|a, b| a.1.cmp(&b.1));
        activities
    }

    fn new(server: LocalServer, cx: &App) -> Self {
        let last_model_selection = KeyValueStore::global(cx)
            .read_kvp(LAST_MODEL_SELECTION_KEY)
            .log_err()
            .flatten()
            .and_then(|value| serde_json::from_str(&value).log_err());
        Self {
            status: Status::Idle,
            server,
            connection: None,
            generation: 0,
            shell: ShellState::default(),
            last_model_selection,
            save_model_selection: None,
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

    pub fn last_model_selection(&self) -> Option<&ModelSelection> {
        self.last_model_selection.as_ref().filter(|selection| {
            self.providers().iter().any(|provider| {
                provider.is_ready()
                    && provider.instance_id == selection.instance_id
                    && provider.model(&selection.model).is_some()
            })
        })
    }

    pub fn remember_model_selection(&mut self, selection: &ModelSelection, cx: &App) {
        self.last_model_selection = Some(selection.clone());

        let selection = selection.clone();
        let database = KeyValueStore::global(cx);
        let previous_save = self.save_model_selection.take();
        self.save_model_selection = Some(cx.background_spawn(async move {
            if let Some(previous_save) = previous_save {
                previous_save.await;
            }
            if let Some(value) = serde_json::to_string(&selection).log_err() {
                database
                    .write_kvp(LAST_MODEL_SELECTION_KEY.into(), value)
                    .await
                    .log_err();
            }
        }));
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

    pub fn get_turn_item(
        &self,
        thread_id: String,
        item_id: String,
        cx: &App,
    ) -> Task<Result<Option<TurnItem>>> {
        let Some(connection) = self.connection.clone() else {
            return Task::ready(Err(anyhow!(t!("Not connected to T3 Code"))));
        };
        cx.background_spawn(async move { Ok(connection.get_turn_item(&thread_id, &item_id).await?) })
    }

    pub fn upload_attachment(
        &self,
        name: String,
        bytes: Vec<u8>,
        cx: &App,
    ) -> Task<Result<ChatAttachment>> {
        let Some(connection) = self.connection.clone() else {
            return Task::ready(Err(anyhow!(t!("Not connected to T3 Code"))));
        };
        gpui_tokio::Tokio::spawn_result(cx, async move {
            connection.upload_attachment(&name, bytes).await
        })
    }

    pub fn download_attachment(
        &self,
        attachment_id: String,
        name: String,
        mime_type: String,
        cx: &App,
    ) -> Task<Result<Vec<u8>>> {
        let Some(connection) = self.connection.clone() else {
            return Task::ready(Err(anyhow!(t!("Not connected to T3 Code"))));
        };
        gpui_tokio::Tokio::spawn_result(cx, async move {
            connection.download_attachment(&attachment_id, &name, &mime_type).await
        })
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
