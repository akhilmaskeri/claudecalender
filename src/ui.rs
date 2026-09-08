use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
    },
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph, Wrap},
    Terminal,
};
use crate::{api, models::{Output, SessionStats, UsageData}};

/// A session the user asked to resume: `claude --resume <id>` run from `cwd`.
struct ResumeRequest {
    session_id: String,
    cwd: String,
}

#[derive(PartialEq)]
enum Focus {
    Calendar,
    Detail,
}

struct App {
    sessions: Vec<SessionStats>,
    billing_start: NaiveDate,
    billing_end: NaiveDate,
    active_day: NaiveDate,
    today: NaiveDate,
    daily_tokens: HashMap<NaiveDate, u64>,
    max_daily_tokens: u64,
    daily_costs: HashMap<NaiveDate, f64>,
    /// First-of-month of the calendar month currently displayed. Defaults to
    /// the month containing the active billing period; `[`/`]` walk it back
    /// through months that have local session history, or forward again.
    view_month: NaiveDate,
    /// First-of-month of the active billing period — the newest month
    /// reachable, and the one that still gets billing-period highlighting.
    current_period_month: NaiveDate,
    /// First-of-month of the oldest month with any locally recorded session.
    min_view_month: NaiveDate,
    focus: Focus,
    plan: String,
    usage: Arc<Mutex<Option<UsageData>>>,
    org_uuid: String,
    session_key: String,
    selected_session: usize,
    detail_scroll: std::cell::Cell<u16>,
    status: Option<String>,
}

impl App {
    fn new(output: Output) -> Result<Self> {
        let billing_start =
            NaiveDate::parse_from_str(&output.subscription_start_date, "%Y-%m-%d")?;
        let billing_end =
            NaiveDate::parse_from_str(&output.subscription_end_date, "%Y-%m-%d")?;
        let today = Local::now().date_naive();
        let last_valid = billing_end - Duration::days(1);
        let active_day = today.max(billing_start).min(last_valid);
        let current_period_month = billing_start.with_day(1).unwrap();

        let mut daily_tokens: HashMap<NaiveDate, u64> = HashMap::new();
        let mut daily_costs: HashMap<NaiveDate, f64> = HashMap::new();
        let mut earliest: Option<NaiveDate> = None;
        for s in &output.sessions {
            if s.started_at.len() >= 10
                && let Ok(d) = NaiveDate::parse_from_str(&s.started_at[..10], "%Y-%m-%d")
            {
                *daily_tokens.entry(d).or_insert(0) +=
                    s.total_input_tokens + s.total_output_tokens;
                *daily_costs.entry(d).or_insert(0.0) += s.cost_usd;
                earliest = Some(earliest.map_or(d, |e: NaiveDate| e.min(d)));
            }
        }
        let max_daily_tokens = daily_tokens.values().copied().max().unwrap_or(0);
        let min_view_month = earliest
            .map(|d| d.with_day(1).unwrap())
            .unwrap_or(current_period_month)
            .min(current_period_month);

        Ok(App {
            sessions: output.sessions,
            billing_start,
            billing_end,
            active_day,
            today,
            daily_tokens,
            max_daily_tokens,
            daily_costs,
            view_month: current_period_month,
            current_period_month,
            min_view_month,
            focus: Focus::Calendar,
            plan: output.plan,
            usage: Arc::new(Mutex::new(output.initial_usage)),
            org_uuid: output.org_uuid,
            session_key: output.session_key,
            selected_session: 0,
            detail_scroll: std::cell::Cell::new(0),
            status: None,
        })
    }

    fn last_valid_day(&self) -> NaiveDate {
        self.billing_end - Duration::days(1)
    }

    fn is_current_view(&self) -> bool {
        self.view_month == self.current_period_month
    }

    // First displayed day: beginning of the displayed month.
    fn nav_start(&self) -> NaiveDate {
        self.view_month
    }

    // Last displayed day. For the current billing period this is the end of
    // the week (Sunday) that contains last_valid_day, capped at month end so
    // we never spill into a third month. For a browsed-back history month
    // it's simply the last day of that month.
    fn nav_end(&self) -> NaiveDate {
        if self.is_current_view() {
            let last = self.last_valid_day();
            let days_to_sun = 6 - last.weekday().num_days_from_monday() as i64;
            let end = last + Duration::days(days_to_sun);
            let month_end = last.with_day(month_days(last.year(), last.month())).unwrap();
            end.min(month_end)
        } else {
            self.view_month
                .with_day(month_days(self.view_month.year(), self.view_month.month()))
                .unwrap()
        }
    }

    fn can_go_prev_month(&self) -> bool {
        self.view_month > self.min_view_month
    }

    fn can_go_next_month(&self) -> bool {
        self.view_month < self.current_period_month
    }

    fn goto_month(&mut self, month: NaiveDate) {
        self.view_month = month;
        if self.is_current_view() {
            self.active_day = self.today.max(self.nav_start()).min(self.last_valid_day());
        } else {
            let month_end = month.with_day(month_days(month.year(), month.month())).unwrap();
            // Default to the most recent day with recorded usage in this month.
            self.active_day = self
                .daily_tokens
                .keys()
                .filter(|d| **d >= month && **d <= month_end)
                .max()
                .copied()
                .unwrap_or(month);
        }
        self.selected_session = 0;
        self.detail_scroll.set(0);
    }

    fn prev_month(&mut self) {
        if !self.can_go_prev_month() {
            return;
        }
        let m = self.view_month;
        let prev = if m.month() == 1 {
            NaiveDate::from_ymd_opt(m.year() - 1, 12, 1).unwrap()
        } else {
            NaiveDate::from_ymd_opt(m.year(), m.month() - 1, 1).unwrap()
        };
        self.goto_month(prev.max(self.min_view_month));
    }

    fn next_month(&mut self) {
        if !self.can_go_next_month() {
            return;
        }
        let m = self.view_month;
        let next = if m.month() == 12 {
            NaiveDate::from_ymd_opt(m.year() + 1, 1, 1).unwrap()
        } else {
            NaiveDate::from_ymd_opt(m.year(), m.month() + 1, 1).unwrap()
        };
        self.goto_month(next.min(self.current_period_month));
    }

    fn sessions_for_day(&self, day: NaiveDate) -> Vec<&SessionStats> {
        let mut result: Vec<&SessionStats> = self
            .sessions
            .iter()
            .filter(|s| {
                s.started_at.len() >= 10
                    && NaiveDate::parse_from_str(&s.started_at[..10], "%Y-%m-%d")
                        .map(|d| d == day)
                        .unwrap_or(false)
            })
            .collect();
        result.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        result
    }

    fn move_active_day(&mut self, delta: i64) {
        let new_day = self.active_day + Duration::days(delta);
        self.active_day = new_day.max(self.nav_start()).min(self.nav_end());
        self.selected_session = 0;
        self.detail_scroll.set(0);
    }

    fn detail_next(&mut self) {
        let count = self.sessions_for_day(self.active_day).len();
        if self.selected_session + 1 < count {
            self.selected_session += 1;
        }
    }

    fn detail_prev(&mut self) {
        self.selected_session = self.selected_session.saturating_sub(1);
    }

    // Err carries a message to show the user instead of resuming.
    fn resume_target(&self) -> Result<ResumeRequest, String> {
        let day_sessions = self.sessions_for_day(self.active_day);
        let Some(s) = day_sessions.get(self.selected_session) else {
            return Err("No session to resume.".to_string());
        };
        let Some(cwd) = s.cwd.as_deref() else {
            return Err("No working directory recorded for this session.".to_string());
        };
        if !std::path::Path::new(cwd).is_dir() {
            return Err(format!("Working directory no longer exists: {cwd}"));
        }
        Ok(ResumeRequest {
            session_id: s.session_id.clone(),
            cwd: cwd.to_string(),
        })
    }
}

pub fn run_app(output: Output) -> Result<()> {
    let mut app = App::new(output)?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, SetTitle("ClaudeCalendar"))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    match result? {
        Some(req) => resume_session(req),
        None => Ok(()),
    }
}

// Hands the terminal over to `claude --resume` in the session's working
// directory. Called only after the TUI has been torn down.
fn resume_session(req: ResumeRequest) -> Result<()> {
    let status = std::process::Command::new("claude")
        .arg("--resume")
        .arg(&req.session_id)
        .current_dir(&req.cwd)
        .status()
        .map_err(|e| anyhow::anyhow!("could not run `claude`: {e}"))?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<Option<ResumeRequest>> {
    let usage_bg = Arc::clone(&app.usage);
    let key_bg = app.session_key.clone();
    let uuid_bg = app.org_uuid.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(120));
        if let Ok(u) = api::fetch_usage(&key_bg, &uuid_bg)
            && let Ok(mut guard) = usage_bg.lock()
        {
            *guard = Some(u);
        }
    });

    loop {
        terminal.draw(|f| draw(f, app))?;

        if event::poll(std::time::Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.status = None;

            match (key.code, &app.focus) {
                (KeyCode::Char('q'), _) => return Ok(None),
                (KeyCode::Esc, Focus::Detail) => app.focus = Focus::Calendar,
                (KeyCode::Esc, Focus::Calendar) => return Ok(None),
                (KeyCode::Enter, Focus::Detail) => match app.resume_target() {
                    Ok(req) => return Ok(Some(req)),
                    Err(msg) => app.status = Some(msg),
                },
                (KeyCode::Enter, Focus::Calendar) => {
                    app.selected_session = 0;
                    app.detail_scroll.set(0);
                    app.focus = Focus::Detail;
                }

                (KeyCode::Left | KeyCode::Char('h'), Focus::Calendar) => app.move_active_day(-1),
                (KeyCode::Right | KeyCode::Char('l'), Focus::Calendar) => app.move_active_day(1),
                (KeyCode::Up | KeyCode::Char('k'), Focus::Calendar) => app.move_active_day(-7),
                (KeyCode::Down | KeyCode::Char('j'), Focus::Calendar) => app.move_active_day(7),

                (KeyCode::Char('[') | KeyCode::PageUp, Focus::Calendar) => app.prev_month(),
                (KeyCode::Char(']') | KeyCode::PageDown, Focus::Calendar) => app.next_month(),

                (KeyCode::Up | KeyCode::Char('k'), Focus::Detail) => app.detail_prev(),
                (KeyCode::Down | KeyCode::Char('j'), Focus::Detail) => app.detail_next(),
                _ => {}
            }
        }
    }
}

fn draw(f: &mut ratatui::Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(24), Constraint::Fill(1)])
        .split(f.area());

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Fill(1)])
        .split(chunks[0]);

    draw_calendar(f, app, left[0]);
    draw_usage(f, app, left[1]);
    draw_sessions(f, app, chunks[1]);
}

// ── Calendar panel ─────────────────────────────────────────────────────────────

fn month_days(year: i32, month: u32) -> u32 {
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1).unwrap()
    };
    (next - NaiveDate::from_ymd_opt(year, month, 1).unwrap()).num_days() as u32
}

fn contribution_color(level: u8) -> Color {
    match level {
        1 => Color::Rgb(22, 101, 52),
        2 => Color::Rgb(21, 128, 61),
        3 => Color::Rgb(22, 163, 74),
        _ => Color::Rgb(74, 222, 128),
    }
}

fn day_style(app: &App, date: NaiveDate) -> Style {
    if date == app.active_day {
        let bg = if app.focus == Focus::Calendar { Color::Cyan } else { Color::Blue };
        return Style::default().bg(bg).fg(Color::White).add_modifier(Modifier::BOLD);
    }

    // Billing-period and overflow-month distinctions only apply while
    // viewing the current period; browsed-back history months are shown as
    // plain calendar months.
    if app.is_current_view() {
        let last_valid = app.last_valid_day();
        let in_period = date >= app.billing_start && date <= last_valid;
        let is_overflow = date.year() != app.billing_start.year()
            || date.month() != app.billing_start.month();
        if !in_period {
            return Style::default().fg(Color::DarkGray);
        }
        return day_usage_style(app, date, is_overflow);
    }

    day_usage_style(app, date, false)
}

fn day_usage_style(app: &App, date: NaiveDate, is_overflow: bool) -> Style {
    let tokens = app.daily_tokens.get(&date).copied().unwrap_or(0);
    if tokens > 0 {
        let level = ((tokens * 4).saturating_sub(1) / app.max_daily_tokens).min(3) as u8 + 1;
        let base = Style::default().bg(contribution_color(level)).fg(Color::White);
        return if date == app.today {
            base.add_modifier(Modifier::BOLD)
        } else {
            base
        };
    }

    if date == app.today {
        return Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    }
    if is_overflow {
        return Style::default().fg(Color::Gray);
    }
    Style::default()
}

fn fmt_plan(raw: &str) -> String {
    let s = raw.strip_prefix("claude_").unwrap_or(raw);
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn fmt_resets_in(ts: &str) -> String {
    let Ok(dt) = DateTime::parse_from_rfc3339(ts) else {
        return "--".to_string();
    };
    let delta = dt.signed_duration_since(Local::now());
    let mins = delta.num_minutes();
    if mins <= 0 {
        return "now".to_string();
    }
    let h = delta.num_hours();
    let m = mins % 60;
    if h > 0 { format!("{}h {}m", h, m) } else { format!("{}m", m) }
}

fn fmt_resets_on(ts: &str) -> String {
    let Ok(dt) = DateTime::parse_from_rfc3339(ts) else {
        return "--".to_string();
    };
    dt.with_timezone(&Local).format("%b %-d").to_string()
}

fn draw_calendar(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let month_start = app.view_month;
    let total_end = app.nav_end();

    let title = Span::styled(
        format!("[ {} ]", month_start.format("%B %Y")),
        Style::default().add_modifier(Modifier::BOLD),
    );
    let block = Block::bordered()
        .title(Line::from(title))
        .padding(Padding::new(1, 1, 1, 1));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(Line::from("Mo Tu We Th Fr Sa Su"));

    let first_wd = month_start.weekday().num_days_from_monday() as usize;
    let mut col = 0usize;
    let mut spans: Vec<Span<'static>> = Vec::new();

    // Leading empty slots
    for i in 0..first_wd {
        spans.push(Span::raw("  "));
        if i < 6 {
            spans.push(Span::raw(" "));
        }
        col += 1;
    }

    let mut current = month_start;
    while current <= total_end {
        let style = day_style(app, current);
        spans.push(Span::styled(format!("{:2}", current.day()), style));
        if col < 6 {
            spans.push(Span::raw(" "));
        }
        col += 1;
        if col == 7 {
            lines.push(Line::from(std::mem::take(&mut spans)));
            col = 0;
        }
        current += Duration::days(1);
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

// ── Usage panel ─────────────────────────────────────────────────────────────────

fn draw_usage(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .title("[ Usage ]")
        .padding(Padding::new(1, 1, 1, 1));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let month_start = app.view_month;
    let total_end = app.nav_end();

    let mut lines: Vec<Line<'static>> = Vec::new();

    let plan_label = fmt_plan(&app.plan);
    let day_cost = app.daily_costs.get(&app.active_day).copied().unwrap_or(0.0);
    let mut range_cost = 0f64;
    let mut d = month_start;
    while d <= total_end {
        range_cost += app.daily_costs.get(&d).copied().unwrap_or(0.0);
        d += Duration::days(1);
    }
    lines.push(Line::from(Span::styled(
        format!("{:<4} ${:.2} ${:.2}", plan_label, range_cost, day_cost),
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    )));

    let day_tokens = app.daily_tokens.get(&app.active_day).copied().unwrap_or(0);
    lines.push(Line::from(Span::styled(
        format!("Tok: {}", day_tokens),
        Style::default().fg(Color::DarkGray),
    )));

    let usage_guard = app.usage.lock().unwrap();
    match &*usage_guard {
        None => {
            lines.push(Line::from(Span::styled("Curr:  --", Style::default().fg(Color::DarkGray))));
            lines.push(Line::from(Span::styled("Week:  --", Style::default().fg(Color::DarkGray))));
        }
        Some(u) => {
            lines.push(Line::from(Span::styled(
                format!("Curr:{:>3}% [{:^8}]", u.five_hour_pct, fmt_resets_in(&u.five_hour_resets_at)),
                Style::default().fg(Color::White),
            )));
            lines.push(Line::from(Span::styled(
                format!("Week:{:>3}% [{:^8}]", u.seven_day_pct, fmt_resets_on(&u.seven_day_resets_at)),
                Style::default().fg(Color::White),
            )));
        }
    }
    drop(usage_guard);

    f.render_widget(Paragraph::new(lines), inner);
}

// ── Session panel ──────────────────────────────────────────────────────────────

fn draw_sessions(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let block = Block::bordered().title("[ Sessions ]");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let area = inner;

    // Reserve a footer line for a status message when there is one.
    let (area, footer) = match app.status {
        Some(_) => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Fill(1), Constraint::Length(1)])
                .split(area);
            (rows[0], Some(rows[1]))
        }
        None => (area, None),
    };

    let day_sessions = app.sessions_for_day(app.active_day);
    // 2 cols left padding; cap summary at 80 chars
    let inner_width = area.width.saturating_sub(2) as usize;
    let wrap_width = inner_width.min(80);

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Track where each session starts
    let mut session_starts: Vec<u16> = Vec::new();

    if day_sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            "No sessions on this day.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let header_len = 0u16;
        for (i, s) in day_sessions.iter().enumerate() {
            session_starts.push(lines.len() as u16 - header_len);
            let is_selected = app.focus == Focus::Detail && i == app.selected_session;

            let title_style = if is_selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            lines.push(Line::from(Span::styled(s.session_id.clone(), title_style)));

            lines.push(Line::from(s.project.clone()));

            let t0 = fmt_time(&s.started_at);
            let t1 = fmt_time(&s.ended_at);
            lines.push(Line::from(format!(
                "{} – {}  ({:.0} min)   In: {}  Out: {}",
                t0, t1, s.duration_mins, s.total_input_tokens, s.total_output_tokens
            )));

            if let Some(ref recap) = s.summary {
                for wrapped in wrap_text(recap, wrap_width) {
                    lines.push(Line::from(wrapped));
                }
            }

            lines.push(Line::from(""));
        }
    }

    // Update scroll so the selected session appears at the top of the visible area
    let scroll = session_starts
        .get(app.selected_session)
        .copied()
        .unwrap_or(0);
    app.detail_scroll.set(scroll);

    let widget = Paragraph::new(lines)
        .block(Block::default().padding(Padding::new(2, 0, 0, 0)))
        .scroll((app.detail_scroll.get(), 0))
        .wrap(Wrap { trim: false });
    f.render_widget(widget, area);

    if let (Some(rect), Some(msg)) = (footer, &app.status) {
        let line = Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(Color::Red),
        ));
        f.render_widget(
            Paragraph::new(line).block(Block::default().padding(Padding::new(2, 0, 0, 0))),
            rect,
        );
    }
}

fn fmt_time(ts: &str) -> &str {
    if ts.len() >= 16 { &ts[11..16] } else { ts }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut result = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current = word.to_string();
        } else if current.len() + 1 + word.len() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            result.push(current);
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}
