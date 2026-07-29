//! The interactive part of `open-interceptor setup`.
//!
//! A small ratatui wizard: pick providers, type the keys they need, confirm.
//! It only *collects* choices — writing the config and touching the daemon
//! happens in [`super`], after the terminal has been restored, so any error
//! from those steps is printed as normal scrollback instead of being wiped by
//! the alternate screen.

use std::collections::HashMap;
use std::io::{self, Stdout};

use anyhow::Context;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Padding, Paragraph, Wrap};

use super::catalog::{CatalogEntry, EntryKind, catalog};

/// What the user chose. Consumed by [`super::apply`].
pub struct Outcome {
    /// Selected catalog entries, in catalog order.
    pub selected: Vec<EntryKind>,
    /// Field answers keyed by `FieldSpec::id`.
    pub values: HashMap<String, String>,
    /// Whether to install and start the background daemon straight away.
    pub start_daemon: bool,
}

/// Wizard steps, in order.
#[derive(Debug, PartialEq, Clone, Copy)]
enum Step {
    Welcome,
    Select,
    Fields,
    Confirm,
}

struct App {
    step: Step,
    entries: Vec<CatalogEntry>,
    checked: Vec<bool>,
    cursor: usize,
    /// Flattened inputs for the currently selected entries: (field index into
    /// the owning entry, current text). Rebuilt whenever the selection changes.
    fields: Vec<ActiveField>,
    field_cursor: usize,
    start_daemon: bool,
    /// Shown in red under the body; cleared on the next keypress.
    error: Option<String>,
    /// Set when the user asks to quit.
    cancelled: bool,
    done: bool,
}

struct ActiveField {
    id: &'static str,
    label: &'static str,
    placeholder: &'static str,
    masked: bool,
    owner: &'static str,
    value: String,
}

impl App {
    fn new() -> Self {
        let entries = catalog();
        let checked = entries.iter().map(|e| e.default_selected).collect();
        Self {
            step: Step::Welcome,
            entries,
            checked,
            cursor: 0,
            fields: Vec::new(),
            field_cursor: 0,
            start_daemon: true,
            error: None,
            cancelled: false,
            done: false,
        }
    }

    fn selected_kinds(&self) -> Vec<EntryKind> {
        self.entries
            .iter()
            .zip(&self.checked)
            .filter(|(_, on)| **on)
            .map(|(e, _)| e.kind)
            .collect()
    }

    /// Rebuild the input list from the current selection, keeping anything the
    /// user already typed for fields that are still relevant.
    fn rebuild_fields(&mut self) {
        let previous: HashMap<&str, String> = self
            .fields
            .iter()
            .map(|f| (f.id, f.value.clone()))
            .collect();
        // Borrow rather than move: the flat_map closure runs once per entry.
        let previous = &previous;

        self.fields = self
            .entries
            .iter()
            .zip(&self.checked)
            .filter(|(_, on)| **on)
            .flat_map(|(entry, _)| {
                entry.fields.iter().map(move |f| ActiveField {
                    id: f.id,
                    label: f.label,
                    placeholder: f.placeholder,
                    masked: f.masked,
                    owner: entry.label,
                    value: previous.get(f.id).cloned().unwrap_or_default(),
                })
            })
            .collect();
        self.field_cursor = 0;
    }

    fn values(&self) -> HashMap<String, String> {
        self.fields
            .iter()
            .map(|f| (f.id.to_string(), f.value.clone()))
            .collect()
    }

    /// Advance, skipping the input step when nothing needs typing.
    fn next_step(&mut self) {
        self.error = None;
        match self.step {
            Step::Welcome => self.step = Step::Select,
            Step::Select => {
                if self.selected_kinds().is_empty() {
                    self.error = Some("Pick at least one provider (space to toggle).".into());
                    return;
                }
                self.rebuild_fields();
                self.step = if self.fields.is_empty() {
                    Step::Confirm
                } else {
                    Step::Fields
                };
            }
            Step::Fields => {
                if let Some(missing) = self.first_missing_required() {
                    self.error = Some(format!("{missing} is required."));
                    return;
                }
                self.step = Step::Confirm;
            }
            Step::Confirm => self.done = true,
        }
    }

    fn prev_step(&mut self) {
        self.error = None;
        self.step = match self.step {
            Step::Welcome => Step::Welcome,
            Step::Select => Step::Welcome,
            Step::Fields => Step::Select,
            Step::Confirm => {
                if self.fields.is_empty() {
                    Step::Select
                } else {
                    Step::Fields
                }
            }
        };
    }

    /// A custom endpoint without a URL cannot work; everything else may be
    /// left blank (local servers often need no key).
    fn first_missing_required(&self) -> Option<&'static str> {
        self.fields
            .iter()
            .find(|f| f.id == "custom_url" && f.value.trim().is_empty())
            .map(|f| f.label)
    }
}

/// Run the wizard. Returns `None` if the user cancelled.
///
/// The terminal is restored before returning in every path, including panics
/// upstream of the caller's error handling.
pub fn run() -> anyhow::Result<Option<Outcome>> {
    let mut terminal = enter().context("could not switch the terminal to raw mode")?;
    let result = event_loop(&mut terminal);
    // Restore first, propagate second: an error message is useless if the
    // terminal is still in raw mode when it prints.
    let restored = leave(&mut terminal);
    let app = result?;
    restored?;

    if app.cancelled || !app.done {
        return Ok(None);
    }
    Ok(Some(Outcome {
        selected: app.selected_kinds(),
        values: app.values(),
        start_daemon: app.start_daemon,
    }))
}

type Term = Terminal<CrosstermBackend<Stdout>>;

fn enter() -> anyhow::Result<Term> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

fn leave(terminal: &mut Term) -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn event_loop(terminal: &mut Term) -> anyhow::Result<App> {
    let mut app = App::new();
    loop {
        terminal.draw(|f| draw(f, &app))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        // Windows reports press *and* release; only act on press.
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // Ctrl-C always aborts, whatever the step.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            app.cancelled = true;
            return Ok(app);
        }

        match app.step {
            Step::Welcome => match key.code {
                KeyCode::Enter => app.next_step(),
                KeyCode::Esc | KeyCode::Char('q') => {
                    app.cancelled = true;
                    return Ok(app);
                }
                _ => {}
            },

            Step::Select => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    app.cursor = app.cursor.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    app.cursor = (app.cursor + 1).min(app.entries.len().saturating_sub(1));
                }
                KeyCode::Char(' ') => {
                    app.checked[app.cursor] = !app.checked[app.cursor];
                    app.error = None;
                }
                KeyCode::Enter => app.next_step(),
                KeyCode::Esc => app.prev_step(),
                KeyCode::Char('q') => {
                    app.cancelled = true;
                    return Ok(app);
                }
                _ => {}
            },

            // Text entry: `q` must be typeable, so only Esc goes back.
            Step::Fields => match key.code {
                KeyCode::Enter | KeyCode::Tab | KeyCode::Down => {
                    if app.field_cursor + 1 < app.fields.len() && key.code != KeyCode::Enter {
                        app.field_cursor += 1;
                    } else if key.code == KeyCode::Enter {
                        if app.field_cursor + 1 < app.fields.len() {
                            app.field_cursor += 1;
                        } else {
                            app.next_step();
                        }
                    }
                }
                KeyCode::BackTab | KeyCode::Up => {
                    app.field_cursor = app.field_cursor.saturating_sub(1);
                }
                KeyCode::Backspace => {
                    if let Some(f) = app.fields.get_mut(app.field_cursor) {
                        f.value.pop();
                    }
                    app.error = None;
                }
                KeyCode::Char(c) => {
                    if let Some(f) = app.fields.get_mut(app.field_cursor) {
                        f.value.push(c);
                    }
                    app.error = None;
                }
                KeyCode::Esc => app.prev_step(),
                _ => {}
            },

            Step::Confirm => match key.code {
                KeyCode::Enter => {
                    app.next_step();
                    if app.done {
                        return Ok(app);
                    }
                }
                KeyCode::Char('d') => app.start_daemon = !app.start_daemon,
                KeyCode::Esc => app.prev_step(),
                KeyCode::Char('q') => {
                    app.cancelled = true;
                    return Ok(app);
                }
                _ => {}
            },
        }
    }
}

// ---- rendering ------------------------------------------------------------

const ACCENT: Color = Color::Cyan;

fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3), // title
        Constraint::Min(6),    // body
        Constraint::Length(3), // footer (hint + error)
    ])
    .split(f.area());

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " open-interceptor ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("setup", Style::default().fg(Color::Gray)),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(title, chunks[0]);

    match app.step {
        Step::Welcome => draw_welcome(f, chunks[1]),
        Step::Select => draw_select(f, chunks[1], app),
        Step::Fields => draw_fields(f, chunks[1], app),
        Step::Confirm => draw_confirm(f, chunks[1], app),
    }

    let hint = match app.step {
        Step::Welcome => "Enter continue · q quit",
        Step::Select => "↑↓ move · space toggle · Enter continue · Esc back",
        Step::Fields => "type to edit · Tab next field · Enter continue · Esc back",
        Step::Confirm => "Enter apply · d toggle daemon · Esc back · q quit",
    };
    let footer = match &app.error {
        Some(e) => Paragraph::new(Line::from(Span::styled(
            format!("  {e}"),
            Style::default().fg(Color::Red),
        ))),
        None => Paragraph::new(Line::from(Span::styled(
            format!("  {hint}"),
            Style::default().fg(Color::DarkGray),
        ))),
    }
    .block(Block::default().borders(Borders::TOP));
    f.render_widget(footer, chunks[2]);
}

fn body_block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::NONE)
        .padding(Padding::new(2, 2, 1, 0))
        .title(Span::styled(
            title.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ))
}

fn draw_welcome(f: &mut Frame, area: Rect) {
    let text = vec![
        Line::from(""),
        Line::from("This will set up the proxy in two steps:"),
        Line::from(""),
        Line::from("  1. Write a config with the providers you pick."),
        Line::from("  2. Install and start the background daemon."),
        Line::from(""),
        Line::from(Span::styled(
            "Nothing is written until the final confirmation.",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: true })
            .block(body_block("Welcome")),
        area,
    );
}

fn draw_select(f: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let selected = i == app.cursor;
            let mark = if app.checked[i] { "[x]" } else { "[ ]" };
            let label_style = if selected {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(if selected { " > " } else { "   " }, label_style),
                    Span::styled(format!("{mark} "), label_style),
                    Span::styled(entry.label, label_style),
                ]),
                Line::from(Span::styled(
                    format!("       {}", entry.blurb),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();

    f.render_widget(List::new(items).block(body_block("Which providers?")), area);
}

fn draw_fields(f: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = vec![Line::from("")];
    let mut current_owner = "";

    for (i, field) in app.fields.iter().enumerate() {
        if field.owner != current_owner {
            current_owner = field.owner;
            lines.push(Line::from(Span::styled(
                field.owner,
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
            )));
        }

        let focused = i == app.field_cursor;
        let shown = if field.value.is_empty() {
            Span::styled(field.placeholder, Style::default().fg(Color::DarkGray))
        } else if field.masked {
            Span::raw("•".repeat(field.value.chars().count().min(40)))
        } else {
            Span::raw(field.value.clone())
        };

        lines.push(Line::from(vec![
            Span::styled(
                if focused { " > " } else { "   " },
                Style::default().fg(ACCENT),
            ),
            Span::styled(
                format!("{}: ", field.label),
                if focused {
                    Style::default().fg(ACCENT)
                } else {
                    Style::default()
                },
            ),
            shown,
            Span::styled(if focused { "▌" } else { "" }, Style::default().fg(ACCENT)),
        ]));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        "Keys are stored in the config file (readable only by you). You can also",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::styled(
        "type ${MY_ENV_VAR} to keep the secret in your environment instead.",
        Style::default().fg(Color::DarkGray),
    )));

    f.render_widget(Paragraph::new(lines).block(body_block("Credentials")), area);
}

fn draw_confirm(f: &mut Frame, area: Rect, app: &App) {
    let mut lines = vec![Line::from("")];

    lines.push(Line::from(Span::styled(
        "Providers",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for entry in app.entries.iter().zip(&app.checked).filter(|(_, on)| **on) {
        lines.push(Line::from(format!("   • {}", entry.0.label)));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Config file",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(format!("   {}", super::config_path().display())));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Daemon  ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            if app.start_daemon {
                "install and start now"
            } else {
                "skip for now"
            },
            Style::default().fg(if app.start_daemon {
                Color::Green
            } else {
                Color::Gray
            }),
        ),
        Span::styled(
            "  (press d to change)",
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(body_block("Ready")),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_step_refuses_an_empty_selection() {
        let mut app = App::new();
        app.checked = vec![false; app.entries.len()];
        app.step = Step::Select;
        app.next_step();
        assert_eq!(
            app.step,
            Step::Select,
            "must not advance with nothing picked"
        );
        assert!(app.error.is_some());
    }

    #[test]
    fn entries_without_fields_skip_the_credentials_step() {
        let mut app = App::new();
        app.step = Step::Select;
        // Anthropic only — it needs no key.
        app.checked = app
            .entries
            .iter()
            .map(|e| e.kind == EntryKind::Anthropic)
            .collect();
        app.next_step();
        assert_eq!(app.step, Step::Confirm);
    }

    #[test]
    fn typed_values_survive_toggling_another_provider() {
        let mut app = App::new();
        app.checked = app
            .entries
            .iter()
            .map(|e| e.kind == EntryKind::OpenAi)
            .collect();
        app.rebuild_fields();
        app.fields[0].value = "sk-typed".into();

        // Also tick OpenCode Go; the OpenAI key must not be lost.
        for (i, e) in app.entries.iter().enumerate() {
            if e.kind == EntryKind::OpenCodeGo {
                app.checked[i] = true;
            }
        }
        app.rebuild_fields();

        assert_eq!(
            app.values().get("openai_key").map(String::as_str),
            Some("sk-typed")
        );
    }

    #[test]
    fn custom_endpoint_requires_a_url() {
        let mut app = App::new();
        app.checked = app
            .entries
            .iter()
            .map(|e| e.kind == EntryKind::CustomOpenAi)
            .collect();
        app.rebuild_fields();
        app.step = Step::Fields;
        app.next_step();
        assert_eq!(app.step, Step::Fields);
        assert!(app.error.is_some());
    }
}
