use std::{ffi::OsString, time::Duration};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    Terminal,
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::{
    cli::LaunchTarget,
    config::{AppConfig, ConfigPaths, ThemeConfig},
    dashboard::{ChangeEntry, ChangeKind, DashboardData},
    diff::{DiffDocument, LineKind},
    highlight::{HighlightedDiff, SyntaxToken},
    repository::Repository,
    theme::{LoadedTheme, Rgb, Theme},
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

    fn background(self) -> Color {
        self.color(self.theme.ui.background)
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
    fn selected(self) -> Color {
        self.color(self.theme.ui.selection)
    }
    fn selection_foreground(self) -> Color {
        self.color(self.theme.ui.selection_foreground)
    }
    fn panel(self) -> Color {
        self.color(self.theme.ui.panel)
    }
    fn focused_border(self) -> Color {
        self.color(self.theme.ui.focused_border)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Branches,
    History,
    Staged,
    Unstaged,
}

impl Focus {
    fn next(self) -> Self {
        match self {
            Self::Branches => Self::History,
            Self::History => Self::Staged,
            Self::Staged => Self::Unstaged,
            Self::Unstaged => Self::Branches,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Branches => Self::Unstaged,
            Self::History => Self::Branches,
            Self::Staged => Self::History,
            Self::Unstaged => Self::Staged,
        }
    }
}

enum Screen {
    Dashboard,
    Diff { target: LaunchTarget, direct: bool },
}

struct AppState {
    screen: Screen,
    focus: Focus,
    data: Option<DashboardData>,
    dashboard_error: Option<String>,
    diff: Option<DiffDocument>,
    diff_error: Option<String>,
    highlighted: Option<HighlightedDiff>,
    highlight_message: Option<String>,
    diff_scroll: usize,
    branch_selection: usize,
    history_selection: usize,
    staged_selection: usize,
    unstaged_selection: usize,
    comparison_base: Option<OsString>,
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
        let focus = match target {
            LaunchTarget::Branches => Focus::Branches,
            LaunchTarget::Changes => Focus::Unstaged,
            _ => Focus::History,
        };
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
            focus,
            data: None,
            dashboard_error: None,
            diff: None,
            diff_error: None,
            highlighted: None,
            highlight_message: None,
            diff_scroll: 0,
            branch_selection: 0,
            history_selection: 0,
            staged_selection: 0,
            unstaged_selection: 0,
            comparison_base: None,
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

    fn move_selection(&mut self, delta: isize) {
        let Some(data) = &self.data else { return };
        let (selection, len) = match self.focus {
            Focus::Branches => (&mut self.branch_selection, data.branches.len()),
            Focus::History => (&mut self.history_selection, data.commits.len()),
            Focus::Staged => (&mut self.staged_selection, data.staged.len()),
            Focus::Unstaged => (&mut self.unstaged_selection, data.unstaged.len()),
        };
        if len == 0 {
            *selection = 0;
        } else {
            *selection =
                (*selection as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize;
        }
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
            Focus::Branches => {
                data.branches
                    .get(self.branch_selection)
                    .map(|branch| LaunchTarget::Show {
                        revision: branch.name.clone(),
                        path: None,
                    })
            }
            Focus::Staged => {
                data.staged
                    .get(self.staged_selection)
                    .map(|change| LaunchTarget::Staged {
                        path: change.path.clone(),
                    })
            }
            Focus::Unstaged => {
                data.unstaged
                    .get(self.unstaged_selection)
                    .map(|change| LaunchTarget::WorkingTree {
                        path: change.path.clone(),
                    })
            }
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
                document
                    .line_text(line)
                    .to_lowercase()
                    .contains(&query)
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
    let mut next_request_id = 1u64;
    let mut dashboard_request = None;
    let mut diff_request = None;
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
                        if state.focus == Focus::Unstaged
                            && data.unstaged.is_empty()
                            && !data.staged.is_empty()
                        {
                            state.focus = Focus::Staged;
                        }
                        state.data = Some(data);
                    }
                    Ok(GitPayload::Diff(_)) => {}
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
                        state.diff = Some(diff);
                    }
                    Ok(GitPayload::Dashboard(_)) => {}
                    Err(error) => state.diff_error = Some(error.to_string()),
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

        if dirty {
            terminal.draw(|frame| render(frame, &repo, &state))?;
            dirty = false;
        }

        let loading =
            dashboard_request.is_some() || diff_request.is_some() || highlight_request.is_some();
        if !event::poll(if loading {
            Duration::from_millis(25)
        } else {
            Duration::from_secs(60)
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
                KeyCode::Tab | KeyCode::Right => state.focus = state.focus.next(),
                KeyCode::BackTab | KeyCode::Left => state.focus = state.focus.previous(),
                KeyCode::Char('j') | KeyCode::Down => state.move_selection(1),
                KeyCode::Char('k') | KeyCode::Up => state.move_selection(-1),
                KeyCode::PageDown => state.move_selection(10),
                KeyCode::PageUp => state.move_selection(-10),
                KeyCode::Home | KeyCode::Char('g') => state.move_selection(isize::MIN),
                KeyCode::End | KeyCode::Char('G') => state.move_selection(isize::MAX),
                KeyCode::Enter => {
                    if let Some(target) = state.selected_target() {
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
                KeyCode::Char('c') if state.focus == Focus::Branches => {
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
                            }
                        } else {
                            state.comparison_base = Some(branch.name.clone());
                        }
                    }
                }
                KeyCode::Char('r') => {
                    state.dashboard_error = None;
                    let request_id = next_request_id;
                    next_request_id += 1;
                    git_worker.request(GitCommand::Dashboard {
                        request_id,
                        history_path: history_path.clone(),
                    })?;
                    dashboard_request = Some(request_id);
                }
                _ => continue,
            },
            Screen::Diff { direct, .. } => match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Esc | KeyCode::Char('b') if !direct => {
                    state.screen = Screen::Dashboard;
                    state.diff = None;
                    state.diff_error = None;
                    state.highlighted = None;
                    state.highlight_message = None;
                    state.search_matches.clear();
                    state.diff_scroll = 0;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    state.diff_scroll = state.diff_scroll.saturating_add(1)
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    state.diff_scroll = state.diff_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => state.diff_scroll = state.diff_scroll.saturating_add(20),
                KeyCode::PageUp => state.diff_scroll = state.diff_scroll.saturating_sub(20),
                KeyCode::Home | KeyCode::Char('g') => state.diff_scroll = 0,
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

fn render(frame: &mut ratatui::Frame<'_>, repo: &Repository, state: &AppState) {
    let colors = state.colors();
    let background = Style::default()
        .fg(colors.foreground())
        .bg(colors.background());
    frame.render_widget(Block::default().style(background), frame.area());
    let areas = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(frame.area());
    let title = match &state.screen {
        Screen::Dashboard => "dashboard",
        Screen::Diff { target, .. } => target_name(target),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " raccoon ",
                Style::default()
                    .fg(colors.accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} — {title}", repo.root.display()),
                Style::default().fg(colors.muted()),
            ),
        ]))
        .style(background),
        areas[0],
    );
    match &state.screen {
        Screen::Dashboard => render_dashboard(frame, areas[1], state),
        Screen::Diff { .. } => render_diff(frame, areas[1], state),
    }
    let help = match &state.screen {
        Screen::Dashboard => {
            " q quit  Tab/←/→ panels  j/k move  Enter open  c compare  r refresh  t themes"
        }
        Screen::Diff { direct: true, .. } => {
            " q quit  j/k scroll  n/N hunks  [/] files  / search  s/S matches  t themes"
        }
        Screen::Diff { direct: false, .. } => {
            " q quit  b/Esc back  j/k scroll  n/N hunks  [/] files  / search  s/S matches"
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
        Paragraph::new(footer).style(Style::default().fg(colors.muted()).bg(colors.background())),
        areas[2],
    );
    if state.theme_picker {
        render_theme_picker(frame, state);
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
    let columns = Layout::horizontal([
        Constraint::Percentage(24),
        Constraint::Percentage(48),
        Constraint::Percentage(28),
    ])
    .split(area);
    render_branches(frame, columns[0], state, data);
    render_history(frame, columns[1], state, data);
    let changes = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(columns[2]);
    render_changes(
        frame,
        changes[0],
        "Staged",
        state.staged_selection,
        &data.staged,
        state.focus == Focus::Staged,
        colors,
    );
    render_changes(
        frame,
        changes[1],
        "Unstaged",
        state.unstaged_selection,
        &data.unstaged,
        state.focus == Focus::Unstaged,
        colors,
    );
}

fn render_branches(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    data: &DashboardData,
) {
    let colors = state.colors();
    let rows = data
        .branches
        .iter()
        .map(|branch| {
            let marker = if branch.current { "●" } else { " " };
            format!(
                "{marker} {}  {}",
                branch.name.to_string_lossy(),
                branch.short_id
            )
        })
        .collect::<Vec<_>>();
    render_rows(
        frame,
        area,
        branch_title(state),
        state.focus == Focus::Branches,
        state.branch_selection,
        &rows,
        colors,
    );
}

fn branch_title(state: &AppState) -> String {
    state
        .comparison_base
        .as_ref()
        .map(|branch| format!("Branches [compare: {}]", branch.to_string_lossy()))
        .unwrap_or_else(|| "Branches".to_owned())
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
            format!(
                "{} {} {}  {}",
                commit.short_id, commit.date, commit.author, commit.subject
            )
        })
        .collect::<Vec<_>>();
    render_rows(
        frame,
        area,
        "History",
        state.focus == Focus::History,
        state.history_selection,
        &rows,
        colors,
    );
}

fn render_changes(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    selection: usize,
    entries: &[ChangeEntry],
    focused: bool,
    colors: Colors<'_>,
) {
    let rows = entries
        .iter()
        .map(|change| {
            format!(
                "{} {}",
                change_marker(change.kind),
                change.path.to_string_lossy()
            )
        })
        .collect::<Vec<_>>();
    render_rows(frame, area, title, focused, selection, &rows, colors);
}

fn render_rows<T: Into<String>>(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: T,
    focused: bool,
    selection: usize,
    rows: &[String],
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
                let selected = focused && index == selection;
                Line::styled(
                    format!("{} {row}", if selected { "›" } else { " " }),
                    if selected {
                        Style::default()
                            .fg(colors.selection_foreground())
                            .bg(colors.selected())
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(colors.foreground()).bg(colors.panel())
                    },
                )
            })
            .collect()
    };
    let block = Block::default()
        .title(format!(" {} ", title.into()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            colors.focused_border()
        } else {
            colors.border()
        }))
        .style(Style::default().bg(colors.panel()));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_diff(frame: &mut ratatui::Frame<'_>, area: Rect, state: &AppState) {
    let colors = state.colors();
    let body = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(colors.border()))
        .style(
            Style::default()
                .fg(colors.foreground())
                .bg(colors.background()),
        );
    if let Some(message) = &state.diff_error {
        frame.render_widget(
            Paragraph::new(format!("Git error\n\n{message}"))
                .style(
                    Style::default()
                        .fg(colors.color(colors.theme.ui.error))
                        .bg(colors.background()),
                )
                .block(body),
            area,
        );
    } else if let Some(document) = &state.diff {
        let available = area.height.saturating_sub(1) as usize;
        let scroll = state
            .diff_scroll
            .min(document.lines.len().saturating_sub(available));
        let lines = document
            .lines
            .iter()
            .enumerate()
            .skip(scroll)
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
                        .fg(colors.selection_foreground())
                        .bg(colors.color(colors.theme.diff.selected_background));
                } else if is_search_match {
                    style = style
                        .fg(colors.color(colors.theme.ui.search_match_foreground))
                        .bg(colors.color(colors.theme.ui.search_match));
                }
                let gutter_background = style.bg.unwrap_or(colors.background());
                let highlighted_foreground = is_search_match.then_some(
                    style
                        .fg
                        .expect("search match styles always define a foreground"),
                );
                let mut spans = vec![
                    Span::styled(
                        line_numbers,
                        Style::default()
                            .fg(highlighted_foreground
                                .unwrap_or_else(|| colors.color(colors.theme.diff.line_number)))
                            .bg(gutter_background),
                    ),
                    Span::styled(
                        " │ ",
                        Style::default()
                            .fg(highlighted_foreground
                                .unwrap_or_else(|| colors.color(colors.theme.diff.gutter)))
                            .bg(gutter_background),
                    ),
                ];
                let text = document.line_text(line);
                if !is_search_match {
                    if let Some(highlighted) = state
                        .highlighted
                        .as_ref()
                        .and_then(|highlighted| highlighted.lines.get(line_index))
                    {
                        let mut offset = 0;
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
                        spans.push(Span::styled(text, style));
                    }
                } else {
                    spans.push(Span::styled(text, style));
                }
                Line::from(spans)
            })
            .collect::<Vec<_>>();
        let paragraph = if lines.is_empty() {
            Paragraph::new("No differences.")
                .style(
                    Style::default()
                        .fg(colors.foreground())
                        .bg(colors.background()),
                )
                .block(body)
        } else {
            Paragraph::new(lines)
                .style(
                    Style::default()
                        .fg(colors.foreground())
                        .bg(colors.background()),
                )
                .block(body)
        };
        frame.render_widget(paragraph, area);
    } else {
        frame.render_widget(
            Paragraph::new("Loading diff…")
                .style(
                    Style::default()
                        .fg(colors.foreground())
                        .bg(colors.background()),
                )
                .block(body),
            area,
        );
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
            .style(Style::default().bg(colors.panel())),
    )
}

fn error_panel<'a>(title: &'a str, message: &'a str, colors: Colors<'_>) -> Paragraph<'a> {
    panel(
        title,
        true,
        Paragraph::new(message).style(
            Style::default()
                .fg(colors.color(colors.theme.ui.error))
                .bg(colors.background()),
        ),
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

fn line_style(kind: LineKind, colors: Colors<'_>) -> Style {
    match kind {
        LineKind::Addition => Style::default()
            .fg(colors.color(colors.theme.diff.addition))
            .bg(colors.color(colors.theme.diff.addition_background)),
        LineKind::Deletion => Style::default()
            .fg(colors.color(colors.theme.diff.deletion))
            .bg(colors.color(colors.theme.diff.deletion_background)),
        LineKind::HunkHeader => Style::default()
            .fg(colors.color(colors.theme.diff.hunk_header))
            .bg(colors.color(colors.theme.ui.panel)),
        LineKind::FileHeader => Style::default()
            .fg(colors.color(colors.theme.diff.header))
            .add_modifier(Modifier::BOLD)
            .bg(colors.background()),
        LineKind::Context => Style::default()
            .fg(colors.color(colors.theme.diff.context))
            .bg(colors.background()),
        LineKind::Metadata => Style::default()
            .fg(colors.color(colors.theme.diff.metadata))
            .bg(colors.background()),
        LineKind::Binary => Style::default()
            .fg(colors.color(colors.theme.ui.warning))
            .bg(colors.color(colors.theme.ui.panel))
            .add_modifier(Modifier::BOLD),
        LineKind::Rename => Style::default()
            .fg(colors.color(colors.theme.ui.info))
            .bg(colors.background()),
        LineKind::NewFile => Style::default()
            .fg(colors.color(colors.theme.diff.addition))
            .bg(colors.background())
            .add_modifier(Modifier::BOLD),
        LineKind::DeletedFile => Style::default()
            .fg(colors.color(colors.theme.diff.deletion))
            .bg(colors.background())
            .add_modifier(Modifier::BOLD),
        LineKind::NoNewline => Style::default()
            .fg(colors.color(colors.theme.ui.warning))
            .bg(colors.background()),
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
    frame.render_widget(
        Block::default().style(Style::default().bg(colors.background())),
        area,
    );
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
                        .fg(colors.selection_foreground())
                        .bg(colors.selected())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(colors.foreground())
                        .bg(colors.background())
                },
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(rows).block(
            Block::default()
                .title(" Themes — ↑/↓ preview, Enter confirm, Esc cancel ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(colors.accent()))
                .style(Style::default().bg(colors.background())),
        ),
        inner,
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

fn target_name(target: &LaunchTarget) -> &'static str {
    match target {
        LaunchTarget::Dashboard => "dashboard",
        LaunchTarget::WorkingTree { .. } => "unstaged diff",
        LaunchTarget::Staged { .. } => "staged diff",
        LaunchTarget::Commit { .. } => "commit diff",
        LaunchTarget::Compare { .. } => "revision comparison",
        LaunchTarget::Show { .. } => "revision",
        LaunchTarget::History { .. } => "history",
        LaunchTarget::Branches => "branches",
        LaunchTarget::Changes => "changes",
    }
}
