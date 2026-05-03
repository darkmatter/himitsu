//! 3-step init wizard: data directory → remote store → key provider.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};

use super::standard_canvas;

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
    /// Configure global default store (skipped if a default already exists).
    RemoteGlobal,
    /// Configure project-scoped store (skipped if not in a git repo).
    RemoteProject,
    Provider,
    Running,
    Success,
}

pub struct InitWizardView {
    step: Step,
    data_dir_input: String,
    /// Global-store slug input. Pre-filled with the personal-GitHub
    /// suggestion (`<user>/secrets`).
    global_remote_input: String,
    /// Project-store slug input. Pre-filled with `<repo-org>/secrets`
    /// derived from the current repo's git origin.
    project_remote_input: String,
    /// Whether a global default_store is already configured. When true,
    /// the global step is skipped entirely.
    has_existing_global: bool,
    /// Git root discovered at wizard construction. When `None`, the
    /// project step is skipped.
    git_root: Option<std::path::PathBuf>,
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

        let has_existing_global = config::Config::load(&config::config_path())
            .ok()
            .and_then(|cfg| cfg.default_store)
            .is_some();
        let git_root = std::env::current_dir()
            .ok()
            .and_then(|cwd| config::find_git_root(&cwd));

        Self {
            step: Step::DataDir,
            data_dir_input: config::data_dir().to_string_lossy().into_owned(),
            global_remote_input: init::suggested_remote_slug(),
            project_remote_input: init::suggested_project_slug(),
            has_existing_global,
            git_root,
            provider_options,
            provider_index: 0,
            error: None,
            pubkey: String::new(),
            outcome: Outcome::Pending,
            pending_init: None,
        }
    }

    /// First remote step to enter after `DataDir`. Skips the global step
    /// when one is already configured, and skips both when there's no git
    /// repo and no global to configure.
    fn first_remote_step(&self) -> Step {
        if !self.has_existing_global {
            Step::RemoteGlobal
        } else if self.git_root.is_some() {
            Step::RemoteProject
        } else {
            Step::Provider
        }
    }

    /// Step that follows `RemoteGlobal`. Goes to `RemoteProject` when in a
    /// git repo, otherwise straight to `Provider`.
    fn after_global_step(&self) -> Step {
        if self.git_root.is_some() {
            Step::RemoteProject
        } else {
            Step::Provider
        }
    }

    /// Total number of input steps shown in the header counter (DataDir +
    /// any remote steps + Provider when more than one option exists).
    fn total_steps(&self) -> usize {
        let mut n = 1; // DataDir
        if !self.has_existing_global {
            n += 1;
        }
        if self.git_root.is_some() {
            n += 1;
        }
        if self.provider_options.len() > 1 {
            n += 1;
        }
        n
    }

    /// 1-based index of `step` in the active sequence, used in the header.
    fn step_index(&self, step: Step) -> usize {
        let mut idx = 1; // DataDir is always step 1
        if step == Step::DataDir {
            return idx;
        }
        if !self.has_existing_global {
            idx += 1;
            if step == Step::RemoteGlobal {
                return idx;
            }
        }
        if self.git_root.is_some() {
            idx += 1;
            if step == Step::RemoteProject {
                return idx;
            }
        }
        if self.provider_options.len() > 1 {
            idx += 1;
            if step == Step::Provider {
                return idx;
            }
        }
        idx
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
        } else if self.git_root.is_some() {
            Step::RemoteProject
        } else if !self.has_existing_global {
            Step::RemoteGlobal
        } else {
            Step::DataDir
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
                        self.step = self.first_remote_step();
                        if self.step == Step::Provider && self.provider_options.len() <= 1 {
                            self.begin_init();
                        }
                    }
                }
                KeyCode::Esc => self.outcome = Outcome::Aborted,
                KeyCode::Backspace => {
                    self.data_dir_input.pop();
                }
                KeyCode::Char(c) => self.data_dir_input.push(c),
                _ => {}
            },
            Step::RemoteGlobal => match key.code {
                KeyCode::Enter => {
                    let trimmed = self.global_remote_input.trim();
                    if !trimmed.is_empty() {
                        if let Err(e) = config::validate_remote_slug(trimmed) {
                            self.error = Some(e.to_string());
                            return;
                        }
                    }
                    self.step = self.after_global_step();
                    if self.step == Step::Provider && self.provider_options.len() <= 1 {
                        self.begin_init();
                    }
                }
                KeyCode::Esc => self.step = Step::DataDir,
                KeyCode::Backspace => {
                    self.global_remote_input.pop();
                }
                KeyCode::Char(c) => self.global_remote_input.push(c),
                _ => {}
            },
            Step::RemoteProject => match key.code {
                KeyCode::Enter => {
                    let trimmed = self.project_remote_input.trim();
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
                KeyCode::Esc => {
                    self.step = if !self.has_existing_global {
                        Step::RemoteGlobal
                    } else {
                        Step::DataDir
                    };
                }
                KeyCode::Backspace => {
                    self.project_remote_input.pop();
                }
                KeyCode::Char(c) => self.project_remote_input.push(c),
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
                KeyCode::Esc => {
                    self.step = if self.git_root.is_some() {
                        Step::RemoteProject
                    } else if !self.has_existing_global {
                        Step::RemoteGlobal
                    } else {
                        Step::DataDir
                    };
                }
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

        let name = if self.has_existing_global {
            None
        } else {
            let r = self.global_remote_input.trim().to_string();
            if r.is_empty() {
                None
            } else {
                Some(r)
            }
        };
        let project = if self.git_root.is_some() {
            let r = self.project_remote_input.trim().to_string();
            if r.is_empty() {
                None
            } else {
                Some(r)
            }
        } else {
            None
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
            project,
        });
        self.step = Step::Running;
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let full = frame.area();
        // Paint the active theme's background across the wizard frame so
        // first-run users see their selected theme immediately.
        frame.render_widget(
            Block::default().style(Style::default().bg(theme::background())),
            full,
        );
        let area = standard_canvas(full);
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(5),
                Constraint::Length(1),
            ])
            .split(area);

        let header_text = match self.step {
            Step::DataDir => format!(
                " himitsu setup — step 1/{}: data directory ",
                self.total_steps()
            ),
            Step::RemoteGlobal => format!(
                " himitsu setup — step {}/{}: configure global store ",
                self.step_index(Step::RemoteGlobal),
                self.total_steps()
            ),
            Step::RemoteProject => format!(
                " himitsu setup — step {}/{}: configure project ",
                self.step_index(Step::RemoteProject),
                self.total_steps()
            ),
            Step::Provider => format!(
                " himitsu setup — step {}/{}: key provider ",
                self.step_index(Step::Provider),
                self.total_steps()
            ),
            Step::Running => " himitsu setup — initializing… ".to_string(),
            Step::Success => " himitsu setup — ready ".to_string(),
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
            Step::RemoteGlobal | Step::RemoteProject => {
                "enter next (blank to skip)   esc back   ctrl-c abort"
            }
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
            Step::RemoteGlobal => {
                lines.push(Line::from(""));
                lines.push(Line::from("  Configure your global default store."));
                lines.push(Line::from(Span::styled(
                    "  This is the store himitsu uses outside any project.",
                    Style::default().fg(theme::muted()),
                )));
                lines.push(Line::from(Span::styled(
                    "  Most people use a private repo on their personal GitHub.",
                    Style::default().fg(theme::muted()),
                )));
                lines.push(Line::from(Span::styled(
                    "  format: your-github-username/repo (for example, alice/secrets)",
                    Style::default().fg(theme::muted()),
                )));
                lines.push(Line::from(""));
                lines.push(self.text_input_line(&self.global_remote_input));
            }
            Step::RemoteProject => {
                let path = self
                    .git_root
                    .as_ref()
                    .map(|r| r.join("himitsu.yaml").display().to_string())
                    .unwrap_or_default();
                lines.push(Line::from(""));
                lines.push(Line::from("  Configure a shared store for this project."));
                lines.push(Line::from(Span::styled(
                    format!("  Writes default_store to {path}"),
                    Style::default().fg(theme::muted()),
                )));
                lines.push(Line::from(Span::styled(
                    "  Defaults to <repo-org>/secrets — overrides the global default in this repo.",
                    Style::default().fg(theme::muted()),
                )));
                lines.push(Line::from(""));
                lines.push(self.text_input_line(&self.project_remote_input));
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
