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

    /// Streams a running command's output. Each frame is `{ kind: "replace" | "append", text,
    /// truncated, running }`; the stream ends after a frame with `running: false`.
    pub fn subscribe_command_output(
        &self,
        thread_id: &str,
        item_id: &str,
    ) -> Result<Subscription, RpcError> {
        self.rpc.subscribe(
            methods::SUBSCRIBE_COMMAND_OUTPUT,
            json!({ "threadId": thread_id, "itemId": item_id }),
        )
    }

    /// Uploads a file for a message to carry. Images go to agents as images, the rest as files
    /// they can read.
    pub async fn upload_attachment(
        &self,
        name: &str,
        bytes: Vec<u8>,
    ) -> anyhow::Result<protocol::ChatAttachment> {
        let (mime_type, is_image) = protocol::attachment_media_type(name);
        let size_bytes = bytes.len() as u64;
        let limit = if is_image {
            protocol::MAX_IMAGE_BYTES
        } else {
            protocol::MAX_FILE_BYTES
        };
        anyhow::ensure!(size_bytes > 0, "{name} is empty");
        anyhow::ensure!(
            size_bytes <= limit,
            "{name} is larger than the {} MB T3 Code accepts",
            limit / 1024 / 1024
        );
        let kind = if is_image { "image" } else { "file" };
        let ticket = self
            .rpc
            .request(
                methods::ATTACHMENTS_CREATE_UPLOAD_URL,
                json!({ "type": kind, "name": name, "mimeType": mime_type, "sizeBytes": size_bytes }),
            )
            .await?;
        let attachment_id = ticket
            .get("attachmentId")
            .and_then(Value::as_str)
            .context("the upload ticket has no attachment id")?
            .to_string();
        let relative_url = ticket
            .get("relativeUrl")
            .and_then(Value::as_str)
            .context("the upload ticket has no address")?;
        let url = self.origin.join(relative_url)?;
        let response = reqwest::Client::new()
            .post(url)
            .header(reqwest::header::CONTENT_LENGTH, size_bytes)
            .header(reqwest::header::CONTENT_TYPE, &mime_type)
            .body(bytes)
            .send()
            .await
            .context("uploading the attachment")?;
        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            anyhow::bail!("T3 Code refused {name} ({status}): {detail}");
        }
        Ok(protocol::ChatAttachment {
            kind: kind.to_string(),
            id: attachment_id,
            name: name.to_string(),
            mime_type,
            size_bytes,
        })
    }

    /// Downloads the bytes of a file a message carried, through the short-lived address the server
    /// issues for it.
    pub async fn download_attachment(
        &self,
        attachment_id: &str,
        name: &str,
        mime_type: &str,
    ) -> anyhow::Result<Vec<u8>> {
        let ticket = self
            .rpc
            .request(
                methods::ASSETS_CREATE_URL,
                json!({
                    "resource": {
                        "_tag": "attachment",
                        "attachmentId": attachment_id,
                        "fileName": name,
                        "mimeType": mime_type,
                        "disposition": "inline",
                    }
                }),
            )
            .await?;
        let relative_url = ticket
            .get("relativeUrl")
            .and_then(Value::as_str)
            .context("the asset ticket has no address")?;
        let response = reqwest::Client::new()
            .get(self.origin.join(relative_url)?)
            .send()
            .await
            .context("downloading the attachment")?;
        let status = response.status();
        anyhow::ensure!(status.is_success(), "T3 Code refused {name} ({status})");
        Ok(response.bytes().await?.to_vec())
    }

    /// Fetches one timeline item in full, including output the thread stream left out.
    pub async fn get_turn_item(
        &self,
        thread_id: &str,
        item_id: &str,
    ) -> Result<Option<protocol::TurnItem>, RpcError> {
        let result = self
            .rpc
            .request(
                methods::GET_TURN_ITEM,
                json!({ "threadId": thread_id, "itemId": item_id }),
            )
            .await?;
        match result.get("item") {
            None | Some(Value::Null) => Ok(None),
            Some(item) => serde_json::from_value(item.clone())
                .map(Some)
                .map_err(|error| RpcError::Protocol(format!("unreadable turn item: {error}"))),
        }
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
