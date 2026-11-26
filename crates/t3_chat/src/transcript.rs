//! How a T3 Code timeline reads as a chat: turns that start at each message the user sent,
//! the agent's work folded into groups, and its replies between them.

use i18n::t;
use serde_json::Value;
use std::{ops::Range, path::Path};
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
        "approval_request" | "user_input_request" | "error" | "proposed_plan" => Class::Card,
        "system_notice" | "run_interrupt_request" | "run_interrupt_result" => {
            if notice_text(item).is_empty() {
                Class::Hidden
            } else {
                Class::Card
            }
        }
        "compaction" => Class::Card,
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
}
