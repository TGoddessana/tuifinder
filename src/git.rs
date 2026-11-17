use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Default)]
pub struct GitState {
    pub available: bool,
    pub branch: String,
    pub entries: BTreeMap<PathBuf, String>,
    pub error: Option<String>,
}

fn command(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C");
    cmd
}

pub fn repository_root(path: &Path) -> Option<PathBuf> {
    let out = command(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let bytes = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
    Some(path_from_bytes(bytes))
}

fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
        PathBuf::from(OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

pub fn parse_status(bytes: &[u8]) -> BTreeMap<PathBuf, String> {
    let mut entries = BTreeMap::new();
    let mut records = bytes.split(|b| *b == 0);
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let status = String::from_utf8_lossy(&record[..2]).into_owned();
        entries.insert(path_from_bytes(&record[3..]), status.clone());
        if status.contains('R') || status.contains('C') {
            records.next();
        }
    }
    entries
}

pub fn status(root: &Path) -> GitState {
    let mut result = GitState::default();
    match command(root)
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ])
        .output()
    {
        Ok(out) if out.status.success() => {
            result.available = true;
            result.entries = parse_status(&out.stdout);
        }
        Ok(out) => {
            let error = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            if error.starts_with("fatal: not a git repository") {
                result.branch = "No Git".into();
            } else {
                result.error = Some(error);
            }
        }
        Err(e) => result.error = Some(format!("git: {e}")),
    }
    if result.available
        && let Ok(out) = command(root)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
        && out.status.success()
    {
        result.branch = String::from_utf8_lossy(&out.stdout).trim().into();
    }
    result
}

pub fn diff(root: &Path, path: &Path) -> Result<String, String> {
    let has_head = command(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    // Git's empty tree also covers repositories before their first commit.
    let base = if has_head {
        "HEAD"
    } else {
        "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
    };
    let output = command(root)
        .args([
            "--literal-pathspecs",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-renames",
            base,
            "--",
        ])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_renames_and_unusual_names() {
        let entries = parse_status(b" M src/a.rs\0R  new name\0old name\0?? line\nfile\0");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[Path::new("new name")], "R ");
        assert!(entries.contains_key(Path::new("line\nfile")));
        assert!(!entries.contains_key(Path::new("old name")));
    }
}
