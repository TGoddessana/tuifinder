use ignore::WalkBuilder;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub depth: usize,
    pub directory: bool,
    pub status: String,
}

pub fn clean_label(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
}

pub fn text(root: &Path, path: &Path) -> Result<String, String> {
    let target = root.join(path);
    let canonical = target.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(root) {
        return Err("Symlink points outside the workspace".into());
    }
    if !canonical.is_file() {
        return Err("Not a regular file".into());
    }
    // shortcut: whole-file previews; add paged reads when large-file use is required.
    let bytes = fs::read(canonical).map_err(|e| e.to_string())?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err("Binary file — preview unavailable".into());
    }
    String::from_utf8(bytes).map_err(|_| "Non-UTF-8 file — preview unavailable".into())
}

pub fn tree(
    root: &Path,
    expanded: &BTreeSet<PathBuf>,
    changes: &BTreeMap<PathBuf, String>,
    changed_only: bool,
    show_ignored: bool,
) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    if changed_only {
        let mut paths = BTreeSet::new();
        for path in changes.keys() {
            paths.insert(path.clone());
            let mut parent = path.parent();
            while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
                paths.insert(p.to_path_buf());
                parent = p.parent();
            }
        }
        append_changed(Path::new(""), 0, &paths, changes, &mut entries);
    } else {
        append_dir(
            root,
            Path::new(""),
            0,
            expanded,
            changes,
            show_ignored,
            &mut entries,
        )?;
    }
    Ok(entries)
}

fn append_changed(
    parent: &Path,
    depth: usize,
    paths: &BTreeSet<PathBuf>,
    changes: &BTreeMap<PathBuf, String>,
    entries: &mut Vec<Entry>,
) {
    let mut children: Vec<_> = paths
        .iter()
        .filter(|p| p.parent() == Some(parent))
        .collect();
    children.sort_by_key(|p| (changes.contains_key(*p), (*p).clone()));
    for path in children {
        let directory = !changes.contains_key(path);
        entries.push(Entry {
            path: path.clone(),
            depth,
            directory,
            status: changes.get(path).cloned().unwrap_or_default(),
        });
        if directory {
            append_changed(path, depth + 1, paths, changes, entries);
        }
    }
}

fn append_dir(
    root: &Path,
    relative: &Path,
    depth: usize,
    expanded: &BTreeSet<PathBuf>,
    changes: &BTreeMap<PathBuf, String>,
    show_ignored: bool,
    entries: &mut Vec<Entry>,
) -> Result<(), String> {
    let mut builder = WalkBuilder::new(root.join(relative));
    builder.max_depth(Some(1)).hidden(false).follow_links(false);
    if show_ignored {
        builder.standard_filters(false);
    }
    let mut children = Vec::new();
    for child in builder.build().skip(1) {
        let child = child.map_err(|e| e.to_string())?;
        if child.file_name() == ".git" {
            continue;
        }
        let directory = child.file_type().is_some_and(|t| t.is_dir());
        if let Ok(path) = child.path().strip_prefix(root) {
            children.push((path.to_path_buf(), directory));
        }
    }
    children.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for (path, directory) in children {
        let status = changes.get(&path).cloned().unwrap_or_else(|| {
            if directory && changes.keys().any(|p| p.starts_with(&path)) {
                " •".into()
            } else {
                String::new()
            }
        });
        entries.push(Entry {
            path: path.clone(),
            depth,
            directory,
            status,
        });
        if directory && expanded.contains(&path) {
            let folder_index = entries.len() - 1;
            if append_dir(
                root,
                &path,
                depth + 1,
                expanded,
                changes,
                show_ignored,
                entries,
            )
            .is_err()
            {
                entries.truncate(folder_index + 1);
                entries[folder_index].status = " !".into();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tree_is_lazy_and_includes_deleted_changes() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/a.rs"), "hello").unwrap();
        let mut changes = BTreeMap::new();
        changes.insert(PathBuf::from("src/deleted.rs"), " D".into());
        let empty = BTreeSet::new();
        assert_eq!(
            tree(temp.path(), &empty, &changes, false, false)
                .unwrap()
                .len(),
            1
        );
        let result = tree(temp.path(), &empty, &changes, true, false).unwrap();
        assert_eq!(result[1].path, Path::new("src/deleted.rs"));
    }
    #[test]
    fn rejects_binary_and_escapes_controls() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("binary"), b"a\0b").unwrap();
        assert!(text(temp.path(), Path::new("binary")).is_err());
        assert_eq!(clean_label("a\x1b[31m"), "a�[31m");
    }
}
