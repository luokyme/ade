//! A client for the Effect RPC protocol that T3 Code serves over WebSocket with JSON serialization.
//!
//! Each frame holds one message or an array of messages. Requests are answered by an `Exit`;
//! streaming requests first send `Chunk`s, and the server waits for an `Ack` after each chunk
//! before it sends the next one.

use async_tungstenite::tungstenite::Message;
use futures::StreamExt as _;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::Instant,
};
use tokio_util::compat::TokioAsyncReadCompatExt as _;
use url::Url;

const PING_INTERVAL: Duration = Duration::from_secs(10);
/// The server answers every ping, so a silent socket for this long is gone.
const SILENCE_TIMEOUT: Duration = Duration::from_secs(35);

#[derive(Clone, Debug, thiserror::Error)]
pub enum RpcError {
    #[error("{message}")]
    Failure { message: String, error: Value },
    #[error("the T3 Code server closed the connection")]
    Closed,
    #[error("unexpected response from the T3 Code server: {0}")]
    Protocol(String),
}

impl RpcError {
    /// The `_tag` of a typed failure, such as `OrchestrationV2DispatchCommandError`.
    pub fn failure_tag(&self) -> Option<&str> {
        match self {
            Self::Failure { error, .. } => error.get("_tag").and_then(Value::as_str),
            _ => None,
        }
    }
}

enum Pending {
    Unary(oneshot::Sender<Result<Value, RpcError>>),
    Stream(mpsc::UnboundedSender<Result<Value, RpcError>>),
}

struct Shared {
    outgoing: mpsc::UnboundedSender<Value>,
    pending: Mutex<HashMap<String, Pending>>,
    next_id: AtomicU64,
    closed: AtomicBool,
    closed_signal: watch::Sender<bool>,
}

impl Shared {
    fn send(&self, message: Value) -> Result<(), RpcError> {
        self.outgoing.send(message).map_err(|_| RpcError::Closed)
    }

    fn take_pending(&self, request_id: &str) -> Option<Pending> {
        self.pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(request_id))
    }

    fn close(&self, error: RpcError) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let drained = self
            .pending
            .lock()
            .map(|mut pending| pending.drain().collect::<Vec<_>>())
            .unwrap_or_default();
        for (_, pending) in drained {
            match pending {
                Pending::Unary(sender) => {
                    sender.send(Err(error.clone())).ok();
                }
                Pending::Stream(sender) => {
                    sender.send(Err(error.clone())).ok();
                }
            }
        }
        self.closed_signal.send_replace(true);
    }
}

struct Tasks {
    handles: Vec<JoinHandle<()>>,
    shared: Arc<Shared>,
}

impl Drop for Tasks {
    fn drop(&mut self) {
        for task in &self.handles {
            task.abort();
        }
        // Aborted tasks never reach their own cleanup, so end the open streams here.
        self.shared.close(RpcError::Closed);
    }
}

/// A connected RPC session. Clones share the socket, which closes when the last clone drops.
#[derive(Clone)]
pub struct RpcClient {
    shared: Arc<Shared>,
    _tasks: Arc<Tasks>,
}

impl RpcClient {
    /// Opens the socket. `url` is the complete `ws://` address, including its query.
    pub async fn connect(url: &Url) -> anyhow::Result<Self> {
        if url.scheme() != "ws" {
            anyhow::bail!("only ws:// connections are supported, not {}", url.scheme());
        }
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("{url} has no host"))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| anyhow::anyhow!("{url} has no port"))?;
        let stream = tokio::net::TcpStream::connect((host, port)).await?;
        stream.set_nodelay(true)?;
        let (socket, _response) =
            async_tungstenite::client_async(url.as_str(), stream.compat()).await?;
        Ok(Self::start(socket))
    }

    fn start<S>(socket: async_tungstenite::WebSocketStream<S>) -> Self
    where
        S: futures::AsyncRead + futures::AsyncWrite + Unpin + Send + 'static,
    {
        let (outgoing, mut outgoing_rx) = mpsc::unbounded_channel::<Value>();
        let (closed_signal, _) = watch::channel(false);
        let shared = Arc::new(Shared {
            outgoing,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            closed_signal,
        });
        let (mut sink, mut stream) = socket.split();

        let writer = tokio::spawn({
            let shared = shared.clone();
            async move {
                while let Some(message) = outgoing_rx.recv().await {
                    if let Err(error) = sink.send(Message::text(message.to_string())).await {
                        log::warn!("T3 Code socket write failed: {error}");
                        break;
                    }
                }
                sink.close(None).await.ok();
                shared.close(RpcError::Closed);
            }
        });

        let last_inbound = Arc::new(Mutex::new(Instant::now()));
        let reader = tokio::spawn({
            let shared = shared.clone();
            let last_inbound = last_inbound.clone();
            async move {
                while let Some(message) = stream.next().await {
                    let text = match message {
                        Ok(Message::Text(text)) => text.to_string(),
                        Ok(Message::Binary(bytes)) => match String::from_utf8(bytes.to_vec()) {
                            Ok(text) => text,
                            Err(_) => continue,
                        },
                        Ok(Message::Close(_)) => break,
                        Ok(_) => continue,
                        Err(error) => {
                            log::info!("T3 Code socket closed: {error}");
                            break;
                        }
                    };
                    if let Ok(mut last) = last_inbound.lock() {
                        *last = Instant::now();
                    }
                    match serde_json::from_str::<Value>(&text) {
                        Ok(Value::Array(messages)) => {
                            for message in messages {
                                handle_incoming(&shared, message);
                            }
                        }
                        Ok(message) => handle_incoming(&shared, message),
                        Err(error) => log::warn!("T3 Code sent invalid JSON: {error}"),
                    }
                }
                shared.close(RpcError::Closed);
            }
        });

        let heartbeat = tokio::spawn({
            let shared = shared.clone();
            async move {
                let mut interval = tokio::time::interval(PING_INTERVAL);
                interval.tick().await;
                loop {
                    interval.tick().await;
                    let silent_for = last_inbound
                        .lock()
                        .map(|last| last.elapsed())
                        .unwrap_or_default();
                    if silent_for > SILENCE_TIMEOUT {
                        log::info!("T3 Code server stopped answering pings");
                        shared.close(RpcError::Closed);
                        break;
                    }
                    if shared.send(json!({ "_tag": "Ping" })).is_err() {
                        break;
                    }
                }
            }
        });

        Self {
            shared: shared.clone(),
            _tasks: Arc::new(Tasks {
                handles: vec![writer, reader, heartbeat],
                shared,
            }),
        }
    }

    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    /// Resolves once the connection has closed for any reason.
    pub async fn closed(&self) {
        let mut receiver = self.shared.closed_signal.subscribe();
        receiver.wait_for(|closed| *closed).await.ok();
    }

    fn next_request_id(&self) -> String {
        self.shared
            .next_id
            .fetch_add(1, Ordering::Relaxed)
            .to_string()
    }

    fn send_request(&self, id: &str, tag: &str, payload: Value) -> Result<(), RpcError> {
        self.shared.send(json!({
            "_tag": "Request",
            "id": id,
            "tag": tag,
            "payload": payload,
            "headers": [],
        }))
    }

    pub async fn request(&self, tag: &str, payload: Value) -> Result<Value, RpcError> {
        if self.is_closed() {
            return Err(RpcError::Closed);
        }
        let id = self.next_request_id();
        let (sender, receiver) = oneshot::channel();
        if let Ok(mut pending) = self.shared.pending.lock() {
            pending.insert(id.clone(), Pending::Unary(sender));
        }
        if let Err(error) = self.send_request(&id, tag, payload) {
            self.shared.take_pending(&id);
            return Err(error);
        }
        receiver.await.unwrap_or(Err(RpcError::Closed))
    }

    pub fn subscribe(&self, tag: &str, payload: Value) -> Result<Subscription, RpcError> {
        if self.is_closed() {
            return Err(RpcError::Closed);
        }
        let id = self.next_request_id();
        let (sender, receiver) = mpsc::unbounded_channel();
        if let Ok(mut pending) = self.shared.pending.lock() {
            pending.insert(id.clone(), Pending::Stream(sender));
        }
        if let Err(error) = self.send_request(&id, tag, payload) {
            self.shared.take_pending(&id);
            return Err(error);
        }
        Ok(Subscription {
            id,
            receiver,
            shared: self.shared.clone(),
        })
    }
}

/// The values of a streaming request. Dropping it before the stream ends interrupts the request.
pub struct Subscription {
    id: String,
    receiver: mpsc::UnboundedReceiver<Result<Value, RpcError>>,
    shared: Arc<Shared>,
}

impl Subscription {
    /// The next value, an error that ends the stream, or `None` once the server finished it.
    pub async fn next(&mut self) -> Option<Result<Value, RpcError>> {
        self.receiver.recv().await
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if self.shared.take_pending(&self.id).is_some() {
            self.shared
                .send(json!({ "_tag": "Interrupt", "requestId": self.id }))
                .ok();
        }
    }
}

fn request_id_of(message: &Value) -> Option<String> {
    match message.get("requestId")? {
        Value::String(id) => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
}

fn handle_incoming(shared: &Shared, message: Value) {
    let tag = message.get("_tag").and_then(Value::as_str).unwrap_or("");
    match tag {
        "Chunk" => {
            let Some(id) = request_id_of(&message) else {
                return;
            };
            let values = match message.get("values") {
                Some(Value::Array(values)) => values.clone(),
                _ => Vec::new(),
            };
            let delivered = shared
                .pending
                .lock()
                .ok()
                .map(|pending| match pending.get(&id) {
                    Some(Pending::Stream(sender)) => {
                        for value in values {
                            sender.send(Ok(value)).ok();
                        }
                        true
                    }
                    _ => false,
                })
                .unwrap_or(false);
            if delivered {
                shared.send(json!({ "_tag": "Ack", "requestId": id })).ok();
            }
        }
        "Exit" => {
            let Some(id) = request_id_of(&message) else {
                return;
            };
            let Some(pending) = shared.take_pending(&id) else {
                return;
            };
            let result = exit_result(message.get("exit"));
            match pending {
                Pending::Unary(sender) => {
                    sender.send(result).ok();
                }
                Pending::Stream(sender) => {
                    if let Err(error) = result {
                        sender.send(Err(error)).ok();
                    }
                }
            }
        }
        "Defect" => {
            let defect = message.get("defect").cloned().unwrap_or(Value::Null);
            shared.close(RpcError::Protocol(describe_value(&defect)));
        }
        "ClientProtocolError" => {
            let error = message.get("error").cloned().unwrap_or(Value::Null);
            let error = RpcError::Protocol(describe_value(&error));
            match message.get("requestIds") {
                Some(Value::Array(ids)) => {
                    for id in ids {
                        let id = match id {
                            Value::String(id) => id.clone(),
                            other => other.to_string(),
                        };
                        match shared.take_pending(&id) {
                            Some(Pending::Unary(sender)) => {
                                sender.send(Err(error.clone())).ok();
                            }
                            Some(Pending::Stream(sender)) => {
                                sender.send(Err(error.clone())).ok();
                            }
                            None => {}
                        }
                    }
                }
                _ => shared.close(error),
            }
        }
        "Pong" => {}
        _ => log::debug!("ignoring T3 Code message {tag:?}"),
    }
}

fn exit_result(exit: Option<&Value>) -> Result<Value, RpcError> {
    let Some(exit) = exit else {
        return Err(RpcError::Protocol("an exit without a result".into()));
    };
    match exit.get("_tag").and_then(Value::as_str) {
        Some("Success") => Ok(exit.get("value").cloned().unwrap_or(Value::Null)),
        Some("Failure") => {
            let causes = exit
                .get("cause")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for cause in &causes {
                match cause.get("_tag").and_then(Value::as_str) {
                    Some("Fail") => {
                        let error = cause.get("error").cloned().unwrap_or(Value::Null);
                        return Err(RpcError::Failure {
                            message: describe_value(&error),
                            error,
                        });
                    }
                    Some("Die") => {
                        let defect = cause.get("defect").cloned().unwrap_or(Value::Null);
                        return Err(RpcError::Failure {
                            message: describe_value(&defect),
                            error: defect,
                        });
                    }
                    _ => {}
                }
            }
            Err(RpcError::Closed)
        }
        _ => Err(RpcError::Protocol(exit.to_string())),
    }
}

fn describe_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Object(object) => object
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                object
                    .get("_tag")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| value.to_string()),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_tungstenite::tungstenite::Message;
    use tokio::net::TcpListener;

    async fn serve_once<F, Fut>(handler: F) -> Url
    where
        F: FnOnce(
                async_tungstenite::WebSocketStream<
                    tokio_util::compat::Compat<tokio::net::TcpStream>,
                >,
            ) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let socket = async_tungstenite::accept_async(stream.compat())
                .await
                .expect("handshake");
            handler(socket).await;
        });
        Url::parse(&format!("ws://{address}/ws")).expect("url")
    }

    async fn next_json<S>(socket: &mut async_tungstenite::WebSocketStream<S>) -> Value
    where
        S: futures::AsyncRead + futures::AsyncWrite + Unpin,
    {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    let value: Value = serde_json::from_str(&text).expect("json");
                    if value["_tag"] != "Ping" {
                        return value;
                    }
                }
                Some(Ok(_)) => continue,
                other => panic!("socket ended: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn unary_requests_resolve_with_their_exit() {
        let url = serve_once(|mut socket| async move {
            let request = next_json(&mut socket).await;
            assert_eq!(request["_tag"], "Request");
            assert_eq!(request["tag"], "server.getConfig");
            let reply = json!({
                "_tag": "Exit",
                "requestId": request["id"],
                "exit": { "_tag": "Success", "value": { "ok": true } },
            });
            socket.send(Message::text(reply.to_string())).await.ok();
            next_json(&mut socket).await;
        })
        .await;
        let client = RpcClient::connect(&url).await.expect("connect");
        let value = client
            .request("server.getConfig", json!({}))
            .await
            .expect("success");
        assert_eq!(value, json!({ "ok": true }));
    }

    #[tokio::test]
    async fn failures_carry_the_server_message() {
        let url = serve_once(|mut socket| async move {
            let request = next_json(&mut socket).await;
            let reply = json!({
                "_tag": "Exit",
                "requestId": request["id"],
                "exit": { "_tag": "Failure", "cause": [{
                    "_tag": "Fail",
                    "error": { "_tag": "SomeError", "message": "thread is busy" },
                }]},
            });
            socket.send(Message::text(reply.to_string())).await.ok();
            next_json(&mut socket).await;
        })
        .await;
        let client = RpcClient::connect(&url).await.expect("connect");
        let error = client
            .request("orchestration.dispatchCommand", json!({}))
            .await
            .expect_err("failure");
        assert_eq!(error.to_string(), "thread is busy");
        assert_eq!(error.failure_tag(), Some("SomeError"));
    }

    #[tokio::test]
    async fn streams_ack_each_chunk_and_interrupt_when_dropped() {
        let (done_tx, done_rx) = oneshot::channel::<Vec<Value>>();
        let url = serve_once(|mut socket| async move {
            let request = next_json(&mut socket).await;
            let id = request["id"].clone();
            let chunk = json!([
                { "_tag": "Chunk", "requestId": id, "values": [{ "n": 1 }, { "n": 2 }] }
            ]);
            socket.send(Message::text(chunk.to_string())).await.ok();
            let ack = next_json(&mut socket).await;
            let interrupt = next_json(&mut socket).await;
            done_tx.send(vec![ack, interrupt]).ok();
        })
        .await;
        let client = RpcClient::connect(&url).await.expect("connect");
        let mut subscription = client
            .subscribe("orchestration.subscribeShell", json!({}))
            .expect("subscribe");
        assert_eq!(
            subscription.next().await.expect("first").expect("ok"),
            json!({ "n": 1 })
        );
        assert_eq!(
            subscription.next().await.expect("second").expect("ok"),
            json!({ "n": 2 })
        );
        drop(subscription);
        let messages = done_rx.await.expect("server saw messages");
        assert_eq!(messages[0]["_tag"], "Ack");
        assert_eq!(messages[1]["_tag"], "Interrupt");
        assert_eq!(messages[0]["requestId"], messages[1]["requestId"]);
    }

    #[tokio::test]
    async fn dropping_the_client_ends_its_streams() {
        let url = serve_once(|mut socket| async move {
            next_json(&mut socket).await;
            next_json(&mut socket).await;
        })
        .await;
        let client = RpcClient::connect(&url).await.expect("connect");
        let mut subscription = client
            .subscribe("orchestration.subscribeShell", json!({}))
            .expect("subscribe");
        drop(client);
        let ended = tokio::time::timeout(Duration::from_secs(5), subscription.next())
            .await
            .expect("the stream ends");
        assert!(matches!(ended, Some(Err(RpcError::Closed))));
    }

    #[tokio::test]
    async fn closing_the_socket_fails_pending_requests() {
        let url = serve_once(|mut socket| async move {
            next_json(&mut socket).await;
            socket.close(None).await.ok();
        })
        .await;
        let client = RpcClient::connect(&url).await.expect("connect");
        let error = client
            .request("server.getConfig", json!({}))
            .await
            .expect_err("closed");
        assert!(matches!(error, RpcError::Closed));
        client.closed().await;
        assert!(client.is_closed());
    }
}
