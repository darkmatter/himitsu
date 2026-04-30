//! 3-step init wizard: data directory → remote store → key provider.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};

use crate::tui::theme;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::cli::init::{self, InitArgs};
use crate::config::{self, KeyProvider};
use crate::error::Result;

pub enum Outcome {
    Pending,
    Aborted,
    Completed,
}

#[derive(Clone, Copy, PartialEq)]
enum Step {
    DataDir,
    Remote,
    Provider,
    Running,
    Success,
}

pub struct InitWizardView {
    step: Step,
    data_dir_input: String,
    remote_input: String,
    provider_options: Vec<KeyProvider>,
    provider_index: usize,
    error: Option<String>,
    pubkey: String,
    outcome: Outcome,
    pending_init: Option<InitArgs>,
}

impl InitWizardView {
    pub fn new() -> Self {
        let mut provider_options = vec![KeyProvider::Disk];
        if crate::keyring::macos::MacOSKeychain::is_available() {
            provider_options.push(KeyProvider::MacosKeychain);
        }
        Self {
            step: Step::DataDir,
            data_dir_input: config::data_dir().to_string_lossy().into_owned(),
            remote_input: init::suggested_remote_slug(),
            provider_options,
            provider_index: 0,
            error: None,
            pubkey: String::new(),
            outcome: Outcome::Pending,
            pending_init: None,
        }
    }

    pub fn outcome(&self) -> &Outcome {
        &self.outcome
    }

    pub fn take_pending_init(&mut self) -> Option<InitArgs> {
        self.pending_init.take()
    }

    pub fn on_init_result(&mut self, result: Result<()>) {
        match result {
            Ok(()) => match init::read_public_key(&config::data_dir()) {
                Ok(pk) => {
                    self.pubkey = pk;
                    self.step = Step::Success;
                }
                Err(e) => {
                    self.error = Some(format!("init ok but pubkey read failed: {e}"));
                    self.step = self.last_input_step();
                }
            },
            Err(e) => {
                self.error = Some(format!("init failed: {e}"));
                self.step = self.last_input_step();
            }
        }
    }

    fn last_input_step(&self) -> Step {
        if self.provider_options.len() > 1 {
            Step::Provider
        } else {
            Step::Remote
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if matches!(
            (key.code, key.modifiers),
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
        ) {
            self.outcome = Outcome::Aborted;
            return;
        }

        // Reset transient error on any keystroke that could advance.
        if !matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace) {
            self.error = None;
        }

        match self.step {
            Step::DataDir => match key.code {
                KeyCode::Enter => {
                    if self.data_dir_input.trim().is_empty() {
                        self.error = Some("Please enter a directory path.".into());
                    } else {
                        self.step = Step::Remote;
                    }
                }
                KeyCode::Esc => self.outcome = Outcome::Aborted,
                KeyCode::Backspace => {
                    self.data_dir_input.pop();
                }
                KeyCode::Char(c) => self.data_dir_input.push(c),
                _ => {}
            },
            Step::Remote => match key.code {
                KeyCode::Enter => {
                    let trimmed = self.remote_input.trim();
                    if !trimmed.is_empty() {
                        if let Err(e) = config::validate_remote_slug(trimmed) {
                            self.error = Some(e.to_string());
                            return;
                        }
                    }
                    if self.provider_options.len() > 1 {
                        self.step = Step::Provider;
                    } else {
                        self.begin_init();
                    }
                }
                KeyCode::Esc => self.step = Step::DataDir,
                KeyCode::Backspace => {
                    self.remote_input.pop();
                }
                KeyCode::Char(c) => self.remote_input.push(c),
                _ => {}
            },
            Step::Provider => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if self.provider_index > 0 {
                        self.provider_index -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.provider_index + 1 < self.provider_options.len() {
                        self.provider_index += 1;
                    }
                }
                KeyCode::Enter => self.begin_init(),
                KeyCode::Esc => self.step = Step::Remote,
                _ => {}
            },
            Step::Running => {}
            Step::Success => match key.code {
                KeyCode::Enter => self.outcome = Outcome::Completed,
                KeyCode::Esc => self.outcome = Outcome::Aborted,
                _ => {}
            },
        }
    }

    fn begin_init(&mut self) {
        // Persist a custom data_dir override before re-deriving paths. Matches
        // the `--home` branch of `init::run` so subsequent calls to
        // `config::data_dir()` pick up the new location.
        let home = self.data_dir_input.trim().to_string();
        if home != config::data_dir().to_string_lossy() {
            let cfg_path = config::config_path();
            match config::Config::load(&cfg_path) {
                Ok(mut cfg) => {
                    cfg.data_dir = Some(home);
                    if let Err(e) = cfg.save(&cfg_path) {
                        self.error = Some(format!("failed to save config: {e}"));
                        return;
                    }
                }
                Err(e) => {
                    self.error = Some(format!("failed to load config: {e}"));
                    return;
                }
            }
        }

        let provider = self
            .provider_options
            .get(self.provider_index)
            .cloned()
            .unwrap_or_default();

        let name = {
            let r = self.remote_input.trim().to_string();
            if r.is_empty() {
                None
            } else {
                Some(r)
            }
        };
        let key_provider = if provider == KeyProvider::Disk {
            None
        } else {
            Some(provider.to_string())
        };

        self.pending_init = Some(InitArgs {
            json: false,
            name,
            url: None,
            home: None,
            key_provider,
            no_tui: true,
        });
        self.step = Step::Running;
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        // Paint the active theme's background across the wizard frame so
        // first-run users see their selected theme immediately.
        frame.render_widget(
            Block::default().style(Style::default().bg(theme::background())),
            area,
        );
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(5),
                Constraint::Length(1),
            ])
            .split(area);

        let header_text = match self.step {
            Step::DataDir => " himitsu setup — step 1/3: data directory ",
            Step::Remote => " himitsu setup — step 2/3: default store ",
            Step::Provider => " himitsu setup — step 3/3: key provider ",
            Step::Running => " himitsu setup — initializing… ",
            Step::Success => " himitsu setup — ready ",
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                header_text,
                Style::default().add_modifier(Modifier::BOLD),
            )),
            layout[0],
        );

        let body = Paragraph::new(self.body_lines()).block(Block::default().borders(Borders::ALL));
        frame.render_widget(body, layout[1]);

        let footer = match self.step {
            Step::DataDir => "enter next   esc abort   ctrl-c abort",
            Step::Remote => "enter next   esc back   ctrl-c abort",
            Step::Provider => "↑/↓ select   enter confirm   esc back   ctrl-c abort",
            Step::Running => "please wait…",
            Step::Success => "enter continue   esc abort",
        };
        frame.render_widget(
            Paragraph::new(Span::raw(footer)).alignment(Alignment::Left),
            layout[2],
        );
    }

    fn body_lines(&self) -> Vec<Line<'_>> {
        let mut lines: Vec<Line<'_>> = Vec::new();
        match self.step {
            Step::DataDir => {
                lines.push(Line::from(""));
                lines.push(Line::from(
                    "  Where should himitsu store its data (age keys, config)?",
                ));
                lines.push(Line::from(""));
                lines.push(self.text_input_line(&self.data_dir_input));
            }
            Step::Remote => {
                lines.push(Line::from(""));
                lines.push(Line::from(
                    "  Primary store on your personal GitHub (blank to skip)",
                ));
                lines.push(Line::from(Span::styled(
                    "  format: your-github-username/repo (for example, alice/secrets)",
                    Style::default().fg(theme::muted()),
                )));
                lines.push(Line::from(""));
                lines.push(self.text_input_line(&self.remote_input));
            }
            Step::Provider => {
                lines.push(Line::from(""));
                lines.push(Line::from("  Key storage backend"));
                lines.push(Line::from(""));
                for (i, opt) in self.provider_options.iter().enumerate() {
                    let selected = i == self.provider_index;
                    let style = if selected {
                        Style::default()
                            .fg(theme::accent())
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    let prefix = if selected { "  > " } else { "    " };
                    let label = match opt {
                        KeyProvider::Disk => "Disk — keys stored in the data directory",
                        KeyProvider::MacosKeychain => {
                            "macOS Keychain — stored via the `security` CLI"
                        }
                    };
                    lines.push(Line::from(vec![
                        Span::styled(prefix, style),
                        Span::styled(label, style),
                    ]));
                }
            }
            Step::Running => {
                lines.push(Line::from(""));
                lines.push(Line::from("  Initializing…"));
            }
            Step::Success => {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "  ✓ Initialized",
                    Style::default()
                        .fg(theme::success())
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::raw("  Public key: "),
                    Span::styled(self.pubkey.clone(), Style::default().fg(theme::accent())),
                ]));
                lines.push(Line::from(""));
                lines.push(Line::from("  Press Enter to continue to the dashboard."));
            }
        }

        if let Some(err) = &self.error {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("  ! {err}"),
                Style::default().fg(theme::danger()),
            )));
        }

        lines
    }

    fn text_input_line<'a>(&self, value: &'a str) -> Line<'a> {
        Line::from(vec![
            Span::styled("  > ", Style::default().fg(theme::accent())),
            Span::raw(value),
            Span::styled("▏", Style::default().fg(theme::accent())),
        ])
    }
}
