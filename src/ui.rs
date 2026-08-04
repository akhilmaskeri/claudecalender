use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
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
    period_cost: f64,
    focus: Focus,
    plan: String,
    usage: Arc<Mutex<Option<UsageData>>>,
    org_uuid: String,
    session_key: String,
    selected_session: usize,
    detail_scroll: std::cell::Cell<u16>,
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

        let mut daily_tokens: HashMap<NaiveDate, u64> = HashMap::new();
        let mut daily_costs: HashMap<NaiveDate, f64> = HashMap::new();
        let mut period_cost = 0f64;
        for s in &output.sessions {
            if s.started_at.len() >= 10
                && let Ok(d) = NaiveDate::parse_from_str(&s.started_at[..10], "%Y-%m-%d")
            {
                *daily_tokens.entry(d).or_insert(0) +=
                    s.total_input_tokens + s.total_output_tokens;
                *daily_costs.entry(d).or_insert(0.0) += s.cost_usd;
                period_cost += s.cost_usd;
            }
        }
        let max_daily_tokens = daily_tokens.values().copied().max().unwrap_or(0);

        Ok(App {
            sessions: output.sessions,
            billing_start,
            billing_end,
            active_day,
            today,
            daily_tokens,
            max_daily_tokens,
            daily_costs,
            period_cost,
            focus: Focus::Calendar,
            plan: output.plan,
            usage: Arc::new(Mutex::new(output.initial_usage)),
            org_uuid: output.org_uuid,
            session_key: output.session_key,
            selected_session: 0,
            detail_scroll: std::cell::Cell::new(0),
        })
    }

    fn last_valid_day(&self) -> NaiveDate {
        self.billing_end - Duration::days(1)
    }

    // First displayed day: beginning of the month that contains billing_start.
    fn nav_start(&self) -> NaiveDate {
        self.billing_start.with_day(1).unwrap()
    }

    // Last displayed day: end of the week (Sunday) that contains last_valid_day,
    // capped at the end of that month so we never spill into a third month.
    fn nav_end(&self) -> NaiveDate {
        let last = self.last_valid_day();
        let days_to_sun = 6 - last.weekday().num_days_from_monday() as i64;
        let end = last + Duration::days(days_to_sun);
        let month_end = last.with_day(month_days(last.year(), last.month())).unwrap();
        end.min(month_end)
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
}

pub fn run_app(output: Output) -> Result<()> {
    let mut app = App::new(output)?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
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
            match (key.code, &app.focus) {
                (KeyCode::Char('q'), _) => return Ok(()),
                (KeyCode::Esc, Focus::Detail) => app.focus = Focus::Calendar,
                (KeyCode::Esc, Focus::Calendar) => return Ok(()),
                (KeyCode::Enter, Focus::Calendar) => {
                    app.selected_session = 0;
                    app.detail_scroll.set(0);
                    app.focus = Focus::Detail;
                }

                (KeyCode::Left | KeyCode::Char('h'), Focus::Calendar) => app.move_active_day(-1),
                (KeyCode::Right | KeyCode::Char('l'), Focus::Calendar) => app.move_active_day(1),
                (KeyCode::Up | KeyCode::Char('k'), Focus::Calendar) => app.move_active_day(-7),
                (KeyCode::Down | KeyCode::Char('j'), Focus::Calendar) => app.move_active_day(7),

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
        .constraints([Constraint::Length(20), Constraint::Fill(1)])
        .split(f.area());
    draw_calendar(f, app, chunks[0]);
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
    let last_valid = app.last_valid_day();
    let in_period = date >= app.billing_start && date <= last_valid;
    let is_overflow = date.year() != app.billing_start.year()
        || date.month() != app.billing_start.month();

    if date == app.active_day {
        let bg = if app.focus == Focus::Calendar { Color::Cyan } else { Color::Blue };
        return Style::default().bg(bg).fg(Color::White).add_modifier(Modifier::BOLD);
    }
    if !in_period {
        return Style::default().fg(Color::DarkGray);
    }

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
    let mut lines: Vec<Line<'static>> = Vec::new();

    let sy = app.billing_start.year();
    let sm = app.billing_start.month();
    let month_start = NaiveDate::from_ymd_opt(sy, sm, 1).unwrap();

    lines.push(Line::from("Mo Tu We Th Fr Sa Su"));

    let last = app.last_valid_day();
    let days_to_sun = 6 - last.weekday().num_days_from_monday() as i64;
    let total_end = {
        let end = last + Duration::days(days_to_sun);
        let month_end = last.with_day(month_days(last.year(), last.month())).unwrap();
        end.min(month_end)
    };

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

    lines.push(Line::from(""));

    let plan_label = fmt_plan(&app.plan);
    let day_cost = app.daily_costs.get(&app.active_day).copied().unwrap_or(0.0);
    lines.push(Line::from(Span::styled(
        format!("{:<4} ${:.2} ${:.2}", plan_label, app.period_cost, day_cost),
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

    f.render_widget(Paragraph::new(lines), area);
}

// ── Session panel ──────────────────────────────────────────────────────────────

fn draw_sessions(f: &mut ratatui::Frame, app: &App, area: Rect) {
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
