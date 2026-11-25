//! A client for T3 Code servers. T3 Code runs the coding agents and owns threads, queues and
//! checkpoints; Ade connects to it like T3 Code's own web, desktop and mobile apps do.

pub mod local;
pub mod protocol;
pub mod rpc;
pub mod state;

use anyhow::{Context as _, Result};
use local::{LocalServer, TicketError, TokenFile};
use protocol::{ServerConfig, methods};
use rpc::{RpcClient, RpcError, Subscription};
use serde_json::{Value, json};
use url::Url;

pub use protocol::{ApprovalDecision, Delivery, ModelSelection};

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("the T3 Code server is not running")]
    NotRunning,
    #[error("the t3 command was not found")]
    NotInstalled,
    #[error(
        "this T3 Code server speaks orchestration protocol {server}, but Ade supports {client}"
    )]
    IncompatibleProtocol { server: u32, client: u32 },
    #[error("{0:#}")]
    Other(#[from] anyhow::Error),
}

pub struct ConnectOptions {
    pub server: LocalServer,
    /// Where the bearer token is kept between launches.
    pub token_file: TokenFile,
    pub app_version: String,
}

/// A live session with one T3 Code environment.
pub struct Connection {
    rpc: RpcClient,
    pub config: ServerConfig,
    pub origin: Url,
}

impl Connection {
    /// Connects to the server already running for `options.server`'s home.
    pub async fn connect_local(options: &ConnectOptions) -> Result<Self, ConnectError> {
        let Some(state) = options.server.running().await? else {
            return Err(ConnectError::NotRunning);
        };
        let origin = Url::parse(&state.origin).context("parsing the T3 Code server address")?;

        let token = match options.token_file.load().await {
            Some(token) => token,
            None => issue_and_save(options).await?,
        };
        let ticket = match local::mint_ticket(&origin, &token).await {
            Ok(ticket) => ticket,
            Err(TicketError::Unauthorized) => {
                let token = issue_and_save(options).await?;
                local::mint_ticket(&origin, &token)
                    .await
                    .map_err(|error| anyhow::anyhow!(error))?
            }
            Err(TicketError::Other(error)) => return Err(error.into()),
        };

        let url = local::socket_url(&origin, &ticket, &options.app_version)?;
        let rpc = RpcClient::connect(&url)
            .await
            .context("connecting to the T3 Code server")?;
        let config = rpc
            .request(methods::SERVER_GET_CONFIG, json!({}))
            .await
            .map_err(anyhow::Error::from)?;
        let config: ServerConfig =
            serde_json::from_value(config).context("reading the T3 Code server configuration")?;
        let server_protocol = config.environment.orchestration_protocol_version;
        if server_protocol != protocol::ORCHESTRATION_PROTOCOL_VERSION {
            return Err(ConnectError::IncompatibleProtocol {
                server: server_protocol,
                client: protocol::ORCHESTRATION_PROTOCOL_VERSION,
            });
        }
        Ok(Self {
            rpc,
            config,
            origin,
        })
    }

    pub fn is_closed(&self) -> bool {
        self.rpc.is_closed()
    }

    pub async fn closed(&self) {
        self.rpc.closed().await
    }

    pub fn subscribe_shell(&self) -> Result<Subscription, RpcError> {
        self.rpc.subscribe(methods::SUBSCRIBE_SHELL, json!({}))
    }

    pub fn subscribe_thread(&self, thread_id: &str) -> Result<Subscription, RpcError> {
        self.rpc
            .subscribe(methods::SUBSCRIBE_THREAD, json!({ "threadId": thread_id }))
    }

    /// Sends a command and returns the sequence it committed at. Its effects arrive as events.
    pub async fn dispatch(&self, command: Value) -> Result<u64, RpcError> {
        let result = self.rpc.request(methods::DISPATCH_COMMAND, command).await?;
        Ok(result.get("sequence").and_then(Value::as_u64).unwrap_or(0))
    }

    /// Creates a thread with its first message and returns the thread's id.
    pub async fn launch_thread(&self, input: Value) -> Result<String, RpcError> {
        let result = self.rpc.request(methods::LAUNCH_THREAD, input).await?;
        result
            .get("threadId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| RpcError::Protocol("launchThread returned no thread id".into()))
    }

    /// Adds a folder as a project and returns the project's id.
    pub async fn create_project(
        &self,
        title: &str,
        workspace_root: &str,
    ) -> Result<String, RpcError> {
        let (project_id, mutation) = protocol::commands::create_project(title, workspace_root);
        self.rpc.request(methods::PROJECTS_MUTATE, mutation).await?;
        Ok(project_id)
    }
}

async fn issue_and_save(options: &ConnectOptions) -> Result<String, ConnectError> {
    if options.server.executable.is_none() {
        return Err(ConnectError::NotInstalled);
    }
    let token = options.server.issue_token().await?;
    if let Err(error) = options.token_file.save(&token).await {
        log::warn!("could not save the T3 Code token: {error:#}");
    }
    Ok(token)
}
