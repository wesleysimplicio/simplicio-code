//! Simplicio Code cockpit shown before the Agent session.
//!
//! This is deliberately a thin client surface: terminals are local PTYs,
//! while the Agent tab hands control to the installed Agent-owned session. It
//! owns no model, provider, billing, or orchestration state.

use std::io::{self, Write};
use std::process::Command;

use anyhow::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
    },
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Tabs, Wrap},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelOutcome {
    OpenAgent,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Terminal,
    Agent,
}

struct PanelState {
    focus: Focus,
    active_tab: usize,
    terminal_count: usize,
    notice: &'static str,
    agent_selection: AgentSelection,
}

#[derive(Debug, Clone)]
struct AgentSelection {
    model: String,
    provider: String,
}

impl Default for AgentSelection {
    fn default() -> Self {
        let mut selection = Self {
            model: "configured model".into(),
            provider: "configured provider".into(),
        };
        let Some(home) = std::env::var_os("HOME") else {
            return selection;
        };
        let path = std::path::PathBuf::from(home)
            .join(".simplicio_agent")
            .join("config.yaml");
        let Ok(contents) = std::fs::read_to_string(path) else {
            return selection;
        };
        let mut in_model = false;
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed == "model:" {
                in_model = true;
                continue;
            }
            if in_model && !line.starts_with(' ') && !line.starts_with('\t') {
                in_model = false;
            }
            if !in_model {
                continue;
            }
            let Some((key, value)) = trimmed.split_once(':') else {
                continue;
            };
            let value = value.trim().trim_matches(['"', '\'']);
            if value.is_empty() {
                continue;
            }
            match key.trim() {
                "default" | "name" => selection.model = value.to_string(),
                "provider" => selection.provider = value.to_string(),
                _ => {}
            }
        }
        selection
    }
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            focus: Focus::Terminal,
            active_tab: 0,
            terminal_count: 1,
            notice: "Enter opens the Simplicio Agent · t opens a terminal · + creates a tab",
            agent_selection: AgentSelection::default(),
        }
    }
}

pub(crate) fn run() -> Result<PanelOutcome> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        SetTitle("simplicio-code"),
        EnterAlternateScreen,
        EnableMouseCapture,
        Hide
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut state = PanelState::default();
    let outcome = loop {
        terminal.draw(|frame| draw(frame, &state))?;
        if !event::poll(std::time::Duration::from_millis(100))? {
            continue;
        }
        let outcome = match event::read()? {
            Event::Key(key) => handle_key(key, &mut state)?,
            Event::Mouse(mouse) => handle_mouse(mouse, &mut state),
            _ => None,
        };
        match outcome {
            Some(outcome) => break outcome,
            None => {}
        }
    };

    restore_terminal(&mut terminal)?;
    Ok(outcome)
}

fn handle_key(key: KeyEvent, state: &mut PanelState) -> Result<Option<PanelOutcome>> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
        return Ok(Some(PanelOutcome::Quit));
    }
    match key.code {
        KeyCode::Esc => Ok(Some(PanelOutcome::Quit)),
        KeyCode::Tab | KeyCode::Down | KeyCode::Up => {
            state.focus = match state.focus {
                Focus::Terminal => Focus::Agent,
                Focus::Agent => Focus::Terminal,
            };
            Ok(None)
        }
        KeyCode::Left => {
            state.active_tab = state.active_tab.saturating_sub(1);
            Ok(None)
        }
        KeyCode::Right => {
            state.active_tab = (state.active_tab + 1).min(state.terminal_count);
            Ok(None)
        }
        KeyCode::Char('+') => {
            state.terminal_count = (state.terminal_count + 1).min(8);
            state.active_tab = state.terminal_count - 1;
            state.focus = Focus::Terminal;
            state.notice = "New terminal tab created · Enter opens it · a switches to the Agent";
            Ok(None)
        }
        KeyCode::Char('a') => Ok(Some(PanelOutcome::OpenAgent)),
        KeyCode::Char('t') => {
            open_shell()?;
            state.notice =
                "Terminal closed · Enter opens the Simplicio Agent · + creates another tab";
            Ok(None)
        }
        KeyCode::Enter => {
            // Selecting a workspace item always enters the Code session. A
            // local shell is intentionally opt-in via `t` so clicking a
            // terminal cannot strand the user in a separate shell surface.
            Ok(Some(PanelOutcome::OpenAgent))
        }
        _ => Ok(None),
    }
}

fn handle_mouse(mouse: MouseEvent, state: &mut PanelState) -> Option<PanelOutcome> {
    if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        return None;
    }

    // The cockpit is a launcher: clicking a space, agent, terminal, or tab
    // opens the same main Code session. The plus tab remains the only
    // clickable control that mutates the cockpit itself.
    if mouse.row < 3 && mouse.column >= 28 {
        if mouse.column >= 30 + (state.terminal_count as u16 * 4) {
            state.terminal_count = (state.terminal_count + 1).min(8);
            state.active_tab = state.terminal_count - 1;
            state.focus = Focus::Terminal;
            state.notice = "New terminal tab created · click any item to open the Code session";
            return None;
        }
    }
    state.focus = if mouse.column < 28 {
        if mouse.row >= 12 {
            Focus::Terminal
        } else {
            Focus::Agent
        }
    } else {
        Focus::Terminal
    };
    Some(PanelOutcome::OpenAgent)
}

fn draw(frame: &mut ratatui::Frame<'_>, state: &PanelState) {
    let area = frame.area();
    let palette = Palette::default();
    frame.render_widget(
        Block::default().style(Style::default().bg(palette.background)),
        area,
    );

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(1)])
        .split(area);
    draw_sidebar(frame, columns[0], state, palette);

    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(columns[1]);
    draw_tabs(frame, main[0], state, palette);
    draw_center(frame, main[1], state, palette);
    draw_prompt(frame, main[2], state, palette);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ↑↓/Tab", Style::default().fg(palette.muted)),
            Span::styled(" navigate  ", Style::default().fg(palette.text)),
            Span::styled(
                "a",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Agent  ", Style::default().fg(palette.text)),
            Span::styled(
                "t",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" terminal  ", Style::default().fg(palette.text)),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit", Style::default().fg(palette.text)),
        ]))
        .style(Style::default().bg(palette.background)),
        main[3],
    );
}

fn draw_sidebar(frame: &mut ratatui::Frame<'_>, area: Rect, state: &PanelState, p: Palette) {
    let selected = |focus: Focus| {
        if state.focus == focus {
            Style::default()
                .fg(p.text)
                .bg(p.selection)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.muted)
        }
    };
    let lines = vec![
        Line::from(Span::styled(
            " spaces",
            Style::default().fg(p.muted).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  ◯  ai",
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  new", Style::default().fg(p.muted)),
            Span::raw("                         "),
            Span::styled("menu", Style::default().fg(p.muted)),
        ]),
        Line::from("──────────────────────────"),
        Line::from(Span::styled(
            " agents                 grouped",
            Style::default().fg(p.muted).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled("  ✓  ai", selected(Focus::Agent))),
        Line::from(Span::styled(
            "     simplicio_agent",
            Style::default().fg(p.muted),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  terminals",
            Style::default().fg(p.muted).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("     {} terminal(s)", state.terminal_count),
            selected(Focus::Terminal),
        )),
    ];
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::RIGHT)
                    .border_style(Style::default().fg(p.border)),
            )
            .style(Style::default().bg(p.sidebar))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_tabs(frame: &mut ratatui::Frame<'_>, area: Rect, state: &PanelState, p: Palette) {
    let mut titles = Vec::with_capacity(state.terminal_count + 1);
    for index in 0..state.terminal_count {
        titles.push(Line::from(format!(" {} ", index + 1)));
    }
    titles.push(Line::from(" + "));
    frame.render_widget(
        Tabs::new(titles)
            .select(state.active_tab.min(state.terminal_count))
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(p.tab_active)
                    .add_modifier(Modifier::BOLD),
            )
            .style(Style::default().fg(p.muted).bg(p.tab_background))
            .divider(Span::raw("")),
        area,
    );
}

fn draw_center(frame: &mut ratatui::Frame<'_>, area: Rect, state: &PanelState, p: Palette) {
    let title = if state.focus == Focus::Agent {
        "simplicio_agent"
    } else {
        "terminal"
    };
    let body = if state.focus == Focus::Agent {
        vec![
            Line::from(Span::styled(
                "Simplicio Agent",
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Session owned by simplicio_agent",
                Style::default().fg(p.green),
            )),
            Line::from(Span::styled(
                format!(
                    "Model/provider: {} / {}",
                    state.agent_selection.model, state.agent_selection.provider
                ),
                Style::default().fg(p.text),
            )),
            Line::from(Span::styled(
                "Tools: Simplicio Runtime MCP",
                Style::default().fg(p.text),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Press Enter or a to open the development session",
                Style::default().fg(p.muted),
            )),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                format!("Terminal {}", state.active_tab + 1),
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| ".".into()),
                Style::default().fg(p.muted),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Ready. Press Enter to open the Code session · t opens a shell.",
                Style::default().fg(p.text),
            )),
        ]
    };
    frame.render_widget(
        Paragraph::new(Text::from(body))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(p.border))
                    .title(format!(" {title} ")),
            )
            .style(Style::default().bg(p.background).fg(p.text))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_prompt(frame: &mut ratatui::Frame<'_>, area: Rect, state: &PanelState, p: Palette) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "❯ ",
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(state.notice, Style::default().fg(p.muted)),
        ]))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(p.border)),
        )
        .style(Style::default().bg(p.background))
        .wrap(Wrap { trim: false }),
        area,
    );
}

fn open_shell() -> Result<()> {
    restore_process_terminal()?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let status = Command::new(shell)
        .current_dir(std::env::current_dir().unwrap_or_else(|_| ".".into()))
        .status()?;
    if !status.success() {
        let _ = writeln!(io::stderr(), "terminal exited with {status}");
    }
    restore_panel_terminal()?;
    Ok(())
}

fn restore_process_terminal() -> Result<()> {
    disable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, DisableMouseCapture, Show, LeaveAlternateScreen)?;
    stdout.flush()?;
    Ok(())
}

fn restore_panel_terminal() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide)?;
    stdout.flush()?;
    Ok(())
}

fn restore_terminal<B: ratatui::backend::Backend + Write>(
    terminal: &mut Terminal<B>,
) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        Show,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

#[derive(Clone, Copy)]
struct Palette {
    background: Color,
    sidebar: Color,
    border: Color,
    selection: Color,
    tab_background: Color,
    tab_active: Color,
    text: Color,
    muted: Color,
    accent: Color,
    green: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            background: Color::Rgb(3, 14, 32),
            sidebar: Color::Rgb(4, 19, 40),
            border: Color::Rgb(31, 48, 77),
            selection: Color::Rgb(35, 34, 60),
            tab_background: Color::Rgb(19, 20, 42),
            tab_active: Color::Rgb(139, 177, 246),
            text: Color::Rgb(222, 229, 247),
            muted: Color::Rgb(127, 135, 164),
            accent: Color::Rgb(135, 218, 205),
            green: Color::Rgb(116, 219, 139),
        }
    }
}
