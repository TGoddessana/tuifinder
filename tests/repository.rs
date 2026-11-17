use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tuifinder::{files, git, worker};

fn git_cmd(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git_cmd(dir.path(), &["init", "-q"]);
    git_cmd(dir.path(), &["config", "user.name", "Test"]);
    git_cmd(
        dir.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    dir
}
#[test]
fn head_diff_combines_staged_and_unstaged_and_handles_deletion() {
    let dir = repo();
    let root = dir.path();
    fs::write(root.join("a.rs"), "original\n").unwrap();
    git_cmd(root, &["add", "."]);
    git_cmd(root, &["commit", "-qm", "initial"]);
    fs::write(root.join("a.rs"), "staged\n").unwrap();
    git_cmd(root, &["add", "."]);
    fs::write(root.join("a.rs"), "working\n").unwrap();
    let state = git::status(root);
    assert_eq!(state.entries[Path::new("a.rs")], "MM");
    let diff = git::diff(root, Path::new("a.rs")).unwrap();
    assert!(diff.contains("-original"));
    assert!(diff.contains("+working"));
    assert!(!diff.contains("+staged"));
    fs::remove_file(root.join("a.rs")).unwrap();
    assert!(
        git::diff(root, Path::new("a.rs"))
            .unwrap()
            .contains("-original")
    );
}
#[test]
fn diff_supports_unborn_repositories_and_literal_pathspecs() {
    let dir = repo();
    let root = dir.path();
    fs::write(root.join("[a].rs"), "first\n").unwrap();
    fs::write(root.join("a.rs"), "other\n").unwrap();
    git_cmd(root, &["add", "."]);
    let diff = git::diff(root, Path::new("[a].rs")).unwrap();
    assert!(diff.contains("+first"));
    assert!(!diff.contains("+other"));
}
#[test]
fn worker_reloads_content_and_reuses_unchanged_highlighting() {
    let dir = repo();
    let root = dir.path().canonicalize().unwrap();
    fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
    let generation = Arc::new(AtomicU64::new(1));
    let (tx, rx) = worker::start(root, Arc::clone(&generation));
    let mut request = worker::Request {
        generation: 1,
        revision: 0,
        expanded: BTreeSet::new(),
        selected: Some(PathBuf::from("main.rs")),
        diff: false,
        changed_only: false,
        show_ignored: false,
    };
    tx.send(request.clone());
    let first = rx
        .recv_timeout(Duration::from_secs(15))
        .unwrap()
        .document
        .unwrap();
    generation.store(2, Ordering::Relaxed);
    request.generation = 2;
    tx.send(request.clone());
    let second = rx
        .recv_timeout(Duration::from_secs(15))
        .unwrap()
        .document
        .unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    fs::write(dir.path().join("main.rs"), "fn changed() {}\n").unwrap();
    generation.store(3, Ordering::Relaxed);
    request.generation = 3;
    request.revision += 1;
    tx.send(request);
    let third = rx
        .recv_timeout(Duration::from_secs(15))
        .unwrap()
        .document
        .unwrap();
    assert_eq!(third.plain[0], "fn changed() {}");
}
#[cfg(unix)]
#[test]
fn preview_does_not_follow_symlinks_outside_workspace() {
    use std::os::unix::fs::symlink;
    let dir = repo();
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join("secret"), "private").unwrap();
    symlink(other.path().join("secret"), dir.path().join("link")).unwrap();
    assert!(
        files::text(&dir.path().canonicalize().unwrap(), Path::new("link"))
            .unwrap_err()
            .contains("outside")
    );
}

#[cfg(unix)]
#[test]
fn unreadable_expanded_directory_does_not_hide_other_files() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let restricted = temp.path().join("restricted");
    fs::create_dir(&restricted).unwrap();
    fs::write(temp.path().join("visible.rs"), "fn main() {}\n").unwrap();
    fs::set_permissions(&restricted, fs::Permissions::from_mode(0o000)).unwrap();
    let denied = fs::read_dir(&restricted).is_err();
    let result = files::tree(
        temp.path(),
        &BTreeSet::from([PathBuf::from("restricted")]),
        &Default::default(),
        false,
        false,
    );
    fs::set_permissions(&restricted, fs::Permissions::from_mode(0o700)).unwrap();
    let entries = result.unwrap();
    assert!(entries.iter().any(|e| e.path == Path::new("visible.rs")));
    if denied {
        assert_eq!(
            entries
                .iter()
                .find(|e| e.path == Path::new("restricted"))
                .unwrap()
                .status,
            " !"
        );
    }
}

#[test]
fn ordinary_folder_is_browsable_without_a_git_error() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("main.rs"), "fn plain_folder() {}\n").unwrap();
    let state = git::status(&root);
    assert!(!state.available);
    assert!(state.error.is_none());
    assert_eq!(state.branch, "No Git");
    let generation = Arc::new(AtomicU64::new(1));
    let (tx, rx) = worker::start(root, generation);
    tx.send(worker::Request {
        generation: 1,
        revision: 0,
        expanded: BTreeSet::new(),
        selected: Some(PathBuf::from("main.rs")),
        diff: false,
        changed_only: false,
        show_ignored: false,
    });
    let snapshot = rx.recv_timeout(Duration::from_secs(15)).unwrap();
    assert!(snapshot.error.is_none());
    assert!(snapshot.git.error.is_none());
    let doc = snapshot.document.unwrap();
    assert!(doc.error.is_none());
    assert_eq!(doc.plain[0], "fn plain_folder() {}");
}
