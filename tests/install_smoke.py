"""Offline installer tests, with optional real --update integration.
Usage: python3 tests/install_smoke.py [target/debug/tuifinder]
"""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "install.sh"
BINARY = Path(sys.argv.pop(1)).resolve() if len(sys.argv) > 1 else None


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="tuifinder-install-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.assets = self.root / "assets"
        self.assets.mkdir()
        self.bin = self.root / "mock-bin"
        self.bin.mkdir()
        self.destination = self.root / "install with spaces"
        self.log = self.root / "requests"
        self.env = {
            **os.environ,
            "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
            "TUIFINDER_REPO": "test/tuifinder",
            "MOCK_ASSETS": str(self.assets),
            "MOCK_LOG": str(self.log),
            "MOCK_OS": "Linux",
            "MOCK_ARCH": "x86_64",
            "MOCK_LATEST": "v0.1.0",
        }
        self.mock("uname", '''#!/bin/sh
case "$1" in -s) printf '%s\\n' "$MOCK_OS" ;; -m) printf '%s\\n' "$MOCK_ARCH" ;; esac
''')
        self.mock("curl", '''#!/usr/bin/env python3
import os, pathlib, sys
args = sys.argv[1:]
assert args[args.index('--proto')+1] == '=https'
assert args[args.index('--proto-redir')+1] == '=https'
url = args[-1]
with open(os.environ['MOCK_LOG'], 'a') as f: f.write(url + '\\n')
if url.endswith('/releases/latest'):
    print('https://github.com/test/tuifinder/releases/tag/' + os.environ['MOCK_LATEST'], end='')
else:
    tag, asset = url.rsplit('/', 2)[1:]
    source = pathlib.Path(os.environ['MOCK_ASSETS']) / tag / asset
    if not source.is_file(): sys.exit(22)
    pathlib.Path(args[args.index('--output')+1]).write_bytes(source.read_bytes())
''')

    def mock(self, name, content):
        path = self.bin / name
        path.write_text(content)
        path.chmod(0o755)

    def release(self, version="0.1.0", target="x86_64-unknown-linux-musl", content=None):
        folder = self.assets / f"v{version}"
        folder.mkdir(exist_ok=True)
        asset = folder / f"tuifinder-{target}"
        asset.write_bytes(content or f"#!/bin/sh\nprintf 'tuifinder {version}\\n'\n".encode())
        digest = hashlib.sha256(asset.read_bytes()).hexdigest()
        asset.with_name(asset.name + ".sha256").write_text(f"{digest}  {asset.name}\n")
        return asset

    def install(self, *args):
        return subprocess.run(["sh", str(SCRIPT), "--dir", str(self.destination), *args],
                              env=self.env, text=True, capture_output=True)

    def assert_no_staging(self):
        self.assertFalse(list(self.destination.glob(".tuifinder-install.*")))

    def test_default_repository(self):
        self.env.pop("TUIFINDER_REPO")
        self.release()
        result = self.install("--version", "v0.1.0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("https://github.com/TGoddessana/tuifinder/releases/download/v0.1.0/", self.log.read_text())

    def test_install_update_and_pinned_version(self):
        self.release()
        first = self.install()
        self.assertEqual(first.returncode, 0, first.stderr)
        installed = self.destination / "tuifinder"
        self.assertTrue(os.access(installed, os.X_OK))
        self.release("0.2.0")
        self.env["MOCK_LATEST"] = "v0.2.0"
        update = self.install()
        self.assertEqual(update.returncode, 0, update.stderr)
        self.assertEqual(subprocess.check_output([str(installed), "--version"], text=True).strip(), "tuifinder 0.2.0")
        pin = self.install("--version", "0.1.0")
        self.assertEqual(pin.returncode, 0, pin.stderr)
        self.assertEqual(subprocess.check_output([str(installed), "--version"], text=True).strip(), "tuifinder 0.1.0")
        self.assert_no_staging()

    def test_corruption_missing_asset_and_wrong_version_preserve_existing_binary(self):
        asset = self.release()
        self.assertEqual(self.install().returncode, 0)
        installed = self.destination / "tuifinder"
        original = installed.read_bytes()
        asset.write_bytes(b"corrupted")
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Checksum mismatch", result.stderr)
        self.assertEqual(installed.read_bytes(), original)
        self.assert_no_staging()
        asset.unlink()
        self.assertNotEqual(self.install().returncode, 0)
        self.assertEqual(installed.read_bytes(), original)
        self.release(content=b"#!/bin/sh\necho tuifinder 9.9.9\n")
        result = self.install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("version does not match", result.stderr)
        self.assertEqual(installed.read_bytes(), original)
        self.assert_no_staging()

    def test_platform_asset_selection(self):
        for system, arch, target in [
            ("Linux", "x86_64", "x86_64-unknown-linux-musl"),
            ("Linux", "aarch64", "aarch64-unknown-linux-musl"),
            ("Darwin", "x86_64", "x86_64-apple-darwin"),
            ("Darwin", "arm64", "aarch64-apple-darwin"),
        ]:
            with self.subTest(target=target):
                self.env.update(MOCK_OS=system, MOCK_ARCH=arch)
                self.release(target=target)
                result = self.install()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(f"/tuifinder-{target}\n", self.log.read_text())

    def test_invalid_inputs_and_symlink_are_rejected(self):
        for args in [("--repo", "../oops"), ("--repo", "bad"), ("--version", "../bad"), ("--dir", ""), ("--version",)]:
            with self.subTest(args=args):
                self.assertNotEqual(self.install(*args).returncode, 0)
        self.assertFalse(self.log.exists())
        self.release()
        self.destination.mkdir()
        target = self.root / "unrelated"
        target.write_text("keep")
        (self.destination / "tuifinder").symlink_to(target)
        self.assertNotEqual(self.install().returncode, 0)
        self.assertEqual(target.read_text(), "keep")

    @unittest.skipUnless(BINARY, "Pass a built binary to test --update")
    def test_real_binary_update_uses_its_own_install_directory(self):
        self.destination.mkdir()
        installed = self.destination / "tuifinder"
        shutil.copy2(BINARY, installed)
        version = subprocess.check_output([str(installed), "--version"], text=True).strip().split()[1]
        self.env["MOCK_LATEST"] = f"v{version}"
        self.release(version=version, content=BINARY.read_bytes())
        result = subprocess.run([str(installed), "--update"], env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(installed.read_bytes(), BINARY.read_bytes())
        self.assert_no_staging()
        link = self.root / "shortcut"
        link.symlink_to(installed)
        result = subprocess.run([str(link), "--update"], env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(link.is_symlink())
        self.assertEqual(installed.read_bytes(), BINARY.read_bytes())


if __name__ == "__main__":
    unittest.main()
