//! Suggestions while typing: `@` mentions a project file, `/` runs one of the agent's commands.

use super::ThreadView;
use crate::style::{icon, ink, ui};
use editor::{MultiBufferOffset, SelectionEffects};
use file_icons::FileIcons;
use gpui::{AnyElement, App, ClickEvent, Context, SharedString, Window};
use i18n::t;
use project::{Candidates, PathMatchCandidateSet};
use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use theme::ActiveTheme as _;
use ui::prelude::*;
use util::ResultExt as _;

const MAX_RESULTS: usize = 40;
const VISIBLE_ROWS: usize = 8;
const ROW_HEIGHT: f32 = 28.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Command,
    Mention,
}

impl TokenKind {
    fn prefix(self) -> char {
        match self {
            TokenKind::Command => '/',
            TokenKind::Mention => '@',
        }
    }

    fn accepts(self, character: char) -> bool {
        match self {
            TokenKind::Mention => !character.is_whitespace() && !matches!(character, '@' | '`'),
            TokenKind::Command => {
                character.is_alphanumeric() || matches!(character, '-' | '_' | ':' | '.')
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub range: Range<usize>,
    pub query: String,
}

/// The `/command` or `@path` the cursor is in, if any.
pub fn token_at(text: &str, cursor: usize) -> Option<(TokenKind, Token)> {
    [TokenKind::Command, TokenKind::Mention]
        .into_iter()
        .find_map(|kind| token_of_kind(text, cursor, kind).map(|token| (kind, token)))
}

fn token_of_kind(text: &str, cursor: usize, kind: TokenKind) -> Option<Token> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    let start = text[..cursor]
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace() || matches!(character, '(' | '[' | '{'))
        .map_or(0, |(index, character)| index + character.len_utf8());
    let query = text[start..cursor].strip_prefix(kind.prefix())?;
    if !query.chars().all(|character| kind.accepts(character)) {
        return None;
    }
    let end = text[start + 1..]
        .char_indices()
        .find(|(_, character)| !kind.accepts(*character))
        .map_or(text.len(), |(offset, _)| start + 1 + offset);
    // A slash right after the name means a path such as /usr/bin, not a command.
    if kind == TokenKind::Command && text[end..].starts_with('/') {
        return None;
    }
    Some(Token {
        range: start..end,
        query: query.to_string(),
    })
}

#[derive(Clone)]
pub enum ItemIcon {
    Command,
    Skill,
    Path { path: PathBuf, is_dir: bool },
}

#[derive(Clone)]
pub struct Item {
    pub label: String,
    pub detail: String,
    pub insertion: String,
    pub icon: ItemIcon,
}

pub struct Completion {
    pub kind: TokenKind,
    pub token: Token,
    pub selected: usize,
    pub items: Vec<Item>,
}

fn matches_query(name: &str, query: &str) -> bool {
    query.is_empty() || name.to_lowercase().contains(&query.to_lowercase())
}

fn mention_text(path: &Path, root: &Path, is_dir: bool) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    let text = if relative.contains(char::is_whitespace) {
        format!("@\"{relative}\"")
    } else {
        format!("@{relative}")
    };
    if is_dir { format!("{text}/") } else { text }
}

impl ThreadView {
    pub(super) fn update_completion(&mut self, cx: &mut Context<Self>) {
        let found = self.composer.update(cx, |editor, cx| {
            let snapshot = editor.display_snapshot(cx);
            let selection = editor.selections.newest::<MultiBufferOffset>(&snapshot);
            if !selection.is_empty() {
                return None;
            }
            token_at(&editor.text(cx), selection.head().0)
        });
        let Some((kind, token)) = found else {
            self.dismissed_token = None;
            if self.completion.take().is_some() {
                cx.notify();
            }
            return;
        };
        if self.dismissed_token.as_ref() == Some(&token) {
            return;
        }
        self.dismissed_token = None;
        if let Some(completion) = &self.completion
            && completion.kind == kind
            && completion.token == token
        {
            return;
        }
        let selected = match &self.completion {
            Some(completion) if completion.kind == kind && completion.token.query == token.query => {
                completion.selected
            }
            _ => 0,
        };
        match kind {
            TokenKind::Command => {
                let items = self
                    .command_items(cx)
                    .into_iter()
                    .filter(|item| matches_query(&item.label, &token.query))
                    .take(MAX_RESULTS)
                    .collect();
                self.completion = Some(Completion {
                    kind,
                    token,
                    selected,
                    items,
                });
            }
            TokenKind::Mention => {
                let previous = self
                    .completion
                    .take()
                    .filter(|completion| completion.kind == TokenKind::Mention)
                    .map(|completion| completion.items)
                    .unwrap_or_default();
                self.search_files(token.query.clone(), cx);
                self.completion = Some(Completion {
                    kind,
                    token,
                    selected,
                    items: previous,
                });
            }
        }
        cx.notify();
    }

    /// The agent's slash commands, with Claude's skills, which Claude also runs as `/name`.
    fn command_items(&self, cx: &App) -> Vec<Item> {
        let Some(provider) = self.current_provider(cx) else {
            return Vec::new();
        };
        let root = self.worktree_root().to_string_lossy().into_owned();
        let snapshots = provider
            .workspace_snapshots
            .iter()
            .filter(|snapshot| snapshot.cwd == root || snapshot.cwd == self.project_root.to_string_lossy());
        let mut items: Vec<Item> = Vec::new();
        let mut push = |name: &str, detail: Option<&str>, icon: ItemIcon| {
            if name.is_empty() || items.iter().any(|item| item.label == format!("/{name}")) {
                return;
            }
            items.push(Item {
                label: format!("/{name}"),
                detail: detail.unwrap_or_default().to_string(),
                insertion: format!("/{name}"),
                icon,
            });
        };
        for command in snapshots
            .clone()
            .flat_map(|snapshot| snapshot.slash_commands.iter())
            .chain(provider.slash_commands.iter())
        {
            push(&command.name, command.description.as_deref(), ItemIcon::Command);
        }
        if matches!(provider.driver.as_str(), "claudeAgent" | "claude") {
            for skill in snapshots
                .flat_map(|snapshot| snapshot.skills.iter())
                .chain(provider.skills.iter())
                .filter(|skill| skill.enabled && skill.user_invocable != Some(false))
            {
                push(
                    &skill.name,
                    skill
                        .short_description
                        .as_deref()
                        .or(skill.description.as_deref()),
                    ItemIcon::Skill,
                );
            }
        }
        items.sort_by_key(|item| item.label.to_lowercase());
        items
    }

    fn search_files(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(project) = self
            .workspace
            .upgrade()
            .map(|workspace| workspace.read(cx).project().clone())
        else {
            return;
        };
        let project = project.read(cx);
        let worktrees = project.visible_worktrees(cx).collect::<Vec<_>>();
        let roots = worktrees
            .iter()
            .map(|worktree| {
                let worktree = worktree.read(cx);
                (worktree.id().to_usize(), worktree.abs_path().to_path_buf())
            })
            .collect::<Vec<_>>();
        let candidate_sets = worktrees
            .iter()
            .map(|worktree| PathMatchCandidateSet {
                snapshot: worktree.read(cx).snapshot(),
                include_ignored: false,
                include_root_name: false,
                candidates: Candidates::Entries,
            })
            .collect::<Vec<_>>();
        let root = self.worktree_root();
        self.file_search = cx.spawn(async move |this, cx| {
            let cancel = AtomicBool::new(false);
            let matches = fuzzy_nucleo::match_path_sets(
                candidate_sets.as_slice(),
                &query,
                &None,
                fuzzy_nucleo::Case::Ignore,
                MAX_RESULTS,
                &cancel,
                cx.background_executor().clone(),
            )
            .await;
            let items = matches
                .into_iter()
                .filter_map(|found| {
                    let worktree_root = roots
                        .iter()
                        .find(|(id, _)| *id == found.worktree_id)
                        .map(|(_, root)| root)?;
                    let path = worktree_root.join(found.path.as_std_path());
                    let relative = path.strip_prefix(&root).unwrap_or(&path).to_path_buf();
                    let label = relative
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| relative.to_string_lossy().into_owned());
                    let detail = relative
                        .parent()
                        .map(|parent| parent.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    Some(Item {
                        label,
                        detail,
                        insertion: mention_text(&path, &root, found.is_dir),
                        icon: ItemIcon::Path {
                            path,
                            is_dir: found.is_dir,
                        },
                    })
                })
                .collect::<Vec<_>>();
            this.update(cx, |this, cx| {
                if let Some(completion) = this.completion.as_mut()
                    && completion.kind == TokenKind::Mention
                    && completion.token.query == query
                {
                    completion.items = items;
                    completion.selected = completion.selected.min(completion.items.len().saturating_sub(1));
                    cx.notify();
                }
            })
            .log_err();
        });
    }

    pub(super) fn completion_visible(&self) -> bool {
        self.completion
            .as_ref()
            .is_some_and(|completion| !completion.items.is_empty())
    }

    pub(super) fn select_next_completion(&mut self, cx: &mut Context<Self>) {
        if let Some(completion) = self.completion.as_mut()
            && !completion.items.is_empty()
        {
            completion.selected = (completion.selected + 1) % completion.items.len();
            cx.notify();
        }
    }

    pub(super) fn select_previous_completion(&mut self, cx: &mut Context<Self>) {
        if let Some(completion) = self.completion.as_mut()
            && !completion.items.is_empty()
        {
            completion.selected = completion
                .selected
                .checked_sub(1)
                .unwrap_or(completion.items.len() - 1);
            cx.notify();
        }
    }

    pub(super) fn dismiss_completion(&mut self, cx: &mut Context<Self>) {
        if let Some(completion) = self.completion.take() {
            self.dismissed_token = Some(completion.token);
            cx.notify();
        }
    }

    pub(super) fn accept_completion(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        let Some(item) = completion
            .items
            .get(index.unwrap_or(completion.selected))
            .cloned()
        else {
            return;
        };
        let range = completion.token.range;
        let text = self.composer.read(cx).text(cx);
        let followed_by_space = text
            .get(range.end..)
            .and_then(|rest| rest.chars().next())
            .is_some_and(char::is_whitespace);
        // A folder keeps the menu open so the next path segment can be picked.
        let keeps_going = matches!(item.icon, ItemIcon::Path { is_dir: true, .. });
        let insertion = if followed_by_space || keeps_going {
            item.insertion
        } else {
            format!("{} ", item.insertion)
        };
        self.composer.update(cx, |editor, cx| {
            let cursor = range.start + insertion.len();
            editor.edit(
                [(
                    MultiBufferOffset(range.start)..MultiBufferOffset(range.end),
                    insertion,
                )],
                cx,
            );
            editor.change_selections(SelectionEffects::default(), window, cx, |selections| {
                selections.select_ranges([MultiBufferOffset(cursor)..MultiBufferOffset(cursor)])
            });
        });
        cx.notify();
    }

    pub(super) fn render_completion(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let completion = self.completion.as_ref().filter(|completion| !completion.items.is_empty())?;
        let colors = cx.theme().colors();
        let first = completion
            .selected
            .saturating_sub(VISIBLE_ROWS - 1)
            .min(completion.items.len().saturating_sub(VISIBLE_ROWS));
        let rows = completion
            .items
            .iter()
            .enumerate()
            .skip(first)
            .take(VISIBLE_ROWS)
            .map(|(index, item)| {
                let selected = index == completion.selected;
                let row_icon = match &item.icon {
                    ItemIcon::Command => icon(IconName::AgentCommand, 14.)
                        .text_color(colors.text_muted)
                        .into_any_element(),
                    ItemIcon::Skill => icon(IconName::AgentWidget, 14.)
                        .text_color(colors.text_muted)
                        .into_any_element(),
                    ItemIcon::Path { path, is_dir } => {
                        let theme_icon = if *is_dir {
                            FileIcons::get_folder_icon(false, path, cx)
                        } else {
                            FileIcons::get_icon(path, cx)
                        };
                        match theme_icon {
                            Some(theme_icon) => Icon::from_path(theme_icon),
                            None if *is_dir => Icon::new(IconName::AgentFolder),
                            None => Icon::new(IconName::AgentDocument),
                        }
                        .size(IconSize::Small)
                        .color(Color::Muted)
                        .into_any_element()
                    }
                };
                h_flex()
                    .id(SharedString::from(format!("t3-completion-{index}")))
                    .h(px(ROW_HEIGHT))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(selected, |this| this.bg(ink(0.08, cx)))
                    .hover(|style| style.bg(ink(0.06, cx)))
                    .child(row_icon)
                    .child(
                        div()
                            .flex_none()
                            .max_w(relative(0.6))
                            .truncate()
                            .text_color(colors.text)
                            .child(item.label.clone()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(ui(12.))
                            .text_color(colors.text_muted)
                            .child(item.detail.clone()),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.accept_completion(Some(index), window, cx)
                    }))
            });
        Some(
            v_flex()
                .w_full()
                .p(px(4.))
                .gap(px(1.))
                .rounded(px(12.))
                .border_1()
                .border_color(colors.border)
                .bg(colors.elevated_surface_background)
                .shadow_lg()
                .text_size(ui(13.))
                .children(rows)
                .child(
                    div()
                        .px(px(8.))
                        .pt(px(4.))
                        .text_size(ui(11.))
                        .text_color(colors.text_placeholder)
                        .child(match completion.kind {
                            TokenKind::Command => t!("↑↓ to choose, Enter to run the command"),
                            TokenKind::Mention => t!("↑↓ to choose, Enter to mention the file"),
                        }),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_found_at_the_cursor() {
        let text = "look at @src/ma and /rev";
        let (kind, token) = token_at(text, 15).expect("a mention");
        assert_eq!(kind, TokenKind::Mention);
        assert_eq!(token.query, "src/ma");
        assert_eq!(&text[token.range], "@src/ma");
        let (kind, token) = token_at(text, text.len()).expect("a command");
        assert_eq!(kind, TokenKind::Command);
        assert_eq!(token.query, "rev");
        assert!(token_at("/usr/bin", 4).is_none());
        assert!(token_at("plain text", 5).is_none());
    }

    #[test]
    fn mentions_are_relative_and_quoted_when_needed() {
        let root = Path::new("/work");
        assert_eq!(mention_text(Path::new("/work/src/a.rs"), root, false), "@src/a.rs");
        assert_eq!(mention_text(Path::new("/work/my dir"), root, true), "@\"my dir\"/");
    }
}
