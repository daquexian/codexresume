use std::io;
use std::sync::Arc;
use std::time::Duration;

use crate::data::SessionDb;
use crate::data::SessionDbOptions;
use crate::data::SessionPage;
use crate::data::SessionRow;
use crate::data::SortKey;
use crate::data::filter_rows;
use chrono::DateTime;
use chrono::Utc;
use crossterm::event;
use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::crossterm::event::KeyModifiers;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::style::Stylize as _;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr;

const LOAD_NEAR_THRESHOLD: usize = 5;

type PageLoader = Arc<dyn Fn(PageLoadRequest) + Send + Sync>;

pub async fn run_picker(
    session_db: SessionDb,
    options: SessionDbOptions,
) -> io::Result<Option<SessionRow>> {
    let (page_tx, mut page_rx) = mpsc::unbounded_channel();
    let page_loader = create_page_loader(session_db, page_tx);
    let mut tui = TuiGuard::new()?;
    let mut state = PickerState::new(options, page_loader);
    state.start_initial_load();

    loop {
        while let Ok(event) = page_rx.try_recv() {
            state.handle_page_loaded(event);
        }

        tui.terminal.draw(|frame| draw(frame, &mut state))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if matches!(key.kind, KeyEventKind::Release) {
            continue;
        }

        match state.handle_key(key) {
            PickerOutcome::Continue => {
                state.maybe_load_more_for_scroll();
                state.continue_search_if_needed();
            }
            PickerOutcome::Reload => state.start_initial_load(),
            PickerOutcome::Submit(row) => return Ok(Some(row)),
            PickerOutcome::Exit => return Ok(None),
        }
    }
}

enum PickerOutcome {
    Continue,
    Reload,
    Submit(SessionRow),
    Exit,
}

#[derive(Clone, Copy)]
struct PageLoadRequest {
    offset: usize,
    request_token: usize,
    sort_key: SortKey,
}

struct PageLoaded {
    offset: usize,
    request_token: usize,
    sort_key: SortKey,
    result: anyhow::Result<SessionPage>,
}

fn create_page_loader(
    session_db: SessionDb,
    page_tx: mpsc::UnboundedSender<PageLoaded>,
) -> PageLoader {
    Arc::new(move |request| {
        let session_db = session_db.clone();
        let page_tx = page_tx.clone();
        tokio::spawn(async move {
            let result = session_db.load_page(request.sort_key, request.offset).await;
            let _ = page_tx.send(PageLoaded {
                offset: request.offset,
                request_token: request.request_token,
                sort_key: request.sort_key,
                result,
            });
        });
    })
}

struct PickerState {
    page_loader: PageLoader,
    all_rows: Vec<SessionRow>,
    filtered_rows: Vec<SessionRow>,
    selected: usize,
    scroll_top: usize,
    view_rows: usize,
    query: String,
    sort_key: SortKey,
    options: SessionDbOptions,
    has_more: bool,
    loading: bool,
    next_offset: usize,
    next_request_token: usize,
    active_request_token: Option<usize>,
    inline_error: Option<String>,
    loaded_rows: usize,
}

impl PickerState {
    fn new(options: SessionDbOptions, page_loader: PageLoader) -> Self {
        Self {
            page_loader,
            all_rows: Vec::new(),
            filtered_rows: Vec::new(),
            selected: 0,
            scroll_top: 0,
            view_rows: 1,
            query: String::new(),
            sort_key: SortKey::UpdatedAt,
            options,
            has_more: true,
            loading: false,
            next_offset: 0,
            next_request_token: 0,
            active_request_token: None,
            inline_error: None,
            loaded_rows: 0,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> PickerOutcome {
        match key.code {
            KeyCode::Esc => PickerOutcome::Exit,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                PickerOutcome::Exit
            }
            KeyCode::Enter => self
                .filtered_rows
                .get(self.selected)
                .cloned()
                .map(PickerOutcome::Submit)
                .unwrap_or(PickerOutcome::Continue),
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.keep_selection_visible();
                PickerOutcome::Continue
            }
            KeyCode::Down => {
                if !self.filtered_rows.is_empty() {
                    self.selected = (self.selected + 1).min(self.filtered_rows.len() - 1);
                }
                self.keep_selection_visible();
                PickerOutcome::Continue
            }
            KeyCode::PageUp => {
                self.selected = self.selected.saturating_sub(self.page_size());
                self.keep_selection_visible();
                PickerOutcome::Continue
            }
            KeyCode::PageDown => {
                if !self.filtered_rows.is_empty() {
                    self.selected =
                        (self.selected + self.page_size()).min(self.filtered_rows.len() - 1);
                }
                self.keep_selection_visible();
                PickerOutcome::Continue
            }
            KeyCode::Home => {
                self.selected = 0;
                self.scroll_top = 0;
                PickerOutcome::Continue
            }
            KeyCode::End => {
                if !self.filtered_rows.is_empty() {
                    self.selected = self.filtered_rows.len() - 1;
                    self.keep_selection_visible();
                }
                PickerOutcome::Continue
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refilter();
                PickerOutcome::Continue
            }
            KeyCode::Tab => {
                self.sort_key = self.sort_key.toggle();
                PickerOutcome::Reload
            }
            KeyCode::Char(ch)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.query.push(ch);
                self.refilter();
                PickerOutcome::Continue
            }
            _ => PickerOutcome::Continue,
        }
    }

    fn start_initial_load(&mut self) {
        self.all_rows.clear();
        self.filtered_rows.clear();
        self.selected = 0;
        self.scroll_top = 0;
        self.has_more = true;
        self.loading = false;
        self.next_offset = 0;
        self.active_request_token = None;
        self.inline_error = None;
        self.loaded_rows = 0;
        self.request_next_page();
    }

    fn refilter(&mut self) {
        self.rebuild_filtered(None);
    }

    fn page_size(&self) -> usize {
        self.view_rows.max(1)
    }

    fn keep_selection_visible(&mut self) {
        let page_height = self.page_size();
        if self.selected < self.scroll_top {
            self.scroll_top = self.selected;
            return;
        }
        if self.selected >= self.scroll_top.saturating_add(page_height) {
            self.scroll_top = self.selected.saturating_sub(page_height.saturating_sub(1));
        }
    }

    fn selected_row(&self) -> Option<&SessionRow> {
        self.filtered_rows.get(self.selected)
    }

    fn rebuild_filtered(&mut self, selected_thread_id: Option<String>) {
        self.filtered_rows = filter_rows(&self.all_rows, &self.query, self.sort_key);
        self.selected = selected_thread_id
            .and_then(|thread_id| {
                self.filtered_rows
                    .iter()
                    .position(|row| row.thread_id == thread_id)
            })
            .unwrap_or(0);
        self.scroll_top = 0;
        self.keep_selection_visible();
    }

    fn request_next_page(&mut self) {
        if self.loading || !self.has_more {
            return;
        }

        self.inline_error = None;
        self.loading = true;
        let request_token = self.next_request_token;
        self.next_request_token = self.next_request_token.wrapping_add(1);
        self.active_request_token = Some(request_token);
        (self.page_loader)(PageLoadRequest {
            offset: self.next_offset,
            request_token,
            sort_key: self.sort_key,
        });
    }

    fn handle_page_loaded(&mut self, loaded: PageLoaded) {
        if self.active_request_token != Some(loaded.request_token)
            || self.sort_key != loaded.sort_key
        {
            return;
        }

        self.loading = false;
        self.active_request_token = None;

        match loaded.result {
            Ok(page) => {
                let selected_thread_id = self.selected_row().map(|row| row.thread_id.clone());
                self.loaded_rows += page.rows.len();
                self.next_offset = loaded.offset + page.rows.len();
                self.has_more = page.has_more;
                self.all_rows.extend(page.rows);
                self.rebuild_filtered(selected_thread_id);
                self.continue_search_if_needed();
            }
            Err(err) => {
                self.inline_error = Some(format!("Load failed: {err}"));
                self.has_more = false;
            }
        }
    }

    fn maybe_load_more_for_scroll(&mut self) {
        if self.loading || !self.has_more || self.filtered_rows.is_empty() {
            return;
        }

        let remaining = self.filtered_rows.len().saturating_sub(self.selected + 1);
        if remaining <= LOAD_NEAR_THRESHOLD {
            self.request_next_page();
        }
    }

    fn continue_search_if_needed(&mut self) {
        if self.loading || !self.has_more || self.query.is_empty() || !self.filtered_rows.is_empty()
        {
            return;
        }

        self.request_next_page();
    }

    fn loaded_label(&self) -> String {
        if self.has_more || self.loading {
            format!("{}+", self.loaded_rows)
        } else {
            self.loaded_rows.to_string()
        }
    }
}

struct TuiGuard {
    terminal: DefaultTerminal,
}

impl TuiGuard {
    fn new() -> io::Result<Self> {
        let terminal = ratatui::try_init()?;
        Ok(Self { terminal })
    }
}

impl Drop for TuiGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

fn draw(frame: &mut Frame, state: &mut PickerState) {
    let area = frame.area();
    let [header, search, columns, list, details, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(area.height.saturating_sub(5)),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    state.view_rows = list.height as usize;
    state.keep_selection_visible();

    let header_line: Line = vec![
        "Resume a previous session".bold().cyan(),
        "  ".into(),
        "Sort:".dim(),
        " ".into(),
        state.sort_key.label().magenta(),
        "  ".into(),
        "Loaded:".dim(),
        " ".into(),
        state.loaded_label().yellow(),
        "  ".into(),
        "Providers:".dim(),
        " ".into(),
        state.options.provider_visibility.label().green(),
        "  ".into(),
        "Scope:".dim(),
        scope_label(state.options.clone()).green(),
    ]
    .into();
    frame.render_widget(Paragraph::new(header_line), header);

    frame.render_widget(Paragraph::new(search_line(state)), search);

    let metrics = calculate_metrics(&state.filtered_rows);
    frame.render_widget(Paragraph::new(column_header(&metrics)), columns);
    frame.render_widget(
        Paragraph::new(list_lines(state, &metrics, list.height as usize)),
        list,
    );
    frame.render_widget(Paragraph::new(details_line(state)), details);
    frame.render_widget(Paragraph::new(hint_line()), hint);
}

fn search_line(state: &PickerState) -> Line<'static> {
    if let Some(error) = state.inline_error.as_deref() {
        return Line::from(error.to_string().red());
    }
    if state.query.is_empty() {
        return Line::from("Type to search".dim());
    }
    Line::from(format!("Search: {}", state.query))
}

fn details_line(state: &PickerState) -> Line<'static> {
    let Some(row) = state.selected_row() else {
        return Line::from("No sessions yet".italic().dim());
    };
    let branch = row.git_branch.as_deref().unwrap_or("-");
    let text = format!(
        "ID {}    provider={}    source={}    branch={}    archived={}    path={}",
        row.thread_id,
        row.provider,
        row.source_label(),
        branch,
        row.archived,
        row.rollout_path.display()
    );
    Line::from(Span::from(text).dim())
}

fn hint_line() -> Line<'static> {
    vec![
        Span::from("Enter").bold(),
        " to resume ".dim(),
        "    ".dim(),
        Span::from("Esc").bold(),
        " to quit ".dim(),
        "    ".dim(),
        Span::from("Ctrl+C").bold(),
        " to quit ".dim(),
        "    ".dim(),
        Span::from("Tab").bold(),
        " to toggle sort ".dim(),
        "    ".dim(),
        Span::from("Up/Down").bold(),
        " to browse".dim(),
    ]
    .into()
}

fn scope_label(options: SessionDbOptions) -> String {
    let sources = if options.include_non_interactive {
        "all sources"
    } else {
        "interactive only"
    };
    let cwd = if options.filter_cwd.is_some() {
        "cwd"
    } else {
        "all cwd"
    };
    let archived = if options.include_archived {
        "archived"
    } else {
        "no archived"
    };
    format!("{sources} / {cwd} / {archived}")
}

fn list_lines(
    state: &PickerState,
    metrics: &ColumnMetrics,
    page_height: usize,
) -> Vec<Line<'static>> {
    if state.filtered_rows.is_empty() {
        return vec![render_empty_state_line(state)];
    }

    let page_height = page_height.max(1);
    let start = state
        .scroll_top
        .min(state.filtered_rows.len().saturating_sub(1));
    let end = state.filtered_rows.len().min(start + page_height);
    let mut lines = Vec::with_capacity(end - start);

    for (idx, row) in state.filtered_rows[start..end].iter().enumerate() {
        let is_selected = start + idx == state.selected;
        let marker = if is_selected {
            "> ".bold()
        } else {
            "  ".into()
        };
        let time_label = format_time_label(row, state.sort_key);
        let provider = pad_or_truncate(&row.provider, metrics.provider_width);
        let short_id = pad_or_truncate(&row.short_id(), metrics.id_width);
        let cwd = pad_or_truncate(&display_cwd(&row.cwd), metrics.cwd_width);
        let preview_text = if row.archived {
            format!("[archived] {}", row.display_preview())
        } else {
            row.display_preview().to_string()
        };
        let preview = truncate_text(&preview_text, metrics.preview_width);
        let preview_span = if row.archived {
            preview.dim()
        } else {
            Span::from(preview)
        };

        let line: Line = vec![
            marker,
            Span::from(format!("{time_label:<width$}", width = metrics.time_width)).dim(),
            "  ".into(),
            Span::from(provider).green(),
            "  ".into(),
            Span::from(short_id).yellow(),
            "  ".into(),
            Span::from(cwd).dim(),
            "  ".into(),
            preview_span,
        ]
        .into();
        lines.push(line);
    }

    if state.loading && lines.len() < page_height {
        lines.push(Line::from("  Loading older sessions...".italic().dim()));
    }

    lines
}

fn render_empty_state_line(state: &PickerState) -> Line<'static> {
    if state.loading && state.query.is_empty() {
        return Line::from("Loading sessions...".italic().dim());
    }
    if state.loading && !state.query.is_empty() {
        return Line::from("Searching...".italic().dim());
    }
    if !state.query.is_empty() {
        return Line::from("No results for your search".italic().dim());
    }
    Line::from("No sessions yet".italic().dim())
}

fn column_header(metrics: &ColumnMetrics) -> Line<'static> {
    vec![
        "  ".into(),
        Span::from(format!("{:<width$}", "Time", width = metrics.time_width)).bold(),
        "  ".into(),
        Span::from(format!(
            "{:<width$}",
            "Provider",
            width = metrics.provider_width
        ))
        .bold(),
        "  ".into(),
        Span::from(format!(
            "{:<width$}",
            "Session ID",
            width = metrics.id_width
        ))
        .bold(),
        "  ".into(),
        Span::from(format!("{:<width$}", "CWD", width = metrics.cwd_width)).bold(),
        "  ".into(),
        "Conversation".bold(),
    ]
    .into()
}

struct ColumnMetrics {
    time_width: usize,
    provider_width: usize,
    id_width: usize,
    cwd_width: usize,
    preview_width: usize,
}

fn calculate_metrics(rows: &[SessionRow]) -> ColumnMetrics {
    let time_width = rows
        .iter()
        .map(|row| UnicodeWidthStr::width(format_time_label(row, SortKey::UpdatedAt).as_str()))
        .max()
        .unwrap_or(10)
        .max(10);
    let provider_width = rows
        .iter()
        .map(|row| UnicodeWidthStr::width(row.provider.as_str()))
        .max()
        .unwrap_or(8)
        .clamp(8, 14);
    let id_width = 12;
    let cwd_width = rows
        .iter()
        .map(|row| UnicodeWidthStr::width(display_cwd(&row.cwd).as_str()))
        .max()
        .unwrap_or(3)
        .clamp(12, 28);
    let preview_width = 80;

    ColumnMetrics {
        time_width,
        provider_width,
        id_width,
        cwd_width,
        preview_width,
    }
}

fn format_time_label(row: &SessionRow, sort_key: SortKey) -> String {
    let ts = match sort_key {
        SortKey::UpdatedAt => row.updated_at.or(row.created_at),
        SortKey::CreatedAt => row.created_at,
    };
    ts.map(human_time_ago).unwrap_or_else(|| "-".to_string())
}

fn human_time_ago(ts: DateTime<Utc>) -> String {
    let now = Utc::now();
    let delta = now - ts;
    let secs = delta.num_seconds().max(0);
    if secs < 60 {
        if secs == 1 {
            format!("{secs} second ago")
        } else {
            format!("{secs} seconds ago")
        }
    } else if secs < 60 * 60 {
        let minutes = secs / 60;
        if minutes == 1 {
            format!("{minutes} minute ago")
        } else {
            format!("{minutes} minutes ago")
        }
    } else if secs < 60 * 60 * 24 {
        let hours = secs / 3600;
        if hours == 1 {
            format!("{hours} hour ago")
        } else {
            format!("{hours} hours ago")
        }
    } else {
        let days = secs / (60 * 60 * 24);
        if days == 1 {
            format!("{days} day ago")
        } else {
            format!("{days} days ago")
        }
    }
}

fn display_cwd(cwd: &std::path::Path) -> String {
    cwd.display().to_string()
}

fn truncate_text(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let text_width = UnicodeWidthStr::width(text);
    if text_width <= max_width {
        return text.to_string();
    }

    let mut output = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let next = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4]));
        if width + next >= max_width {
            break;
        }
        output.push(ch);
        width += next;
    }
    output.push('…');
    output
}

fn pad_or_truncate(text: &str, width: usize) -> String {
    let truncated = truncate_text(text, width);
    format!("{truncated:<width$}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::SessionRow;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    fn row() -> SessionRow {
        SessionRow {
            thread_id: "00000000-0000-0000-0000-000000000001".to_string(),
            thread_name: None,
            preview: "hello world".to_string(),
            created_at: chrono::DateTime::from_timestamp(1, 0).map(|dt| dt.with_timezone(&Utc)),
            updated_at: chrono::DateTime::from_timestamp(2, 0).map(|dt| dt.with_timezone(&Utc)),
            archived: false,
            provider: "openai".to_string(),
            source: "exec".to_string(),
            cwd: PathBuf::from("/tmp/example"),
            rollout_path: PathBuf::from("/tmp/example.jsonl"),
            git_branch: None,
        }
    }

    #[test]
    fn truncate_text_adds_ellipsis() {
        assert_eq!(truncate_text("abcdef", 4), "abc…");
    }

    #[test]
    fn format_time_label_uses_updated_at_by_default() {
        let label = format_time_label(&row(), SortKey::UpdatedAt);
        assert!(label.contains("ago"));
    }
}
