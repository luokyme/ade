//! The parts of T3 Code's orchestration contract that Ade reads and sends.
//!
//! Records are decoded leniently: fields Ade does not use are ignored, missing optional fields
//! default, and unknown stream items and event types are kept as `Unknown` so a newer server
//! keeps working with this client.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The orchestration protocol version that the `/ws` route checks for.
pub const ORCHESTRATION_PROTOCOL_VERSION: u32 = 2;

pub mod methods {
    pub const SERVER_GET_CONFIG: &str = "server.getConfig";
    pub const SUBSCRIBE_SHELL: &str = "orchestration.subscribeShell";
    pub const SUBSCRIBE_THREAD: &str = "orchestration.subscribeThread";
    pub const DISPATCH_COMMAND: &str = "orchestration.dispatchCommand";
    pub const LAUNCH_THREAD: &str = "orchestration.launchThread";
    pub const PROJECTS_MUTATE: &str = "projects.mutate";
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub environment: EnvironmentInfo,
    #[serde(default)]
    pub providers: Vec<ProviderInfo>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentInfo {
    pub environment_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub server_version: String,
    #[serde(default)]
    pub orchestration_protocol_version: u32,
    #[serde(default)]
    pub capabilities: Map<String, Value>,
}

impl EnvironmentInfo {
    pub fn has_capability(&self, name: &str) -> bool {
        self.capabilities.get(name).and_then(Value::as_bool) == Some(true)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub instance_id: String,
    #[serde(default)]
    pub driver: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub models: Vec<ProviderModel>,
}

impl ProviderInfo {
    pub fn is_ready(&self) -> bool {
        self.enabled && self.installed && self.status == "ready"
    }

    pub fn default_model(&self) -> Option<&ProviderModel> {
        self.models
            .iter()
            .find(|model| model.is_default)
            .or_else(|| self.models.first())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModel {
    pub slug: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelection {
    pub instance_id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lineage {
    #[serde(default)]
    pub parent_thread_id: Option<String>,
    #[serde(default)]
    pub relationship_to_parent: Option<String>,
}

/// A thread as the sidebar sees it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadShell {
    pub id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub active_run_id: Option<String>,
    #[serde(default)]
    pub latest_run_id: Option<String>,
    #[serde(default)]
    pub pending_runtime_request: Option<Value>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub model_selection: Option<ModelSelection>,
    #[serde(default)]
    pub runtime_mode: Option<String>,
    #[serde(default)]
    pub lineage: Lineage,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub archived_at: Option<String>,
    #[serde(default)]
    pub settled_at: Option<String>,
    #[serde(default)]
    pub pinned_at: Option<String>,
    #[serde(default)]
    pub deleted_at: Option<String>,
}

impl ThreadShell {
    /// A subagent or side chat, which T3 Code lists under its parent rather than on its own.
    pub fn is_child(&self) -> bool {
        self.lineage.parent_thread_id.is_some()
    }

    pub fn is_working(&self) -> bool {
        self.active_run_id.is_some()
    }

    pub fn needs_attention(&self) -> bool {
        self.pending_runtime_request.is_some()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectShell {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub workspace_root: String,
    #[serde(default)]
    pub default_model_selection: Option<ModelSelection>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellSnapshot {
    #[serde(default)]
    pub snapshot_sequence: u64,
    #[serde(default)]
    pub threads: Vec<ThreadShell>,
    #[serde(default)]
    pub archived_threads: Vec<ThreadShell>,
    #[serde(default)]
    pub projects: Vec<ProjectShell>,
}

#[derive(Clone, Debug)]
pub enum ShellStreamItem {
    Synchronized,
    SnapshotRequired,
    Snapshot(ShellSnapshot),
    ProjectUpdated {
        sequence: u64,
        project: ProjectShell,
    },
    ProjectRemoved {
        sequence: u64,
        project_id: String,
    },
    ThreadUpdated {
        sequence: u64,
        archived: bool,
        thread: ThreadShell,
    },
    ThreadRemoved {
        sequence: u64,
        archived: bool,
        thread_id: String,
    },
    Unknown(String),
}

fn sequence_of(value: &Value) -> u64 {
    value.get("sequence").and_then(Value::as_u64).unwrap_or(0)
}

fn string_field(value: &Value, name: &str) -> Option<String> {
    value.get(name).and_then(Value::as_str).map(str::to_string)
}

fn decode<T: serde::de::DeserializeOwned>(value: Option<&Value>, what: &str) -> anyhow::Result<T> {
    let value = value.ok_or_else(|| anyhow::anyhow!("missing {what}"))?;
    serde_json::from_value(value.clone())
        .map_err(|error| anyhow::anyhow!("invalid {what}: {error}"))
}

impl ShellStreamItem {
    pub fn parse(value: &Value) -> anyhow::Result<Self> {
        let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
        let archived = value.get("location").and_then(Value::as_str) == Some("archive");
        Ok(match kind {
            "synchronized" => Self::Synchronized,
            "snapshot-required" => Self::SnapshotRequired,
            "snapshot" => Self::Snapshot(decode(value.get("snapshot"), "shell snapshot")?),
            "project.updated" => Self::ProjectUpdated {
                sequence: sequence_of(value),
                project: decode(value.get("project"), "project")?,
            },
            "project.removed" => Self::ProjectRemoved {
                sequence: sequence_of(value),
                project_id: string_field(value, "projectId").unwrap_or_default(),
            },
            "thread.updated" => Self::ThreadUpdated {
                sequence: sequence_of(value),
                archived,
                thread: decode(value.get("thread"), "thread")?,
            },
            "thread.removed" => Self::ThreadRemoved {
                sequence: sequence_of(value),
                archived,
                thread_id: string_field(value, "threadId").unwrap_or_default(),
            },
            other => Self::Unknown(other.to_string()),
        })
    }
}

/// The thread record inside a thread projection.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRecord {
    pub id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub model_selection: Option<ModelSelection>,
    #[serde(default)]
    pub runtime_mode: Option<String>,
    #[serde(default)]
    pub interaction_mode: Option<String>,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    #[serde(default)]
    pub ordinal: u64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub queue_position: Option<i64>,
    #[serde(default)]
    pub user_message_id: Option<String>,
    #[serde(default)]
    pub model_selection: Option<ModelSelection>,
    #[serde(default)]
    pub requested_at: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
}

impl Run {
    /// Statuses from which a run still moves on by itself.
    pub fn is_unfinished(&self) -> bool {
        matches!(
            self.status.as_str(),
            "preparing" | "queued" | "starting" | "running" | "waiting"
        )
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.status.as_str(),
            "preparing" | "starting" | "running" | "waiting"
        )
    }

    pub fn is_queued(&self) -> bool {
        self.status == "queued"
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunAttempt {
    pub id: String,
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub root_node_id: Option<String>,
    #[serde(default)]
    pub status: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRequest {
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub response_capability: Option<Value>,
}

impl RuntimeRequest {
    pub fn is_pending(&self) -> bool {
        self.status == "pending"
    }

    /// Whether the agent is still waiting live for this answer, rather than taking it as a message.
    pub fn accepts_decision(&self) -> bool {
        self.response_capability
            .as_ref()
            .and_then(|capability| capability.get("type"))
            .and_then(Value::as_str)
            .is_none_or(|kind| kind == "live")
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub run_id: Option<String>,
}

/// One row of a thread's timeline. The fields that depend on `kind` stay in `fields`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnItem {
    pub id: String,
    #[serde(default)]
    pub thread_id: String,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub parent_item_id: Option<String>,
    #[serde(default)]
    pub ordinal: i64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

impl TurnItem {
    pub fn str_field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).and_then(Value::as_str)
    }

    pub fn bool_field(&self, name: &str) -> bool {
        self.fields.get(name).and_then(Value::as_bool) == Some(true)
    }

    pub fn i64_field(&self, name: &str) -> Option<i64> {
        self.fields.get(name).and_then(Value::as_i64)
    }

    pub fn text(&self) -> &str {
        self.str_field("text").unwrap_or("")
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status.as_str(), "running" | "pending" | "in_progress")
    }

    pub fn is_failed(&self) -> bool {
        matches!(self.status.as_str(), "failed" | "error" | "declined")
            || self.bool_field("outputIndicatesFailure")
            || self.i64_field("exitCode").is_some_and(|code| code != 0)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisibleTurnItem {
    #[serde(default)]
    pub position: u64,
    #[serde(default)]
    pub visibility: String,
    #[serde(default)]
    pub source_thread_id: String,
    #[serde(default)]
    pub source_item_id: String,
    pub item: TurnItem,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadProjection {
    pub thread: ThreadRecord,
    #[serde(default)]
    pub runs: Vec<Run>,
    #[serde(default)]
    pub attempts: Vec<RunAttempt>,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub runtime_requests: Vec<RuntimeRequest>,
    #[serde(default)]
    pub turn_items: Vec<TurnItem>,
    #[serde(default)]
    pub visible_turn_items: Vec<VisibleTurnItem>,
}

#[derive(Clone, Debug)]
pub enum ThreadEvent {
    ThreadChanged(ThreadRecord),
    RunUpdated(Run),
    AttemptUpdated(RunAttempt),
    MessageUpdated(Message),
    RuntimeRequestUpdated(RuntimeRequest),
    TurnItemUpdated(TurnItem),
    Other(String),
}

#[derive(Clone, Debug)]
pub enum ThreadStreamItem {
    Synchronized,
    Snapshot {
        sequence: u64,
        projection: Box<ThreadProjection>,
    },
    Event {
        sequence: u64,
        thread_id: String,
        event: ThreadEvent,
    },
    Unknown(String),
}

impl ThreadStreamItem {
    pub fn parse(value: &Value) -> anyhow::Result<Self> {
        let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
        Ok(match kind {
            "synchronized" => Self::Synchronized,
            "snapshot" => Self::Snapshot {
                sequence: value
                    .get("snapshotSequence")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                projection: Box::new(decode(value.get("projection"), "thread projection")?),
            },
            "event" => {
                let event = value.get("event").cloned().unwrap_or(Value::Null);
                Self::Event {
                    sequence: sequence_of(value),
                    thread_id: string_field(&event, "threadId").unwrap_or_default(),
                    event: ThreadEvent::parse(&event)?,
                }
            }
            other => Self::Unknown(other.to_string()),
        })
    }
}

impl ThreadEvent {
    fn parse(event: &Value) -> anyhow::Result<Self> {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let payload = event.get("payload");
        Ok(match kind {
            "run.created" | "run.updated" => Self::RunUpdated(decode(payload, kind)?),
            "run-attempt.created" | "run-attempt.updated" => {
                Self::AttemptUpdated(decode(payload, kind)?)
            }
            "message.updated" => Self::MessageUpdated(decode(payload, kind)?),
            "runtime-request.updated" => Self::RuntimeRequestUpdated(decode(payload, kind)?),
            "turn-item.updated" => Self::TurnItemUpdated(decode(payload, kind)?),
            // These events carry the whole thread record as their payload.
            kind if kind.starts_with("thread.")
                && payload.is_some_and(|payload| {
                    payload.get("id").is_some() && payload.get("title").is_some()
                }) =>
            {
                Self::ThreadChanged(decode(payload, kind)?)
            }
            other => Self::Other(other.to_string()),
        })
    }
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
}

impl ApprovalDecision {
    fn wire(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::AcceptForSession => "acceptForSession",
            Self::Decline => "decline",
        }
    }
}

/// How a message joins a thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Starts a turn now; the thread must be idle.
    StartNow,
    /// Runs after the current turn and anything queued before it.
    Queue,
    /// Joins the running turn.
    Steer { run_id: String },
}

/// Commands for `orchestration.dispatchCommand`. Ade sends them as a desktop client.
pub mod commands {
    use super::*;

    fn creation_fields(command: &mut Map<String, Value>) {
        command.insert("createdBy".into(), json!("user"));
        command.insert("creationSource".into(), json!("web"));
    }

    pub fn send_message(thread_id: &str, text: &str, delivery: &Delivery) -> Value {
        let dispatch_mode = match delivery {
            Delivery::StartNow => json!({ "type": "start_immediately" }),
            Delivery::Queue => json!({ "type": "queue_after_active" }),
            Delivery::Steer { run_id } => json!({ "type": "steer_active", "targetRunId": run_id }),
        };
        let mut command = Map::new();
        command.insert("type".into(), json!("message.dispatch"));
        creation_fields(&mut command);
        command.insert("commandId".into(), json!(new_id()));
        command.insert("threadId".into(), json!(thread_id));
        command.insert("messageId".into(), json!(new_id()));
        command.insert("text".into(), json!(text));
        command.insert("attachments".into(), json!([]));
        command.insert("dispatchMode".into(), dispatch_mode);
        Value::Object(command)
    }

    pub fn interrupt_run(thread_id: &str, run_id: &str) -> Value {
        json!({
            "type": "run.interrupt",
            "commandId": new_id(),
            "threadId": thread_id,
            "runId": run_id,
        })
    }

    pub fn stop_thread(thread_id: &str) -> Value {
        json!({
            "type": "thread.stop",
            "commandId": new_id(),
            "threadId": thread_id,
        })
    }

    pub fn cancel_queued_run(thread_id: &str, run_id: &str) -> Value {
        json!({
            "type": "queued-run.cancel",
            "commandId": new_id(),
            "threadId": thread_id,
            "runId": run_id,
        })
    }

    pub fn respond_to_request(
        thread_id: &str,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Value {
        json!({
            "type": "runtime-request.respond",
            "commandId": new_id(),
            "threadId": thread_id,
            "requestId": request_id,
            "decision": decision.wire(),
        })
    }

    /// Input for `orchestration.launchThread`, which creates a thread and sends its first message.
    pub fn launch_thread(
        project_id: &str,
        model_selection: &ModelSelection,
        runtime_mode: &str,
        text: &str,
    ) -> Value {
        json!({
            "commandId": new_id(),
            "creationSource": "web",
            "threadId": new_id(),
            "projectId": project_id,
            "title": provisional_title(text),
            "generateTitle": true,
            "modelSelection": model_selection,
            "runtimeMode": runtime_mode,
            "interactionMode": "default",
            "workspaceStrategy": { "type": "root" },
            "initialMessage": {
                "messageId": new_id(),
                "text": text,
                "attachments": [],
            },
        })
    }

    /// A `projects.mutate` payload that adds `workspace_root` as a project.
    pub fn create_project(title: &str, workspace_root: &str) -> (String, Value) {
        let project_id = new_id();
        let mutation = json!({
            "type": "project.create",
            "commandId": new_id(),
            "projectId": project_id,
            "title": title,
            "workspaceRoot": workspace_root,
        });
        (project_id, mutation)
    }

    fn provisional_title(text: &str) -> String {
        const MAX_CHARS: usize = 80;
        let first_line = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("New thread");
        let mut title: String = first_line.chars().take(MAX_CHARS).collect();
        if first_line.chars().count() > MAX_CHARS {
            title.push('…');
        }
        title
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_items_decode_and_unknown_kinds_survive() {
        let snapshot = json!({
            "kind": "snapshot",
            "snapshot": {
                "schemaVersion": 2,
                "snapshotSequence": 53,
                "threads": [{
                    "id": "t1", "projectId": "p1", "title": "Fix it", "status": "completed",
                    "activeRunId": null, "lineage": { "parentThreadId": null },
                    "createdAt": "2026-11-25T10:24:59.408Z", "updatedAt": "2026-11-25T10:25:05.827Z",
                    "someNewField": { "nested": true },
                }],
                "archivedThreads": [],
                "projects": [{ "id": "p1", "title": "play", "workspaceRoot": "/tmp/play" }],
            },
        });
        let ShellStreamItem::Snapshot(snapshot) =
            ShellStreamItem::parse(&snapshot).expect("parses")
        else {
            panic!("expected a snapshot");
        };
        assert_eq!(snapshot.snapshot_sequence, 53);
        assert_eq!(snapshot.threads[0].title, "Fix it");
        assert!(!snapshot.threads[0].is_child());
        assert_eq!(snapshot.projects[0].workspace_root, "/tmp/play");

        let future = json!({ "kind": "thread.teleported", "sequence": 9 });
        assert!(matches!(
            ShellStreamItem::parse(&future).expect("parses"),
            ShellStreamItem::Unknown(kind) if kind == "thread.teleported"
        ));
    }

    #[test]
    fn turn_items_keep_their_type_specific_fields() {
        let item: TurnItem = serde_json::from_value(json!({
            "id": "i1", "threadId": "t1", "runId": "r1", "ordinal": 1000004,
            "status": "completed", "title": null, "type": "command_execution",
            "input": "cat README.md", "exitCode": 2,
        }))
        .expect("parses");
        assert_eq!(item.kind, "command_execution");
        assert_eq!(item.str_field("input"), Some("cat README.md"));
        assert!(item.is_failed());
    }

    #[test]
    fn thread_events_decode_by_type() {
        let event = json!({
            "kind": "event",
            "sequence": 86,
            "event": {
                "type": "turn-item.updated",
                "threadId": "t1",
                "payload": {
                    "id": "i2", "threadId": "t1", "ordinal": 1000006, "status": "completed",
                    "type": "assistant_message", "text": "done", "streaming": false,
                },
            },
        });
        let ThreadStreamItem::Event {
            sequence,
            thread_id,
            event: ThreadEvent::TurnItemUpdated(item),
        } = ThreadStreamItem::parse(&event).expect("parses")
        else {
            panic!("expected a turn item event");
        };
        assert_eq!(sequence, 86);
        assert_eq!(thread_id, "t1");
        assert_eq!(item.text(), "done");

        let other = json!({
            "kind": "event", "sequence": 87,
            "event": { "type": "provider-turn.updated", "threadId": "t1", "payload": {} },
        });
        assert!(matches!(
            ThreadStreamItem::parse(&other).expect("parses"),
            ThreadStreamItem::Event {
                event: ThreadEvent::Other(_),
                ..
            }
        ));
    }

    #[test]
    fn messages_carry_the_fields_the_server_requires() {
        let command = commands::send_message("t1", "hello", &Delivery::Queue);
        assert_eq!(command["type"], "message.dispatch");
        assert_eq!(command["createdBy"], "user");
        assert_eq!(command["creationSource"], "web");
        assert_eq!(command["dispatchMode"]["type"], "queue_after_active");
        assert_eq!(command["attachments"], json!([]));

        let steer = commands::send_message(
            "t1",
            "also this",
            &Delivery::Steer {
                run_id: "r1".into(),
            },
        );
        assert_eq!(steer["dispatchMode"]["targetRunId"], "r1");
    }

    #[test]
    fn launched_threads_take_a_title_from_the_first_line() {
        let selection = ModelSelection {
            instance_id: "claudeAgent".into(),
            model: "claude-haiku-4-5".into(),
            options: None,
        };
        let input =
            commands::launch_thread("p1", &selection, "full-access", "\n  Fix the build\nmore");
        assert_eq!(input["title"], "Fix the build");
        assert_eq!(input["modelSelection"]["instanceId"], "claudeAgent");
        assert!(input["modelSelection"].get("options").is_none());
        assert_eq!(input["initialMessage"]["text"], "\n  Fix the build\nmore");
    }
}
