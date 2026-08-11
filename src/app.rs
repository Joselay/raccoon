use std::{
    collections::BTreeMap,
    ffi::OsString,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    Terminal,
    backend::Backend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::{
    cli::LaunchTarget,
    clipboard,
    config::{AppConfig, ConfigPaths, ThemeConfig},
    dashboard::{ChangeEntry, ChangeKind, DashboardData},
    diff::{DiffDocument, LineKind},
    file_icon::{self, IconColor},
    highlight::{HighlightedDiff, SyntaxToken},
    repository::Repository,
    theme::{LoadedTheme, Rgb, Theme},
    watcher::RepositoryWatcher,
    worker::{GitCommand, GitPayload, GitWorker, HighlightCommand, HighlightWorker},
};

#[derive(Clone, Copy)]
struct Colors<'a> {
    theme: &'a Theme,
    true_color: bool,
}

impl Colors<'_> {
    fn color(self, rgb: Rgb) -> Color {
        if self.true_color {
            rgb.into()
        } else {
            let r = (u16::from(rgb.red) * 5 / 255) as u8;
            let g = (u16::from(rgb.green) * 5 / 255) as u8;
            let b = (u16::from(rgb.blue) * 5 / 255) as u8;
            Color::Indexed(16 + 36 * r + 6 * g + b)
        }
    }

    fn foreground(self) -> Color {
        self.color(self.theme.ui.foreground)
    }
    fn muted(self) -> Color {
        self.color(self.theme.ui.muted)
    }
    fn accent(self) -> Color {
        self.color(self.theme.ui.accent)
    }
    fn border(self) -> Color {
        self.color(self.theme.ui.border)
    }
    fn focused_border(self) -> Color {
        self.color(self.theme.ui.focused_border)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    History,
    CommitFiles,
    Staged,
    Unstaged,
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DashboardPage {
    History,
    Changes,
}

enum Screen {
    Dashboard,
    Diff { target: LaunchTarget, direct: bool },
}

struct AppState {
    screen: Screen,
    dashboard_page: DashboardPage,
    focus: Focus,
    preview_focus: Focus,
    data: Option<DashboardData>,
    dashboard_error: Option<String>,
    diff: Option<DiffDocument>,
    diff_error: Option<String>,
    highlighted: Option<HighlightedDiff>,
    highlight_message: Option<String>,
    pending_change_preview: bool,
    preview_target: Option<LaunchTarget>,
    diff_scroll: usize,
    branch_selection: usize,
    history_selection: usize,
    commit_file_selection: usize,
    commit_files: Vec<ChangeEntry>,
    commit_files_target: Option<OsString>,
    commit_files_loading: bool,
    commit_files_error: Option<String>,
    staged_selection: usize,
    unstaged_selection: usize,
    comparison_base: Option<OsString>,
    branch_picker: bool,
    discard_confirmation: bool,
    themes: Vec<Theme>,
    theme_index: usize,
    confirmed_theme_index: usize,
    theme_picker: bool,
    theme_paths: ConfigPaths,
    theme_message: Option<String>,
    true_color: bool,
    search_input: bool,
    search_query: String,
    search_matches: Vec<usize>,
    search_match_index: usize,
}

impl AppState {
    fn new(target: &LaunchTarget, loaded_theme: LoadedTheme) -> Self {
        let dashboard_page = if matches!(target, LaunchTarget::History { .. }) {
            DashboardPage::History
        } else {
            DashboardPage::Changes
        };
        let focus = if dashboard_page == DashboardPage::Changes {
            Focus::Unstaged
        } else {
            Focus::History
        };
        let branch_picker = matches!(target, LaunchTarget::Branches);
        let screen = if needs_diff(target) {
            Screen::Diff {
                target: target.clone(),
                direct: true,
            }
        } else {
            Screen::Dashboard
        };
        let themes = loaded_theme.catalog.iter().cloned().collect::<Vec<_>>();
        let theme_index = themes
            .iter()
            .position(|theme| theme.name == loaded_theme.selected.name)
            .unwrap_or(0);
        let true_color = terminal_supports_true_color();
        Self {
            screen,
            dashboard_page,
            focus,
            preview_focus: focus,
            data: None,
            dashboard_error: None,
            diff: None,
            diff_error: None,
            highlighted: None,
            highlight_message: None,
            pending_change_preview: false,
            preview_target: None,
            diff_scroll: 0,
            branch_selection: 0,
            history_selection: 0,
            commit_file_selection: 0,
            commit_files: Vec::new(),
            commit_files_target: None,
            commit_files_loading: false,
            commit_files_error: None,
            staged_selection: 0,
            unstaged_selection: 0,
            comparison_base: None,
            branch_picker,
            discard_confirmation: false,
            themes,
            theme_index,
            confirmed_theme_index: theme_index,
            theme_picker: false,
            theme_paths: loaded_theme.paths,
            theme_message: loaded_theme
                .diagnostics
                .first()
                .map(|diagnostic| diagnostic.message.clone()),
            true_color,
            search_input: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_match_index: 0,
        }
    }

    fn colors(&self) -> Colors<'_> {
        Colors {
            theme: &self.themes[self.theme_index],
            true_color: self.true_color,
        }
    }

    fn move_selection(&mut self, delta: isize) -> bool {
        let Some(data) = &self.data else {
            return false;
        };
        if self.dashboard_page == DashboardPage::Changes {
            let staged_len = data.staged.len();
            let unstaged_len = data.unstaged.len();
            let len = staged_len + unstaged_len;
            let current = match self.focus {
                Focus::Staged => self.staged_selection.min(staged_len.saturating_sub(1)),
                Focus::Unstaged => {
                    staged_len + self.unstaged_selection.min(unstaged_len.saturating_sub(1))
                }
                _ => 0,
            };
            let next = if len == 0 {
                0
            } else {
                (current as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize
            };
            if next < staged_len {
                self.focus = Focus::Staged;
                self.preview_focus = Focus::Staged;
                self.staged_selection = next;
            } else {
                self.focus = Focus::Unstaged;
                self.preview_focus = Focus::Unstaged;
                self.unstaged_selection = next.saturating_sub(staged_len);
            }
            return current != next;
        }
        let (selection, len) = match self.focus {
            Focus::History => (&mut self.history_selection, data.commits.len()),
            Focus::CommitFiles => (&mut self.commit_file_selection, self.commit_files.len()),
            Focus::Staged => (&mut self.staged_selection, data.staged.len()),
            Focus::Unstaged => (&mut self.unstaged_selection, data.unstaged.len()),
            Focus::Diff => return false,
        };
        let previous = *selection;
        if len == 0 {
            *selection = 0;
        } else {
            *selection =
                (*selection as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize;
        }
        previous != *selection
    }

    fn cycle_focus(&mut self) {
        self.focus = match self.dashboard_page {
            DashboardPage::History => match self.focus {
                Focus::History => Focus::CommitFiles,
                Focus::CommitFiles => Focus::History,
                _ => Focus::History,
            },
            DashboardPage::Changes => {
                let (has_staged, has_unstaged) = self
                    .data
                    .as_ref()
                    .map(|data| (!data.staged.is_empty(), !data.unstaged.is_empty()))
                    .unwrap_or_default();
                let (focus, preview_focus) =
                    next_changes_focus(self.focus, self.preview_focus, has_staged, has_unstaged);
                self.preview_focus = preview_focus;
                focus
            }
        };
    }

    fn selected_target(&self) -> Option<LaunchTarget> {
        let data = self.data.as_ref()?;
        match self.focus {
            Focus::History => {
                data.commits
                    .get(self.history_selection)
                    .map(|commit| LaunchTarget::Commit {
                        revision: commit.id.clone(),
                        path: None,
                    })
            }
            Focus::CommitFiles => {
                let commit = data.commits.get(self.history_selection)?;
                selected_tree_entry(&self.commit_files, self.commit_file_selection).map(|file| {
                    LaunchTarget::Commit {
                        revision: commit.id.clone(),
                        path: Some(file.path.clone()),
                    }
                })
            }
            Focus::Staged => {
                selected_tree_entry(&data.staged, self.staged_selection).map(|change| {
                    LaunchTarget::Staged {
                        path: change.path.clone(),
                    }
                })
            }
            Focus::Unstaged => {
                selected_tree_entry(&data.unstaged, self.unstaged_selection).map(|change| {
                    LaunchTarget::WorkingTree {
                        path: change.path.clone(),
                    }
                })
            }
            Focus::Diff => match self.preview_focus {
                Focus::Staged => {
                    selected_tree_entry(&data.staged, self.staged_selection).map(|change| {
                        LaunchTarget::Staged {
                            path: change.path.clone(),
                        }
                    })
                }
                _ => selected_tree_entry(&data.unstaged, self.unstaged_selection).map(|change| {
                    LaunchTarget::WorkingTree {
                        path: change.path.clone(),
                    }
                }),
            },
        }
    }

    fn update_search(&mut self) {
        self.search_matches.clear();
        let Some(document) = &self.diff else { return };
        if self.search_query.is_empty() {
            return;
        }
        let query = self.search_query.to_lowercase();
        self.search_matches = document
            .lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                (is_visible_diff_line(line.kind)
                    && document.line_text(line).to_lowercase().contains(&query))
                .then_some(index)
            })
            .collect();
        self.search_match_index = 0;
        if let Some(first) = self.search_matches.first() {
            self.diff_scroll = *first;
        }
        self.highlight_message = Some(format!(
            "{} search match(es) for “{}”",
            self.search_matches.len(),
            self.search_query
        ));
    }

    fn next_search_match(&mut self, backwards: bool) {
        if self.search_matches.is_empty() {
            return;
        }
        if backwards {
            self.search_match_index = self
                .search_match_index
                .checked_sub(1)
                .unwrap_or(self.search_matches.len() - 1);
        } else {
            self.search_match_index = (self.search_match_index + 1) % self.search_matches.len();
        }
        self.diff_scroll = self.search_matches[self.search_match_index];
    }

    fn move_diff_scroll(&mut self, delta: isize) {
        self.diff_scroll = move_diff_scroll(self.diff.as_ref(), self.diff_scroll, delta);
    }
}

pub fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    repo: Repository,
    initial_target: LaunchTarget,
    loaded_theme: LoadedTheme,
) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    let mut state = AppState::new(&initial_target, loaded_theme);
    let history_path = match &initial_target {
        LaunchTarget::History { path } => path.clone(),
        _ => None,
    };
    let git_worker = GitWorker::start(repo.clone());
    let highlight_worker = HighlightWorker::start();
    let repository_watcher = RepositoryWatcher::start(&repo)?;
    let mut refresh_due = None;
    let mut next_request_id = 1u64;
    let mut dashboard_request = None;
    let mut diff_request = None;
    let mut commit_files_request = None;
    let mut discard_request = None;
    let mut highlight_request = None;
    if needs_diff(&initial_target) {
        git_worker.request(GitCommand::Diff {
            request_id: next_request_id,
            target: initial_target,
        })?;
        diff_request = Some(next_request_id);
    } else {
        git_worker.request(GitCommand::Dashboard {
            request_id: next_request_id,
            history_path: history_path.clone(),
        })?;
        dashboard_request = Some(next_request_id);
    }
    next_request_id += 1;
    let mut dirty = true;

    loop {
        while let Ok(response) = git_worker.responses.try_recv() {
            if Some(response.request_id) == dashboard_request {
                dashboard_request = None;
                match response.result {
                    Ok(GitPayload::Dashboard(data)) => {
                        reconcile_change_selection(&mut state, &data);
                        if state.dashboard_page == DashboardPage::Changes {
                            let active_file_list = if state.focus == Focus::Diff {
                                state.preview_focus
                            } else {
                                state.focus
                            };
                            if active_file_list == Focus::Unstaged
                                && data.unstaged.is_empty()
                                && !data.staged.is_empty()
                            {
                                state.preview_focus = Focus::Staged;
                                if state.focus != Focus::Diff {
                                    state.focus = Focus::Staged;
                                }
                            } else if active_file_list == Focus::Staged
                                && data.staged.is_empty()
                                && !data.unstaged.is_empty()
                            {
                                state.preview_focus = Focus::Unstaged;
                                if state.focus != Focus::Diff {
                                    state.focus = Focus::Unstaged;
                                }
                            }
                        }
                        state.history_selection = state
                            .history_selection
                            .min(data.commits.len().saturating_sub(1));
                        state.staged_selection = state
                            .staged_selection
                            .min(data.staged.len().saturating_sub(1));
                        state.unstaged_selection = state
                            .unstaged_selection
                            .min(data.unstaged.len().saturating_sub(1));
                        state.branch_selection = data
                            .branches
                            .iter()
                            .position(|branch| branch.current)
                            .unwrap_or(0);
                        state.data = Some(data);
                        if matches!(state.screen, Screen::Dashboard)
                            && state.dashboard_page == DashboardPage::History
                        {
                            select_commit_preview(
                                &mut state,
                                &git_worker,
                                &mut next_request_id,
                                &mut commit_files_request,
                                history_path.as_ref(),
                            )?;
                        } else if matches!(state.screen, Screen::Dashboard) {
                            request_change_preview(
                                &mut state,
                                &git_worker,
                                &mut next_request_id,
                                &mut diff_request,
                            )?;
                        }
                    }
                    Ok(
                        GitPayload::Diff(_)
                        | GitPayload::CommitFiles { .. }
                        | GitPayload::DiscardedWorkingTree,
                    ) => {}
                    Err(error) => state.dashboard_error = Some(error.to_string()),
                }
                dirty = true;
            } else if Some(response.request_id) == diff_request {
                diff_request = None;
                match response.result {
                    Ok(GitPayload::Diff(diff)) => {
                        highlight_worker.request(HighlightCommand {
                            request_id: response.request_id,
                            document: diff.clone(),
                        })?;
                        highlight_request = Some(response.request_id);
                        state.highlighted = None;
                        state.search_matches.clear();
                        state.diff = Some(diff);
                    }
                    Ok(
                        GitPayload::Dashboard(_)
                        | GitPayload::CommitFiles { .. }
                        | GitPayload::DiscardedWorkingTree,
                    ) => {}
                    Err(error) => state.diff_error = Some(error.to_string()),
                }
                if state.pending_change_preview
                    && matches!(state.screen, Screen::Dashboard)
                    && state.dashboard_page == DashboardPage::Changes
                {
                    request_change_preview(
                        &mut state,
                        &git_worker,
                        &mut next_request_id,
                        &mut diff_request,
                    )?;
                }
                dirty = true;
            } else if Some(response.request_id) == discard_request {
                discard_request = None;
                match response.result {
                    Ok(GitPayload::DiscardedWorkingTree) => {
                        let request_id = next_request_id;
                        next_request_id += 1;
                        git_worker.request(GitCommand::Dashboard {
                            request_id,
                            history_path: history_path.clone(),
                        })?;
                        dashboard_request = Some(request_id);
                    }
                    Ok(_) => {}
                    Err(error) => state.dashboard_error = Some(error.to_string()),
                }
                dirty = true;
            } else if commit_files_request
                .as_ref()
                .is_some_and(|(request_id, _)| *request_id == response.request_id)
            {
                let (_, requested_revision) = commit_files_request.take().unwrap();
                match response.result {
                    Ok(GitPayload::CommitFiles { revision, files })
                        if state.commit_files_target.as_ref() == Some(&revision) =>
                    {
                        state.commit_files = files;
                        state.commit_file_selection = 0;
                        state.commit_files_loading = false;
                        state.commit_files_error = None;
                    }
                    Ok(_) => {}
                    Err(error)
                        if state.commit_files_target.as_ref() == Some(&requested_revision) =>
                    {
                        state.commit_files_loading = false;
                        state.commit_files_error = Some(error.to_string());
                    }
                    Err(_) => {}
                }
                if state.commit_files_target.as_ref() != Some(&requested_revision) {
                    request_commit_files(
                        &state,
                        &git_worker,
                        &mut next_request_id,
                        &mut commit_files_request,
                        history_path.as_ref(),
                    )?;
                }
                dirty = true;
            }
        }
        while let Ok(response) = highlight_worker.responses.try_recv() {
            if Some(response.request_id) != highlight_request {
                continue;
            }
            highlight_request = None;
            match response.result {
                Ok(highlighted) => {
                    if highlighted.skipped_files > 0 {
                        state.highlight_message = Some(format!(
                            "Syntax highlighting skipped for {} large file(s)",
                            highlighted.skipped_files
                        ));
                    }
                    state.highlighted = Some(highlighted);
                }
                Err(error) => {
                    state.highlight_message =
                        Some(format!("Syntax highlighting unavailable: {error}"));
                }
            }
            dirty = true;
        }

        while repository_watcher.events.try_recv().is_ok() {
            refresh_due = Some(Instant::now() + Duration::from_millis(120));
        }
        if refresh_due.is_some_and(|due| Instant::now() >= due)
            && dashboard_request.is_none()
            && diff_request.is_none()
            && commit_files_request.is_none()
            && discard_request.is_none()
        {
            refresh_due = None;
            request_realtime_refresh(
                &mut state,
                &git_worker,
                &mut next_request_id,
                &mut dashboard_request,
                &mut diff_request,
                history_path.as_ref(),
            )?;
        }

        if dirty {
            terminal.draw(|frame| render(frame, &state))?;
            dirty = false;
        }

        let loading = dashboard_request.is_some()
            || diff_request.is_some()
            || commit_files_request.is_some()
            || discard_request.is_some()
            || highlight_request.is_some();
        if !event::poll(if loading || refresh_due.is_some() {
            Duration::from_millis(25)
        } else {
            Duration::from_millis(100)
        })? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            dirty = true;
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if !state.theme_picker {
            state.theme_message = None;
        }

        if state.discard_confirmation {
            match key.code {
                KeyCode::Esc | KeyCode::Char('n') => state.discard_confirmation = false,
                KeyCode::Enter | KeyCode::Char('y') => {
                    state.discard_confirmation = false;
                    let request_id = next_request_id;
                    next_request_id += 1;
                    git_worker.request(GitCommand::DiscardWorkingTree { request_id })?;
                    discard_request = Some(request_id);
                }
                _ => continue,
            }
            dirty = true;
            continue;
        }

        if state.search_input {
            match key.code {
                KeyCode::Esc => state.search_input = false,
                KeyCode::Enter => {
                    state.search_input = false;
                    state.update_search();
                }
                KeyCode::Backspace => {
                    state.search_query.pop();
                }
                KeyCode::Char(character) => state.search_query.push(character),
                _ => continue,
            }
            dirty = true;
            continue;
        }

        if state.theme_picker {
            match key.code {
                KeyCode::Esc | KeyCode::Char('t') => {
                    state.theme_index = state.confirmed_theme_index;
                    state.theme_picker = false;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    state.theme_index = (state.theme_index + 1).min(state.themes.len() - 1);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    state.theme_index = state.theme_index.saturating_sub(1);
                }
                KeyCode::Enter => {
                    let config = AppConfig {
                        theme: ThemeConfig {
                            name: state.themes[state.theme_index].name.clone(),
                        },
                    };
                    match config.save(&state.theme_paths) {
                        Ok(()) => {
                            state.confirmed_theme_index = state.theme_index;
                            state.theme_message = Some(format!(
                                "Selected theme: {}",
                                state.themes[state.theme_index].name
                            ));
                            state.theme_picker = false;
                        }
                        Err(error) => state.theme_message = Some(error.to_string()),
                    }
                }
                _ => continue,
            }
            dirty = true;
            continue;
        }

        if state.branch_picker {
            match key.code {
                KeyCode::Esc | KeyCode::Char('b') => {
                    state.branch_picker = false;
                    state.comparison_base = None;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    let last = state
                        .data
                        .as_ref()
                        .map(|data| data.branches.len().saturating_sub(1))
                        .unwrap_or(0);
                    state.branch_selection = state.branch_selection.saturating_add(1).min(last);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    state.branch_selection = state.branch_selection.saturating_sub(1);
                }
                KeyCode::PageDown => {
                    let last = state
                        .data
                        .as_ref()
                        .map(|data| data.branches.len().saturating_sub(1))
                        .unwrap_or(0);
                    state.branch_selection = state.branch_selection.saturating_add(10).min(last);
                }
                KeyCode::PageUp => {
                    state.branch_selection = state.branch_selection.saturating_sub(10);
                }
                KeyCode::Enter => {
                    if let Some(branch) = state
                        .data
                        .as_ref()
                        .and_then(|data| data.branches.get(state.branch_selection))
                    {
                        let target = LaunchTarget::Show {
                            revision: branch.name.clone(),
                            path: None,
                        };
                        state.branch_picker = false;
                        state.screen = Screen::Diff {
                            target: target.clone(),
                            direct: false,
                        };
                        state.diff = None;
                        state.diff_error = None;
                        state.highlighted = None;
                        state.highlight_message = None;
                        state.search_matches.clear();
                        state.diff_scroll = 0;
                        request_diff(&git_worker, &mut next_request_id, &mut diff_request, target)?;
                    }
                }
                KeyCode::Char('c') => {
                    if let Some(branch) = state
                        .data
                        .as_ref()
                        .and_then(|data| data.branches.get(state.branch_selection))
                    {
                        if let Some(left) = state.comparison_base.take() {
                            if left != branch.name {
                                let target = LaunchTarget::Compare {
                                    left,
                                    right: branch.name.clone(),
                                    path: None,
                                };
                                state.branch_picker = false;
                                state.screen = Screen::Diff {
                                    target: target.clone(),
                                    direct: false,
                                };
                                state.diff = None;
                                state.diff_error = None;
                                state.highlighted = None;
                                state.highlight_message = None;
                                state.search_matches.clear();
                                state.diff_scroll = 0;
                                request_diff(
                                    &git_worker,
                                    &mut next_request_id,
                                    &mut diff_request,
                                    target,
                                )?;
                            } else {
                                state.comparison_base = Some(left);
                            }
                        } else {
                            state.comparison_base = Some(branch.name.clone());
                        }
                    }
                }
                _ => continue,
            }
            dirty = true;
            continue;
        }

        if key.code == KeyCode::Char('t') {
            state.theme_picker = true;
            dirty = true;
            continue;
        }
        if key.code == KeyCode::Char('/') && matches!(state.screen, Screen::Diff { .. }) {
            state.search_input = true;
            state.search_query.clear();
            state.theme_message = None;
            dirty = true;
            continue;
        }

        match &state.screen {
            Screen::Dashboard => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Tab | KeyCode::BackTab => {
                    let previous_target = state.selected_target();
                    state.cycle_focus();
                    if state.dashboard_page == DashboardPage::Changes
                        && state.selected_target() != previous_target
                    {
                        request_change_preview(
                            &mut state,
                            &git_worker,
                            &mut next_request_id,
                            &mut diff_request,
                        )?;
                    }
                }
                KeyCode::Right | KeyCode::Left => {
                    if state.dashboard_page == DashboardPage::History {
                        state.cycle_focus();
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if state.focus == Focus::Diff {
                        state.move_diff_scroll(1);
                    } else if state.move_selection(1) {
                        refresh_dashboard_preview(
                            &mut state,
                            &git_worker,
                            &mut next_request_id,
                            &mut diff_request,
                            &mut commit_files_request,
                            history_path.as_ref(),
                        )?;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if state.focus == Focus::Diff {
                        state.move_diff_scroll(-1);
                    } else if state.move_selection(-1) {
                        refresh_dashboard_preview(
                            &mut state,
                            &git_worker,
                            &mut next_request_id,
                            &mut diff_request,
                            &mut commit_files_request,
                            history_path.as_ref(),
                        )?;
                    }
                }
                KeyCode::PageDown => {
                    if state.focus == Focus::Diff {
                        state.move_diff_scroll(20);
                    } else if state.move_selection(10) {
                        refresh_dashboard_preview(
                            &mut state,
                            &git_worker,
                            &mut next_request_id,
                            &mut diff_request,
                            &mut commit_files_request,
                            history_path.as_ref(),
                        )?;
                    }
                }
                KeyCode::PageUp => {
                    if state.focus == Focus::Diff {
                        state.move_diff_scroll(-20);
                    } else if state.move_selection(-10) {
                        refresh_dashboard_preview(
                            &mut state,
                            &git_worker,
                            &mut next_request_id,
                            &mut diff_request,
                            &mut commit_files_request,
                            history_path.as_ref(),
                        )?;
                    }
                }
                KeyCode::Home | KeyCode::Char('g') => {
                    if state.focus == Focus::Diff {
                        state.diff_scroll = 0;
                    } else if state.move_selection(isize::MIN) {
                        refresh_dashboard_preview(
                            &mut state,
                            &git_worker,
                            &mut next_request_id,
                            &mut diff_request,
                            &mut commit_files_request,
                            history_path.as_ref(),
                        )?;
                    }
                }
                KeyCode::End | KeyCode::Char('G') => {
                    if state.focus == Focus::Diff {
                        state.move_diff_scroll(isize::MAX);
                    } else if state.move_selection(isize::MAX) {
                        refresh_dashboard_preview(
                            &mut state,
                            &git_worker,
                            &mut next_request_id,
                            &mut diff_request,
                            &mut commit_files_request,
                            history_path.as_ref(),
                        )?;
                    }
                }
                KeyCode::Enter => {
                    if let Some(target) = state.selected_target() {
                        state.pending_change_preview = false;
                        state.screen = Screen::Diff {
                            target: target.clone(),
                            direct: false,
                        };
                        state.diff = None;
                        state.diff_error = None;
                        state.highlighted = None;
                        state.highlight_message = None;
                        state.search_matches.clear();
                        state.diff_scroll = 0;
                        request_diff(&git_worker, &mut next_request_id, &mut diff_request, target)?;
                    }
                }
                KeyCode::Char('b') => {
                    state.branch_picker = true;
                    state.comparison_base = None;
                }
                KeyCode::Char('c') => {
                    state.branch_picker = true;
                    state.comparison_base = state
                        .data
                        .as_ref()
                        .and_then(|data| data.branches.get(state.branch_selection))
                        .map(|branch| branch.name.clone());
                }
                KeyCode::Char('y') if state.dashboard_page == DashboardPage::History => {
                    if let Some(commit) = state
                        .data
                        .as_ref()
                        .and_then(|data| data.commits.get(state.history_selection))
                    {
                        let full_id = commit.id.to_string_lossy();
                        state.theme_message = Some(match clipboard::copy(&full_id) {
                            Ok(()) => format!("Copied commit SHA: {}", commit.short_id),
                            Err(error) => format!("Could not copy commit SHA: {error}"),
                        });
                    }
                }
                KeyCode::Char('h') => {
                    state.dashboard_page = DashboardPage::History;
                    state.focus = Focus::History;
                    select_commit_preview(
                        &mut state,
                        &git_worker,
                        &mut next_request_id,
                        &mut commit_files_request,
                        history_path.as_ref(),
                    )?;
                }
                KeyCode::Char('u') => {
                    state.dashboard_page = DashboardPage::Changes;
                    state.focus = if state
                        .data
                        .as_ref()
                        .is_some_and(|data| data.unstaged.is_empty() && !data.staged.is_empty())
                    {
                        Focus::Staged
                    } else {
                        Focus::Unstaged
                    };
                    state.preview_focus = state.focus;
                    request_change_preview(
                        &mut state,
                        &git_worker,
                        &mut next_request_id,
                        &mut diff_request,
                    )?;
                }
                KeyCode::Char('D') if state.dashboard_page == DashboardPage::Changes => {
                    let has_worktree_changes = state.data.as_ref().is_some_and(|data| {
                        data.unstaged
                            .iter()
                            .any(|change| change.kind != ChangeKind::Untracked)
                    });
                    if has_worktree_changes {
                        state.discard_confirmation = true;
                    } else {
                        state.theme_message =
                            Some("No tracked working-tree changes to discard".to_owned());
                    }
                }
                _ => continue,
            },
            Screen::Diff { direct, .. } => match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('B') if state.data.is_some() => {
                    state.pending_change_preview = false;
                    state.branch_picker = true;
                    state.comparison_base = None;
                }
                KeyCode::Esc | KeyCode::Char('b') if !direct => {
                    state.screen = Screen::Dashboard;
                    if state.dashboard_page == DashboardPage::Changes {
                        state.preview_target = None;
                        request_change_preview(
                            &mut state,
                            &git_worker,
                            &mut next_request_id,
                            &mut diff_request,
                        )?;
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => state.move_diff_scroll(1),
                KeyCode::Char('k') | KeyCode::Up => state.move_diff_scroll(-1),
                KeyCode::PageDown => state.move_diff_scroll(20),
                KeyCode::PageUp => state.move_diff_scroll(-20),
                KeyCode::Home | KeyCode::Char('g') => state.diff_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => state.move_diff_scroll(isize::MAX),
                KeyCode::Char('n') => {
                    state.diff_scroll = next_hunk(state.diff.as_ref(), state.diff_scroll)
                }
                KeyCode::Char('N') => {
                    state.diff_scroll = previous_hunk(state.diff.as_ref(), state.diff_scroll)
                }
                KeyCode::Char(']') => {
                    state.diff_scroll = next_file(state.diff.as_ref(), state.diff_scroll)
                }
                KeyCode::Char('[') => {
                    state.diff_scroll = previous_file(state.diff.as_ref(), state.diff_scroll)
                }
                KeyCode::Char('s') => state.next_search_match(false),
                KeyCode::Char('S') => state.next_search_match(true),
                _ => continue,
            },
        }
        dirty = true;
    }
    Ok(())
}

fn next_changes_focus(
    focus: Focus,
    preview_focus: Focus,
    has_staged: bool,
    has_unstaged: bool,
) -> (Focus, Focus) {
    match focus {
        Focus::Staged => (Focus::Diff, Focus::Staged),
        Focus::Unstaged if has_staged => (Focus::Staged, Focus::Staged),
        Focus::Unstaged => (Focus::Diff, Focus::Unstaged),
        Focus::Diff if has_unstaged => (Focus::Unstaged, Focus::Unstaged),
        Focus::Diff if has_staged => (Focus::Staged, Focus::Staged),
        Focus::Diff => (Focus::Diff, preview_focus),
        _ if has_staged => (Focus::Staged, Focus::Staged),
        _ if has_unstaged => (Focus::Unstaged, Focus::Unstaged),
        _ => (Focus::Diff, preview_focus),
    }
}

fn move_diff_scroll(document: Option<&DiffDocument>, current_line: usize, delta: isize) -> usize {
    let Some(document) = document else {
        return current_line;
    };
    let visible_lines = document
        .lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| is_visible_diff_line(line.kind).then_some(index))
        .collect::<Vec<_>>();
    if visible_lines.is_empty() {
        return 0;
    }
    let current_position = visible_lines
        .partition_point(|line_index| *line_index < current_line)
        .min(visible_lines.len() - 1);
    let next_position = if delta == isize::MAX {
        visible_lines.len() - 1
    } else {
        (current_position as isize + delta).clamp(0, visible_lines.len() as isize - 1) as usize
    };
    visible_lines[next_position]
}

fn needs_diff(target: &LaunchTarget) -> bool {
    matches!(
        target,
        LaunchTarget::WorkingTree { .. }
            | LaunchTarget::Staged { .. }
            | LaunchTarget::Commit { .. }
            | LaunchTarget::Compare { .. }
            | LaunchTarget::Show { .. }
    )
}

fn request_diff(
    worker: &GitWorker,
    next_request_id: &mut u64,
    current_request: &mut Option<u64>,
    target: LaunchTarget,
) -> Result<()> {
    let request_id = *next_request_id;
    *next_request_id += 1;
    worker.request(GitCommand::Diff { request_id, target })?;
    *current_request = Some(request_id);
    Ok(())
}

fn request_realtime_refresh(
    state: &mut AppState,
    worker: &GitWorker,
    next_request_id: &mut u64,
    dashboard_request: &mut Option<u64>,
    diff_request: &mut Option<u64>,
    history_path: Option<&std::path::PathBuf>,
) -> Result<()> {
    state.dashboard_error = None;
    state.diff_error = None;

    if state.data.is_some() || matches!(state.screen, Screen::Dashboard) {
        let request_id = *next_request_id;
        *next_request_id += 1;
        worker.request(GitCommand::Dashboard {
            request_id,
            history_path: history_path.cloned(),
        })?;
        *dashboard_request = Some(request_id);
    }

    if let Screen::Diff { target, .. } = &state.screen {
        request_diff(worker, next_request_id, diff_request, target.clone())?;
    }

    Ok(())
}

fn reconcile_change_selection(state: &mut AppState, data: &DashboardData) {
    let selected = state.selected_target();
    match selected {
        Some(LaunchTarget::Staged { path }) => {
            if let Some(index) = data.staged.iter().position(|entry| entry.path == path) {
                state.staged_selection =
                    tree_selection_for_entry(&data.staged, index).unwrap_or_default();
            }
        }
        Some(LaunchTarget::WorkingTree { path }) => {
            if let Some(index) = data.unstaged.iter().position(|entry| entry.path == path) {
                state.unstaged_selection =
                    tree_selection_for_entry(&data.unstaged, index).unwrap_or_default();
            }
        }
        _ => {}
    }
}

fn request_change_preview(
    state: &mut AppState,
    worker: &GitWorker,
    next_request_id: &mut u64,
    current_request: &mut Option<u64>,
) -> Result<()> {
    if !matches!(state.focus, Focus::Staged | Focus::Unstaged | Focus::Diff) {
        return Ok(());
    }
    let Some(target) = state.selected_target() else {
        state.pending_change_preview = false;
        state.preview_target = None;
        state.diff = None;
        state.diff_error = None;
        state.highlighted = None;
        state.highlight_message = None;
        return Ok(());
    };
    let target_changed = state.preview_target.as_ref() != Some(&target);
    state.preview_target = Some(target.clone());
    if current_request.is_some() {
        // Keep only the newest selection while Git is busy. This makes rapid
        // navigation responsive without filling the bounded worker queue.
        state.pending_change_preview = true;
        if target_changed {
            state.diff = None;
            state.diff_error = None;
            state.highlighted = None;
            state.diff_scroll = 0;
        }
        return Ok(());
    }
    state.pending_change_preview = false;
    state.diff_error = None;
    state.highlight_message = None;
    state.search_matches.clear();
    if target_changed {
        state.diff = None;
        state.highlighted = None;
        state.diff_scroll = 0;
    }
    request_diff(worker, next_request_id, current_request, target)
}

fn refresh_dashboard_preview(
    state: &mut AppState,
    worker: &GitWorker,
    next_request_id: &mut u64,
    diff_request: &mut Option<u64>,
    commit_files_request: &mut Option<(u64, OsString)>,
    history_path: Option<&std::path::PathBuf>,
) -> Result<()> {
    match state.dashboard_page {
        DashboardPage::History => select_commit_preview(
            state,
            worker,
            next_request_id,
            commit_files_request,
            history_path,
        ),
        DashboardPage::Changes => {
            request_change_preview(state, worker, next_request_id, diff_request)
        }
    }
}

fn select_commit_preview(
    state: &mut AppState,
    worker: &GitWorker,
    next_request_id: &mut u64,
    current_request: &mut Option<(u64, OsString)>,
    history_path: Option<&std::path::PathBuf>,
) -> Result<()> {
    let revision = state.data.as_ref().and_then(|data| {
        data.commits
            .get(state.history_selection)
            .map(|commit| commit.id.clone())
    });
    if state.commit_files_target == revision {
        return Ok(());
    }
    state.commit_files_target = revision;
    state.commit_files.clear();
    state.commit_file_selection = 0;
    state.commit_files_loading = state.commit_files_target.is_some();
    state.commit_files_error = None;
    if current_request.is_none() {
        request_commit_files(
            state,
            worker,
            next_request_id,
            current_request,
            history_path,
        )?;
    }
    Ok(())
}

fn request_commit_files(
    state: &AppState,
    worker: &GitWorker,
    next_request_id: &mut u64,
    current_request: &mut Option<(u64, OsString)>,
    history_path: Option<&std::path::PathBuf>,
) -> Result<()> {
    let Some(revision) = state.commit_files_target.clone() else {
        return Ok(());
    };
    let request_id = *next_request_id;
    *next_request_id += 1;
    worker.request(GitCommand::CommitFiles {
        request_id,
        revision: revision.clone(),
        path: history_path.cloned(),
    })?;
    *current_request = Some((request_id, revision));
    Ok(())
}

fn render(frame: &mut ratatui::Frame<'_>, state: &AppState) {
    let colors = state.colors();
    frame.render_widget(
        Block::default().style(Style::default().fg(colors.foreground())),
        frame.area(),
    );
    let areas = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(frame.area());
    match &state.screen {
        Screen::Dashboard => render_dashboard(frame, areas[0], state),
        Screen::Diff { .. } => render_diff(frame, areas[0], state),
    }
    let help = match (&state.screen, state.dashboard_page) {
        (Screen::Dashboard, DashboardPage::History) => {
            " q quit  u changes  Tab panels  j/k move  Enter open  y copy SHA  b branches  c compare"
        }
        (Screen::Dashboard, DashboardPage::Changes) => {
            " q quit  h history  Tab focus  j/k select or scroll  Enter diff  D discard changes  b branches"
        }
        (Screen::Diff { direct: true, .. }, _) => {
            " q quit  j/k scroll  n/N hunks  [/] files  / search  s/S matches  t themes"
        }
        (Screen::Diff { direct: false, .. }, _) => {
            " q quit  b/Esc back  B branches  j/k scroll  n/N hunks  [/] files  / search"
        }
    };
    let search_prompt;
    let footer = if state.search_input {
        search_prompt = format!("/{}█", state.search_query);
        search_prompt.as_str()
    } else {
        state
            .theme_message
            .as_deref()
            .or(state.highlight_message.as_deref())
            .unwrap_or(help)
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(colors.muted())),
        areas[1],
    );
    if state.branch_picker {
        render_branch_picker(frame, areas[0], state);
    }
    if state.theme_picker {
        render_theme_picker(frame, state);
    }
    if state.discard_confirmation {
        render_discard_confirmation(frame, state);
    }
}

fn render_dashboard(frame: &mut ratatui::Frame<'_>, area: Rect, state: &AppState) {
    let colors = state.colors();
    if let Some(error) = &state.dashboard_error {
        frame.render_widget(error_panel("Dashboard error", error, colors), area);
        return;
    }
    let Some(data) = &state.data else {
        frame.render_widget(
            panel(
                "Dashboard",
                true,
                Paragraph::new("Loading repository…"),
                colors,
            ),
            area,
        );
        return;
    };
    match state.dashboard_page {
        DashboardPage::History => {
            let columns =
                Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)])
                    .split(area);
            render_history(frame, columns[0], state, data);
            render_commit_files(frame, columns[1], state, data);
        }
        DashboardPage::Changes => {
            let columns =
                Layout::horizontal([Constraint::Percentage(36), Constraint::Percentage(64)])
                    .split(area);
            let sidebar =
                Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(columns[0]);
            render_change_tree(
                frame,
                sidebar[0],
                format!("UNSTAGED · {}", data.unstaged.len()),
                RowFocus {
                    focused: state.focus == Focus::Unstaged,
                    selection_visible: state.focus == Focus::Unstaged
                        || (state.focus == Focus::Diff && state.preview_focus == Focus::Unstaged),
                },
                state.unstaged_selection,
                &data.unstaged,
                colors,
            );
            render_change_tree(
                frame,
                sidebar[1],
                format!("STAGED · {}", data.staged.len()),
                RowFocus {
                    focused: state.focus == Focus::Staged,
                    selection_visible: state.focus == Focus::Staged
                        || (state.focus == Focus::Diff && state.preview_focus == Focus::Staged),
                },
                state.staged_selection,
                &data.staged,
                colors,
            );
            render_change_preview(frame, columns[1], state);
        }
    }
}

fn render_change_preview(frame: &mut ratatui::Frame<'_>, area: Rect, state: &AppState) {
    if state.selected_target().is_none() {
        let colors = state.colors();
        frame.render_widget(
            Paragraph::new("No changed file selected.\n\nYour working tree is clean.")
                .style(Style::default().fg(colors.muted()))
                .block(diff_block(colors, state.focus == Focus::Diff)),
            area,
        );
    } else {
        render_diff(frame, area, state);
    }
}

fn render_history(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    data: &DashboardData,
) {
    let colors = state.colors();
    let rows = data
        .commits
        .iter()
        .map(|commit| {
            Line::from(vec![
                Span::styled(
                    commit.short_id.clone(),
                    Style::default()
                        .fg(colors.accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(commit.date.clone(), Style::default().fg(colors.muted())),
                Span::raw(" "),
                Span::styled(
                    commit.author.clone(),
                    Style::default().fg(colors.color(colors.theme.ui.info)),
                ),
                Span::raw("  "),
                Span::styled(
                    commit.subject.clone(),
                    Style::default().fg(colors.foreground()),
                ),
            ])
        })
        .collect::<Vec<_>>();
    render_rows(
        frame,
        area,
        "History",
        RowFocus::focused(state.focus == Focus::History),
        state.history_selection,
        &rows,
        colors,
    );
}

fn render_commit_files(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    data: &DashboardData,
) {
    let colors = state.colors();
    let title = data
        .commits
        .get(state.history_selection)
        .map(|commit| format!("Files changed in {}", commit.short_id))
        .unwrap_or_else(|| "Files changed".to_owned());
    if let Some(error) = &state.commit_files_error {
        frame.render_widget(error_panel(&title, error, colors), area);
        return;
    }
    if state.commit_files_loading {
        frame.render_widget(
            panel(
                &title,
                state.focus == Focus::CommitFiles,
                Paragraph::new("Loading changed files…"),
                colors,
            ),
            area,
        );
        return;
    }
    render_change_tree(
        frame,
        area,
        title,
        RowFocus::focused(state.focus == Focus::CommitFiles),
        state.commit_file_selection,
        &state.commit_files,
        colors,
    );
}

#[derive(Default)]
struct ChangeTreeNode {
    directories: BTreeMap<OsString, ChangeTreeNode>,
    files: BTreeMap<OsString, usize>,
}

struct ChangeTreeRow {
    line: Line<'static>,
    entry_index: Option<usize>,
}

#[derive(Clone, Copy)]
struct RowFocus {
    focused: bool,
    selection_visible: bool,
}

impl RowFocus {
    fn focused(focused: bool) -> Self {
        Self {
            focused,
            selection_visible: focused,
        }
    }
}

fn render_change_tree<T: Into<String>>(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: T,
    row_focus: RowFocus,
    selection: usize,
    entries: &[ChangeEntry],
    colors: Colors<'_>,
) {
    let rows = change_tree_rows(entries, colors);
    let selected_entry_index = rows.iter().filter_map(|row| row.entry_index).nth(selection);
    let selected_row = selected_entry_index
        .and_then(|entry_index| {
            rows.iter()
                .position(|row| row.entry_index == Some(entry_index))
        })
        .unwrap_or(0);
    let lines = rows.into_iter().map(|row| row.line).collect::<Vec<_>>();
    render_rows(frame, area, title, row_focus, selected_row, &lines, colors);
}

fn change_tree_rows(entries: &[ChangeEntry], colors: Colors<'_>) -> Vec<ChangeTreeRow> {
    let root = build_change_tree(entries);
    let mut rows = Vec::new();
    append_change_tree_rows(&root, "", entries, colors, &mut rows);
    rows
}

fn build_change_tree(entries: &[ChangeEntry]) -> ChangeTreeNode {
    let mut root = ChangeTreeNode::default();
    for (entry_index, entry) in entries.iter().enumerate() {
        let components = entry
            .path
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(name) => Some(name.to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>();
        insert_change_path(&mut root, &components, entry_index);
    }
    root
}

fn selected_tree_entry(entries: &[ChangeEntry], selection: usize) -> Option<&ChangeEntry> {
    selected_tree_entry_index(entries, selection).and_then(|entry_index| entries.get(entry_index))
}

fn selected_tree_entry_index(entries: &[ChangeEntry], selection: usize) -> Option<usize> {
    let root = build_change_tree(entries);
    let mut indexes = Vec::with_capacity(entries.len());
    append_change_tree_indexes(&root, &mut indexes);
    indexes.get(selection).copied()
}

fn tree_selection_for_entry(entries: &[ChangeEntry], entry_index: usize) -> Option<usize> {
    let root = build_change_tree(entries);
    let mut indexes = Vec::with_capacity(entries.len());
    append_change_tree_indexes(&root, &mut indexes);
    indexes.iter().position(|index| *index == entry_index)
}

fn append_change_tree_indexes(node: &ChangeTreeNode, indexes: &mut Vec<usize>) {
    for child in node.directories.values() {
        append_change_tree_indexes(child, indexes);
    }
    indexes.extend(node.files.values().copied());
}

fn insert_change_path(node: &mut ChangeTreeNode, components: &[OsString], entry_index: usize) {
    let Some((name, remaining)) = components.split_first() else {
        return;
    };
    if remaining.is_empty() {
        node.files.insert(name.clone(), entry_index);
    } else {
        insert_change_path(
            node.directories.entry(name.clone()).or_default(),
            remaining,
            entry_index,
        );
    }
}

fn append_change_tree_rows(
    node: &ChangeTreeNode,
    prefix: &str,
    entries: &[ChangeEntry],
    colors: Colors<'_>,
    rows: &mut Vec<ChangeTreeRow>,
) {
    let child_count = node.directories.len() + node.files.len();
    let mut child_index = 0;
    for (name, child) in &node.directories {
        child_index += 1;
        let last = child_index == child_count;
        rows.push(ChangeTreeRow {
            line: directory_tree_row(prefix, last, name, colors),
            entry_index: None,
        });
        let child_prefix = if prefix.is_empty() {
            "   ".to_owned()
        } else {
            format!("{prefix}{}", if last { "   " } else { "│  " })
        };
        append_change_tree_rows(child, &child_prefix, entries, colors, rows);
    }
    for (name, entry_index) in &node.files {
        child_index += 1;
        let last = child_index == child_count;
        rows.push(ChangeTreeRow {
            line: file_tree_row(prefix, last, name, &entries[*entry_index], colors),
            entry_index: Some(*entry_index),
        });
    }
}

fn directory_tree_row(
    prefix: &str,
    last: bool,
    name: &std::ffi::OsStr,
    colors: Colors<'_>,
) -> Line<'static> {
    let branch = if prefix.is_empty() {
        "  ".to_owned()
    } else {
        format!("{prefix}{} ", if last { "└─" } else { "├─" })
    };
    Line::from(vec![
        Span::styled(branch, Style::default().fg(colors.border())),
        Span::styled(
            "",
            Style::default().fg(colors.color(colors.theme.syntax.constant)),
        ),
        Span::raw(" "),
        Span::styled(
            name.to_string_lossy().into_owned(),
            Style::default()
                .fg(colors.accent())
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn file_tree_row(
    prefix: &str,
    last: bool,
    name: &std::ffi::OsStr,
    change: &ChangeEntry,
    colors: Colors<'_>,
) -> Line<'static> {
    let marker_color = match change.kind {
        ChangeKind::Added => colors.color(colors.theme.diff.addition),
        ChangeKind::Modified | ChangeKind::TypeChanged => colors.color(colors.theme.ui.warning),
        ChangeKind::Deleted => colors.color(colors.theme.diff.deletion),
        ChangeKind::Renamed | ChangeKind::Copied => colors.color(colors.theme.diff.header),
        ChangeKind::Untracked => colors.color(colors.theme.ui.info),
        ChangeKind::Unmerged => colors.color(colors.theme.ui.error),
        ChangeKind::Unknown => colors.muted(),
    };
    let icon = file_icon::for_path(&change.path);
    let branch = if prefix.is_empty() {
        "  ".to_owned()
    } else {
        format!("{prefix}{} ", if last { "└─" } else { "├─" })
    };
    Line::from(vec![
        Span::styled(branch, Style::default().fg(colors.border())),
        Span::styled(
            change_marker(change.kind),
            Style::default()
                .fg(marker_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            icon.glyph,
            Style::default().fg(file_icon_color(icon.color, colors)),
        ),
        Span::raw(" "),
        Span::styled(
            name.to_string_lossy().into_owned(),
            Style::default().fg(colors.foreground()),
        ),
    ])
}

fn render_rows<T: Into<String>>(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: T,
    row_focus: RowFocus,
    selection: usize,
    rows: &[Line<'static>],
    colors: Colors<'_>,
) {
    let visible = area.height.saturating_sub(2) as usize;
    let start = selection
        .saturating_sub(visible.saturating_sub(1))
        .min(rows.len().saturating_sub(visible));
    let lines = if rows.is_empty() {
        vec![Line::styled(
            "  (none)",
            Style::default().fg(colors.muted()),
        )]
    } else {
        rows.iter()
            .enumerate()
            .skip(start)
            .take(visible)
            .map(|(index, row)| {
                let selected = row_focus.selection_visible && index == selection;
                let mut spans = Vec::with_capacity(row.spans.len() + 1);
                spans.push(Span::styled(
                    if selected { "▎ " } else { "  " },
                    if selected {
                        Style::default()
                            .fg(colors.accent())
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(colors.muted())
                    },
                ));
                spans.extend(row.spans.iter().cloned().map(|mut span| {
                    let selection_style = if selected {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    span.style = span.style.patch(selection_style);
                    span
                }));
                Line::from(spans)
            })
            .collect()
    };
    let block = Block::default()
        .title(format!(" {} ", title.into()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if row_focus.focused {
            colors.focused_border()
        } else {
            colors.border()
        }))
        .title_style(
            Style::default()
                .fg(if row_focus.focused {
                    colors.accent()
                } else {
                    colors.muted()
                })
                .add_modifier(if row_focus.focused {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        );
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_diff(frame: &mut ratatui::Frame<'_>, area: Rect, state: &AppState) {
    let colors = state.colors();
    let focused = matches!(state.screen, Screen::Diff { .. }) || state.focus == Focus::Diff;
    if let Some(message) = &state.diff_error {
        frame.render_widget(
            Paragraph::new(format!("Git error\n\n{message}"))
                .style(Style::default().fg(colors.color(colors.theme.ui.error)))
                .block(diff_block(colors, focused)),
            area,
        );
    } else if let Some(document) = &state.diff {
        let available = area.height.saturating_sub(2) as usize;
        let scroll = diff_scroll_start(document, state.diff_scroll, available);
        let lines = document
            .lines
            .iter()
            .enumerate()
            .skip(scroll)
            .filter(|(_, line)| is_visible_diff_line(line.kind))
            .take(available)
            .map(|(line_index, line)| {
                let line_numbers = match (line.old_line, line.new_line) {
                    (Some(old), Some(new)) => format!("{old:>5} {new:>5}"),
                    (Some(old), None) => format!("{old:>5}      "),
                    (None, Some(new)) => format!("      {new:>5}"),
                    _ => "           ".to_owned(),
                };
                let mut style = line_style(line.kind, colors);
                let is_search_match = state.search_matches.binary_search(&line_index).is_ok();
                let is_active_match =
                    state.search_matches.get(state.search_match_index) == Some(&line_index);
                if is_active_match {
                    style = style
                        .fg(colors.accent())
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
                } else if is_search_match {
                    style = style
                        .fg(colors.color(colors.theme.ui.search_match))
                        .add_modifier(Modifier::UNDERLINED);
                }
                let gutter_background = style.bg;
                let highlighted_foreground = is_search_match.then_some(
                    style
                        .fg
                        .expect("search match styles always define a foreground"),
                );
                let mut line_number_style = Style::default().fg(highlighted_foreground
                    .unwrap_or_else(|| colors.color(colors.theme.diff.line_number)));
                let mut gutter_style = Style::default().fg(highlighted_foreground
                    .unwrap_or_else(|| colors.color(colors.theme.diff.gutter)));
                if let Some(background) = gutter_background {
                    line_number_style = line_number_style.bg(background);
                    gutter_style = gutter_style.bg(background);
                }
                let mut spans = vec![
                    Span::styled(line_numbers, line_number_style),
                    Span::styled(" │ ", gutter_style),
                ];
                let text = document.line_text(line);
                let content_offset = diff_content_offset(line.kind, text);
                if !is_search_match {
                    if let Some(highlighted) = state
                        .highlighted
                        .as_ref()
                        .and_then(|highlighted| highlighted.lines.get(line_index))
                    {
                        let mut offset = content_offset;
                        for highlighted_span in highlighted {
                            if highlighted_span.range.start > offset {
                                spans.push(Span::styled(
                                    &text[offset..highlighted_span.range.start],
                                    style,
                                ));
                            }
                            let syntax_style =
                                style.fg(syntax_color(highlighted_span.token, colors));
                            spans.push(Span::styled(
                                &text[highlighted_span.range.clone()],
                                syntax_style,
                            ));
                            offset = highlighted_span.range.end;
                        }
                        if offset < text.len() {
                            spans.push(Span::styled(&text[offset..], style));
                        }
                    } else {
                        spans.push(Span::styled(&text[content_offset..], style));
                    }
                } else {
                    spans.push(Span::styled(&text[content_offset..], style));
                }
                if style.bg.is_some() {
                    spans.push(Span::styled(" ".repeat(area.width as usize), style));
                }
                Line::from(spans)
            })
            .collect::<Vec<_>>();
        let paragraph = if lines.is_empty() {
            Paragraph::new("No differences.")
                .style(Style::default().fg(colors.foreground()))
                .block(diff_block(colors, focused))
        } else {
            Paragraph::new(lines)
                .style(Style::default().fg(colors.foreground()))
                .block(diff_block(colors, focused))
        };
        frame.render_widget(paragraph, area);
    } else {
        frame.render_widget(
            Paragraph::new("Loading diff…")
                .style(Style::default().fg(colors.foreground()))
                .block(diff_block(colors, focused)),
            area,
        );
    }
}

fn diff_block(colors: Colors<'_>, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            colors.focused_border()
        } else {
            colors.border()
        }))
        .style(Style::default().fg(colors.foreground()))
}

fn is_visible_diff_line(kind: LineKind) -> bool {
    // Keep the viewport focused on reviewable changes. Git patch transport
    // syntax and hunk coordinates remain in the parsed document for
    // navigation, but are not rendered.
    matches!(
        kind,
        LineKind::Addition
            | LineKind::Deletion
            | LineKind::Context
            | LineKind::Binary
            | LineKind::Rename
            | LineKind::NewFile
            | LineKind::DeletedFile
    )
}

fn diff_scroll_start(document: &DiffDocument, requested_line: usize, available: usize) -> usize {
    let visible_lines = document
        .lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| is_visible_diff_line(line.kind).then_some(index))
        .collect::<Vec<_>>();
    if visible_lines.is_empty() {
        return 0;
    }
    let requested_position = visible_lines
        .partition_point(|line_index| *line_index < requested_line)
        .min(visible_lines.len() - 1);
    let last_full_page_start = visible_lines.len().saturating_sub(available.max(1));
    visible_lines[requested_position.min(last_full_page_start)]
}

fn diff_content_offset(kind: LineKind, text: &str) -> usize {
    if matches!(
        kind,
        LineKind::Addition | LineKind::Deletion | LineKind::Context
    ) && !text.is_empty()
    {
        1
    } else {
        0
    }
}

fn panel<'a>(
    title: &'a str,
    focused: bool,
    content: Paragraph<'a>,
    colors: Colors<'_>,
) -> Paragraph<'a> {
    content.block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused {
                colors.focused_border()
            } else {
                colors.border()
            }))
            .title_style(
                Style::default()
                    .fg(if focused {
                        colors.accent()
                    } else {
                        colors.muted()
                    })
                    .add_modifier(if focused {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
    )
}

fn error_panel<'a>(title: &'a str, message: &'a str, colors: Colors<'_>) -> Paragraph<'a> {
    panel(
        title,
        true,
        Paragraph::new(message).style(Style::default().fg(colors.color(colors.theme.ui.error))),
        colors,
    )
}

fn change_marker(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "A",
        ChangeKind::Modified => "M",
        ChangeKind::Deleted => "D",
        ChangeKind::Renamed => "R",
        ChangeKind::Copied => "C",
        ChangeKind::Untracked => "?",
        ChangeKind::TypeChanged => "T",
        ChangeKind::Unmerged => "U",
        ChangeKind::Unknown => "·",
    }
}

fn file_icon_color(color: IconColor, colors: Colors<'_>) -> Color {
    colors.color(match color {
        IconColor::Blue => colors.theme.syntax.function,
        IconColor::Cyan => colors.theme.syntax.r#type,
        IconColor::Green => colors.theme.syntax.string,
        IconColor::Yellow => colors.theme.syntax.constant,
        IconColor::Orange => colors.theme.syntax.number,
        IconColor::Red => colors.theme.ui.error,
        IconColor::Purple => colors.theme.syntax.keyword,
        IconColor::Pink => colors.theme.syntax.tag,
        IconColor::Muted => colors.theme.ui.muted,
    })
}

fn line_style(kind: LineKind, colors: Colors<'_>) -> Style {
    match kind {
        LineKind::Addition => Style::default()
            .fg(colors.color(colors.theme.diff.addition))
            .bg(colors.color(colors.theme.diff.addition_background)),
        LineKind::Deletion => Style::default()
            .fg(colors.color(colors.theme.diff.deletion))
            .bg(colors.color(colors.theme.diff.deletion_background)),
        LineKind::HunkHeader => Style::default().fg(colors.color(colors.theme.diff.hunk_header)),
        LineKind::FileHeader => Style::default()
            .fg(colors.color(colors.theme.diff.header))
            .add_modifier(Modifier::BOLD),
        LineKind::Context => Style::default().fg(colors.color(colors.theme.diff.context)),
        LineKind::Metadata => Style::default().fg(colors.color(colors.theme.diff.metadata)),
        LineKind::Binary => Style::default()
            .fg(colors.color(colors.theme.ui.warning))
            .add_modifier(Modifier::BOLD),
        LineKind::Rename => Style::default().fg(colors.color(colors.theme.ui.info)),
        LineKind::NewFile => Style::default()
            .fg(colors.color(colors.theme.diff.addition))
            .add_modifier(Modifier::BOLD),
        LineKind::DeletedFile => Style::default()
            .fg(colors.color(colors.theme.diff.deletion))
            .add_modifier(Modifier::BOLD),
        LineKind::NoNewline => Style::default().fg(colors.color(colors.theme.ui.warning)),
    }
}

fn syntax_color(token: SyntaxToken, colors: Colors<'_>) -> Color {
    let syntax = &colors.theme.syntax;
    colors.color(match token {
        SyntaxToken::Comment => syntax.comment,
        SyntaxToken::Keyword => syntax.keyword,
        SyntaxToken::String => syntax.string,
        SyntaxToken::Number => syntax.number,
        SyntaxToken::Function => syntax.function,
        SyntaxToken::Type => syntax.r#type,
        SyntaxToken::Variable => syntax.variable,
        SyntaxToken::Constant => syntax.constant,
        SyntaxToken::Operator => syntax.operator,
        SyntaxToken::Punctuation => syntax.punctuation,
        SyntaxToken::Property => syntax.property,
        SyntaxToken::Tag => syntax.tag,
        SyntaxToken::Attribute => syntax.attribute,
        SyntaxToken::Invalid => syntax.invalid,
    })
}

fn render_branch_picker(frame: &mut ratatui::Frame<'_>, content_area: Rect, state: &AppState) {
    let colors = state.colors();
    let vertical = Layout::vertical([
        Constraint::Percentage(10),
        Constraint::Percentage(80),
        Constraint::Percentage(10),
    ])
    .split(content_area);
    let area = Layout::horizontal([
        Constraint::Percentage(18),
        Constraint::Percentage(64),
        Constraint::Percentage(18),
    ])
    .split(vertical[1])[1];
    frame.render_widget(Clear, area);
    let Some(data) = &state.data else {
        frame.render_widget(
            panel(
                "Branches",
                true,
                Paragraph::new("Loading branches…"),
                colors,
            ),
            area,
        );
        return;
    };
    let rows = data
        .branches
        .iter()
        .map(|branch| {
            Line::from(vec![
                Span::styled(
                    if branch.current { "●" } else { "○" },
                    Style::default().fg(if branch.current {
                        colors.color(colors.theme.diff.addition)
                    } else {
                        colors.muted()
                    }),
                ),
                Span::raw(" "),
                Span::styled(
                    branch.name.to_string_lossy().into_owned(),
                    Style::default()
                        .fg(if branch.current {
                            colors.accent()
                        } else {
                            colors.foreground()
                        })
                        .add_modifier(if branch.current {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
                Span::raw("  "),
                Span::styled(
                    branch.short_id.clone(),
                    Style::default().fg(colors.color(colors.theme.ui.warning)),
                ),
                Span::raw("  "),
                Span::styled(branch.subject.clone(), Style::default().fg(colors.muted())),
            ])
        })
        .collect::<Vec<_>>();
    let title = state
        .comparison_base
        .as_ref()
        .map(|branch| {
            format!(
                "Compare from {} — c select, Esc cancel",
                branch.to_string_lossy()
            )
        })
        .unwrap_or_else(|| "Branches — Enter inspect, c compare, j/k move, b/Esc close".to_owned());
    render_rows(
        frame,
        area,
        title,
        RowFocus::focused(true),
        state.branch_selection,
        &rows,
        colors,
    );
}

fn render_theme_picker(frame: &mut ratatui::Frame<'_>, state: &AppState) {
    let colors = state.colors();
    let vertical = Layout::vertical([
        Constraint::Percentage(15),
        Constraint::Percentage(70),
        Constraint::Percentage(15),
    ])
    .split(frame.area());
    let area = Layout::horizontal([
        Constraint::Percentage(25),
        Constraint::Percentage(50),
        Constraint::Percentage(25),
    ])
    .split(vertical[1])[1];
    frame.render_widget(Clear, area);
    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    let visible = inner.height.saturating_sub(2) as usize;
    let start = state
        .theme_index
        .saturating_sub(visible.saturating_sub(1))
        .min(state.themes.len().saturating_sub(visible));
    let rows = state
        .themes
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(index, theme)| {
            let confirmed = index == state.confirmed_theme_index;
            Line::styled(
                format!(
                    "{} {}{}",
                    if index == state.theme_index {
                        "›"
                    } else {
                        " "
                    },
                    theme.name,
                    if confirmed { "  ✓" } else { "" }
                ),
                if index == state.theme_index {
                    Style::default()
                        .fg(colors.accent())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(colors.foreground())
                },
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(rows).block(
            Block::default()
                .title(" Themes — ↑/↓ preview, Enter confirm, Esc cancel ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(colors.accent())),
        ),
        inner,
    );
}

fn render_discard_confirmation(frame: &mut ratatui::Frame<'_>, state: &AppState) {
    let colors = state.colors();
    let vertical = Layout::vertical([
        Constraint::Percentage(35),
        Constraint::Length(9),
        Constraint::Percentage(35),
    ])
    .split(frame.area());
    let area = Layout::horizontal([
        Constraint::Percentage(22),
        Constraint::Percentage(56),
        Constraint::Percentage(22),
    ])
    .split(vertical[1])[1];
    frame.render_widget(Clear, area);
    let warning = colors.color(colors.theme.ui.warning);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                "Discard every tracked change in the CHANGES section?",
                Style::default()
                    .fg(colors.foreground())
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::styled(
                "Files will be restored to their staged versions.",
                Style::default().fg(colors.muted()),
            ),
            Line::styled(
                "Staged changes and untracked files will be kept.",
                Style::default().fg(colors.muted()),
            ),
            Line::raw(""),
            Line::styled(
                "Enter/y confirm    Esc/n cancel",
                Style::default().fg(warning),
            ),
        ])
        .block(
            Block::default()
                .title(" Confirm discard ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(warning)),
        ),
        area,
    );
}

fn terminal_supports_true_color() -> bool {
    std::env::var("TERM_PROGRAM").is_ok_and(|value| value.eq_ignore_ascii_case("ghostty"))
        || std::env::var("COLORTERM").is_ok_and(|value| {
            value.eq_ignore_ascii_case("truecolor") || value.eq_ignore_ascii_case("24bit")
        })
        || std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
}

fn next_hunk(document: Option<&DiffDocument>, scroll: usize) -> usize {
    document
        .and_then(|doc| {
            doc.lines
                .iter()
                .enumerate()
                .skip(scroll + 1)
                .find(|(_, line)| line.kind == LineKind::HunkHeader)
                .map(|(index, _)| index)
        })
        .unwrap_or(scroll)
}

fn previous_hunk(document: Option<&DiffDocument>, scroll: usize) -> usize {
    document
        .and_then(|doc| {
            doc.lines
                .iter()
                .enumerate()
                .take(scroll)
                .rev()
                .find(|(_, line)| line.kind == LineKind::HunkHeader)
                .map(|(index, _)| index)
        })
        .unwrap_or(scroll)
}

fn next_file(document: Option<&DiffDocument>, scroll: usize) -> usize {
    document
        .and_then(|doc| {
            doc.lines
                .iter()
                .enumerate()
                .skip(scroll + 1)
                .find(|(_, line)| {
                    line.kind == LineKind::FileHeader
                        && doc.line_text(line).starts_with("diff --git ")
                })
                .map(|(index, _)| index)
        })
        .unwrap_or(scroll)
}

fn previous_file(document: Option<&DiffDocument>, scroll: usize) -> usize {
    document
        .and_then(|doc| {
            doc.lines
                .iter()
                .enumerate()
                .take(scroll)
                .rev()
                .find(|(_, line)| {
                    line.kind == LineKind::FileHeader
                        && doc.line_text(line).starts_with("diff --git ")
                })
                .map(|(index, _)| index)
        })
        .unwrap_or(scroll)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_selection_follows_visual_directory_order() {
        let entries = [
            change("README.md"),
            change("src/lib.rs"),
            change("src/app.rs"),
            change("assets/icon.svg"),
        ];

        let ordered = (0..entries.len())
            .map(|selection| {
                selected_tree_entry(&entries, selection)
                    .unwrap()
                    .path
                    .clone()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            ordered,
            ["assets/icon.svg", "src/app.rs", "src/lib.rs", "README.md"]
                .map(std::path::PathBuf::from)
        );
        assert_eq!(tree_selection_for_entry(&entries, 0), Some(3));
        assert_eq!(tree_selection_for_entry(&entries, 2), Some(1));
    }

    #[test]
    fn hides_redundant_raw_patch_headers() {
        assert!(!is_visible_diff_line(LineKind::FileHeader));
        assert!(!is_visible_diff_line(LineKind::HunkHeader));
        assert!(!is_visible_diff_line(LineKind::Metadata));
        assert!(!is_visible_diff_line(LineKind::NoNewline));
        assert!(is_visible_diff_line(LineKind::Addition));
        assert!(is_visible_diff_line(LineKind::Context));
    }

    #[test]
    fn removes_patch_markers_from_rendered_code() {
        assert_eq!(diff_content_offset(LineKind::Addition, "+new"), 1);
        assert_eq!(diff_content_offset(LineKind::Deletion, "-old"), 1);
        assert_eq!(diff_content_offset(LineKind::Context, " same"), 1);
        assert_eq!(
            diff_content_offset(LineKind::Binary, "Binary files differ"),
            0
        );
    }

    #[test]
    fn tab_cycles_unstaged_staged_and_diff() {
        let (focus, source) = next_changes_focus(Focus::Unstaged, Focus::Unstaged, true, true);
        assert_eq!((focus, source), (Focus::Staged, Focus::Staged));

        let (focus, source) = next_changes_focus(focus, source, true, true);
        assert_eq!((focus, source), (Focus::Diff, Focus::Staged));

        let (focus, source) = next_changes_focus(focus, source, true, true);
        assert_eq!((focus, source), (Focus::Unstaged, Focus::Unstaged));
    }

    #[test]
    fn tab_skips_empty_change_lists() {
        let (focus, source) = next_changes_focus(Focus::Staged, Focus::Staged, true, false);
        assert_eq!((focus, source), (Focus::Diff, Focus::Staged));
        let (focus, source) = next_changes_focus(focus, source, true, false);
        assert_eq!((focus, source), (Focus::Staged, Focus::Staged));

        let (focus, source) = next_changes_focus(Focus::Unstaged, Focus::Unstaged, false, true);
        assert_eq!((focus, source), (Focus::Diff, Focus::Unstaged));
        let (focus, source) = next_changes_focus(focus, source, false, true);
        assert_eq!((focus, source), (Focus::Unstaged, Focus::Unstaged));
    }

    #[test]
    fn tab_returns_to_an_available_file_list_after_live_update() {
        assert_eq!(
            next_changes_focus(Focus::Diff, Focus::Staged, false, true),
            (Focus::Unstaged, Focus::Unstaged)
        );
        assert_eq!(
            next_changes_focus(Focus::Diff, Focus::Unstaged, true, false),
            (Focus::Staged, Focus::Staged)
        );
    }

    #[test]
    fn diff_scrolling_counts_only_visible_lines() {
        let document = DiffDocument::parse(
            b"diff --git a/a b/a\nindex 1..2 100644\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n-old\n+new\n same\n"
                .to_vec(),
        );

        assert_eq!(move_diff_scroll(Some(&document), 0, 1), 6);
        assert_eq!(move_diff_scroll(Some(&document), 6, -1), 5);
        assert_eq!(move_diff_scroll(Some(&document), 0, 20), 7);
        assert_eq!(diff_scroll_start(&document, usize::MAX, 2), 6);
    }

    fn change(path: &str) -> ChangeEntry {
        ChangeEntry {
            path: path.into(),
            kind: ChangeKind::Modified,
        }
    }
}
