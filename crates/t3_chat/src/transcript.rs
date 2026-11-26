//! How a T3 Code timeline reads as a chat: turns that start at each message the user sent,
//! the agent's work folded into groups, and its replies between them.

use i18n::t;
use serde_json::{Map, Value};
use std::{borrow::Cow, ops::Range, path::Path};
use t3_client::protocol::TurnItem;
use ui::{IconName, SharedString};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    User,
    Reply,
    Work,
    Card,
    Hidden,
}

pub fn classify(item: &TurnItem) -> Class {
    match item.kind.as_str() {
        "user_message" => Class::User,
        "assistant_message" if item.text().trim().is_empty() => Class::Hidden,
        "assistant_message" => Class::Reply,
        "reasoning" if item.text().trim().is_empty() => Class::Hidden,
        "command_execution" if is_workspace_setup(item) && !item.is_failed() => Class::Hidden,
        "dynamic_tool" if sent_message(item).is_some() => Class::Reply,
        "dynamic_tool" if html_render(item).is_some() => Class::Card,
        "reasoning" | "command_execution" | "file_change" | "file_search" | "web_search"
        | "dynamic_tool" | "subagent" => Class::Work,
        "todo_list" => Class::Card,
        "checkpoint" => {
            let has_files = item
                .fields
                .get("files")
                .and_then(Value::as_array)
                .is_some_and(|files| !files.is_empty());
            if has_files {
                Class::Card
            } else {
                Class::Hidden
            }
        }
        "approval_request" | "user_input_request" | "secret_request" | "error"
        | "proposed_plan" => Class::Card,
        "system_notice" | "run_interrupt_request" | "run_interrupt_result" => {
            if notice_text(item).is_empty() {
                Class::Hidden
            } else {
                Class::Card
            }
        }
        "compaction" | "fork" | "thread_created" | "handoff" | "notification" | "link_request"
        | "feedback_request" => Class::Card,
        _ if item
            .title
            .as_deref()
            .is_some_and(|title| !title.trim().is_empty()) =>
        {
            Class::Card
        }
        _ => Class::Hidden,
    }
}

pub fn notice_text(item: &TurnItem) -> String {
    match item.kind.as_str() {
        "compaction" => t!("Conversation compacted").to_string(),
        "system_notice" | "run_interrupt_request" | "run_interrupt_result" => {
            item.str_field("message").unwrap_or("").trim().to_string()
        }
        _ => item.title.clone().unwrap_or_default(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub run_id: Option<String>,
    pub user: Option<usize>,
    pub items: Range<usize>,
    pub first_reply: Option<usize>,
    pub last_reply: Option<usize>,
}

pub fn group_turns(items: &[TurnItem]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let class = classify(item);
        if class == Class::Hidden {
            continue;
        }
        let starts_turn = match turns.last() {
            None => true,
            Some(_) if class == Class::User => true,
            Some(turn) => match (&turn.run_id, &item.run_id) {
                (Some(current), Some(next)) => current != next,
                _ => false,
            },
        };
        if starts_turn {
            let is_user = class == Class::User;
            turns.push(Turn {
                run_id: item.run_id.clone(),
                user: is_user.then_some(index),
                items: if is_user {
                    index + 1..index + 1
                } else {
                    index..index
                },
                first_reply: None,
                last_reply: None,
            });
            if is_user {
                continue;
            }
        }
        if let Some(turn) = turns.last_mut() {
            turn.items.end = index + 1;
            if turn.run_id.is_none() {
                turn.run_id = item.run_id.clone();
            }
            if class == Class::Reply {
                turn.first_reply.get_or_insert(index);
                turn.last_reply = Some(index);
            }
        }
    }
    turns
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Row {
    User(usize),
    /// Folds everything before the last reply of a finished turn.
    Worked,
    Work(Range<usize>),
    Reply(usize),
    Card(usize),
    Working,
    Actions,
}

/// The rows of one turn. A finished turn folds its work behind a "Worked" header unless the
/// user `expanded` it; a `live` one shows everything as it happens.
pub fn turn_rows(turn: &Turn, items: &[TurnItem], live: bool, expanded: bool) -> Vec<Row> {
    let mut rows = Vec::new();
    if let Some(user) = turn.user {
        rows.push(Row::User(user));
    }
    let end = turn.items.end.min(items.len());
    let folds_until = turn.last_reply.filter(|reply| {
        !live
            && items
                .get(turn.items.start..*reply)
                .unwrap_or_default()
                .iter()
                .any(|item| classify(item) == Class::Work)
    });
    if folds_until.is_some() {
        rows.push(Row::Worked);
    }
    let shown = match folds_until {
        Some(reply) if !expanded => reply..end,
        _ => turn.items.start..end,
    };
    let mut group: Option<usize> = None;
    for index in shown.clone() {
        let Some(item) = items.get(index) else {
            break;
        };
        let class = classify(item);
        if class == Class::Hidden {
            continue;
        }
        if class == Class::Work {
            group.get_or_insert(index);
            continue;
        }
        if let Some(start) = group.take() {
            rows.push(Row::Work(start..index));
        }
        match class {
            Class::Reply => rows.push(Row::Reply(index)),
            Class::Card => rows.push(Row::Card(index)),
            Class::User | Class::Work | Class::Hidden => {}
        }
    }
    if let Some(start) = group {
        rows.push(Row::Work(start..shown.end));
    }
    if live {
        rows.push(Row::Working);
    } else if turn.last_reply.is_some() {
        rows.push(Row::Actions);
    }
    rows
}

/// What each step counts as in a group's one-line summary.
#[derive(Clone, Debug, PartialEq)]
pub enum Tally {
    Thought,
    Command,
    Edit(String),
    Read,
    Search,
    Fetch,
    Todo,
    Agent,
    Other,
}

pub struct Step {
    pub icon: IconName,
    pub label: SharedString,
    pub detail: String,
    pub tally: Tally,
}

pub fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The MCP server names T3 Code registers its own tools under.
const T3_MCP_SERVERS: [&str; 5] = [
    "t3-code",
    "t3_code",
    "t3code",
    "t3-code-direct",
    "t3_code_direct",
];

/// What a T3 Code tool does, as T3 Code's own apps describe it.
fn t3_tool_label(tool: &str) -> Option<(IconName, SharedString)> {
    Some(match tool {
        "link_pull_request" => (IconName::PullRequest, t!("Link a pull request")),
        "link_pull_requests" => (IconName::PullRequest, t!("Link pull requests")),
        "unlink_pull_request" => (IconName::PullRequest, t!("Unlink a pull request")),
        "unlink_pull_requests" => (IconName::PullRequest, t!("Unlink pull requests by state")),
        "list_thread_pull_requests" => (IconName::PullRequest, t!("Check linked pull requests")),
        "watch_pull_request" => (IconName::PullRequest, t!("Watch a pull request")),
        "unwatch_pull_request" => (IconName::PullRequest, t!("Stop watching a pull request")),
        "write_pull_request_guides" => (IconName::PullRequest, t!("Write review guides")),
        "read_pull_request_guide" => (IconName::PullRequest, t!("Read a review guide")),
        "list_issues" => (IconName::AgentWidget, t!("List issues")),
        "read_issue" => (IconName::AgentWidget, t!("Read an issue")),
        "create_issue" => (IconName::AgentWidget, t!("File an issue")),
        "comment_on_issue" => (IconName::AgentWidget, t!("Comment on an issue")),
        "set_issue_state" => (IconName::AgentWidget, t!("Change an issue's state")),
        "link_issue" => (IconName::AgentWidget, t!("Link an issue")),
        "unlink_issue" => (IconName::AgentWidget, t!("Unlink an issue")),
        "list_thread_issues" => (IconName::AgentWidget, t!("Check linked issues")),
        "pull_request_read" => (IconName::PullRequest, t!("Read a pull request")),
        "pull_request_list" => (IconName::PullRequest, t!("List pull requests")),
        "pull_request_create" => (IconName::PullRequest, t!("Open a pull request")),
        "pull_request_update" => (IconName::PullRequest, t!("Update a pull request")),
        "pull_request_respond" => (IconName::PullRequest, t!("Respond on a pull request")),
        "pull_request_review" => (IconName::PullRequest, t!("Review a pull request")),
        "ci_logs" => (IconName::PullRequest, t!("Read CI logs")),
        "ci_rerun" => (IconName::PullRequest, t!("Re-run CI")),
        "pull_request_merge" => (IconName::PullRequest, t!("Merge a pull request")),
        "orchestrator_capabilities" => {
            (IconName::AgentWidget, t!("Get orchestration capabilities"))
        }
        "delegate_task" => (IconName::AgentBot, t!("Delegate a child task")),
        "task_status" => (IconName::AgentWidget, t!("Get delegated task status")),
        "task_cancel" => (IconName::AgentWidget, t!("Cancel a delegated task")),
        "schedule_task" => (IconName::AgentWidget, t!("Schedule a task")),
        "list_scheduled_tasks" => (IconName::AgentWidget, t!("List scheduled tasks")),
        "update_scheduled_task" => (IconName::AgentWidget, t!("Update a scheduled task")),
        "delete_scheduled_task" => (IconName::AgentWidget, t!("Delete a scheduled task")),
        "request_secret" => (IconName::Lock, t!("Ask for a secret")),
        "create_threads" => (IconName::AgentWidget, t!("Create T3 threads")),
        "t3_thread_start" => (IconName::AgentWidget, t!("Start a T3 thread")),
        "t3_thread_list" => (IconName::AgentWidget, t!("List T3 threads")),
        "t3_thread_read" => (IconName::AgentWidget, t!("Read a T3 thread")),
        "t3_thread_send" => (IconName::AgentWidget, t!("Send to a T3 thread")),
        "t3_thread_wait" => (IconName::AgentWidget, t!("Wait for a T3 thread")),
        "t3_workflow_stop" => (IconName::AgentWidget, t!("Stop a workflow")),
        "t3_background_wait" => (IconName::AgentWidget, t!("Update background waits")),
        "t3_thread_interrupt" => (IconName::AgentWidget, t!("Interrupt a T3 thread")),
        "t3_worktree_handoff" => (
            IconName::AgentWidget,
            t!("Hand off thread to a git worktree"),
        ),
        "t3_worktree_status" => (IconName::AgentWidget, t!("Get thread worktree status")),
        "preview_status" => (IconName::AgentGlobal, t!("Get preview browser status")),
        "preview_open" => (
            IconName::AgentGlobal,
            t!("Open a page in the preview browser"),
        ),
        "preview_navigate" => (IconName::AgentGlobal, t!("Navigate the preview browser")),
        "preview_dialog" => (
            IconName::AgentGlobal,
            t!("Respond to a preview browser dialog"),
        ),
        "preview_snapshot" => (
            IconName::AgentGlobal,
            t!("Take a snapshot of the preview page"),
        ),
        "preview_act" => (IconName::AgentGlobal, t!("Use the preview browser")),
        "preview_click" => (IconName::AgentGlobal, t!("Click in the preview browser")),
        "preview_press" => (
            IconName::AgentGlobal,
            t!("Press a key in the preview browser"),
        ),
        "preview_type" => (IconName::AgentGlobal, t!("Type in the preview browser")),
        "preview_hover" => (IconName::AgentGlobal, t!("Hover in the preview browser")),
        "preview_select" => (
            IconName::AgentGlobal,
            t!("Choose an option in the preview browser"),
        ),
        "preview_drag" => (IconName::AgentGlobal, t!("Drag in the preview browser")),
        "preview_upload" => (
            IconName::AgentGlobal,
            t!("Upload files to the preview browser"),
        ),
        "preview_scroll" => (IconName::AgentGlobal, t!("Scroll the preview browser")),
        "preview_resize" => (IconName::AgentGlobal, t!("Resize the preview browser")),
        "preview_evaluate" => (
            IconName::AgentGlobal,
            t!("Evaluate script in the preview browser"),
        ),
        "preview_wait_for" => (IconName::AgentGlobal, t!("Wait for the preview page")),
        "preview_set_appearance" => (IconName::AgentGlobal, t!("Set preview browser appearance")),
        "preview_recording_start" => (
            IconName::AgentGlobal,
            t!("Start recording the preview browser"),
        ),
        "preview_recording_stop" => (
            IconName::AgentGlobal,
            t!("Stop recording the preview browser"),
        ),
        "device_list" => (IconName::Screen, t!("List simulators and emulators")),
        "device_open" => (IconName::Screen, t!("Open a device in the Device panel")),
        "device_screenshot" => (IconName::Screen, t!("Take a screenshot of the device")),
        "device_close" => (IconName::Screen, t!("Close a device")),
        "run_scheduled_task_now" => (IconName::AgentWidget, t!("Run a scheduled task")),
        "t3_queue" => (IconName::AgentWidget, t!("Manage queued messages")),
        "t3_pending_request" => (IconName::AgentWidget, t!("Handle pending questions")),
        "t3_workflow" => (IconName::AgentWidget, t!("Use workflow runs")),
        "t3_submit_output" => (IconName::AgentWidget, t!("Submit the result")),
        "t3_queue_list" => (IconName::AgentWidget, t!("List queued messages")),
        "t3_queue_read" => (IconName::AgentWidget, t!("Read a queued message")),
        "t3_queue_edit" => (IconName::AgentWidget, t!("Edit a queued message")),
        "t3_queue_cancel" => (IconName::AgentWidget, t!("Cancel a queued run")),
        "t3_queue_reorder" => (IconName::AgentWidget, t!("Reorder a queued run")),
        "t3_queue_promote_to_steer" => (IconName::AgentWidget, t!("Steer with a queued message")),
        "t3_pending_request_list" => (IconName::AgentWidget, t!("List pending questions")),
        "t3_pending_request_read" => (IconName::AgentWidget, t!("Read pending questions")),
        "t3_pending_request_respond" => (IconName::AgentWidget, t!("Answer pending questions")),
        "t3_thread_configuration" => (IconName::AgentWidget, t!("Read thread configuration")),
        "t3_thread_configure" => (IconName::AgentWidget, t!("Use thread model settings")),
        "t3_thread_transfer" => (IconName::AgentWidget, t!("Use thread transfers")),
        "t3_thread_fork" => (IconName::AgentWidget, t!("Fork this thread")),
        "t3_thread_merge_back" => (IconName::AgentWidget, t!("Merge thread context")),
        "t3_thread_search" => (IconName::AgentWidget, t!("Search thread content")),
        "t3_thread_transfers" => (IconName::AgentWidget, t!("Read thread transfers")),
        "t3_thread_organize" => (IconName::AgentWidget, t!("Organize a thread")),
        "t3_thread_update" => (IconName::AgentWidget, t!("Update T3 thread metadata")),
        "t3_worktree_list" => (IconName::AgentWidget, t!("List workspace branches")),
        "t3_preview_list" => (IconName::AgentGlobal, t!("List preview tabs")),
        "t3_preview_close" => (IconName::AgentGlobal, t!("Close a preview tab")),
        "t3_environment_read" => (IconName::AgentWidget, t!("Read environment preferences")),
        "t3_environment_links" => (IconName::AgentWidget, t!("List linked environments")),
        "t3_environment_link" => (IconName::AgentWidget, t!("Ask to link an environment")),
        "t3_environment_unlink" => (IconName::AgentWidget, t!("Forget a linked environment")),
        "t3_environment_preferences_update" => {
            (IconName::AgentWidget, t!("Update environment preferences"))
        }
        "t3_theme_read" => (IconName::AgentWidget, t!("Read themes")),
        "t3_theme_set" => (IconName::AgentWidget, t!("Set the theme")),
        "t3_thread_launch" => (IconName::AgentWidget, t!("Launch a project thread")),
        "t3_project_list" => (IconName::AgentWidget, t!("List projects")),
        "t3_project_read" => (IconName::AgentWidget, t!("Read a project")),
        "t3_project_create" => (IconName::AgentWidget, t!("Register a project")),
        "t3_project_update" => (IconName::AgentWidget, t!("Update a project")),
        "t3_project_delete" => (IconName::AgentWidget, t!("Delete a project")),
        "t3_project_clone" => (IconName::AgentWidget, t!("Clone a repository")),
        "t3_attachment_prepare_upload" => {
            (IconName::AgentWidget, t!("Prepare an attachment upload"))
        }
        "t3_attachment_discard" => (IconName::AgentWidget, t!("Discard a pending attachment")),
        "t3_thread_send_attachments" => (IconName::AgentWidget, t!("Send attachments")),
        "html_preview" => (IconName::AgentDocument, t!("Preview an HTML page")),
        "html_render" => (IconName::AgentDocument, t!("Render an HTML page")),
        "send_to_user" => (IconName::AgentChatRoundLine, t!("Send a message")),
        _ => return None,
    })
}

/// Strips how a provider prefixes T3 Code's tools, such as `t3-code.` or `mcp_t3code_`.
fn strip_t3_prefix(name: &str) -> Option<&str> {
    let lower = name.to_ascii_lowercase();
    let mut rest = lower.as_str();
    for prefix in ["mcp__", "mcp--", "mcp_-", "mcp-_", "mcp_", "mcp-"] {
        if let Some(stripped) = rest.strip_prefix(prefix) {
            rest = stripped;
            break;
        }
    }
    rest = rest.strip_prefix("t3")?;
    rest = rest
        .strip_prefix(['-', '_', ' '])
        .unwrap_or(rest)
        .strip_prefix("code")?;
    rest = rest
        .strip_prefix("-direct")
        .or_else(|| rest.strip_prefix("_direct"))
        .unwrap_or(rest);
    let separated = rest.trim_start();
    let tool = if let Some(tool) = separated.strip_prefix('·') {
        tool.trim_start()
    } else if let Some(tool) = rest.strip_prefix("__") {
        tool
    } else {
        rest.strip_prefix(['-', '_', '.', ':', '/', ' '])?
    };
    let start = name.len().checked_sub(tool.len())?;
    name.get(start..)
}

/// The T3 Code tool a `dynamic_tool` item ran, such as `send_to_user`, however the provider
/// named it; `None` for any other tool.
pub fn t3_tool(item: &TurnItem) -> Option<String> {
    let name = item.str_field("toolName")?.trim();
    let name = name
        .strip_suffix(" completed")
        .or_else(|| name.strip_suffix(" complete"))
        .unwrap_or(name)
        .trim();
    let tool = if let Some(rest) = name
        .get(..5)
        .filter(|prefix| prefix.eq_ignore_ascii_case("mcp__"))
        .and_then(|_| name.get(5..))
    {
        let (server, tool) = rest.split_once("__")?;
        T3_MCP_SERVERS
            .contains(&server.to_ascii_lowercase().as_str())
            .then_some(tool)?
    } else if let Some(tool) = strip_t3_prefix(name).filter(|tool| t3_tool_label(tool).is_some()) {
        tool
    } else if t3_tool_label(name).is_some() {
        name
    } else if name.to_ascii_lowercase().starts_with("t3-code-") {
        // Longest known tool the name ends with, such as `t3-code-xyz_send_to_user`.
        name.match_indices('_')
            .map(|(position, _)| &name[position + 1..])
            .find(|tool| t3_tool_label(tool).is_some())?
    } else {
        return None;
    };
    t3_tool_label(tool)?;
    // Tools that take an `action` are described by it, like T3 Code does.
    let action = item
        .fields
        .get("input")
        .and_then(|input| input.get("action"))
        .and_then(Value::as_str);
    let specific = action.and_then(|action| match tool {
        "preview_act" => Some(format!("preview_{action}")),
        "t3_queue" => Some(format!("t3_queue_{action}")),
        "t3_pending_request" => Some(format!("t3_pending_request_{action}")),
        "t3_thread_transfer" => match action {
            "fork" => Some("t3_thread_fork".to_string()),
            "merge_back" => Some("t3_thread_merge_back".to_string()),
            "status" => Some("t3_thread_transfers".to_string()),
            _ => None,
        },
        _ => None,
    });
    Some(
        specific
            .filter(|specific| t3_tool_label(specific).is_some())
            .unwrap_or_else(|| tool.to_string()),
    )
}

/// Codex runs T3 Code's tools from a script under this name, so its output decides.
const CODE_MODE_TOOL: &str = "execute";
const OUTPUT_MARKER: &str = "\nCommand output:\n";
const MAX_RESULT_DEPTH: usize = 4;

/// The result object a tool call returned, digging through MCP content blocks and JSON text the
/// way T3 Code does, and whether the call failed.
fn tool_result(value: &Value, depth: usize) -> (Option<Map<String, Value>>, bool) {
    if depth > MAX_RESULT_DEPTH {
        return (None, false);
    }
    match value {
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(parsed) => tool_result(&parsed, depth + 1),
            Err(_) => (None, false),
        },
        Value::Array(entries) => {
            let mut data = None;
            let mut failed = false;
            for entry in entries {
                let Some(text) = entry.get("text") else {
                    continue;
                };
                let text = text.get("text").unwrap_or(text);
                let (entry_data, entry_failed) = tool_result(text, depth + 1);
                failed |= entry_failed;
                if data.is_none() {
                    data = entry_data;
                }
            }
            (data, failed)
        }
        Value::Object(object) => {
            let failed = object.get("isError").and_then(Value::as_bool) == Some(true)
                || object.get("is_error").and_then(Value::as_bool) == Some(true)
                || object.get("_tag").and_then(Value::as_str) == Some("OrchestratorMcpFailure")
                || object.get("error").is_some_and(|error| !error.is_null());
            let after_marker = object
                .get("output")
                .and_then(Value::as_str)
                .and_then(|output| output.split_once(OUTPUT_MARKER))
                .map(|(_, rest)| Value::String(rest.to_string()));
            let inner = object
                .get("structuredContent")
                .filter(|inner| !inner.is_null())
                .or_else(|| object.get("content").filter(|inner| !inner.is_null()))
                .or(after_marker.as_ref());
            match inner {
                Some(inner) => {
                    let (data, inner_failed) = tool_result(inner, depth + 1);
                    (data, failed || inner_failed)
                }
                None => (Some(object.clone()), failed),
            }
        }
        _ => (None, false),
    }
}

/// The field `key` of what a finished T3 Code tool returned, when the call succeeded.
fn t3_tool_output(item: &TurnItem, tool: &str, key: &str) -> Option<Value> {
    if item.kind != "dynamic_tool" || item.status != "completed" {
        return None;
    }
    let ran = item.str_field("toolName") == Some(CODE_MODE_TOOL)
        || t3_tool(item).as_deref() == Some(tool);
    if !ran {
        return None;
    }
    let (data, failed) = tool_result(item.fields.get("output")?, 0);
    if failed {
        return None;
    }
    data?.remove(key)
}

const MAX_SENT_MESSAGE_LENGTH: usize = 4000;

/// The message an agent sent with `send_to_user`, which reads as a reply.
pub fn sent_message(item: &TurnItem) -> Option<String> {
    let text = t3_tool_output(item, "send_to_user", "sentMessage")?
        .as_str()?
        .to_string();
    (!text.trim().is_empty() && text.chars().count() <= MAX_SENT_MESSAGE_LENGTH).then_some(text)
}

/// A page an agent rendered with `html_render`, stored as one of the thread's attachments.
#[derive(Clone, Debug, PartialEq)]
pub struct HtmlRender {
    pub attachment_id: String,
    pub title: String,
}

pub fn html_render(item: &TurnItem) -> Option<HtmlRender> {
    let render = t3_tool_output(item, "html_render", "htmlRender")?;
    let attachment_id = render.get("attachmentId")?.as_str()?.trim().to_string();
    let title = render.get("title")?.as_str()?.trim().to_string();
    render.get("height")?.as_f64()?;
    (!attachment_id.is_empty()).then(|| HtmlRender {
        attachment_id,
        title: if title.is_empty() {
            "HTML".to_string()
        } else {
            title.chars().take(200).collect()
        },
    })
}

/// The text of an item that reads as a reply.
pub fn reply_text(item: &TurnItem) -> Cow<'_, str> {
    match sent_message(item) {
        Some(text) => Cow::Owned(text),
        None => Cow::Borrowed(item.text()),
    }
}

/// One input of a `secret_request` card. A card without `fields` asks for a single secret, whose
/// key is empty.
#[derive(Clone, Debug, PartialEq)]
pub struct SecretField {
    pub key: String,
    pub label: String,
    pub placeholder: String,
    pub secret: bool,
}

pub fn secret_fields(item: &TurnItem) -> Vec<SecretField> {
    let Some(fields) = item.fields.get("fields").and_then(Value::as_array) else {
        return vec![SecretField {
            key: String::new(),
            label: item.str_field("label").unwrap_or("").to_string(),
            placeholder: item
                .str_field("placeholder")
                .map(str::to_string)
                .unwrap_or_else(|| t!("Paste the secret").to_string()),
            secret: true,
        }];
    };
    fields
        .iter()
        .filter_map(|field| {
            Some(SecretField {
                key: field.get("name")?.as_str()?.to_string(),
                label: field
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                placeholder: field
                    .get("placeholder")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                secret: field.get("secret").and_then(Value::as_bool) != Some(false),
            })
        })
        .collect()
}

/// The host of an http(s) page a secret card links to, or `None` for any other address.
pub fn web_page_host(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    (!host.is_empty() && !host.contains(char::is_whitespace)).then_some(host)
}

pub fn display_path(path: &str, root: &Path) -> String {
    Path::new(path)
        .strip_prefix(root)
        .map(|relative| relative.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string())
}

fn input_str<'a>(input: Option<&'a Value>, keys: &[&str]) -> Option<&'a str> {
    let input = input?;
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
}

/// The command inside a `bash -lc '…'` wrapper, which some agents put around everything they run.
pub fn unwrap_shell(command: &str) -> String {
    let trimmed = command.trim();
    for flag in [" -lc ", " -c "] {
        let Some((shell, rest)) = trimmed.split_once(flag) else {
            continue;
        };
        let program = shell.rsplit('/').next().unwrap_or(shell);
        if !matches!(program, "bash" | "zsh" | "sh") {
            continue;
        }
        if let Some(inner) = rest
            .strip_prefix('\'')
            .and_then(|rest| rest.strip_suffix('\''))
        {
            return inner.replace(r"'\''", "'");
        }
        if let Some(inner) = rest
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
        {
            return inner.replace(r#"\""#, "\"");
        }
    }
    trimmed.to_string()
}

pub fn is_workspace_setup(item: &TurnItem) -> bool {
    item.kind == "command_execution" && item.id.contains("workspace-preparation")
}

pub fn describe(item: &TurnItem, root: &Path) -> Step {
    let title = item
        .title
        .as_deref()
        .map(single_line)
        .filter(|title| !title.is_empty());
    match item.kind.as_str() {
        "reasoning" => Step {
            icon: IconName::AgentChatRoundLine,
            label: t!("Thought process"),
            detail: String::new(),
            tally: Tally::Thought,
        },
        "command_execution" if is_workspace_setup(item) => Step {
            icon: IconName::AgentFolder,
            label: title.map_or_else(|| t!("Workspace"), SharedString::from),
            detail: String::new(),
            tally: Tally::Other,
        },
        "command_execution" => Step {
            icon: IconName::AgentTerminal,
            label: t!("Run"),
            detail: single_line(&unwrap_shell(item.str_field("input").unwrap_or(""))),
            tally: Tally::Command,
        },
        "file_change" => {
            let path = item.str_field("fileName").unwrap_or("").to_string();
            let added = item
                .fields
                .get("changes")
                .and_then(Value::as_array)
                .is_some_and(|changes| {
                    changes.iter().any(|change| {
                        change.get("operation").and_then(Value::as_str) == Some("add")
                    })
                });
            Step {
                icon: if added {
                    IconName::AgentDocumentAdd
                } else {
                    IconName::AgentPen
                },
                label: if added { t!("Write") } else { t!("Edit") },
                detail: display_path(&path, root),
                tally: Tally::Edit(path),
            }
        }
        "file_search" => Step {
            icon: IconName::AgentMagnifer,
            label: t!("Search"),
            detail: single_line(item.str_field("pattern").unwrap_or("")),
            tally: Tally::Search,
        },
        "web_search" => {
            let patterns = item
                .fields
                .get("patterns")
                .and_then(Value::as_array)
                .map(|patterns| {
                    patterns
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" · ")
                })
                .unwrap_or_default();
            Step {
                icon: IconName::AgentGlobal,
                label: t!("Web"),
                detail: single_line(&patterns),
                tally: Tally::Search,
            }
        }
        "todo_list" => {
            let (done, total) = todo_progress(item);
            Step {
                icon: IconName::AgentChecklist,
                label: t!("Todo"),
                detail: t!("{done}/{total} done", done = done, total = total).to_string(),
                tally: Tally::Todo,
            }
        }
        "subagent" => Step {
            icon: IconName::AgentBot,
            label: t!("Agent"),
            detail: single_line(
                item.str_field("progress")
                    .filter(|progress| !progress.trim().is_empty() && item.is_running())
                    .or_else(|| item.str_field("prompt"))
                    .unwrap_or(""),
            ),
            tally: Tally::Agent,
        },
        "dynamic_tool" => describe_tool(item, title, root),
        _ => Step {
            icon: IconName::AgentWidget,
            label: t!("Tool"),
            detail: title.unwrap_or_default(),
            tally: Tally::Other,
        },
    }
}

fn describe_tool(item: &TurnItem, title: Option<String>, root: &Path) -> Step {
    let name = item.str_field("toolName").unwrap_or("");
    let input = item.fields.get("input");
    let path = |keys: &[&str]| {
        input_str(input, keys)
            .map(|path| display_path(path, root))
            .unwrap_or_default()
    };
    let text = |keys: &[&str]| input_str(input, keys).map(single_line).unwrap_or_default();
    let fallback = || title.clone().unwrap_or_default();
    if let Some((icon, label)) = t3_tool(item).and_then(|tool| t3_tool_label(&tool)) {
        return Step {
            icon,
            label,
            detail: text(&[
                "label",
                "title",
                "description",
                "url",
                "query",
                "prompt",
                "message",
                "name",
            ]),
            tally: Tally::Other,
        };
    }
    match name {
        "Read" | "read_file" | "view" => Step {
            icon: IconName::AgentDocument,
            label: t!("Read"),
            detail: path(&["file_path", "path"]),
            tally: Tally::Read,
        },
        "Grep" | "grep" => Step {
            icon: IconName::AgentMagnifer,
            label: t!("Search"),
            detail: text(&["pattern", "query"]),
            tally: Tally::Search,
        },
        "Glob" | "glob" | "LS" | "list_directory" => Step {
            icon: IconName::AgentFolderWithFiles,
            label: t!("Glob"),
            detail: text(&["pattern", "path"]),
            tally: Tally::Search,
        },
        "WebFetch" | "web_fetch" => Step {
            icon: IconName::AgentGlobal,
            label: t!("Fetch"),
            detail: text(&["url"]),
            tally: Tally::Fetch,
        },
        "WebSearch" => Step {
            icon: IconName::AgentGlobal,
            label: t!("Web"),
            detail: text(&["query"]),
            tally: Tally::Search,
        },
        "Task" | "Agent" => Step {
            icon: IconName::AgentBot,
            label: t!("Agent"),
            detail: text(&["description", "prompt"]),
            tally: Tally::Agent,
        },
        _ if name.starts_with("mcp__") => {
            let mut parts = name.trim_start_matches("mcp__").splitn(2, "__");
            let server = parts.next().unwrap_or("");
            let tool = parts.next().unwrap_or("");
            Step {
                icon: IconName::AgentWidget,
                label: "MCP".into(),
                detail: format!("{server} · {tool}"),
                tally: Tally::Other,
            }
        }
        "" => Step {
            icon: IconName::AgentWidget,
            label: t!("Tool"),
            detail: fallback(),
            tally: Tally::Other,
        },
        name => Step {
            icon: IconName::AgentWidget,
            label: t!("Tool"),
            detail: title.unwrap_or_else(|| name.to_string()),
            tally: Tally::Other,
        },
    }
}

pub fn todo_progress(item: &TurnItem) -> (usize, usize) {
    let steps = item
        .fields
        .get("steps")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let done = steps
        .iter()
        .filter(|step| step.get("status").and_then(Value::as_str) == Some("completed"))
        .count();
    (done, steps.len())
}

fn plural(count: usize, one: SharedString, many: SharedString) -> String {
    if count == 1 { one } else { many }.to_string()
}

/// One line for a group of steps, such as "Thought process · ran 2 commands · edited 1 file".
pub fn group_summary(steps: &[(Tally, bool)]) -> String {
    let mut thoughts = 0;
    let mut commands = 0;
    let mut edited: Vec<&str> = Vec::new();
    let mut reads = 0;
    let mut searches = 0;
    let mut fetches = 0;
    let mut todos = 0;
    let mut agents = 0;
    let mut other = 0;
    let mut failed = 0;
    for (tally, is_failed) in steps {
        if *is_failed {
            failed += 1;
        }
        match tally {
            Tally::Thought => thoughts += 1,
            Tally::Command => commands += 1,
            Tally::Edit(path) => {
                if !edited.contains(&path.as_str()) {
                    edited.push(path);
                }
            }
            Tally::Read => reads += 1,
            Tally::Search => searches += 1,
            Tally::Fetch => fetches += 1,
            Tally::Todo => todos += 1,
            Tally::Agent => agents += 1,
            Tally::Other => other += 1,
        }
    }
    let mut segments: Vec<String> = Vec::new();
    match thoughts {
        0 => {}
        1 => segments.push(t!("thought process").to_string()),
        count => segments.push(t!("thought {count} times", count = count).to_string()),
    }
    if commands > 0 {
        segments.push(plural(
            commands,
            t!("ran 1 command"),
            t!("ran {count} commands", count = commands),
        ));
    }
    if !edited.is_empty() {
        segments.push(plural(
            edited.len(),
            t!("edited 1 file"),
            t!("edited {count} files", count = edited.len()),
        ));
    }
    if reads > 0 {
        segments.push(plural(
            reads,
            t!("read 1 file"),
            t!("read {count} files", count = reads),
        ));
    }
    if searches > 0 {
        segments.push(plural(
            searches,
            t!("searched 1 time"),
            t!("searched {count} times", count = searches),
        ));
    }
    if fetches > 0 {
        segments.push(plural(
            fetches,
            t!("fetched 1 page"),
            t!("fetched {count} pages", count = fetches),
        ));
    }
    if todos > 0 {
        segments.push(t!("updated todos").to_string());
    }
    if agents > 0 {
        segments.push(plural(
            agents,
            t!("ran 1 agent"),
            t!("ran {count} agents", count = agents),
        ));
    }
    if other > 0 {
        segments.push(plural(
            other,
            t!("called 1 tool"),
            t!("called {count} tools", count = other),
        ));
    }
    if failed > 0 {
        segments.push(t!("{count} failed", count = failed).to_string());
    }
    let mut summary = segments.join(" · ");
    if let Some(first) = summary.chars().next() {
        let upper = first.to_uppercase().to_string();
        summary.replace_range(..first.len_utf8(), &upper);
    }
    summary
}

/// Cuts long text to whole lines so a step's details stay short until asked for in full.
#[derive(Debug, PartialEq)]
pub struct ClippedLines {
    pub lines: Vec<String>,
    pub hidden: usize,
}

pub fn clip_lines(text: &str, max_lines: usize) -> ClippedLines {
    let all = text.trim_end().lines().collect::<Vec<_>>();
    let hidden = all.len().saturating_sub(max_lines);
    ClippedLines {
        lines: all
            .into_iter()
            .take(max_lines)
            .map(|line| line.replace('\t', "    "))
            .collect(),
        hidden,
    }
}

/// Removes terminal color and cursor sequences, which some commands print even when piped.
pub fn strip_ansi(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            if character != '\r' {
                result.push(character);
            }
            continue;
        }
        match characters.peek() {
            Some('[') => {
                characters.next();
                for next in characters.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                characters.next();
                while let Some(next) = characters.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && characters.peek() == Some(&'\\') {
                        characters.next();
                        break;
                    }
                }
            }
            _ => {
                characters.next();
            }
        }
    }
    result
}

/// Readable text for a tool's input or output, which T3 Code passes through as JSON.
pub fn value_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(parts) if parts.iter().all(|part| part.get("text").is_some()) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(id: &str, run: &str, kind: &str, extra: Value) -> TurnItem {
        let mut value = json!({
            "id": id,
            "threadId": "thread",
            "runId": run,
            "type": kind,
            "status": "completed",
        });
        if let (Some(object), Value::Object(extra)) = (value.as_object_mut(), extra) {
            object.extend(extra);
        }
        serde_json::from_value(value).expect("valid turn item")
    }

    fn sample() -> Vec<TurnItem> {
        vec![
            item("u1", "r1", "user_message", json!({ "text": "hi" })),
            item("t1", "r1", "reasoning", json!({ "text": "hmm" })),
            item("c1", "r1", "command_execution", json!({ "input": "ls" })),
            item(
                "a1",
                "r1",
                "assistant_message",
                json!({ "text": "looking" }),
            ),
            item("c2", "r1", "command_execution", json!({ "input": "cat x" })),
            item("a2", "r1", "assistant_message", json!({ "text": "done" })),
            item(
                "k1",
                "r1",
                "checkpoint",
                json!({ "files": [{ "path": "x", "kind": "modified", "additions": 1, "deletions": 0 }] }),
            ),
            item("u2", "r2", "user_message", json!({ "text": "again" })),
            item("e1", "r2", "reasoning", json!({ "text": "" })),
            item("a3", "r2", "assistant_message", json!({ "text": "ok" })),
        ]
    }

    #[test]
    fn turns_start_at_each_user_message() {
        let items = sample();
        let turns = group_turns(&items);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].user, Some(0));
        assert_eq!(turns[0].items, 1..7);
        assert_eq!(turns[0].first_reply, Some(3));
        assert_eq!(turns[0].last_reply, Some(5));
        assert_eq!(turns[1].user, Some(7));
        assert_eq!(turns[1].last_reply, Some(9));
    }

    #[test]
    fn finished_turns_fold_work_before_the_last_reply() {
        let items = sample();
        let turns = group_turns(&items);
        assert_eq!(
            turn_rows(&turns[0], &items, false, false),
            vec![
                Row::User(0),
                Row::Worked,
                Row::Reply(5),
                Row::Card(6),
                Row::Actions
            ]
        );
        assert_eq!(
            turn_rows(&turns[0], &items, false, true),
            vec![
                Row::User(0),
                Row::Worked,
                Row::Work(1..3),
                Row::Reply(3),
                Row::Work(4..5),
                Row::Reply(5),
                Row::Card(6),
                Row::Actions
            ]
        );
    }

    #[test]
    fn live_turns_show_everything_and_a_working_row() {
        let items = sample();
        let turns = group_turns(&items);
        let rows = turn_rows(&turns[0], &items, true, false);
        assert!(!rows.contains(&Row::Worked));
        assert_eq!(rows.last(), Some(&Row::Working));
    }

    #[test]
    fn turns_without_work_do_not_fold() {
        let items = sample();
        let turns = group_turns(&items);
        assert_eq!(
            turn_rows(&turns[1], &items, false, false),
            vec![Row::User(7), Row::Reply(9), Row::Actions]
        );
    }

    #[test]
    fn summaries_count_each_kind_of_step() {
        let summary = group_summary(&[
            (Tally::Thought, false),
            (Tally::Command, false),
            (Tally::Command, true),
            (Tally::Edit("a".into()), false),
            (Tally::Edit("a".into()), false),
        ]);
        assert_eq!(
            summary,
            "Thought process · ran 2 commands · edited 1 file · 1 failed"
        );
    }

    #[test]
    fn terminal_sequences_are_removed() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m\r\n"), "red\n");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}text"), "text");
    }

    #[test]
    fn shell_wrappers_are_unwrapped() {
        assert_eq!(unwrap_shell("/bin/bash -lc 'ls -la'"), "ls -la");
        assert_eq!(unwrap_shell(r#"zsh -c "echo hi""#), "echo hi");
        assert_eq!(unwrap_shell(r"bash -lc 'echo '\''x'\'''"), "echo 'x'");
        assert_eq!(unwrap_shell("cargo test"), "cargo test");
    }

    #[test]
    fn clipping_counts_hidden_lines() {
        let clipped = clip_lines("a\nb\nc\n", 2);
        assert_eq!(clipped.lines, vec!["a", "b"]);
        assert_eq!(clipped.hidden, 1);
    }

    #[test]
    fn secret_cards_ask_for_one_value_or_named_fields() {
        let single = item(
            "s1",
            "r1",
            "secret_request",
            json!({ "label": "API key", "reason": "", "secretStatus": "pending" }),
        );
        let fields = secret_fields(&single);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].key, "");
        assert_eq!(fields[0].label, "API key");
        assert!(fields[0].secret);

        let login = item(
            "s2",
            "r1",
            "secret_request",
            json!({ "label": "Login", "reason": "", "secretStatus": "pending", "fields": [
                { "name": "email", "label": "Email", "secret": false },
                { "name": "password", "label": "Password", "placeholder": "••••" },
            ] }),
        );
        let fields = secret_fields(&login);
        assert_eq!(
            fields
                .iter()
                .map(|field| (field.key.as_str(), field.secret))
                .collect::<Vec<_>>(),
            vec![("email", false), ("password", true)]
        );
        assert_eq!(fields[1].placeholder, "••••");
        assert_eq!(classify(&login), Class::Card);
    }

    #[test]
    fn secret_cards_only_link_to_web_pages() {
        assert_eq!(
            web_page_host("https://github.com/settings/tokens"),
            Some("github.com")
        );
        assert_eq!(
            web_page_host("http://user@192.168.1.1:8080?x"),
            Some("192.168.1.1:8080")
        );
        assert_eq!(web_page_host("javascript:alert(1)"), None);
        assert_eq!(web_page_host("https://"), None);
    }

    fn tool(name: &str, status: &str, input: Value, output: Value) -> TurnItem {
        let mut item = item(
            "tool",
            "r1",
            "dynamic_tool",
            json!({ "toolName": name, "input": input, "output": output }),
        );
        item.status = status.to_string();
        item
    }

    #[test]
    fn t3_tools_are_recognized_however_providers_name_them() {
        for name in [
            "mcp__t3-code__send_to_user",
            "mcp__T3_Code__send_to_user",
            "t3-code.send_to_user",
            "t3-code · send_to_user",
            "mcp_t3code_send_to_user",
            "send_to_user",
            "t3-code-1234_send_to_user",
            "send_to_user completed",
        ] {
            let item = tool(name, "completed", json!({}), Value::Null);
            assert_eq!(t3_tool(&item).as_deref(), Some("send_to_user"), "{name}");
        }
        for name in [
            "mcp__github__send_to_user",
            "Read",
            "mcp__t3-code",
            "t3-codex.send_to_user",
        ] {
            let item = tool(name, "completed", json!({}), Value::Null);
            assert_eq!(t3_tool(&item), None, "{name}");
        }
        let act = tool(
            "mcp__t3-code__preview_act",
            "completed",
            json!({ "action": "click" }),
            Value::Null,
        );
        assert_eq!(t3_tool(&act).as_deref(), Some("preview_click"));
        let step = describe(&act, Path::new("/"));
        assert_eq!(step.label.as_ref(), "Click in the preview browser");
    }

    #[test]
    fn sent_messages_read_as_replies() {
        let claude = tool(
            "mcp__t3-code__send_to_user",
            "completed",
            json!({ "message": "Here it is" }),
            json!([{ "type": "text", "text": "{\"sentMessage\":\"Here **it** is\"}" }]),
        );
        assert_eq!(sent_message(&claude).as_deref(), Some("Here **it** is"));
        assert_eq!(classify(&claude), Class::Reply);
        assert_eq!(reply_text(&claude), "Here **it** is");

        let codex = tool(
            "t3-code.send_to_user",
            "completed",
            json!({}),
            json!({ "content": [], "structuredContent": { "sentMessage": "From Codex" } }),
        );
        assert_eq!(sent_message(&codex).as_deref(), Some("From Codex"));

        let failed = tool(
            "mcp__t3-code__send_to_user",
            "completed",
            json!({}),
            json!({ "isError": true, "structuredContent": { "sentMessage": "nope" } }),
        );
        assert_eq!(sent_message(&failed), None);
        assert_eq!(classify(&failed), Class::Work);

        // What the thread stream carries: the server's compact summary of the output.
        let streamed = tool(
            "mcp__t3-code__send_to_user",
            "completed",
            json!({}),
            json!({ "sentMessage": "Progress: done" }),
        );
        assert_eq!(sent_message(&streamed).as_deref(), Some("Progress: done"));
        let streamed_failure = tool(
            "mcp__t3-code__send_to_user",
            "completed",
            json!({}),
            json!({ "isError": true }),
        );
        assert_eq!(sent_message(&streamed_failure), None);

        let running = tool(
            "mcp__t3-code__send_to_user",
            "running",
            json!({}),
            Value::Null,
        );
        assert_eq!(classify(&running), Class::Work);
    }

    #[test]
    fn rendered_pages_become_cards() {
        let page = tool(
            "mcp__t3-code__html_render",
            "completed",
            json!({}),
            json!({ "structuredContent": { "htmlRender": {
                "attachmentId": "att-1", "title": " Sales chart ", "height": 480 } } }),
        );
        assert_eq!(
            html_render(&page),
            Some(HtmlRender {
                attachment_id: "att-1".into(),
                title: "Sales chart".into()
            })
        );
        assert_eq!(classify(&page), Class::Card);
    }
}
