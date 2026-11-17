mod chrome;
mod update;

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseEventKind,
    },
    execute,
};
use notify::{EventKind, RecursiveMode, Watcher};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};
use std::{
    collections::BTreeSet,
    error::Error,
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tuifinder::{
    files::{Entry, clean_label},
    git,
    search::{Hit, Kind},
    worker::{self, Document, Request, SearchRequest},
};

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    Tree,
    Code,
}
struct Search {
    kind: Kind,
    query: String,
    hits: Vec<Hit>,
    selected: usize,
    offset: usize,
    editing: bool,
    pending: bool,
    note: String,
    started: Option<Instant>,
}
struct App {
    root: PathBuf,
    entries: Vec<Entry>,
    tree_index: usize,
    tree_offset: usize,
    expanded: BTreeSet<PathBuf>,
    selected: Option<PathBuf>,
    document: Option<Arc<Document>>,
    diff: bool,
    changes: bool,
    ignored: bool,
    focus: Focus,
    scroll: usize,
    horizontal: u16,
    branch: String,
    git_available: bool,
    changed_count: usize,
    status: String,
    loading: bool,
    search: Option<Search>,
    help: bool,
    tree_area: Rect,
    code_area: Rect,
    search_area: Rect,
    revision: u64,
    generation: Arc<AtomicU64>,
    search_generation: Arc<AtomicU64>,
    jump: Option<usize>,
    pending_measurement: Option<(&'static str, Instant)>,
    measurement: Option<chrome::Measurement>,
}
impl App {
    fn request(&mut self, tx: &worker::Mailbox<Request>) {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.loading = true;
        tx.send(Request {
            generation,
            revision: self.revision,
            expanded: self.expanded.clone(),
            selected: self.selected.clone(),
            diff: self.diff,
            changed_only: self.changes,
            show_ignored: self.ignored,
        });
    }
    fn open(&mut self, path: PathBuf, line: usize, tx: &worker::Mailbox<Request>) {
        self.pending_measurement = Some((if self.diff { "diff" } else { "open" }, Instant::now()));
        self.selected = Some(path);
        self.scroll = line;
        self.horizontal = 0;
        self.jump = Some(line);
        self.focus = Focus::Code;
        self.document = None;
        self.request(tx);
    }
    fn activate(&mut self, tx: &worker::Mailbox<Request>) {
        let Some(entry) = self.entries.get(self.tree_index) else {
            return;
        };
        if entry.directory {
            if self.changes {
                return;
            }
            if !self.expanded.remove(&entry.path) {
                self.expanded.insert(entry.path.clone());
            }
            self.pending_measurement = Some(("tree", Instant::now()));
            self.request(tx);
        } else {
            self.open(entry.path.clone(), 0, tx);
        }
    }
    fn start_search(&mut self, kind: Kind) {
        self.search_generation.fetch_add(1, Ordering::Relaxed);
        self.search = Some(Search {
            kind,
            query: String::new(),
            hits: vec![],
            selected: 0,
            offset: 0,
            editing: true,
            pending: false,
            note: String::new(),
            started: None,
        });
    }
    fn submit_search(&mut self, tx: &worker::Mailbox<SearchRequest>) {
        let Some(search) = self.search.as_mut() else {
            return;
        };
        search.selected = 0;
        search.offset = 0;
        search.hits.clear();
        search.note.clear();
        search.editing = false;
        search.pending = false;
        if search.query.is_empty() {
            search.note = "Empty query".into();
            return;
        }
        let started = *search.started.get_or_insert_with(Instant::now);
        if search.kind == Kind::Current {
            if self.loading {
                search.pending = true;
                return;
            }
            if let Some(doc) = &self.document {
                for (line, text) in doc.plain.iter().enumerate() {
                    if text.contains(&search.query) {
                        search.hits.push(Hit {
                            path: doc.path.clone(),
                            line,
                            preview: clean_label(&text.chars().take(160).collect::<String>()),
                        });
                    }
                    if search.hits.len() == tuifinder::search::LIMIT {
                        search.note = "First 1000 matches".into();
                        break;
                    }
                }
            }
            search.started = None;
            self.measurement = Some(chrome::Measurement {
                label: "find",
                elapsed: started.elapsed(),
                failed: self.document.as_ref().is_none_or(|d| d.error.is_some()),
            });
        } else {
            search.pending = true;
            let generation = self.search_generation.fetch_add(1, Ordering::Relaxed) + 1;
            tx.send(SearchRequest {
                generation,
                query: search.query.clone(),
                kind: search.kind,
                show_ignored: self.ignored,
            });
        }
    }
}

fn watch(root: PathBuf, dirty: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let rescan = Arc::new(AtomicBool::new(true));
        let signal = Arc::clone(&rescan);
        let changed = Arc::clone(&dirty);
        let watcher =
            notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
                match event {
                    Ok(e) if !matches!(e.kind, EventKind::Access(_)) => {
                        changed.store(true, Ordering::Relaxed);
                        if matches!(
                            e.kind,
                            EventKind::Create(_)
                                | EventKind::Remove(_)
                                | EventKind::Modify(notify::event::ModifyKind::Name(_))
                        ) {
                            signal.store(true, Ordering::Relaxed);
                        }
                    }
                    Err(_) => {
                        changed.store(true, Ordering::Relaxed);
                        signal.store(true, Ordering::Relaxed);
                    }
                    _ => {}
                }
            });
        let Ok(mut watcher) = watcher else {
            return;
        };
        let mut watched = BTreeSet::new();
        loop {
            if rescan.swap(false, Ordering::Relaxed) {
                let mut dirs = BTreeSet::new();
                let mut walk = ignore::WalkBuilder::new(&root);
                walk.hidden(false)
                    .follow_links(false)
                    .filter_entry(|e| e.file_name() != ".git");
                for entry in walk.build().flatten() {
                    if entry.file_type().is_some_and(|t| t.is_dir()) {
                        dirs.insert(entry.path().to_path_buf());
                    }
                }
                // Git metadata is small; repository file watching respects ignore rules.
                if root.join(".git").is_dir() {
                    let _ = watcher.watch(&root.join(".git"), RecursiveMode::Recursive);
                }
                for path in dirs.difference(&watched) {
                    let _ = watcher.watch(path, RecursiveMode::NonRecursive);
                }
                for path in watched.difference(&dirs) {
                    let _ = watcher.unwatch(path);
                }
                watched = dirs;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let arg = args.next();
    if args.next().is_some() {
        return Err("Usage: tuifinder [PATH] | --version | --update".into());
    }
    if arg.as_deref() == Some(std::ffi::OsStr::new("--help"))
        || arg.as_deref() == Some(std::ffi::OsStr::new("-h"))
    {
        println!(
            "tuifinder [PATH]\ntuifinder --version | --update\n\nRead-only live code explorer for SSH and tmux.\n\nEnter/click open · Tab focus · d diff · g changes · i ignored\np/Ctrl-p files · / current search · Ctrl-f repository search\nr refresh · ? help · q quit\n\nGit repository paths open at their repository root. Requires Git for Git features."
        );
        return Ok(());
    }
    if arg.as_deref() == Some(std::ffi::OsStr::new("--version"))
        || arg.as_deref() == Some(std::ffi::OsStr::new("-V"))
    {
        println!("tuifinder {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if arg.as_deref() == Some(std::ffi::OsStr::new("--update")) {
        return update::run();
    }
    if arg
        .as_ref()
        .is_some_and(|value| value.to_string_lossy().starts_with('-'))
    {
        return Err("Unknown option; use tuifinder --help".into());
    }
    let path = PathBuf::from(arg.unwrap_or_else(|| ".".into())).canonicalize()?;
    if !path.is_dir() {
        return Err("PATH must be a directory".into());
    }
    let root = git::repository_root(&path).unwrap_or(path);
    let generation = Arc::new(AtomicU64::new(0));
    let search_generation = Arc::new(AtomicU64::new(0));
    let (tx, rx) = worker::start(root.clone(), Arc::clone(&generation));
    let (search_tx, search_rx) = worker::start_search(root.clone(), Arc::clone(&search_generation));
    let dirty = Arc::new(AtomicBool::new(false));
    watch(root.clone(), Arc::clone(&dirty));
    let mut app = App {
        root,
        entries: vec![],
        tree_index: 0,
        tree_offset: 0,
        expanded: BTreeSet::new(),
        selected: None,
        document: None,
        diff: false,
        changes: false,
        ignored: false,
        focus: Focus::Tree,
        scroll: 0,
        horizontal: 0,
        branch: String::new(),
        git_available: false,
        changed_count: 0,
        status: String::new(),
        loading: false,
        search: None,
        help: false,
        tree_area: Rect::default(),
        code_area: Rect::default(),
        search_area: Rect::default(),
        revision: 0,
        generation,
        search_generation,
        jump: None,
        pending_measurement: Some(("load", Instant::now())),
        measurement: None,
    };
    app.request(&tx);
    let mut terminal = ratatui::init();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        ratatui::restore();
        previous(info);
    }));
    let result = (|| {
        execute!(io::stdout(), EnableMouseCapture)?;
        run(
            &mut terminal,
            &mut app,
            &tx,
            &rx,
            &search_tx,
            &search_rx,
            &dirty,
        )
    })();
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result?;
    Ok(())
}

fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    tx: &worker::Mailbox<Request>,
    rx: &std::sync::mpsc::Receiver<worker::Snapshot>,
    search_tx: &worker::Mailbox<SearchRequest>,
    search_rx: &std::sync::mpsc::Receiver<worker::SearchReply>,
    dirty: &AtomicBool,
) -> io::Result<()> {
    let mut redraw = true;
    let mut refresh = Instant::now();
    loop {
        while let Ok(snapshot) = rx.try_recv() {
            if snapshot.generation != app.generation.load(Ordering::Relaxed) {
                continue;
            }
            let old = app.entries.get(app.tree_index).map(|e| e.path.clone());
            app.entries = snapshot.entries;
            app.tree_index = old
                .and_then(|p| app.entries.iter().position(|e| e.path == p))
                .unwrap_or(app.tree_index)
                .min(app.entries.len().saturating_sub(1));
            let tree_failed = snapshot.error.is_some();
            app.git_available = snapshot.git.available;
            app.branch = snapshot.git.branch;
            app.changed_count = snapshot.git.entries.len();
            app.status = snapshot
                .error
                .or(snapshot.git.error)
                .unwrap_or_else(|| "Synced".into());
            app.document = snapshot.document;
            if let Some(line) = app.jump.take() {
                app.scroll = line;
            }
            if let Some(doc) = &app.document {
                app.scroll = app.scroll.min(doc.lines.len().saturating_sub(1));
            }
            app.loading = false;
            if let Some((label, started)) = app.pending_measurement.take() {
                app.measurement = Some(chrome::Measurement {
                    label,
                    elapsed: started.elapsed(),
                    failed: match label {
                        "tree" => tree_failed,
                        "load" | "refresh" => {
                            app.status != "Synced"
                                || app.document.as_ref().is_some_and(|d| d.error.is_some())
                        }
                        _ => app.document.as_ref().is_some_and(|d| d.error.is_some()),
                    },
                });
            }
            if app
                .search
                .as_ref()
                .is_some_and(|s| s.kind == Kind::Current && s.pending)
            {
                app.submit_search(search_tx);
            }
            redraw = true;
        }
        while let Ok(reply) = search_rx.try_recv() {
            if reply.generation != app.search_generation.load(Ordering::Relaxed) {
                continue;
            }
            if let Some(search) = &mut app.search {
                search.hits = reply.results.hits;
                search.pending = false;
                if let Some(started) = search.started.take() {
                    app.measurement = Some(chrome::Measurement {
                        label: if search.kind == Kind::Files {
                            "files"
                        } else {
                            "grep"
                        },
                        elapsed: started.elapsed(),
                        failed: false,
                    });
                }
                search.note = format!(
                    "{} matches{}{}",
                    search.hits.len(),
                    if reply.results.truncated {
                        " (limit reached)"
                    } else {
                        ""
                    },
                    if reply.results.errors > 0 {
                        " · some paths unreadable"
                    } else {
                        ""
                    }
                );
                redraw = true;
            }
        }
        if !app.loading
            && ((dirty.load(Ordering::Relaxed) && refresh.elapsed() >= Duration::from_millis(250))
                || refresh.elapsed() >= Duration::from_secs(10))
        {
            dirty.store(false, Ordering::Relaxed);
            app.revision += 1;
            app.request(tx);
            refresh = Instant::now();
            redraw = true;
        }
        if redraw {
            terminal.draw(|frame| draw(frame, app))?;
            redraw = false;
        }
        let waiting_for_result = app.loading || app.search.as_ref().is_some_and(|s| s.pending);
        let poll_ms = if waiting_for_result { 10 } else { 100 };
        if !event::poll(Duration::from_millis(poll_ms))? {
            continue;
        }
        let event = event::read()?;
        redraw = true;
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    break;
                }
                if app.help {
                    app.help = false;
                    continue;
                }
                if app.search.is_some() {
                    if key.code == KeyCode::Esc {
                        app.search = None;
                        app.search_generation.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    let s = app.search.as_mut().unwrap();
                    match key.code {
                        KeyCode::Enter if s.editing => app.submit_search(search_tx),
                        KeyCode::Enter => {
                            let selection = s.hits.get(s.selected).cloned();
                            let current = s.kind == Kind::Current;
                            if let Some(hit) = selection {
                                app.search = None;
                                if current {
                                    app.scroll = hit.line;
                                    app.focus = Focus::Code;
                                } else {
                                    app.diff = false;
                                    app.open(hit.path, hit.line, tx);
                                }
                            }
                        }
                        KeyCode::Down => {
                            s.selected = (s.selected + 1).min(s.hits.len().saturating_sub(1));
                        }
                        KeyCode::Up => {
                            s.selected = s.selected.saturating_sub(1);
                        }
                        KeyCode::Backspace => {
                            s.query.pop();
                            s.editing = true;
                            s.pending = false;
                            s.started = None;
                            app.search_generation.fetch_add(1, Ordering::Relaxed);
                        }
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            s.query.push(c);
                            s.editing = true;
                            s.pending = false;
                            s.started = None;
                            app.search_generation.fetch_add(1, Ordering::Relaxed);
                        }
                        _ => {}
                    }
                    continue;
                }
                if matches!(key.code, KeyCode::Char('d' | 'g')) && !app.git_available {
                    app.status =
                        "Git view unavailable — open a Git repository to use diff/changes".into();
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('?') => app.help = true,
                    KeyCode::Tab => {
                        app.focus = if app.focus == Focus::Tree {
                            Focus::Code
                        } else {
                            Focus::Tree
                        }
                    }
                    KeyCode::Char('d') => {
                        app.diff = !app.diff;
                        if app.selected.is_some() {
                            app.pending_measurement =
                                Some((if app.diff { "diff" } else { "code" }, Instant::now()));
                        }
                        app.document = None;
                        app.scroll = 0;
                        app.request(tx);
                    }
                    KeyCode::Char('g') => {
                        app.pending_measurement = Some(("tree", Instant::now()));
                        app.changes = !app.changes;
                        app.tree_index = 0;
                        app.tree_offset = 0;
                        app.request(tx);
                    }
                    KeyCode::Char('i') => {
                        app.pending_measurement = Some(("tree", Instant::now()));
                        app.ignored = !app.ignored;
                        app.request(tx);
                    }
                    KeyCode::Char('r') => {
                        app.pending_measurement = Some(("refresh", Instant::now()));
                        app.revision += 1;
                        app.request(tx);
                    }
                    KeyCode::Char('p') => app.start_search(Kind::Files),
                    KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.start_search(Kind::Content)
                    }
                    KeyCode::Char('/') => app.start_search(Kind::Current),
                    KeyCode::Enter => {
                        if app.focus == Focus::Tree {
                            app.activate(tx);
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => move_vertical(app, 1),
                    KeyCode::Up | KeyCode::Char('k') => move_vertical(app, -1),
                    KeyCode::PageDown => {
                        move_vertical(app, app.code_area.height.max(1) as isize - 1)
                    }
                    KeyCode::PageUp => {
                        move_vertical(app, -(app.code_area.height.max(1) as isize - 1))
                    }
                    KeyCode::Home => {
                        if app.focus == Focus::Tree {
                            app.tree_index = 0;
                        } else {
                            app.scroll = 0;
                        }
                    }
                    KeyCode::End => {
                        if app.focus == Focus::Tree {
                            app.tree_index = app.entries.len().saturating_sub(1);
                        } else {
                            app.scroll = app
                                .document
                                .as_ref()
                                .map_or(0, |d| d.lines.len().saturating_sub(1));
                        }
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        if app.focus == Focus::Tree {
                            app.activate(tx);
                        } else {
                            app.horizontal = app.horizontal.saturating_add(4);
                        }
                    }
                    KeyCode::Left | KeyCode::Char('h') => {
                        if app.focus == Focus::Code {
                            app.horizontal = app.horizontal.saturating_sub(4);
                        } else if let Some(e) = app.entries.get(app.tree_index) {
                            if app.expanded.remove(&e.path) {
                                app.pending_measurement = Some(("tree", Instant::now()));
                                app.request(tx);
                            } else if let Some(parent) = e.path.parent()
                                && let Some(i) = app.entries.iter().position(|e| e.path == parent)
                            {
                                app.tree_index = i;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::Mouse(mouse) if app.search.is_none() && !app.help => {
                let position = (mouse.column, mouse.row).into();
                match mouse.kind {
                    MouseEventKind::Down(event::MouseButton::Left)
                        if app.tree_area.contains(position) =>
                    {
                        app.focus = Focus::Tree;
                        app.tree_index = (app.tree_offset + (mouse.row - app.tree_area.y) as usize)
                            .min(app.entries.len().saturating_sub(1));
                        app.activate(tx);
                    }
                    MouseEventKind::Down(event::MouseButton::Left)
                        if app.code_area.contains(position) =>
                    {
                        app.focus = Focus::Code
                    }
                    MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                        app.focus = if app.tree_area.contains(position) {
                            Focus::Tree
                        } else {
                            Focus::Code
                        };
                        move_vertical(
                            app,
                            if mouse.kind == MouseEventKind::ScrollDown {
                                3
                            } else {
                                -3
                            },
                        );
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn move_vertical(app: &mut App, amount: isize) {
    if app.focus == Focus::Tree {
        app.tree_index = app
            .tree_index
            .saturating_add_signed(amount)
            .min(app.entries.len().saturating_sub(1));
    } else {
        app.scroll = app.scroll.saturating_add_signed(amount).min(
            app.document
                .as_ref()
                .map_or(0, |d| d.lines.len().saturating_sub(1)),
        );
    }
}
fn selected_style() -> Style {
    Style::default()
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD)
}
fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 35 || area.height < 8 {
        frame.render_widget(Paragraph::new("Pane too small (minimum 35 × 8)"), area);
        return;
    }
    let footer_height = if area.height >= 12 { 2 } else { 1 };
    let has_error = !app.status.is_empty() && app.status != "Synced";
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::from(has_error)),
        Constraint::Min(1),
        Constraint::Length(footer_height),
    ])
    .split(area);
    let git_label = if app.git_available {
        format!("{} · {} changed", app.branch, app.changed_count)
    } else if app.status.is_empty() {
        "Loading".into()
    } else {
        "No Git".into()
    };
    let header = format!(
        " tuifinder v{}  {}  | {}{}",
        env!("CARGO_PKG_VERSION"),
        clean_label(
            &app.root
                .file_name()
                .unwrap_or(app.root.as_os_str())
                .to_string_lossy()
        ),
        git_label,
        if app.loading { " · loading" } else { "" }
    );
    let metric = app
        .measurement
        .as_ref()
        .map(chrome::Measurement::text)
        .or_else(|| {
            app.pending_measurement
                .map(|(label, _)| format!("{label} …"))
        })
        .unwrap_or_else(|| "ready —".into());
    let metric_width = if metric.is_empty() {
        0
    } else {
        Line::raw(&metric).width() as u16 + 1
    };
    let header_areas =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(metric_width)]).split(rows[0]);
    let header = if Line::raw(&header).width() > header_areas[0].width as usize {
        format!(" tuifinder v{}", env!("CARGO_PKG_VERSION"))
    } else {
        header
    };
    frame.render_widget(
        Paragraph::new(header).style(Style::default().fg(Color::Green)),
        header_areas[0],
    );
    frame.render_widget(
        Paragraph::new(metric)
            .alignment(Alignment::Right)
            .style(Style::default().fg(Color::Gray)),
        header_areas[1],
    );
    if has_error {
        frame.render_widget(
            Paragraph::new(clean_label(&app.status)).style(Style::default().fg(Color::Yellow)),
            rows[1],
        );
    }
    let tree_width = (area.width / 4).clamp(14, 32);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(tree_width), Constraint::Min(1)])
        .split(rows[2]);
    let tree_title = format!(
        " {}{} ",
        if app.changes { "Changes" } else { "Files" },
        if app.ignored { " +ignored" } else { "" }
    );
    let tree_block = Block::default()
        .borders(Borders::ALL)
        .title(tree_title)
        .border_style(Style::default().fg(if app.focus == Focus::Tree {
            Color::Green
        } else {
            Color::DarkGray
        }));
    app.tree_area = tree_block.inner(columns[0]);
    frame.render_widget(tree_block, columns[0]);
    let visible = app.tree_area.height as usize;
    if app.tree_index < app.tree_offset {
        app.tree_offset = app.tree_index;
    }
    if app.tree_index >= app.tree_offset + visible {
        app.tree_offset = app.tree_index.saturating_sub(visible.saturating_sub(1));
    }
    let items: Vec<_> = app
        .entries
        .iter()
        .enumerate()
        .skip(app.tree_offset)
        .take(visible)
        .map(|(index, e)| {
            let symbol = if e.directory {
                if app.changes || app.expanded.contains(&e.path) {
                    "▾"
                } else {
                    "▸"
                }
            } else {
                " "
            };
            let name = clean_label(&e.path.file_name().unwrap_or_default().to_string_lossy());
            let status = if e.status.is_empty() { "  " } else { &e.status };
            let line = Line::from(vec![
                Span::raw(format!("{}{} ", " ".repeat(e.depth * 2), symbol)),
                Span::styled(format!("{status} "), Style::default().fg(Color::Yellow)),
                Span::raw(name),
            ]);
            ListItem::new(line).style(if index == app.tree_index {
                selected_style()
            } else {
                Style::default()
            })
        })
        .collect();
    frame.render_widget(List::new(items), app.tree_area);
    let title = format!(
        " {} · {} ",
        app.selected
            .as_ref()
            .map(|p| clean_label(&p.display().to_string()))
            .unwrap_or_else(|| "Select a file".into()),
        if app.diff { "Diff: HEAD" } else { "Code" }
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(if app.focus == Focus::Code {
            Color::Green
        } else {
            Color::DarkGray
        }));
    app.code_area = block.inner(columns[1]);
    frame.render_widget(block, columns[1]);
    if let Some(doc) = &app.document {
        if let Some(error) = &doc.error {
            frame.render_widget(
                Paragraph::new(clean_label(error)).style(Style::default().fg(Color::Yellow)),
                app.code_area,
            );
        } else {
            let height = app.code_area.height as usize;
            let gutter = if app.diff {
                0
            } else {
                (doc.lines.len().max(1).ilog10() + 3) as u16
            };
            let parts = Layout::horizontal([Constraint::Length(gutter), Constraint::Min(1)])
                .split(app.code_area);
            if !app.diff {
                let numbers: Vec<_> = (app.scroll..(app.scroll + height).min(doc.lines.len()))
                    .map(|i| {
                        Line::styled(
                            format!(
                                "{:>width$} ",
                                i + 1,
                                width = gutter.saturating_sub(1) as usize
                            ),
                            Style::default().fg(Color::DarkGray),
                        )
                    })
                    .collect();
                frame.render_widget(Paragraph::new(numbers), parts[0]);
            }
            let lines: Vec<_> = doc
                .lines
                .iter()
                .skip(app.scroll)
                .take(height)
                .cloned()
                .collect();
            frame.render_widget(Paragraph::new(lines).scroll((0, app.horizontal)), parts[1]);
        }
    } else {
        frame.render_widget(
            Paragraph::new(if app.loading {
                "Loading…"
            } else {
                "Click a file or press Enter\np Find file  ·  Ctrl-f Search repository"
            }),
            app.code_area,
        );
    }
    let footer_mode = if app.help {
        chrome::FooterMode::Help
    } else if let Some(search) = &app.search {
        chrome::FooterMode::Search {
            editing: search.editing,
        }
    } else {
        chrome::FooterMode::Browse
    };
    frame.render_widget(
        Paragraph::new(chrome::footer(area.width, footer_height, footer_mode)),
        rows[3],
    );
    if app.help {
        let popup = center(rows[2], 80, 21);
        frame.render_widget(Clear, popup);
        frame.render_widget(Paragraph::new("Enter / click     Open file or expand folder\nTab               Switch tree / code focus\n↑↓ / j k / wheel  Navigate or scroll\n←→ / h l          Collapse / expand, horizontal scroll\nHome End PgUp/Dn  Jump / page\nd                 Code ↔ HEAD diff\ng                 All files ↔ changed files\ni                 Show ignored files\np / Ctrl-p        Find filename (case-insensitive)\n/                 Search current code / diff\nCtrl-f            Search repository (literal, case-sensitive)\nr                 Refresh now\nEsc               Close search\nq / Ctrl-c        Quit\n\nSearch: type → Enter → ↑↓ → Enter. Any key closes help.\nTop-right: last action to UI-ready time; excludes SSH/display.").block(Block::bordered().title(" Help · read only ")),popup);
    }
    if let Some(search) = &mut app.search {
        let popup = center(rows[2], 90, 18);
        frame.render_widget(Clear, popup);
        let title = match search.kind {
            Kind::Files => "Find file",
            Kind::Content => "Search repository",
            Kind::Current => "Search current view",
        };
        let block = Block::bordered().title(format!(" {title} · Esc close "));
        let inside = block.inner(popup);
        frame.render_widget(block, popup);
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inside);
        frame.render_widget(
            Paragraph::new(format!(
                "> {}{}",
                search.query,
                if search.editing { "▌" } else { "" }
            )),
            rows[0],
        );
        app.search_area = rows[1];
        let visible = rows[1].height as usize;
        if search.selected < search.offset {
            search.offset = search.selected;
        }
        if search.selected >= search.offset + visible {
            search.offset = search.selected.saturating_sub(visible.saturating_sub(1));
        }
        let items: Vec<_> = search
            .hits
            .iter()
            .enumerate()
            .skip(search.offset)
            .take(visible)
            .map(|(i, h)| {
                let suffix = if search.kind == Kind::Files {
                    String::new()
                } else {
                    format!(":{}  {}", h.line + 1, h.preview)
                };
                ListItem::new(format!(
                    "{}{}",
                    clean_label(&h.path.display().to_string()),
                    suffix
                ))
                .style(if i == search.selected {
                    selected_style()
                } else {
                    Style::default()
                })
            })
            .collect();
        frame.render_widget(List::new(items), rows[1]);
        frame.render_widget(
            Paragraph::new(if search.pending {
                "Searching…"
            } else if search.editing {
                "Enter to search"
            } else if search.hits.is_empty() {
                "No matches"
            } else if search.note.is_empty() {
                "↑↓ select · Enter open"
            } else {
                &search.note
            })
            .style(Style::default().fg(Color::Gray)),
            rows[2],
        );
    }
}
fn center(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}
