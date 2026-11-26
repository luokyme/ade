//! Client-side state built from the shell and thread streams: a snapshot, then events applied
//! in sequence order. Every event carries the whole record it changes, so applying one replaces
//! the record with the same id.

use crate::protocol::{
    ChatAttachment, Checkpoint, ContextTokenUsage, Message, ProjectShell, ProviderThread,
    ProviderTurn, Run, RunAttempt, RuntimeRequest, ShellStreamItem, ThreadEvent, ThreadRecord,
    ThreadShell, ThreadStreamItem, TurnItem,
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

    /// The project's top-level threads: pinned first, in T3 Code's pin order, then running ones,
    /// then the rest. Outside the pinned group the thread the user wrote to most recently comes
    /// first. Sorting by `updated_at` would reshuffle running threads on every agent event.
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
                .then_with(|| {
                    if a.pinned_at.is_none() || b.pinned_at.is_none() {
                        return std::cmp::Ordering::Equal;
                    }
                    // Matches the T3 Code sidebar: keyed pins in key order, then pins without a
                    // key (made by older servers) newest first.
                    match (&a.pin_order_key, &b.pin_order_key) {
                        (Some(a_key), Some(b_key)) => a_key.cmp(b_key),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => b.created_at.cmp(&a.created_at),
                    }
                })
                .then_with(|| b.is_working().cmp(&a.is_working()))
                .then_with(|| b.last_activity_key().cmp(a.last_activity_key()))
                .then_with(|| a.id.cmp(&b.id))
        });
        threads
    }

    /// An order key that puts a newly pinned thread above every pinned thread, like T3 Code's
    /// sidebar does. `None` when the existing keys are malformed; pin without a key then.
    pub fn top_pin_order_key(&self) -> Option<String> {
        let first = self
            .threads
            .values()
            .filter(|thread| thread.pinned_at.is_some())
            .filter_map(|thread| thread.pin_order_key.as_deref())
            .min();
        pin_order_key_between(None, first)
    }
}

// Pin order keys are base-26 fractions written with `a`–`z`, optionally behind a `0` tier marker.
// This mirrors `pinOrderKeyBetween` in the T3 Code server so keys stay compatible.
const PIN_ORDER_TIER: u8 = b'0';
const PIN_ORDER_FIRST_DIGIT: u8 = b'a';
const PIN_ORDER_BASE: u8 = 26;

fn is_valid_pin_order_key(key: &[u8]) -> bool {
    let digits = key.strip_prefix(&[PIN_ORDER_TIER]).unwrap_or(key);
    digits
        .last()
        .is_some_and(|last| *last != PIN_ORDER_FIRST_DIGIT)
        && digits.iter().all(u8::is_ascii_lowercase)
}

/// A digit string strictly between `low` and `high`, read as fractions in (0, 1). An empty
/// bound is open on that side.
fn pin_order_midpoint(low: &[u8], high: &[u8]) -> Option<Vec<u8>> {
    if !high.is_empty() && low >= high {
        return None;
    }
    if !high.is_empty() {
        let mut common = 0;
        while Some(low.get(common).copied().unwrap_or(PIN_ORDER_FIRST_DIGIT))
            == high.get(common).copied()
        {
            common += 1;
        }
        if common > 0 {
            let mut key = high[..common].to_vec();
            key.extend(pin_order_midpoint(
                low.get(common..).unwrap_or_default(),
                &high[common..],
            )?);
            return Some(key);
        }
    }
    let low_digit = low.first().map_or(0, |digit| digit - PIN_ORDER_FIRST_DIGIT);
    let high_digit = high
        .first()
        .map_or(PIN_ORDER_BASE, |digit| digit - PIN_ORDER_FIRST_DIGIT);
    if high_digit > low_digit + 1 {
        return Some(vec![
            PIN_ORDER_FIRST_DIGIT + (low_digit + high_digit).div_ceil(2),
        ]);
    }
    if high.len() > 1 {
        return Some(vec![high[0]]);
    }
    let mut key = vec![PIN_ORDER_FIRST_DIGIT + low_digit];
    key.extend(pin_order_midpoint(low.get(1..).unwrap_or_default(), &[])?);
    Some(key)
}

fn pin_order_key_between(before: Option<&str>, after: Option<&str>) -> Option<String> {
    let low = before.unwrap_or_default().as_bytes();
    let high = after.unwrap_or_default().as_bytes();
    if (!low.is_empty() && !is_valid_pin_order_key(low))
        || (!high.is_empty() && !is_valid_pin_order_key(high))
        || (!high.is_empty() && low >= high)
    {
        return None;
    }
    let key = if let Some(high_digits) = high.strip_prefix(&[PIN_ORDER_TIER]) {
        let mut key = vec![PIN_ORDER_TIER];
        key.extend(pin_order_midpoint(
            low.get(1..).unwrap_or_default(),
            high_digits,
        )?);
        key
    } else if let Some(low_digits) = low.strip_prefix(&[PIN_ORDER_TIER]) {
        let mut key = vec![PIN_ORDER_TIER];
        key.extend(pin_order_midpoint(low_digits, &[])?);
        key
    } else {
        pin_order_midpoint(low, high)?
    };
    String::from_utf8(key).ok()
}

/// A row of history that came from another thread, such as the parent of a fork.
#[derive(Clone, Debug)]
pub struct InheritedItem {
    pub source_thread_id: String,
    pub item: TurnItem,
}

/// A message waiting in the thread's queue for its run to start.
pub struct QueuedMessage<'a> {
    pub run: &'a Run,
    pub text: &'a str,
    pub attachments: Vec<ChatAttachment>,
}

/// How full the model's context window is for the thread's current agent session.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextWindow {
    pub used_tokens: u64,
    pub max_tokens: Option<u64>,
    pub auto_compact_threshold: Option<u64>,
}

impl ContextWindow {
    pub fn used_fraction(&self) -> Option<f64> {
        let max_tokens = self.max_tokens.filter(|max_tokens| *max_tokens > 0)?;
        Some((self.used_tokens as f64 / max_tokens as f64).min(1.0))
    }

    fn from_usage(usage: &ContextTokenUsage) -> Self {
        Self {
            used_tokens: usage.used_tokens,
            max_tokens: usage.max_tokens,
            auto_compact_threshold: usage.auto_compact_threshold,
        }
    }

    fn after_compaction(item: &TurnItem, max_tokens: Option<u64>) -> Option<Self> {
        Some(Self {
            used_tokens: item.fields.get("afterTokenCount")?.as_u64()?,
            max_tokens,
            auto_compact_threshold: None,
        })
    }
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
    pub provider_threads: Vec<ProviderThread>,
    pub provider_turns: Vec<ProviderTurn>,
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
            provider_threads: Vec::new(),
            provider_turns: Vec::new(),
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
                self.provider_threads = projection.provider_threads;
                self.provider_turns = projection.provider_turns;
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
                upsert(&mut self.checkpoints, checkpoint, |checkpoint| {
                    &checkpoint.id
                })
            }
            ThreadEvent::ProviderThreadUpdated(provider_thread) => {
                if let Some(thread) = self.thread.as_mut()
                    && provider_thread.app_thread_id.as_deref() == Some(thread.id.as_str())
                    && !provider_thread.is_placeholder()
                {
                    thread.active_provider_thread_id = Some(provider_thread.id.clone());
                }
                upsert(
                    &mut self.provider_threads,
                    provider_thread,
                    |provider_thread| &provider_thread.id,
                )
            }
            ThreadEvent::ProviderTurnUpdated(mut provider_turn) => {
                // Status updates leave out the usage the turn already reported.
                if provider_turn.token_usage.is_none() {
                    provider_turn.token_usage = self
                        .provider_turns
                        .iter()
                        .find(|known| known.id == provider_turn.id)
                        .and_then(|known| known.token_usage.clone());
                }
                upsert(&mut self.provider_turns, provider_turn, |provider_turn| {
                    &provider_turn.id
                })
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

    fn active_provider_thread(&self) -> Option<&ProviderThread> {
        let active_id = self.thread.as_ref()?.active_provider_thread_id.as_deref()?;
        self.provider_threads
            .iter()
            .find(|provider_thread| provider_thread.id == active_id)
    }

    /// Mirrors T3 Code's composer meter: the newest usage the active session reported, unless a
    /// compaction finished after it.
    pub fn context_window(&self) -> Option<ContextWindow> {
        let active = self.active_provider_thread();
        let from_other_session = |item: &TurnItem| {
            active.is_some_and(|active| {
                item.str_field("providerThreadId")
                    .is_some_and(|provider_thread_id| provider_thread_id != active.id)
            })
        };
        let latest_usage = self
            .provider_turns
            .iter()
            .filter(|turn| {
                active.is_none_or(|active| turn.provider_thread_id.as_deref() == Some(&active.id))
            })
            .filter_map(|turn| turn.token_usage.as_ref())
            .fold(
                None,
                |newest: Option<&ContextTokenUsage>, usage| match newest {
                    Some(newest) if usage.updated_at < newest.updated_at => Some(newest),
                    _ => Some(usage),
                },
            );
        let session_usage = active.and_then(|active| {
            Some((
                active.context_usage.as_ref()?,
                active.updated_at.as_deref()?,
            ))
        });
        let compaction = self
            .timeline()
            .filter(|item| item.kind == "compaction" && !from_other_session(item))
            .collect::<Vec<_>>();
        let completed_compaction = compaction
            .iter()
            .rev()
            .find(|item| item.status == "completed");

        let reported_at = latest_usage
            .and_then(|usage| usage.updated_at.as_deref())
            .or(session_usage.map(|(_, updated_at)| updated_at));
        if let Some(item) = completed_compaction {
            let compacted_at = item
                .str_field("completedAt")
                .or(item.str_field("updatedAt"))
                .unwrap_or_default();
            if reported_at.is_none_or(|reported_at| compacted_at > reported_at) {
                let max_tokens = latest_usage
                    .and_then(|usage| usage.max_tokens)
                    .or(session_usage.and_then(|(usage, _)| usage.max_tokens));
                return ContextWindow::after_compaction(item, max_tokens);
            }
        }
        if let Some(usage) = latest_usage {
            return Some(ContextWindow::from_usage(usage));
        }
        if let Some((usage, _)) = session_usage {
            return Some(ContextWindow::from_usage(usage));
        }
        compaction
            .iter()
            .rev()
            .find_map(|item| ContextWindow::after_compaction(item, None))
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

    /// Queued runs in the order they will start, with the message each one will send.
    pub fn queue(&self) -> Vec<QueuedMessage<'_>> {
        let mut queued = self
            .runs
            .iter()
            .filter(|run| run.is_queued())
            .collect::<Vec<_>>();
        queued.sort_by_key(|run| (run.queue_position.unwrap_or(i64::MAX), run.ordinal));
        queued
            .into_iter()
            .map(|run| {
                let message_id = run.user_message_id.as_deref();
                let text = message_id
                    .and_then(|message_id| {
                        self.messages
                            .iter()
                            .rev()
                            .find(|message| message.id == message_id)
                    })
                    .map(|message| message.text.as_str())
                    .unwrap_or("");
                // Messages carry no files; the queued user message item does.
                let attachments = message_id
                    .and_then(|message_id| {
                        self.items.iter().rev().find(|item| {
                            item.kind == "user_message"
                                && item.str_field("messageId") == Some(message_id)
                        })
                    })
                    .and_then(|item| item.fields.get("attachments"))
                    .and_then(|attachments| {
                        serde_json::from_value::<Vec<ChatAttachment>>(attachments.clone())
                            .map_err(|error| {
                                log::warn!("unreadable queued message attachments: {error}")
                            })
                            .ok()
                    })
                    .unwrap_or_default();
                QueuedMessage {
                    run,
                    text,
                    attachments,
                }
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
            "messageId": "m2",
            "attachments": [{ "type": "file", "id": "f1", "name": "notes.txt",
                              "mimeType": "text/plain", "sizeBytes": 4 }],
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
        assert_eq!(queue[0].text, "later");
        assert_eq!(queue[0].attachments.len(), 1);
        assert_eq!(queue[0].attachments[0].id, "f1");
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
        let id = |run| {
            state
                .checkpoint_before_run(run)
                .map(|checkpoint| checkpoint.id.as_str())
        };
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

    #[test]
    fn pinned_threads_follow_their_order_keys() {
        let mut shell = ShellState::default();
        shell.apply(
            ShellStreamItem::parse(&json!({
                "kind": "snapshot",
                "snapshot": {
                    "snapshotSequence": 1,
                    "threads": [
                        { "id": "plain", "projectId": "p", "title": "Plain", "updatedAt": "2026-05-01",
                          "activeRunId": "r" },
                        { "id": "old-unkeyed", "projectId": "p", "title": "A", "createdAt": "2026-01-01",
                          "pinnedAt": "2026-04-01" },
                        { "id": "new-unkeyed", "projectId": "p", "title": "B", "createdAt": "2026-02-01",
                          "pinnedAt": "2026-04-01" },
                        { "id": "second", "projectId": "p", "title": "C", "createdAt": "2026-01-01",
                          "pinnedAt": "2026-03-01", "pinOrderKey": "n" },
                        { "id": "first", "projectId": "q", "title": "D", "createdAt": "2026-01-01",
                          "pinnedAt": "2026-03-02", "pinOrderKey": "h" },
                        { "id": "top", "projectId": "p", "title": "E", "createdAt": "2026-01-01",
                          "pinnedAt": "2026-03-03", "pinOrderKey": "d" },
                    ],
                    "projects": [],
                },
            }))
            .expect("parses"),
        );
        let ids = shell
            .project_threads("p")
            .iter()
            .map(|thread| thread.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            ["top", "second", "new-unkeyed", "old-unkeyed", "plain"]
        );
        // Like T3 Code, a new pin goes above the pins of every project.
        assert_eq!(shell.top_pin_order_key().as_deref(), Some("c"));
    }

    #[test]
    fn shell_threads_carry_their_background_tasks() {
        let mut shell = ShellState::default();
        shell.apply(
            ShellStreamItem::parse(&json!({
                "kind": "snapshot",
                "snapshot": {
                    "snapshotSequence": 1,
                    "threads": [
                        { "id": "t", "projectId": "p", "title": "T", "pendingBackgroundTasks": [
                            { "kind": "command", "taskId": "b1", "description": "Build release",
                              "command": "cargo build --release", "itemId": "item-1", "awaited": true },
                            { "kind": "subagent", "taskId": "s1", "childThreadId": "child" },
                            { "kind": "something-newer", "taskId": "x1" },
                        ] },
                        { "id": "idle", "projectId": "p", "title": "Idle" },
                    ],
                    "projects": [],
                },
            }))
            .expect("parses"),
        );
        let tasks = &shell.threads["t"].pending_background_tasks;
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].kind, "command");
        assert_eq!(tasks[0].command.as_deref(), Some("cargo build --release"));
        assert_eq!(tasks[0].item_id.as_deref(), Some("item-1"));
        assert_eq!(tasks[1].child_thread_id.as_deref(), Some("child"));
        assert!(shell.threads["idle"].pending_background_tasks.is_empty());
    }

    #[test]
    fn pin_order_keys_match_t3_code() {
        let top = |first: Option<&str>| pin_order_key_between(None, first);
        assert_eq!(top(None).as_deref(), Some("n"));
        assert_eq!(top(Some("n")).as_deref(), Some("h"));
        assert_eq!(top(Some("b")).as_deref(), Some("an"));
        assert_eq!(top(Some("an")).as_deref(), Some("ah"));
        assert_eq!(top(Some("ab")).as_deref(), Some("aan"));
        assert_eq!(top(Some("0b")).as_deref(), Some("0an"));
        assert_eq!(top(Some("A")), None);
        assert_eq!(top(Some("na")), None);
        assert_eq!(
            pin_order_key_between(Some("h"), Some("n")).as_deref(),
            Some("k")
        );
        assert_eq!(pin_order_key_between(Some("n"), None).as_deref(), Some("u"));
        assert_eq!(pin_order_key_between(Some("n"), Some("h")), None);

        let mut key = top(None);
        for _ in 0..200 {
            let next = top(key.as_deref()).expect("a key above any valid key");
            assert!(next < key.expect("valid key"));
            key = Some(next);
        }
    }

    #[test]
    fn context_window_follows_the_active_session_and_compactions() {
        let usage = |used: u64, at: &str| json!({ "usedTokens": used, "maxTokens": 200000, "updatedAt": at });
        let mut state = ThreadState::new("t1");
        state.apply(parse_thread(json!({
            "kind": "snapshot",
            "snapshotSequence": 10,
            "projection": {
                "thread": { "id": "t1", "title": "Play", "activeProviderThreadId": "pt1" },
                "providerThreads": [
                    { "id": "pt1", "appThreadId": "t1", "status": "idle" },
                    { "id": "pt0", "appThreadId": "t1", "status": "closed" },
                ],
                "providerTurns": [
                    { "id": "a", "providerThreadId": "pt1",
                      "tokenUsage": usage(50000, "2026-11-25T12:00:02.000Z") },
                    { "id": "b", "providerThreadId": "pt1",
                      "tokenUsage": usage(20000, "2026-11-25T12:00:01.000Z") },
                    { "id": "c", "providerThreadId": "pt0",
                      "tokenUsage": usage(190000, "2026-11-25T12:00:09.000Z") },
                ],
                "turnItems": [],
                "visibleTurnItems": [],
            },
        })));
        let window = state.context_window().expect("usage reported");
        assert_eq!(window.used_tokens, 50000);
        assert_eq!(window.used_fraction(), Some(0.25));

        // A status-only update keeps the usage the turn already reported.
        state.apply(event(
            11,
            "provider-turn.updated",
            json!({ "id": "a", "providerThreadId": "pt1", "status": "completed" }),
        ));
        assert_eq!(
            state.context_window().map(|window| window.used_tokens),
            Some(50000)
        );

        state.apply(event(
            12,
            "turn-item.updated",
            item(
                "compact",
                5,
                "compaction",
                json!({
                    "providerThreadId": "pt1", "afterTokenCount": 8000,
                    "completedAt": "2026-11-25T12:00:05.000Z",
                }),
            ),
        ));
        let window = state.context_window().expect("compaction reported");
        assert_eq!(
            (window.used_tokens, window.max_tokens),
            (8000, Some(200000))
        );

        state.apply(event(
            13,
            "provider-thread.updated",
            json!({ "id": "pt2", "appThreadId": "t1", "status": "active" }),
        ));
        assert_eq!(state.context_window(), None);
    }
}
