//! Client-side state built from the shell and thread streams: a snapshot, then events applied
//! in sequence order. Every event carries the whole record it changes, so applying one replaces
//! the record with the same id.

use crate::protocol::{
    Checkpoint, Message, ProjectShell, Run, RunAttempt, RuntimeRequest, ShellStreamItem, ThreadEvent,
    ThreadRecord, ThreadShell, ThreadStreamItem, TurnItem,
};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    Changed,
    Unchanged,
    /// The server could not replay the gap since the last sequence; subscribe again.
    NeedsResubscribe,
}

#[derive(Clone, Debug, Default)]
pub struct ShellState {
    pub sequence: u64,
    pub synchronized: bool,
    pub projects: Vec<ProjectShell>,
    pub threads: HashMap<String, ThreadShell>,
    pub archived_threads: HashMap<String, ThreadShell>,
}

impl ShellState {
    pub fn apply(&mut self, item: ShellStreamItem) -> Applied {
        match item {
            ShellStreamItem::Synchronized => {
                self.synchronized = true;
                Applied::Changed
            }
            ShellStreamItem::SnapshotRequired => Applied::NeedsResubscribe,
            ShellStreamItem::Snapshot(snapshot) => {
                self.sequence = snapshot.snapshot_sequence;
                self.projects = snapshot.projects;
                self.threads = snapshot
                    .threads
                    .into_iter()
                    .map(|thread| (thread.id.clone(), thread))
                    .collect();
                self.archived_threads = snapshot
                    .archived_threads
                    .into_iter()
                    .map(|thread| (thread.id.clone(), thread))
                    .collect();
                Applied::Changed
            }
            ShellStreamItem::ProjectIdentities(snapshot) => {
                for project in snapshot.projects {
                    match self
                        .projects
                        .iter_mut()
                        .find(|known| known.id == project.id)
                    {
                        Some(known) => *known = project,
                        None => self.projects.push(project),
                    }
                }
                Applied::Changed
            }
            ShellStreamItem::ProjectUpdated { sequence, project } => {
                self.advance(sequence);
                match self
                    .projects
                    .iter_mut()
                    .find(|known| known.id == project.id)
                {
                    Some(known) => *known = project,
                    None => self.projects.push(project),
                }
                Applied::Changed
            }
            ShellStreamItem::ProjectRemoved {
                sequence,
                project_id,
            } => {
                self.advance(sequence);
                self.projects.retain(|project| project.id != project_id);
                Applied::Changed
            }
            ShellStreamItem::ThreadUpdated {
                sequence,
                archived,
                thread,
            } => {
                self.advance(sequence);
                if archived {
                    self.threads.remove(&thread.id);
                    self.archived_threads.insert(thread.id.clone(), thread);
                } else {
                    self.archived_threads.remove(&thread.id);
                    self.threads.insert(thread.id.clone(), thread);
                }
                Applied::Changed
            }
            ShellStreamItem::ThreadRemoved {
                sequence,
                archived,
                thread_id,
            } => {
                self.advance(sequence);
                if archived {
                    self.archived_threads.remove(&thread_id);
                } else {
                    self.threads.remove(&thread_id);
                }
                Applied::Changed
            }
            ShellStreamItem::Unknown(kind) => {
                log::debug!("ignoring T3 Code shell item {kind:?}");
                Applied::Unchanged
            }
        }
    }

    fn advance(&mut self, sequence: u64) {
        self.sequence = self.sequence.max(sequence);
    }

    pub fn project_for_root(&self, root: &std::path::Path) -> Option<&ProjectShell> {
        self.projects
            .iter()
            .find(|project| std::path::Path::new(&project.workspace_root) == root)
    }

    /// The project's top-level threads: pinned first, then running ones, then the rest. Within
    /// each group the thread the user wrote to most recently comes first. Sorting by `updated_at`
    /// would reshuffle running threads on every agent event.
    pub fn project_threads(&self, project_id: &str) -> Vec<&ThreadShell> {
        let mut threads = self
            .threads
            .values()
            .filter(|thread| {
                thread.project_id.as_deref() == Some(project_id)
                    && !thread.is_child()
                    && thread.deleted_at.is_none()
            })
            .collect::<Vec<_>>();
        threads.sort_by(|a, b| {
            b.pinned_at
                .is_some()
                .cmp(&a.pinned_at.is_some())
                .then_with(|| b.is_working().cmp(&a.is_working()))
                .then_with(|| b.last_activity_key().cmp(a.last_activity_key()))
                .then_with(|| a.id.cmp(&b.id))
        });
        threads
    }
}

/// A row of history that came from another thread, such as the parent of a fork.
#[derive(Clone, Debug)]
pub struct InheritedItem {
    pub source_thread_id: String,
    pub item: TurnItem,
}

#[derive(Clone, Debug)]
pub struct ThreadState {
    pub thread_id: String,
    pub sequence: u64,
    pub synchronized: bool,
    pub thread: Option<ThreadRecord>,
    pub runs: Vec<Run>,
    pub attempts: Vec<RunAttempt>,
    pub messages: Vec<Message>,
    pub runtime_requests: Vec<RuntimeRequest>,
    pub checkpoints: Vec<Checkpoint>,
    pub inherited: Vec<InheritedItem>,
    items: Vec<TurnItem>,
}

impl ThreadState {
    pub fn new(thread_id: impl Into<String>) -> Self {
        Self {
            thread_id: thread_id.into(),
            sequence: 0,
            synchronized: false,
            thread: None,
            runs: Vec::new(),
            attempts: Vec::new(),
            messages: Vec::new(),
            runtime_requests: Vec::new(),
            checkpoints: Vec::new(),
            inherited: Vec::new(),
            items: Vec::new(),
        }
    }

    pub fn has_snapshot(&self) -> bool {
        self.thread.is_some()
    }

    pub fn apply(&mut self, item: ThreadStreamItem) -> Applied {
        match item {
            ThreadStreamItem::Synchronized => {
                self.synchronized = true;
                Applied::Changed
            }
            ThreadStreamItem::Snapshot {
                sequence,
                projection,
            } => {
                let projection = *projection;
                self.sequence = sequence;
                self.thread = Some(projection.thread);
                self.runs = projection.runs;
                self.attempts = projection.attempts;
                self.messages = projection.messages;
                self.runtime_requests = projection.runtime_requests;
                self.checkpoints = projection.checkpoints;
                self.inherited = projection
                    .visible_turn_items
                    .into_iter()
                    .filter(|row| row.visibility != "local")
                    .map(|row| InheritedItem {
                        source_thread_id: row.source_thread_id,
                        item: row.item,
                    })
                    .collect();
                self.items = projection.turn_items;
                self.sort_items();
                Applied::Changed
            }
            ThreadStreamItem::Event {
                sequence,
                thread_id,
                event,
            } => {
                if thread_id != self.thread_id || (self.has_snapshot() && sequence <= self.sequence)
                {
                    return Applied::Unchanged;
                }
                self.sequence = sequence;
                self.apply_event(event)
            }
            ThreadStreamItem::Unknown(kind) => {
                log::debug!("ignoring T3 Code thread item {kind:?}");
                Applied::Unchanged
            }
        }
    }

    fn apply_event(&mut self, event: ThreadEvent) -> Applied {
        match event {
            ThreadEvent::ThreadChanged(thread) => self.thread = Some(thread),
            ThreadEvent::RunUpdated(run) => upsert(&mut self.runs, run, |run| &run.id),
            ThreadEvent::AttemptUpdated(attempt) => {
                upsert(&mut self.attempts, attempt, |attempt| &attempt.id)
            }
            ThreadEvent::MessageUpdated(message) => {
                upsert(&mut self.messages, message, |message| &message.id)
            }
            ThreadEvent::RuntimeRequestUpdated(request) => {
                upsert(&mut self.runtime_requests, request, |request| &request.id)
            }
            ThreadEvent::TurnItemUpdated(item) => {
                // Streaming updates land on the newest items, so search from the end.
                match self.items.iter().rposition(|known| known.id == item.id) {
                    Some(index) if self.items[index].ordinal == item.ordinal => {
                        self.items[index] = item;
                    }
                    Some(index) => {
                        self.items.remove(index);
                        self.items.push(item);
                        self.sort_items();
                    }
                    None => {
                        let in_order = self
                            .items
                            .last()
                            .is_none_or(|last| order_key(last) <= order_key(&item));
                        self.items.push(item);
                        if !in_order {
                            self.sort_items();
                        }
                    }
                }
            }
            ThreadEvent::CheckpointUpdated(checkpoint) => {
                upsert(&mut self.checkpoints, checkpoint, |checkpoint| &checkpoint.id)
            }
            ThreadEvent::Other(_) => return Applied::Unchanged,
        }
        Applied::Changed
    }

    fn sort_items(&mut self) {
        self.items.sort_by(|a, b| order_key(a).cmp(&order_key(b)));
    }

    fn run(&self, run_id: &str) -> Option<&Run> {
        self.runs.iter().rev().find(|run| run.id == run_id)
    }

    /// Whether T3 Code's timeline shows this item now.
    fn is_visible(&self, item: &TurnItem) -> bool {
        let Some(run) = item.run_id.as_deref().and_then(|run_id| self.run(run_id)) else {
            return true;
        };
        if run.status == "rolled_back" {
            return false;
        }
        // A queued message shows in the queue until its run starts.
        let queued_input = item.kind == "user_message"
            && item.str_field("inputIntent") == Some("queued_turn")
            && matches!(run.status.as_str(), "queued" | "cancelled");
        !queued_input && !self.is_superseded_interrupt(item) && !self.is_answered_interrupt(item)
    }

    /// The interrupt request and its result carry the same message, so only the result stays.
    fn is_answered_interrupt(&self, item: &TurnItem) -> bool {
        item.kind == "run_interrupt_request"
            && item.run_id.is_some()
            && self.items.iter().any(|candidate| {
                candidate.kind == "run_interrupt_result" && candidate.run_id == item.run_id
            })
    }

    fn is_superseded_interrupt(&self, item: &TurnItem) -> bool {
        if item.kind != "run_interrupt_result" {
            return false;
        }
        let (Some(run_id), Some(node_id)) = (item.run_id.as_deref(), item.node_id.as_deref())
        else {
            return false;
        };
        let superseded = self.attempts.iter().any(|attempt| {
            attempt.status == "superseded"
                && attempt.run_id == run_id
                && attempt.root_node_id.as_deref() == Some(node_id)
        });
        let requested = self.items.iter().any(|candidate| {
            candidate.kind == "run_interrupt_request" && candidate.run_id.as_deref() == Some(run_id)
        });
        superseded && !requested
    }

    /// This thread's own visible timeline, oldest first. Inherited rows come before it.
    pub fn timeline(&self) -> impl Iterator<Item = &TurnItem> {
        self.items.iter().filter(|item| self.is_visible(item))
    }

    pub fn item(&self, item_id: &str) -> Option<&TurnItem> {
        self.items.iter().rev().find(|item| item.id == item_id)
    }

    pub fn active_run(&self) -> Option<&Run> {
        self.runs.iter().rev().find(|run| run.is_active())
    }

    /// Queued runs in the order they will start, with the text each one will send.
    pub fn queue(&self) -> Vec<(&Run, &str)> {
        let mut queued = self
            .runs
            .iter()
            .filter(|run| run.is_queued())
            .collect::<Vec<_>>();
        queued.sort_by_key(|run| (run.queue_position.unwrap_or(i64::MAX), run.ordinal));
        queued
            .into_iter()
            .map(|run| {
                let text = run
                    .user_message_id
                    .as_deref()
                    .and_then(|message_id| {
                        self.messages
                            .iter()
                            .rev()
                            .find(|message| message.id == message_id)
                    })
                    .map(|message| message.text.as_str())
                    .unwrap_or("");
                (run, text)
            })
            .collect()
    }

    /// The checkpoint to return to so that `run_id` and everything after it is undone.
    pub fn checkpoint_before_run(&self, run_id: &str) -> Option<&Checkpoint> {
        let run = self.runs.iter().find(|run| run.id == run_id)?;
        let scope = self
            .checkpoints
            .iter()
            .find(|checkpoint| checkpoint.run_id.as_deref() == Some(run_id))
            .or_else(|| {
                self.checkpoints
                    .iter()
                    .find(|checkpoint| checkpoint.ordinal_within_scope == 0)
            })?
            .scope_id
            .clone();
        let target = run.ordinal.checked_sub(1)?;
        self.checkpoints.iter().find(|checkpoint| {
            checkpoint.scope_id == scope
                && checkpoint.ordinal_within_scope == target
                && checkpoint.status == "ready"
        })
    }

    pub fn pending_request(&self, request_id: &str) -> Option<&RuntimeRequest> {
        self.runtime_requests
            .iter()
            .rev()
            .find(|request| request.id == request_id && request.is_pending())
    }
}

fn order_key(item: &TurnItem) -> (i64, &str) {
    (item.ordinal, item.id.as_str())
}

fn upsert<T>(records: &mut Vec<T>, record: T, id: impl Fn(&T) -> &String) {
    match records.iter().position(|known| id(known) == id(&record)) {
        Some(index) => records[index] = record,
        None => records.push(record),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn parse_thread(value: Value) -> ThreadStreamItem {
        ThreadStreamItem::parse(&value).expect("parses")
    }

    fn item(id: &str, ordinal: i64, kind: &str, extra: Value) -> Value {
        let mut item = json!({
            "id": id, "threadId": "t1", "runId": "r1", "ordinal": ordinal,
            "status": "completed", "type": kind,
        });
        if let (Some(item), Value::Object(extra)) = (item.as_object_mut(), extra) {
            item.extend(extra);
        }
        item
    }

    fn snapshot(items: Vec<Value>, runs: Vec<Value>) -> ThreadStreamItem {
        parse_thread(json!({
            "kind": "snapshot",
            "snapshotSequence": 10,
            "projection": {
                "thread": { "id": "t1", "title": "Play" },
                "runs": runs,
                "turnItems": items,
                "visibleTurnItems": [],
            },
        }))
    }

    fn event(sequence: u64, kind: &str, payload: Value) -> ThreadStreamItem {
        parse_thread(json!({
            "kind": "event",
            "sequence": sequence,
            "event": { "type": kind, "threadId": "t1", "payload": payload },
        }))
    }

    fn timeline_ids(state: &ThreadState) -> Vec<String> {
        state.timeline().map(|item| item.id.clone()).collect()
    }

    #[test]
    fn streamed_items_replace_their_earlier_version_in_place() {
        let mut state = ThreadState::new("t1");
        state.apply(snapshot(
            vec![item("a", 1, "user_message", json!({ "text": "hi" }))],
            vec![json!({ "id": "r1", "ordinal": 1, "status": "running" })],
        ));
        state.apply(event(
            11,
            "turn-item.updated",
            item(
                "b",
                2,
                "assistant_message",
                json!({ "text": "Hel", "streaming": true }),
            ),
        ));
        state.apply(event(
            12,
            "turn-item.updated",
            item(
                "b",
                2,
                "assistant_message",
                json!({ "text": "Hello", "streaming": false }),
            ),
        ));
        assert_eq!(timeline_ids(&state), ["a", "b"]);
        assert_eq!(state.item("b").map(TurnItem::text), Some("Hello"));
    }

    #[test]
    fn replayed_and_foreign_events_are_ignored() {
        let mut state = ThreadState::new("t1");
        state.apply(snapshot(
            vec![item("a", 1, "user_message", json!({}))],
            vec![],
        ));
        let stale = event(9, "turn-item.updated", item("z", 0, "reasoning", json!({})));
        assert_eq!(state.apply(stale), Applied::Unchanged);
        let foreign = parse_thread(json!({
            "kind": "event", "sequence": 30,
            "event": { "type": "turn-item.updated", "threadId": "other",
                       "payload": item("y", 5, "reasoning", json!({})) },
        }));
        assert_eq!(state.apply(foreign), Applied::Unchanged);
        assert_eq!(timeline_ids(&state), ["a"]);
    }

    #[test]
    fn out_of_order_items_are_placed_by_ordinal() {
        let mut state = ThreadState::new("t1");
        state.apply(snapshot(
            vec![
                item("a", 1, "user_message", json!({})),
                item("c", 3, "assistant_message", json!({})),
            ],
            vec![],
        ));
        state.apply(event(
            11,
            "turn-item.updated",
            item("b", 2, "reasoning", json!({})),
        ));
        assert_eq!(timeline_ids(&state), ["a", "b", "c"]);
    }

    #[test]
    fn a_stopped_run_shows_one_notice() {
        let mut state = ThreadState::new("t1");
        let notice = json!({ "message": "Stopped by user" });
        state.apply(snapshot(
            vec![
                item("a", 1, "user_message", json!({})),
                item("request", 2, "run_interrupt_request", notice.clone()),
            ],
            vec![json!({ "id": "r1", "ordinal": 1, "status": "running" })],
        ));
        assert_eq!(timeline_ids(&state), ["a", "request"]);

        state.apply(event(
            11,
            "turn-item.updated",
            item("result", 3, "run_interrupt_result", notice),
        ));
        assert_eq!(timeline_ids(&state), ["a", "result"]);
    }

    #[test]
    fn queued_messages_leave_the_timeline_until_their_run_starts() {
        let mut state = ThreadState::new("t1");
        let queued_message = json!({
            "id": "q", "threadId": "t1", "runId": "r2", "ordinal": 5, "status": "completed",
            "type": "user_message", "inputIntent": "queued_turn", "text": "later",
        });
        state.apply(snapshot(
            vec![item("a", 1, "user_message", json!({})), queued_message],
            vec![
                json!({ "id": "r1", "ordinal": 1, "status": "running" }),
                json!({ "id": "r2", "ordinal": 2, "status": "queued", "queuePosition": 0,
                        "userMessageId": "m2" }),
            ],
        ));
        state.apply(event(
            11,
            "message.updated",
            json!({ "id": "m2", "role": "user", "text": "later" }),
        ));
        assert_eq!(timeline_ids(&state), ["a"]);
        let queue = state.queue();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].1, "later");
        assert_eq!(state.active_run().map(|run| run.id.as_str()), Some("r1"));

        state.apply(event(
            12,
            "run.updated",
            json!({ "id": "r2", "ordinal": 2, "status": "running" }),
        ));
        assert_eq!(timeline_ids(&state), ["a", "q"]);
        assert!(state.queue().is_empty());
    }

    #[test]
    fn rewinding_a_run_returns_to_the_checkpoint_before_it() {
        let mut state = ThreadState::new("t1");
        state.apply(snapshot(
            vec![],
            vec![
                json!({ "id": "r1", "ordinal": 1, "status": "completed" }),
                json!({ "id": "r2", "ordinal": 2, "status": "completed" }),
            ],
        ));
        let checkpoint = |ordinal: u64, run: Option<&str>| {
            json!({ "id": format!("c{ordinal}"), "scopeId": "root", "runId": run,
                    "ordinalWithinScope": ordinal, "status": "ready" })
        };
        for (sequence, payload) in [
            (11, checkpoint(0, None)),
            (12, checkpoint(1, Some("r1"))),
            (13, checkpoint(2, Some("r2"))),
        ] {
            state.apply(event(sequence, "checkpoint.captured", payload));
        }
        let id = |run| state.checkpoint_before_run(run).map(|checkpoint| checkpoint.id.as_str());
        assert_eq!(id("r1"), Some("c0"));
        assert_eq!(id("r2"), Some("c1"));
        assert_eq!(id("missing"), None);
    }

    #[test]
    fn rolled_back_runs_hide_their_items() {
        let mut state = ThreadState::new("t1");
        state.apply(snapshot(
            vec![item("a", 1, "user_message", json!({}))],
            vec![json!({ "id": "r1", "ordinal": 1, "status": "completed" })],
        ));
        state.apply(event(
            11,
            "run.updated",
            json!({ "id": "r1", "ordinal": 1, "status": "rolled_back" }),
        ));
        assert!(timeline_ids(&state).is_empty());
    }

    #[test]
    fn shell_state_lists_a_projects_own_threads() {
        let mut shell = ShellState::default();
        shell.apply(
            ShellStreamItem::parse(&json!({
                "kind": "snapshot",
                "snapshot": {
                    "snapshotSequence": 3,
                    "threads": [
                        { "id": "old", "projectId": "p", "title": "Old", "updatedAt": "2026-09-01",
                          "latestUserMessageAt": "2026-01-01" },
                        { "id": "new", "projectId": "p", "title": "New", "updatedAt": "2026-02-01",
                          "latestUserMessageAt": "2026-02-01" },
                        { "id": "busy", "projectId": "p", "title": "Busy", "updatedAt": "2026-01-01",
                          "activeRunId": "r", "latestUserMessageAt": "2026-01-15" },
                        { "id": "busier", "projectId": "p", "title": "Busier", "updatedAt": "2026-01-01",
                          "activeRunId": "r", "latestUserMessageAt": "2026-01-20" },
                        { "id": "pin", "projectId": "p", "title": "Pin", "updatedAt": "2025-01-01",
                          "pinnedAt": "2026-01-02" },
                        { "id": "child", "projectId": "p", "title": "Sub", "updatedAt": "2026-03-01",
                          "lineage": { "parentThreadId": "new" } },
                        { "id": "elsewhere", "projectId": "q", "title": "Q", "updatedAt": "2026-03-01" },
                    ],
                    "projects": [{ "id": "p", "title": "P", "workspaceRoot": "/work/p" }],
                },
            }))
            .expect("parses"),
        );
        let ids = shell
            .project_threads("p")
            .iter()
            .map(|thread| thread.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["pin", "busier", "busy", "new", "old"]);
        assert_eq!(
            shell
                .project_for_root(std::path::Path::new("/work/p"))
                .map(|project| project.id.as_str()),
            Some("p")
        );

        shell.apply(
            ShellStreamItem::parse(&json!({
                "kind": "thread.updated", "sequence": 4, "location": "archive",
                "thread": { "id": "old", "projectId": "p", "title": "Old" },
            }))
            .expect("parses"),
        );
        assert!(!shell.threads.contains_key("old"));
        assert!(shell.archived_threads.contains_key("old"));
        assert_eq!(shell.sequence, 4);

        // The server follows its snapshot with one that only fills in repository identities.
        shell.apply(
            ShellStreamItem::parse(&json!({
                "kind": "snapshot",
                "resolvedRepositoryIdentityRoots": ["/work/p"],
                "snapshot": {
                    "snapshotSequence": 4,
                    "threads": [],
                    "archivedThreads": [],
                    "projects": [{ "id": "p", "title": "P renamed", "workspaceRoot": "/work/p" }],
                },
            }))
            .expect("parses"),
        );
        assert_eq!(shell.project_threads("p").len(), 4);
        assert_eq!(shell.projects[0].title, "P renamed");
    }
}
