"""Tests for the COP-47 release tooling.

Run: python3 -m unittest discover -s tools/release -v
"""

from __future__ import annotations

import hashlib
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import unittest
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import release  # noqa: E402

SCRIPT = HERE / "release.py"
REPO_ROOT = release.REPO_ROOT
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "release.yml"

# Daemon features the release deliberately leaves out. Empty today: every
# feature coppad declares ships (decision D4 in
# docs/DECISIONS/2026-11-09-release-binaries-and-versioning.md). A new daemon
# feature makes test_release_workflow_features_match_daemon fail until someone
# adds it to RELEASE_FEATURES in release.yml or lists it here with a reason.
EXCLUDED_DAEMON_FEATURES: set[str] = set()


def run(*args: str, cwd: Path | None = None, env: dict[str, str] | None = None):
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        capture_output=True,
        text=True,
        cwd=cwd,
        env=env,
        check=False,
    )


def write_manifest(path: Path, workspace: str, package: str) -> Path:
    path.write_text(
        f'[workspace.package]\nversion = "{workspace}"\n\n'
        f'[package]\nname = "coppa"\nversion = "{package}"\n',
        encoding="utf-8",
    )
    return path


GIT_ENV = {
    **os.environ,
    "GIT_AUTHOR_NAME": "t",
    "GIT_AUTHOR_EMAIL": "t@example.invalid",
    "GIT_COMMITTER_NAME": "t",
    "GIT_COMMITTER_EMAIL": "t@example.invalid",
    "GIT_CONFIG_NOSYSTEM": "1",
}


def git(repo: Path, *args: str) -> str:
    out = subprocess.run(
        ["git", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", "-C", str(repo), *args],
        capture_output=True,
        text=True,
        env=GIT_ENV,
        check=True,
    )
    return out.stdout.strip()


class VersionAndTag(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)

    def test_version_prints_cargo_toml_version(self) -> None:
        cargo = tomllib.loads((REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        expected = cargo["workspace"]["package"]["version"]
        r = run("version")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stdout.strip(), expected)

    def test_version_rejects_disagreeing_manifests(self) -> None:
        m = write_manifest(self.tmp / "Cargo.toml", "0.11.0", "0.12.0")
        r = run("version", "--manifest", str(m))
        self.assertEqual(r.returncode, 1)
        self.assertIn("0.11.0", r.stderr)
        self.assertIn("0.12.0", r.stderr)

    def test_tag_must_match_version(self) -> None:
        m = write_manifest(self.tmp / "Cargo.toml", "0.11.0", "0.11.0")
        r = run("version", "--manifest", str(m), "--tag", "v0.11.1")
        self.assertEqual(r.returncode, 1)
        self.assertIn("tag v0.11.1 does not match Cargo.toml version 0.11.0", r.stderr)
        ok = run("version", "--manifest", str(m), "--tag", "v0.11.0")
        self.assertEqual(ok.returncode, 0, ok.stderr)
        self.assertEqual(ok.stdout.strip(), "0.11.0")

    def test_tag_must_be_pre_1_0(self) -> None:
        m = write_manifest(self.tmp / "Cargo.toml", "1.0.0", "1.0.0")
        r = run("version", "--manifest", str(m), "--tag", "v1.0.0")
        self.assertEqual(r.returncode, 1)
        self.assertIn("coppa stays pre-1.0", r.stderr)

    def test_tag_needs_v_prefix_and_three_numbers(self) -> None:
        m = write_manifest(self.tmp / "Cargo.toml", "0.11.0", "0.11.0")
        for tag in ("0.11.0", "v0.11", "v0.11.0-rc.1", "v0.011.0"):
            with self.subTest(tag=tag):
                r = run("version", "--manifest", str(m), "--tag", tag)
                self.assertEqual(r.returncode, 1)
                self.assertIn("v0.MINOR.PATCH", r.stderr)

    def _repo(self) -> Path:
        repo = self.tmp / "repo"
        repo.mkdir()
        git(repo, "init", "-q", "-b", "main")
        write_manifest(repo / "Cargo.toml", "0.11.0", "0.11.0")
        git(repo, "add", "Cargo.toml")
        git(repo, "commit", "-q", "-m", "release 0.11.0")
        return repo

    def test_tag_on_main_passes(self) -> None:
        repo = self._repo()
        git(repo, "tag", "-a", "v0.11.0", "-m", "coppa 0.11.0")
        r = run("version", "--manifest", str(repo / "Cargo.toml"), "--tag", "v0.11.0",
                "--require-on-main", "main")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stdout.strip(), "0.11.0")

    def test_tag_off_main_fails(self) -> None:
        repo = self._repo()
        git(repo, "checkout", "-q", "-b", "side")
        (repo / "side.txt").write_text("side\n", encoding="utf-8")
        git(repo, "add", "side.txt")
        git(repo, "commit", "-q", "-m", "side commit")
        git(repo, "tag", "-a", "v0.11.0", "-m", "coppa 0.11.0")
        git(repo, "checkout", "-q", "main")
        r = run("version", "--manifest", str(repo / "Cargo.toml"), "--tag", "v0.11.0",
                "--require-on-main", "main")
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("not on main", r.stderr)

    def test_missing_main_ref_cannot_judge(self) -> None:
        repo = self._repo()
        git(repo, "tag", "-a", "v0.11.0", "-m", "coppa 0.11.0")
        r = run("version", "--manifest", str(repo / "Cargo.toml"), "--tag", "v0.11.0",
                "--require-on-main", "origin/main")
        self.assertEqual(r.returncode, 2, r.stderr)
        self.assertIn("cannot judge", r.stderr)

    def test_annotated_and_lightweight_tags_both_peel(self) -> None:
        for annotated in (True, False):
            with self.subTest(annotated=annotated):
                repo = self._repo()
                if annotated:
                    git(repo, "tag", "-a", "v0.11.0", "-m", "coppa 0.11.0")
                else:
                    git(repo, "tag", "v0.11.0")
                r = run("version", "--manifest", str(repo / "Cargo.toml"), "--tag", "v0.11.0",
                        "--require-on-main", "main")
                self.assertEqual(r.returncode, 0, r.stderr)
                shutil.rmtree(repo)


class AssetNames(unittest.TestCase):
    def test_asset_names_for_all_five_targets(self) -> None:
        expected = {
            "x86_64-unknown-linux-gnu": "coppa-0.11.0-x86_64-unknown-linux-gnu.tar.gz",
            "aarch64-unknown-linux-gnu": "coppa-0.11.0-aarch64-unknown-linux-gnu.tar.gz",
            "armv7-unknown-linux-gnueabihf": "coppa-0.11.0-armv7-unknown-linux-gnueabihf.tar.gz",
            "x86_64-pc-windows-msvc": "coppa-0.11.0-x86_64-pc-windows-msvc.zip",
            "universal-apple-darwin": "coppa-0.11.0-universal-apple-darwin.tar.gz",
        }
        self.assertEqual(set(release.TARGETS), set(expected))
        for target, name in expected.items():
            with self.subTest(target=target):
                self.assertEqual(release.asset_name("0.11.0", target), name)
                r = run("asset-name", "--version", "0.11.0", "--target", target)
                self.assertEqual(r.returncode, 0, r.stderr)
                self.assertEqual(r.stdout.strip(), name)
        bad = run("asset-name", "--version", "0.11.0", "--target", "x86_64-unknown-linux-musl")
        self.assertEqual(bad.returncode, 1)


# The Design section 2 listing in the COP-47 plan, below the top directory.
UNIX_LAYOUT = [
    "coppa",
    "coppad",
    "README.md",
    "LICENSE-APACHE",
    "LICENSE-MIT",
    "THIRD-PARTY-LICENSES.txt",
    "coppad.toml.example",
    "contrib/README.md",
    "contrib/coppad.service",
    "contrib/coppad@.service",
    "contrib/io.github.hagaletechnologies.coppad.plist",
]


class Packaging(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.bins = self.tmp / "bin"
        self.bins.mkdir()
        self.notices = self.tmp / "notices.txt"
        self.notices.write_text("third-party notices\n", encoding="utf-8")

    def _fake_bins(self, ext: str = "") -> None:
        for b in ("coppa", "coppad"):
            (self.bins / f"{b}{ext}").write_bytes(f"fake {b}\n".encode())

    def _package(self, target: str, out: Path, env: dict[str, str] | None = None):
        return run("package", "--version", "0.11.0", "--target", target, "--bin-dir",
                   str(self.bins), "--notices", str(self.notices), "--out", str(out), env=env)

    def test_package_tar_layout_and_modes(self) -> None:
        self._fake_bins()
        target = "aarch64-unknown-linux-gnu"
        r = self._package(target, self.tmp / "dist")
        self.assertEqual(r.returncode, 0, r.stderr)
        path = self.tmp / "dist" / "coppa-0.11.0-aarch64-unknown-linux-gnu.tar.gz"
        self.assertTrue(path.is_file())
        root = "coppa-0.11.0-aarch64-unknown-linux-gnu"
        with tarfile.open(path) as tar:
            members = tar.getmembers()
            names = [m.name for m in members]
            # tarfile drops the trailing slash it writes on directory names.
            self.assertEqual(names[0].rstrip("/"), root)
            self.assertTrue(members[0].isdir())
            files = {m.name[len(root) + 1:]: m for m in members[1:]}
            self.assertEqual(sorted(files), sorted(UNIX_LAYOUT))
            self.assertTrue(all(n == root or n.startswith(f"{root}/") for n in names))
            for name, m in files.items():
                with self.subTest(name=name):
                    self.assertTrue(m.isfile())
                    self.assertEqual(m.mode, 0o755 if name in ("coppa", "coppad") else 0o644)
                    self.assertEqual((m.uid, m.gid, m.uname, m.gname), (0, 0, "", ""))
            self.assertEqual(tar.extractfile(f"{root}/coppad").read(), b"fake coppad\n")
            self.assertEqual(
                tar.extractfile(f"{root}/THIRD-PARTY-LICENSES.txt").read(),
                b"third-party notices\n",
            )
            self.assertEqual(
                tar.extractfile(f"{root}/README.md").read(),
                (REPO_ROOT / "README.md").read_bytes(),
            )

    def test_package_zip_layout_for_windows(self) -> None:
        self._fake_bins(".exe")
        r = self._package("x86_64-pc-windows-msvc", self.tmp / "dist")
        self.assertEqual(r.returncode, 0, r.stderr)
        path = self.tmp / "dist" / "coppa-0.11.0-x86_64-pc-windows-msvc.zip"
        root = "coppa-0.11.0-x86_64-pc-windows-msvc"
        expected = [n + ".exe" if n in ("coppa", "coppad") else n for n in UNIX_LAYOUT]
        with zipfile.ZipFile(path) as zf:
            names = zf.namelist()
            self.assertEqual(names[0], f"{root}/")
            self.assertEqual(sorted(n[len(root) + 1:] for n in names[1:]), sorted(expected))
            self.assertEqual(zf.read(f"{root}/coppa.exe"), b"fake coppa\n")
            self.assertEqual((zf.getinfo(f"{root}/coppad.exe").external_attr >> 16) & 0o777, 0o755)
            self.assertEqual((zf.getinfo(f"{root}/README.md").external_attr >> 16) & 0o777, 0o644)

    def test_package_is_reproducible(self) -> None:
        env = {**os.environ, "SOURCE_DATE_EPOCH": "1762732800"}
        for target, ext in (("x86_64-unknown-linux-gnu", ""), ("x86_64-pc-windows-msvc", ".exe")):
            with self.subTest(target=target):
                self._fake_bins(ext)
                a = self._package(target, self.tmp / f"a-{target}", env)
                b = self._package(target, self.tmp / f"b-{target}", env)
                self.assertEqual(a.returncode, 0, a.stderr)
                self.assertEqual(b.returncode, 0, b.stderr)
                name = release.asset_name("0.11.0", target)
                self.assertEqual(
                    (self.tmp / f"a-{target}" / name).read_bytes(),
                    (self.tmp / f"b-{target}" / name).read_bytes(),
                )

    def test_package_fails_when_an_input_is_missing(self) -> None:
        self._fake_bins()
        self.notices.unlink()
        r = self._package("x86_64-unknown-linux-gnu", self.tmp / "dist")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn(str(self.notices), r.stderr)
        self.assertFalse((self.tmp / "dist" / "coppa-0.11.0-x86_64-unknown-linux-gnu.tar.gz").exists())


class Checksums(unittest.TestCase):
    def test_sums_lists_archives_sorted_in_sha256sum_format(self) -> None:
        tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp)
        files = {
            "coppa-0.11.0-x86_64-pc-windows-msvc.zip": b"zip",
            "coppa-0.11.0-aarch64-unknown-linux-gnu.tar.gz": b"arm",
            "coppa-0.11.0-universal-apple-darwin.tar.gz": b"mac",
            "notes.md": b"not an archive",
            "other.tar.gz": b"not ours",
        }
        for name, data in files.items():
            (tmp / name).write_bytes(data)
        r = run("sums", str(tmp))
        self.assertEqual(r.returncode, 0, r.stderr)
        lines = (tmp / "SHA256SUMS").read_text(encoding="utf-8").splitlines()
        archives = sorted(n for n in files if n.startswith("coppa-"))
        self.assertEqual([line.split("  ", 1)[1] for line in lines], archives)
        for line in lines:
            self.assertRegex(line, r"^[0-9a-f]{64}  coppa-\S+$")
            digest, name = line.split("  ", 1)
            self.assertEqual(digest, hashlib.sha256(files[name]).hexdigest())
        if shutil.which("sha256sum"):
            check = subprocess.run(["sha256sum", "-c", "SHA256SUMS"], cwd=tmp,
                                   capture_output=True, text=True, check=False)
            self.assertEqual(check.returncode, 0, check.stdout + check.stderr)


class Notes(unittest.TestCase):
    def test_notes_state_pre_1_0_and_no_ota(self) -> None:
        r = run("notes", "--version", "0.11.0")
        self.assertEqual(r.returncode, 0, r.stderr)
        first_paragraph = r.stdout.split("\n\n", 1)[0]
        for phrase in ("pre-1.0", "simulation-validated only",
                       "no over-the-air validation yet", "COP-20"):
            with self.subTest(phrase=phrase):
                self.assertIn(phrase, " ".join(first_paragraph.split()))

    def test_notes_carry_version_everywhere(self) -> None:
        r = run("notes", "--version", "0.11.0")
        self.assertEqual(r.returncode, 0, r.stderr)
        for target in release.TARGETS:
            self.assertIn(release.asset_name("0.11.0", target), r.stdout)
        for crate in ("coppa-cli", "coppa-daemon"):
            self.assertIn(
                f"cargo binstall --git https://github.com/HagaleTechnologies/coppa {crate}@0.11.0",
                r.stdout,
            )
        self.assertIsNone(re.search(r"\{[A-Za-z_][A-Za-z0-9_-]*\}", r.stdout))

    def test_notes_refuse_leftover_placeholders(self) -> None:
        tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp)
        template = tmp / "notes.md"
        template.write_text("coppa {version} {unknown}\n", encoding="utf-8")
        r = run("notes", "--version", "0.11.0", "--template", str(template))
        self.assertEqual(r.returncode, 1)
        self.assertIn("{unknown}", r.stderr)

    def test_first_release_notes_link_history(self) -> None:
        link = "https://github.com/HagaleTechnologies/coppa/commits/v0.11.0"
        first = run("notes", "--version", "0.11.0", "--first")
        later = run("notes", "--version", "0.11.0")
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertIn(link, first.stdout)
        self.assertIn("This is the first tagged release.", first.stdout)
        self.assertNotIn(link, later.stdout)


def substitute(template: str, values: dict[str, str]) -> str:
    """cargo-binstall's `{ key }` substitution, as its metadata docs describe it."""

    def repl(match: re.Match[str]) -> str:
        key = match.group(1)
        if key not in values:
            raise KeyError(f"unknown binstall placeholder {key!r} in {template!r}")
        return values[key]

    return re.sub(r"\{\s*([a-z-]+)\s*\}", repl, template)


class Binstall(unittest.TestCase):
    def test_binstall_metadata_matches_release_assets(self) -> None:
        root = tomllib.loads((REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        version = root["workspace"]["package"]["version"]
        for crate, binary in (("coppa-cli", "coppa"), ("coppa-daemon", "coppad")):
            manifest = tomllib.loads(
                (REPO_ROOT / "crates" / crate / "Cargo.toml").read_text(encoding="utf-8")
            )
            package = manifest["package"]
            repo = package["repository"]
            if repo == {"workspace": True}:
                repo = root["workspace"]["package"]["repository"]
            self.assertEqual(repo, release.REPOSITORY)
            self.assertIn(binary, [b["name"] for b in manifest["bin"]])
            meta = package["metadata"]["binstall"]
            for target in release.TARGETS:
                with self.subTest(crate=crate, target=target):
                    settings = {k: v for k, v in meta.items() if k != "overrides"}
                    settings.update(meta.get("overrides", {}).get(target, {}))
                    windows = target == release.WINDOWS_TARGET
                    values = {
                        "repo": repo,
                        "version": version,
                        "target": target,
                        "archive-suffix": ".zip" if windows else ".tar.gz",
                        "bin": binary,
                        "binary-ext": ".exe" if windows else "",
                    }
                    self.assertEqual(settings["pkg-fmt"], "zip" if windows else "tgz")
                    self.assertEqual(
                        substitute(settings["pkg-url"], values),
                        f"{release.REPOSITORY}/releases/download/v{version}/"
                        + release.asset_name(version, target),
                    )
                    bin_path = substitute(settings["bin-dir"], values)
                    self.assertEqual(
                        bin_path,
                        f"coppa-{version}-{target}/{binary}{values['binary-ext']}",
                    )
                    # The path binstall extracts is one `package` writes.
                    below_top = bin_path.split("/", 1)[1]
                    self.assertIn(below_top, release.archive_members(version, target))


def features(manifest: Path) -> set[str]:
    data = tomllib.loads(manifest.read_text(encoding="utf-8"))
    return set(data["features"]) - {"default"}


class Workflow(unittest.TestCase):
    def _text(self) -> str:
        self.assertTrue(WORKFLOW.is_file(), f"{WORKFLOW} is missing")
        return WORKFLOW.read_text(encoding="utf-8")

    def test_release_workflow_features_match_daemon(self) -> None:
        m = re.search(r"^\s*RELEASE_FEATURES:\s*(\S+)\s*$", self._text(), re.MULTILINE)
        self.assertIsNotNone(m, "release.yml has no RELEASE_FEATURES line")
        entries = m.group(1).strip("\"'").split(",")
        self.assertEqual(len(entries), len(set(entries)), "duplicate RELEASE_FEATURES entry")
        by_crate: dict[str, set[str]] = {}
        for entry in entries:
            crate, _, feature = entry.partition("/")
            self.assertTrue(feature, f"{entry!r} is not package-qualified")
            by_crate.setdefault(crate, set()).add(feature)
        self.assertEqual(set(by_crate), {"coppa-cli", "coppa-daemon"})
        daemon = features(REPO_ROOT / "crates" / "coppa-daemon" / "Cargo.toml")
        cli = features(REPO_ROOT / "crates" / "coppa-cli" / "Cargo.toml")
        self.assertEqual(by_crate["coppa-daemon"], daemon - EXCLUDED_DAEMON_FEATURES)
        self.assertEqual(by_crate["coppa-cli"], cli - {"file-backend"})
        # The ticket's five features all ship.
        self.assertLessEqual(
            {"cpal-backend", "websocket", "serial-ptt", "gpio-ptt", "kiss-tnc"},
            by_crate["coppa-daemon"],
        )

    def test_release_workflow_pins(self) -> None:
        lines = self._text().splitlines()
        uses = [line for line in lines if re.match(r"^\s*(-\s+)?uses:", line)]
        self.assertTrue(uses, "release.yml has no uses: lines")
        for line in uses:
            with self.subTest(line=line.strip()):
                self.assertRegex(line, r"uses:\s*[\w.-]+/[\w./-]+@[0-9a-f]{40}\s+#\s*\S+\s*$")
        self.assertNotIn("Swatinem/rust-cache", "\n".join(lines))
        for start, body in run_blocks(lines):
            with self.subTest(run_line=start + 1):
                self.assertNotIn("${{", body)


SMOKE = HERE / "smoke-linux.sh"

# Stands in for the Debian 11 ld-2.31.so: drop `--library-path DIR`, run the rest.
FAKE_LOADER = '#!/bin/sh\nshift 2\nexec "$@"\n'
FAKE_COPPA = """#!/bin/sh
case $1 in
  --version) printf 'coppa 0.11.0\\ncommit: 0123456789ab\\nfeatures: cpal-backend,file-backend,kiss-tnc\\n' ;;
  loopback) echo 'PASS: Loopback test successful!' ;;
esac
"""
# Prints a good banner, then exits with $FAKE_COPPAD_STATUS.
FAKE_COPPAD = """#!/bin/sh
if [ "$1" = --version ]; then echo 'coppad 0.11.0'; exit 0; fi
echo 'INFO coppad: coppad - Coppa Daemon starting version="0.11.0" git_sha="0123456789ab" \
features=ardop,cpal-backend,gpio-ptt,kiss-tnc,mdns,serial-ptt,websocket'
exit "$FAKE_COPPAD_STATUS"
"""


class SmokeRun(unittest.TestCase):
    def _smoke_run(self, coppad_status: int) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            bindir = Path(tmp) / "bin"
            (root / "lib" / "x86_64-linux-gnu").mkdir(parents=True)
            bindir.mkdir()
            for path, body in (
                (root / "lib" / "x86_64-linux-gnu" / "ld-2.31.so", FAKE_LOADER),
                (bindir / "coppa", FAKE_COPPA),
                (bindir / "coppad", FAKE_COPPAD),
            ):
                path.write_text(body, encoding="utf-8")
                path.chmod(0o755)
            return subprocess.run(
                ["bash", str(SMOKE), "run", "x86_64-unknown-linux-gnu", str(bindir), str(root), "0.11.0"],
                capture_output=True,
                text=True,
                env={**os.environ, "FAKE_COPPAD_STATUS": str(coppad_status)},
                check=False,
            )

    def test_coppad_banner_run_accepts_only_expected_exit_statuses(self) -> None:
        # 1: no sound card, startup error after the banner. 124: timeout
        # stopped a daemon that was still running.
        for status in (1, 124):
            with self.subTest(status=status):
                result = self._smoke_run(status)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("smoke run passed", result.stdout)
        # Stopping by itself (0), a panic (101), timeout failing (125) or a
        # crash (139) fails the run even though the banner printed.
        for status in (0, 101, 125, 139):
            with self.subTest(status=status):
                result = self._smoke_run(status)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn(f"coppad exited with status {status}", result.stderr)


def run_blocks(lines: list[str]) -> list[tuple[int, str]]:
    """(line index, text) of every `run:` value, including block scalars."""
    blocks = []
    i = 0
    while i < len(lines):
        m = re.match(r"^(\s*)(-\s+)?run:\s*(.*)$", lines[i])
        if not m:
            i += 1
            continue
        indent = len(m.group(1)) + len(m.group(2) or "")
        value = m.group(3)
        body = [value]
        j = i + 1
        if value.strip() and value.strip()[0] in "|>":
            while j < len(lines):
                line = lines[j]
                if line.strip() and len(line) - len(line.lstrip()) <= indent:
                    break
                body.append(line)
                j += 1
        blocks.append((i, "\n".join(body)))
        i = j
    return blocks


if __name__ == "__main__":
    unittest.main()

class DebianCandidate(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        for name in release.BINARIES:
            (self.bin / name).write_bytes(b"#!/bin/sh\nexit 0\n")
        self.notices = self.tmp / "notices.txt"
        self.notices.write_text("Test notice\n")

    def package(self, target: str):
        return run("deb", "--version", "0.17.0", "--target", target,
                   "--bin-dir", str(self.bin), "--notices", str(self.notices),
                   "--out", str(self.tmp / "out"))

    def test_deb_payload_and_metadata(self) -> None:
        for target, arch in release.DEB_ARCH.items():
            with self.subTest(arch=arch):
                result = self.package(target)
                self.assertEqual(result.returncode, 0, result.stderr)
                package = self.tmp / "out" / f"coppa_0.17.0_{arch}.deb"
                metadata = subprocess.check_output(["dpkg-deb", "-f", str(package)], text=True)
                self.assertIn(f"Architecture: {arch}", metadata)
                self.assertIn("libasound2t64 | libasound2", metadata)
                self.assertIn("Section: hamradio", metadata)
                self.assertIn("Priority: optional", metadata)
                self.assertIn(f"Homepage: {release.REPOSITORY}", metadata)
                self.assertIn("Maintainer: Tony Hagale <tony@hagale.net>", metadata)
                self.assertNotIn(".invalid", metadata)
                contents = subprocess.check_output(["dpkg-deb", "-c", str(package)], text=True)
                for name in ("usr/bin/coppa", "usr/bin/coppad", "lib/systemd/system/coppad.service",
                             "LICENSE-MIT", "coppad.toml.example"):
                    self.assertIn(name, contents)
                self.assertIn("-rwxr-xr-x", contents)

    def sandbox_script(self, name: str, *args: str, disabled: bool = False,
                      deny_start: bool = False):
        root = self.tmp / "sandbox"
        etc = root / "etc/coppa"
        state = root / "state"
        systemd = root / "run/systemd/system"
        systemd.mkdir(parents=True, exist_ok=True)
        shims = root / "shims"
        shims.mkdir(exist_ok=True)
        log = root / "commands.log"
        shim = """#!/bin/sh
printf '%s %s\n' "$(basename "$0")" "$*" >> "$COPPA_TEST_LOG"
case "$(basename "$0")" in
  getent) [ "$2" != coppa ] && [ "$2" != gpio ]; exit $? ;;
  id) exit 1 ;;
  install) for arg do last=$arg; done; mkdir -p "$last" ;;
  systemctl) if [ "$1" = is-active ] || [ "$1" = is-enabled ]; then [ "$COPPA_TEST_DISABLED" != 1 ]; exit $?; fi ;;
  deb-systemd-invoke) [ "$COPPA_TEST_DENY_START" = 1 ] && exit 1 ;;
  deb-systemd-helper) if [ "$1" = --quiet ] && [ "$2" = was-enabled ]; then [ "$COPPA_TEST_DISABLED" != 1 ]; exit $?; fi ;;
esac
exit 0
"""
        for command in ("getent", "id", "addgroup", "adduser", "install", "systemctl",
                        "deb-systemd-helper", "deb-systemd-invoke"):
            path = shims / command
            path.write_text(shim)
            path.chmod(0o755)
        before = log.read_text() if log.exists() else ""
        source = (REPO_ROOT / "tools/release/debian" / name).read_text()
        source = source.replace("/etc/coppa", str(etc))
        source = source.replace("/var/lib/coppa-appliance", str(state))
        source = source.replace("/run/systemd/system", str(systemd))
        source = source.replace("/usr/bin/deb-systemd-helper", str(shims / "deb-systemd-helper"))
        source = source.replace("/usr/bin/deb-systemd-invoke", str(shims / "deb-systemd-invoke"))
        script = root / name
        script.write_text(source)
        env = {**os.environ, "PATH": str(shims) + os.pathsep + os.environ["PATH"],
               "COPPA_TEST_LOG": str(log), "COPPA_TEST_DISABLED": "1" if disabled else "0",
               "COPPA_TEST_DENY_START": "1" if deny_start else "0"}
        result = subprocess.run(["sh", str(script), *args], env=env, text=True,
                                capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        return root, (log.read_text() if log.exists() else "")[len(before):]

    def test_deb_abort_remove_starts_unit_prerm_stopped(self) -> None:
        root, stopped = self.sandbox_script("prerm", "remove")
        self.assertIn("deb-systemd-invoke stop coppad.service", stopped)
        _, restored = self.sandbox_script("postinst", "abort-remove")
        self.assertIn("systemctl daemon-reload", restored)
        self.assertIn("deb-systemd-invoke start coppad.service", restored)
        for command in ("addgroup", "adduser", "install -d", "deb-systemd-helper"):
            self.assertNotIn(command, restored)
        self.assertFalse((root / "etc/coppa/setup-token").exists())

    def test_deb_abort_upgrade_and_abort_deconfigure_start_unit(self) -> None:
        for args in (("abort-upgrade", "0.17.0"),
                     ("abort-deconfigure", "in-favour", "coppa", "0.17.0")):
            with self.subTest(args=args):
                _, commands = self.sandbox_script("postinst", *args)
                self.assertIn("systemctl daemon-reload", commands)
                self.assertIn("deb-systemd-invoke start coppad.service", commands)
                for command in ("addgroup", "adduser", "install -d", "deb-systemd-helper"):
                    self.assertNotIn(command, commands)

    def test_deb_abort_respects_disabled_unit(self) -> None:
        _, commands = self.sandbox_script("postinst", "abort-remove", disabled=True)
        self.assertIn("systemctl daemon-reload", commands)
        self.assertNotIn("deb-systemd-invoke", commands)
        self.assertNotIn("deb-systemd-helper", commands)

    def test_deb_abort_remove_survives_policy_denial(self) -> None:
        _, commands = self.sandbox_script("postinst", "abort-remove", deny_start=True)
        self.assertIn("deb-systemd-invoke start coppad.service", commands)

    def test_deb_first_configure(self) -> None:
        scripts = REPO_ROOT / "tools/release/debian"
        postinst = (scripts / "postinst").read_text()
        self.assertIn("openssl rand -hex 16", postinst)
        self.assertIn("deb-systemd-helper enable", postinst)
        self.assertIn("deb-systemd-invoke start", postinst)
        self.assertIn("if [ ! -e /etc/coppa/setup-token ]", postinst)
        self.assertIn("LoadCredential=setup-token", (scripts / "coppad.service").read_text())
        root, commands = self.sandbox_script("postinst", "configure")
        token = root / "etc/coppa/setup-token"
        self.assertEqual(len(token.read_text().strip()), 32)
        self.assertEqual(token.stat().st_mode & 0o777, 0o600)
        # The admin directory must stay searchable by the service account:
        # with 0700 the daemon cannot stat the admin config and exits before
        # serving setup, while the token itself stays root-only at 0600.
        self.assertIn("install -d -o root -g root -m 0711", commands)
        self.assertIn("deb-systemd-helper enable", commands)
        self.assertIn("deb-systemd-invoke start", commands)
        original = token.read_text()
        # A retry of a failed first install (still an empty previous version)
        # must enable and start again, or the unit stays disabled and stopped.
        _, retry = self.sandbox_script("postinst", "configure")
        self.assertEqual(token.read_text(), original)
        self.assertIn("deb-systemd-helper enable", retry)
        self.assertIn("deb-systemd-invoke start", retry)

    def test_deb_first_configure_with_retained_token_still_enables(self) -> None:
        # First configure with an operator-owned token already present: the
        # token is preserved, and enablement/start must not depend on the
        # package having created the token itself.
        directory = self.tmp / "sandbox" / "etc/coppa"
        directory.mkdir(parents=True)
        (directory / "setup-token").write_text("operator-owned-token")
        root, commands = self.sandbox_script("postinst", "configure")
        self.assertEqual((root / "etc/coppa/setup-token").read_text(), "operator-owned-token")
        self.assertIn("deb-systemd-helper enable", commands)
        self.assertIn("deb-systemd-invoke start", commands)

    def test_deb_first_configure_provisions_before_starting(self) -> None:
        # Account, admin directory and unit enablement happen before the
        # service is asked to start.
        _, commands = self.sandbox_script("postinst", "configure")
        marks = {
            "account": commands.find("addgroup --system coppa"),
            "admin directory": commands.find("install -d -o root -g root -m 0711"),
            "enable": commands.find("deb-systemd-helper enable"),
            "start": commands.find("deb-systemd-invoke start"),
        }
        for name, position in marks.items():
            self.assertGreaterEqual(position, 0, f"missing {name} call")
        self.assertLess(marks["account"], marks["admin directory"])
        self.assertLess(marks["admin directory"], marks["enable"])
        self.assertLess(marks["enable"], marks["start"])

    def test_deb_service_policy_denial_does_not_fail_configure(self) -> None:
        # deb-systemd-invoke is policy-aware: a denied start must be logged,
        # not fail the package configuration.
        _, commands = self.sandbox_script("postinst", "configure", deny_start=True)
        self.assertIn("deb-systemd-invoke start coppad.service", commands)

    def test_deb_upgrade_keeps_admin_directory_and_disabled_unit(self) -> None:
        # An upgrade must not reset an administrator's choices: the existing
        # /etc/coppa mode stays as set, and a unit the administrator disabled
        # is only state-tracked, never re-enabled or started.
        root, _ = self.sandbox_script("postinst", "configure")
        directory = root / "etc/coppa"
        directory.chmod(0o750)
        _, upgrade = self.sandbox_script("postinst", "configure", "0.16.0", disabled=True)
        self.assertEqual(directory.stat().st_mode & 0o777, 0o750)
        self.assertNotIn("install -d", upgrade)
        self.assertNotIn("deb-systemd-helper enable", upgrade)
        self.assertIn("deb-systemd-helper update-state coppad.service", upgrade)
        self.assertNotIn("deb-systemd-invoke", upgrade)
        self.assertNotIn("Coppa first run", upgrade)

    def test_deb_remove_reloads_and_purge_unmasks(self) -> None:
        _, removed = self.sandbox_script("postrm", "remove")
        self.assertIn("systemctl daemon-reload", removed)
        _, purged = self.sandbox_script("postrm", "purge")
        self.assertIn("deb-systemd-helper purge coppad.service", purged)
        self.assertIn("deb-systemd-helper unmask coppad.service", purged)

    def test_deb_build_step_pins_the_commit_epoch(self) -> None:
        # Without SOURCE_DATE_EPOCH the Debian build falls back to the wall
        # clock and candidate digests vary between otherwise identical builds.
        workflow = WORKFLOW.read_text()
        step = workflow.split("Build Debian candidate", 1)[1].split("- name:", 1)[0]
        self.assertIn("SOURCE_DATE_EPOCH", step)

    def test_deb_upgrade_remove_and_purge(self) -> None:
        scripts = REPO_ROOT / "tools/release/debian"
        postinst = (scripts / "postinst").read_text()
        self.assertIn("systemctl is-active --quiet", postinst)
        self.assertIn("systemctl is-enabled --quiet", postinst)
        self.assertIn("deb-systemd-invoke restart", postinst)
        self.assertIn("[ \"$1\" = purge ]", (scripts / "postrm").read_text())
        self.assertNotIn("/etc/coppa/coppad.toml", (scripts / "postrm").read_text())
        root, _ = self.sandbox_script("postinst", "configure")
        managed = root / "state/coppad.toml"
        managed.parent.mkdir(exist_ok=True)
        managed.write_text("station data")
        admin = root / "etc/coppa/coppad.toml"
        admin.write_text("admin data")
        _, stopped = self.sandbox_script("postinst", "configure", "0.16.0", disabled=True)
        self.assertNotIn("deb-systemd-invoke restart", stopped)
        _, running = self.sandbox_script("postinst", "configure", "0.16.0")
        self.assertIn("deb-systemd-invoke restart", running)
        self.sandbox_script("prerm", "remove")
        self.sandbox_script("postrm", "remove")
        self.assertTrue(managed.exists())
        self.assertTrue((root / "etc/coppa/setup-token").exists())
        self.sandbox_script("postrm", "purge")
        self.assertFalse(managed.exists())
        self.assertFalse((root / "etc/coppa/setup-token").exists())
        self.assertTrue(admin.exists())

    def test_deb_rehearsal_artifacts_are_separate_from_publish(self) -> None:
        workflow = WORKFLOW.read_text()
        self.assertIn("debs-${{ matrix.deb_arch }}", workflow)
        self.assertIn("name: dist-${{ matrix.target }}", workflow)
        self.assertIn("Exactly the five archives", workflow)

    def test_deb_preserves_preexisting_token_on_purge(self) -> None:
        root = self.tmp / "sandbox"
        directory = root / "etc/coppa"
        directory.mkdir(parents=True)
        token = directory / "setup-token"
        token.write_text("operator-owned-token")
        self.sandbox_script("postinst", "configure")
        self.sandbox_script("postrm", "purge")
        self.assertEqual(token.read_text(), "operator-owned-token")

    def test_deb_rejects_manual_unit_before_install(self) -> None:
        root = self.tmp / "manual-unit"
        root.mkdir()
        unit = root / "coppad.service"
        unit.symlink_to("/tmp/operator-coppad.service")
        source = (REPO_ROOT / "tools/release/debian/preinst").read_text()
        script = self.tmp / "preinst"
        script.write_text(source.replace("/etc/systemd/system", str(root)))
        result = subprocess.run(["sh", str(script), "install"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn("manual coppad.service", result.stderr)
