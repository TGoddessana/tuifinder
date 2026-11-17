use crate::{
    files::{self, Entry},
    git::{self, GitState},
    search::{self, Kind, Results},
};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
};
use syntect::{
    easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet, util::LinesWithEndings,
};

// A single pending request prevents rapid navigation from growing a work queue.
pub struct Mailbox<T> {
    latest: Arc<Mutex<Option<T>>>,
    signal: SyncSender<()>,
}
impl<T> Mailbox<T> {
    pub fn send(&self, request: T) {
        *self.latest.lock().unwrap() = Some(request);
        let _ = self.signal.try_send(());
    }
}
fn mailbox<T>() -> (Mailbox<T>, Receiver<()>) {
    let (signal, rx) = mpsc::sync_channel(1);
    (
        Mailbox {
            latest: Arc::new(Mutex::new(None)),
            signal,
        },
        rx,
    )
}

#[derive(Clone)]
pub struct Request {
    pub generation: u64,
    pub revision: u64,
    pub expanded: BTreeSet<PathBuf>,
    pub selected: Option<PathBuf>,
    pub diff: bool,
    pub changed_only: bool,
    pub show_ignored: bool,
}
pub struct Document {
    pub path: PathBuf,
    pub diff: bool,
    pub lines: Vec<Line<'static>>,
    pub plain: Vec<String>,
    pub error: Option<String>,
}
pub struct Snapshot {
    pub generation: u64,
    pub entries: Vec<Entry>,
    pub git: GitState,
    pub document: Option<Arc<Document>>,
    pub error: Option<String>,
}
pub struct SearchRequest {
    pub generation: u64,
    pub query: String,
    pub kind: Kind,
    pub show_ignored: bool,
}
pub struct SearchReply {
    pub generation: u64,
    pub results: Results,
}

pub fn start(root: PathBuf, current: Arc<AtomicU64>) -> (Mailbox<Request>, Receiver<Snapshot>) {
    let (tx, rx) = mailbox::<Request>();
    let latest = Arc::clone(&tx.latest);
    let (out, results) = mpsc::channel();
    thread::spawn(move || {
        let syntaxes = SyntaxSet::load_defaults_newlines();
        let themes = ThemeSet::load_defaults();
        let theme = &themes.themes["base16-ocean.dark"];
        let mut cached: Option<(u64, String, Arc<Document>)> = None;
        let mut git_cache: Option<(u64, GitState)> = None;
        while rx.recv().is_ok() {
            let Some(req) = latest.lock().unwrap().take() else {
                continue;
            };
            if current.load(Ordering::Relaxed) != req.generation {
                continue;
            }
            if git_cache
                .as_ref()
                .is_none_or(|(revision, _)| *revision != req.revision)
            {
                git_cache = Some((req.revision, git::status(&root)));
            }
            let git = &git_cache.as_ref().unwrap().1;
            let (entries, error) = match files::tree(
                &root,
                &req.expanded,
                &git.entries,
                req.changed_only,
                req.show_ignored,
            ) {
                Ok(e) => (e, None),
                Err(e) => (Vec::new(), Some(e)),
            };
            let document = req.selected.as_ref().map(|path| {
                if let Some((revision, _, doc)) = &cached
                    && *revision == req.revision
                    && doc.path == *path
                    && doc.diff == req.diff
                {
                    return Arc::clone(doc);
                }
                let raw = if req.diff && !git.available {
                    Err(git.error.clone().unwrap_or_else(|| {
                        "Git diff is unavailable outside a Git repository".into()
                    }))
                } else if req.diff && git.entries.get(path).is_some_and(|s| s == "??") {
                    files::text(&root, path).map(|s| {
                        format!(
                            "New untracked file\n{}",
                            s.lines().map(|l| format!("+{l}\n")).collect::<String>()
                        )
                    })
                } else if req.diff {
                    git::diff(&root, path)
                } else {
                    files::text(&root, path)
                };
                match raw {
                    Err(e) => Arc::new(Document {
                        path: path.clone(),
                        diff: req.diff,
                        lines: vec![],
                        plain: vec![],
                        error: Some(e),
                    }),
                    Ok(raw) => {
                        if let Some((revision, text, doc)) = &mut cached
                            && doc.path == *path
                            && doc.diff == req.diff
                            && text == &raw
                        {
                            *revision = req.revision;
                            return Arc::clone(doc);
                        }
                        let syntax = syntaxes
                            .find_syntax_for_file(root.join(path))
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
                        let mut highlighter = HighlightLines::new(syntax, theme);
                        let mut lines = Vec::new();
                        let mut plain = Vec::new();
                        for line in LinesWithEndings::from(&raw) {
                            if current.load(Ordering::Relaxed) != req.generation {
                                break;
                            }
                            let clean = line.trim_end_matches(['\r', '\n']).replace('\t', "    ");
                            let clean = files::clean_label(&clean);
                            plain.push(clean.clone());
                            if req.diff {
                                let color = if clean.starts_with('+') {
                                    Color::Green
                                } else if clean.starts_with('-') {
                                    Color::Red
                                } else if clean.starts_with("@@") {
                                    Color::Cyan
                                } else {
                                    Color::Reset
                                };
                                lines.push(Line::styled(clean, Style::default().fg(color)));
                            } else {
                                match highlighter.highlight_line(line, &syntaxes) {
                                    Ok(ranges) => lines.push(Line::from(
                                        ranges
                                            .into_iter()
                                            .map(|(style, text)| {
                                                let text = files::clean_label(
                                                    &text
                                                        .trim_end_matches(['\r', '\n'])
                                                        .replace('\t', "    "),
                                                );
                                                Span::styled(
                                                    text,
                                                    Style::default().fg(Color::Rgb(
                                                        style.foreground.r,
                                                        style.foreground.g,
                                                        style.foreground.b,
                                                    )),
                                                )
                                            })
                                            .collect::<Vec<_>>(),
                                    )),
                                    Err(_) => lines.push(Line::raw(clean)),
                                }
                            }
                        }
                        if lines.is_empty() {
                            lines.push(Line::raw(if req.diff {
                                "No changes against HEAD"
                            } else {
                                ""
                            }));
                        }
                        let doc = Arc::new(Document {
                            path: path.clone(),
                            diff: req.diff,
                            lines,
                            plain,
                            error: None,
                        });
                        if current.load(Ordering::Relaxed) == req.generation {
                            cached = Some((req.revision, raw, Arc::clone(&doc)));
                        }
                        doc
                    }
                }
            });
            if current.load(Ordering::Relaxed) == req.generation
                && out
                    .send(Snapshot {
                        generation: req.generation,
                        entries,
                        git: git.clone(),
                        document,
                        error,
                    })
                    .is_err()
            {
                break;
            }
        }
    });
    (tx, results)
}

pub fn start_search(
    root: PathBuf,
    current: Arc<AtomicU64>,
) -> (Mailbox<SearchRequest>, Receiver<SearchReply>) {
    let (tx, rx) = mailbox::<SearchRequest>();
    let latest = Arc::clone(&tx.latest);
    let (out, results) = mpsc::channel();
    thread::spawn(move || {
        while rx.recv().is_ok() {
            let Some(req) = latest.lock().unwrap().take() else {
                continue;
            };
            let results = search::search(
                &root,
                &req.query,
                req.kind,
                req.show_ignored,
                req.generation,
                &current,
            );
            if current.load(Ordering::Relaxed) == req.generation
                && out
                    .send(SearchReply {
                        generation: req.generation,
                        results,
                    })
                    .is_err()
            {
                break;
            }
        }
    });
    (tx, results)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_work_keeps_only_the_latest_request() {
        let (mailbox, signal) = mailbox();
        mailbox.send(1);
        mailbox.send(2);
        assert_eq!(mailbox.latest.lock().unwrap().take(), Some(2));
        assert!(signal.try_recv().is_ok());
        assert!(signal.try_recv().is_err());
    }
}
