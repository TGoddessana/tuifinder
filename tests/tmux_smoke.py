"""Isolated tmux integration test. Usage: python3 tests/tmux_smoke.py [binary]."""
import os
import re
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import time

binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/tuifinder").resolve()
assert binary.is_file(), f"Build first: {binary}"
with tempfile.TemporaryDirectory(prefix="tuifinder-smoke-") as temporary:
    base = Path(temporary)
    root = base / "repo"
    root.mkdir()
    source = root / "main.rs"
    source.write_text('fn main() { println!("before"); }\n')
    def git(*args):
        subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True)
    git("init", "-q")
    git("-c", "user.name=Test", "-c", "user.email=test@example.invalid", "add", ".")
    git("-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-qm", "initial")
    socket = str(base / "tmux.sock")
    def tmux(*args, check=True):
        return subprocess.run(["tmux", "-S", socket, *args], check=check, capture_output=True, text=True).stdout
    def screen():
        return tmux("capture-pane", "-p", "-t", "smoke:0.1")
    def wait_for(text, timeout=15):
        start = time.monotonic()
        while time.monotonic() - start < timeout:
            current = screen()
            if text in current:
                return time.monotonic() - start
            time.sleep(0.05)
        raise AssertionError(f"Missing {text!r}:\n{screen()}")
    def keys(*keys):
        tmux("send-keys", "-t", "smoke:0.1", *keys)
    started = time.monotonic()
    command = f"{shlex.quote(str(binary))} {shlex.quote(str(root))}; printf '\\nTUIFINDER_EXITED\\n'; exec /bin/sh"
    try:
        tmux("-f", "/dev/null", "new-session", "-d", "-x", "180", "-y", "32", "-s", "smoke", "printf 'Agent pane\n'; exec /bin/sh")
        tmux("split-window", "-h", "-p", "70", "-t", "smoke:0.0", command)
        wait_for("main.rs")
        startup = time.monotonic() - started
        tmux("send-keys", "-t", "smoke:0.1", "-l", "\x1b[<0;8;3M\x1b[<0;8;3m")
        wait_for("before")
        assert "q Quit" in screen() and "r Refresh" in screen()
        open_metric = re.search(r"open (?:<1|[0-9]+)ms", screen()).group()
        source.write_text('fn main() { println!("after-save"); }\n')
        latency = wait_for("after-save")
        assert open_metric in screen(), "Background refresh must not overwrite action timing"
        keys("d")
        wait_for("Diff: HEAD")
        wait_for("-fn main()")
        wait_for("+fn main()")
        assert re.search(r"diff (?:<1|[0-9]+)ms", screen())
        keys("g")
        wait_for("Changes")
        keys("p")
        keys("main", "Enter")
        wait_for("1 matches")
        assert re.search(r"files (?:<1|[0-9]+)ms", screen())
        keys("Enter")
        wait_for("Code")
        keys("/")
        keys("after-save", "Enter")
        wait_for("select")
        assert re.search(r"find (?:<1|[0-9]+)ms", screen())
        keys("Enter")
        keys("C-f")
        keys("after-save", "Enter")
        wait_for("1 matches")
        assert re.search(r"grep (?:<1|[0-9]+)ms", screen())
        keys("Escape")
        wait_for("q Quit")
        keys("r")
        wait_for("refresh ")
        (root / "new.rs").write_text("fn new_file() {}\n")
        wait_for("new.rs")
        source.unlink()
        wait_for("No such file")
        keys("d")
        wait_for("-fn main()")
        keys("?")
        wait_for("Help")
        keys("Escape")
        (root / "src").mkdir()
        (root / "src" / "nested.rs").write_text("fn nested() {}\n")
        wait_for("nested.rs")
        keys("g")
        wait_for("Files")
        keys("Tab", "Home", "Enter")
        wait_for("nested.rs")
        wait_for("tree ")
        tmux("resize-window", "-t", "smoke:0", "-x", "60", "-y", "14")
        wait_for("q Quit")
        assert "? Help" in screen()
        assert re.search(r"tree (?:<1|[0-9]+)ms", screen())
        tmux("resize-window", "-t", "smoke:0", "-x", "30", "-y", "7")
        wait_for("Pane too small")
        tmux("resize-window", "-t", "smoke:0", "-x", "180", "-y", "32")
        wait_for("changed")
        print(screen())
        keys("q")
        wait_for("TUIFINDER_EXITED")
        plain = base / "plain"
        plain.mkdir()
        (plain / "main.rs").write_text("fn plain_folder() {}\n")
        tmux("send-keys", "-t", "smoke:0.1", "-l", f"{shlex.quote(str(binary))} {shlex.quote(str(plain))}")
        keys("Enter")
        wait_for("No Git")
        wait_for("main.rs")
        assert "fatal:" not in screen()
        assert "q Quit" in screen() and "r Refresh" in screen()
        assert re.search(r"load (?:<1|[0-9]+)ms", screen())
        keys("Enter")
        wait_for("plain_folder")
        assert re.search(r"open (?:<1|[0-9]+)ms", screen())
        keys("d")
        wait_for("Git view unavailable")
        assert "q Quit" in screen() and "plain_folder" in screen()
        keys("q")
        print(f"PASS: tmux split pane, mouse open, diff, filters, 3 searches, save/create/delete, resize, clean exit. Startup {startup:.3f}s; save-to-screen {latency:.3f}s (small local fixture).")
    finally:
        tmux("kill-server", check=False)
