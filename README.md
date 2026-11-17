# tuifinder

A read-only terminal code explorer for SSH and tmux. Browse files, review Git changes, and watch your coding agent's edits as they happen.

## Features

- File tree with mouse and keyboard navigation
- Syntax-highlighted previews and HEAD-based Git diffs
- Live updates that preserve your reading position
- Filename, in-file, and repository-wide search
- Context-aware shortcuts and per-action timing

Targets macOS and Linux on x86_64 and ARM64. Git is required for Git features; `bat`, `rg`, and tmux are optional.

## Quick Start

Build from source with Rust stable:

```sh
cargo build --release --locked
./target/release/tuifinder /path/to/project
```

Omit the path to open the current directory. Paths inside a Git repository open its root. Ordinary folders display `No Git` and still support browsing and search.

In tmux, split with `Ctrl-b` then `%` and run tuifinder in that pane. It does not start or manage tmux. Use `Ctrl-b` then `z` to zoom the pane.

## Installation

Install a binary from [GitHub Releases](https://github.com/TGoddessana/tuifinder/releases):

```sh
curl -fsSL https://raw.githubusercontent.com/TGoddessana/tuifinder/main/install.sh | sh
```

Installs to `~/.local/bin` without sudo. Requires `curl` and either `sha256sum` or `shasum`. Add the directory to your shell's PATH if needed:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

For a specific version or installation directory:

```sh
sh install.sh --version v0.1.0 --dir "$HOME/.local/bin"
```

To install directly from source:

```sh
cargo install --path . --locked
```

## Updates

```sh
tuifinder --version  # Also shown in the header; -V works too
tuifinder --update   # Install the latest stable release
```

Updates verify the checksum and version before atomically replacing the binary. Failed downloads or verification leave the existing installation intact. Restart running instances to use the new version. No update checks run in the background.

Updates use this repository by default. To use a fork:

```sh
TUIFINDER_REPO=OWNER/tuifinder tuifinder --update
```

## Keyboard Shortcuts

| Key | Action |
|---|---|
| Click / Enter | Open a file or toggle a folder |
| Tab | Switch tree / preview focus |
| ↑↓ / j k / mouse wheel | Move or scroll |
| ←→ / h l | Navigate folders or scroll horizontally |
| Home / End / PageUp / PageDown | Jump or page |
| d | Toggle code / diff |
| g | Toggle changed files only |
| i | Toggle ignored files |
| p / Ctrl-p | Find a filename |
| / | Search the current view |
| Ctrl-f | Search repository contents |
| r | Refresh |
| ? | Help |
| q / Ctrl-c | Quit |

In search, type a query, press Enter, select with ↑↓, and press Enter to open. Esc closes search. Shortcuts stay at the bottom and adapt to the available space.

## Search and Git

Search uses literal strings and returns up to 1,000 results. Filename matching ignores case; content matching is case-sensitive. Ignore rules apply unless toggled with `i`; `.git` is always excluded.

Diffs compare **HEAD with the working tree**, combining staged and unstaged changes. Committed changes disappear from the list. Git status columns show index and working-tree state: `M` modified, `A` added, `D` deleted, and `??` untracked. Renames appear as deletion and addition. Select a deleted file in the changes tree and press `d` to review it.

## Performance

The UI redraws only on changes and renders visible rows. File loading, highlighting, Git operations, and repository searches run off the UI thread. Unchanged highlighting is reused, and stale results are discarded.

File notifications are coalesced at 250ms intervals, with a 10-second fallback refresh. Press `r` to refresh immediately.

The top-right counter shows the latest completed action:

- `load`, `tree`: initial listing and tree navigation
- `open`, `code`, `diff`: preview preparation
- `files`, `find`, `grep`: searches
- `refresh`: manual refresh

Times include queueing and processing until the result reaches UI state. They **exclude final rendering, SSH transport, and terminal display**. `<1ms` means under one millisecond; `!` marks a failed action. Automatic refreshes and cancelled actions do not overwrite the counter.

## Limitations

Files are loaded in full; large files and long lines can increase memory use and latency. Binary files, non-UTF-8 text, and symlinks outside the workspace are not previewed. Unsupported syntax is shown as plain text. The color theme is designed for dark terminals.

No editing, file management, clipboard integration, staging, or commits.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 tests/install_smoke.py target/debug/tuifinder
cargo build --release --locked
python3 tests/tmux_smoke.py
```

Installer tests mock downloads and use temporary directories. The tmux test requires Python 3 and tmux, and runs in an isolated session without touching existing sessions.

## Releases

Update `Cargo.toml`, run `cargo check` to update `Cargo.lock`, and commit and push the changes. Then push a matching version tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The release workflow builds and tests macOS and Linux binaries for x86_64 and ARM64, using musl for Linux. It uploads binaries and SHA-256 checksums before publishing. Tags must match the Cargo version; prerelease tags are excluded from default stable updates.

## License

[MIT](LICENSE) © 2026 JAEGYUN JUNG
