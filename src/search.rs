use crate::files::clean_label;
use ignore::WalkBuilder;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub const LIMIT: usize = 1000;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Files,
    Content,
    Current,
}
#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    pub line: usize,
    pub preview: String,
}
#[derive(Default, Debug)]
pub struct Results {
    pub hits: Vec<Hit>,
    pub truncated: bool,
    pub errors: usize,
}

pub fn search(
    root: &Path,
    query: &str,
    kind: Kind,
    show_ignored: bool,
    generation: u64,
    current: &AtomicU64,
) -> Results {
    let mut results = Results::default();
    if query.is_empty() {
        return results;
    }
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git");
    if show_ignored {
        builder.standard_filters(false);
    }
    let needle = query.to_lowercase();
    for entry in builder.build() {
        if current.load(Ordering::Relaxed) != generation {
            return results;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                results.errors += 1;
                continue;
            }
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(path) = entry.path().strip_prefix(root) else {
            continue;
        };
        if kind == Kind::Files {
            if path.to_string_lossy().to_lowercase().contains(&needle) {
                results.hits.push(Hit {
                    path: path.into(),
                    line: 0,
                    preview: String::new(),
                });
            }
        } else {
            let file = match File::open(entry.path()) {
                Ok(f) => f,
                Err(_) => {
                    results.errors += 1;
                    continue;
                }
            };
            let mut reader = BufReader::new(file);
            let mut bytes = Vec::new();
            let mut line = 0;
            loop {
                if current.load(Ordering::Relaxed) != generation {
                    return results;
                }
                bytes.clear();
                match reader.read_until(b'\n', &mut bytes) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => {
                        results.errors += 1;
                        break;
                    }
                }
                line += 1;
                if bytes.contains(&0) {
                    break;
                }
                let Ok(text) = std::str::from_utf8(&bytes) else {
                    break;
                };
                if text.contains(query) {
                    results.hits.push(Hit {
                        path: path.into(),
                        line: line - 1,
                        preview: clean_label(&text.trim().chars().take(160).collect::<String>()),
                    });
                    if results.hits.len() >= LIMIT {
                        break;
                    }
                }
            }
        }
        if results.hits.len() >= LIMIT {
            results.truncated = true;
            break;
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn respects_ignore_literal_queries_and_cancellation() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        std::fs::write(temp.path().join(".gitignore"), "skip.rs\n").unwrap();
        std::fs::write(temp.path().join("skip.rs"), "a.b").unwrap();
        std::fs::write(temp.path().join("keep.rs"), "axb\na.b\n").unwrap();
        let generation = AtomicU64::new(1);
        let found = search(temp.path(), "a.b", Kind::Content, false, 1, &generation);
        assert_eq!(found.hits.len(), 1);
        assert_eq!(found.hits[0].line, 1);
        assert!(
            search(temp.path(), "a", Kind::Content, false, 2, &generation)
                .hits
                .is_empty()
        );
    }
}
