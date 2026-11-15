#!/usr/bin/env python3
"""Release tooling for coppa (COP-47).

Used by .github/workflows/release.yml and tested by test_release.py.
Standard library only (Python 3.11+ for tomllib).

Subcommands:
  version     print the workspace version; optionally check a release tag
  asset-name  print the archive name for a version and target
  package     build one release archive from two binaries and the repo docs
  sums        write SHA256SUMS for the archives in a directory
  notes       render the release notes template

The asset names and the archive layout are a public contract: the
[package.metadata.binstall] blocks in crates/coppa-cli and crates/coppa-daemon
point at them, and cargo-binstall applies main's metadata to every older
version a user pins. See docs/RELEASING.md before changing either.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import os
import re
import subprocess
import sys
import tarfile
import time
import tomllib
import zipfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
NOTES_TEMPLATE = Path(__file__).resolve().parent / "notes.md"

REPOSITORY = "https://github.com/HagaleTechnologies/coppa"

# The five release targets, in the order the release page lists them.
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "armv7-unknown-linux-gnueabihf",
    "x86_64-pc-windows-msvc",
    "universal-apple-darwin",
)
WINDOWS_TARGET = "x86_64-pc-windows-msvc"

# The binaries every archive carries: the CLI and the daemon.
BINARIES = ("coppa", "coppad")

# Repository files copied into every archive, by their path in the archive
# (which is also their path in the repository).
DOC_FILES = (
    "README.md",
    "LICENSE-APACHE",
    "LICENSE-MIT",
    "coppad.toml.example",
    "contrib/README.md",
    "contrib/coppad.service",
    "contrib/coppad@.service",
    "contrib/io.github.hagaletechnologies.coppad.plist",
)
NOTICES_NAME = "THIRD-PARTY-LICENSES.txt"

# coppa stays pre-1.0 (ticket COP-47, comment 2 and comment 3).
TAG_RE = re.compile(r"^v0\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
VERSION_RE = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
PLACEHOLDER_RE = re.compile(r"\{[A-Za-z_][A-Za-z0-9_-]*\}")

FIRST_RELEASE_PARAGRAPH = (
    "This is the first tagged release. The full history is at\n"
    f"{REPOSITORY}/commits/v{{version}}\n"
)


class ReleaseError(Exception):
    """A check failed. `code` is the process exit status."""

    def __init__(self, message: str, code: int = 1) -> None:
        super().__init__(message)
        self.code = code


def binary_ext(target: str) -> str:
    return ".exe" if target == WINDOWS_TARGET else ""


def archive_suffix(target: str) -> str:
    return ".zip" if target == WINDOWS_TARGET else ".tar.gz"


def check_target(target: str) -> None:
    if target not in TARGETS:
        raise ReleaseError(f"unknown target {target}; release targets are: {', '.join(TARGETS)}")


def check_version_string(version: str) -> None:
    if not VERSION_RE.match(version):
        raise ReleaseError(f"version {version!r} is not MAJOR.MINOR.PATCH")


def top_dir(version: str, target: str) -> str:
    return f"coppa-{version}-{target}"


def asset_name(version: str, target: str) -> str:
    check_version_string(version)
    check_target(target)
    return f"{top_dir(version, target)}{archive_suffix(target)}"


def archive_members(version: str, target: str) -> list[str]:
    """Every file path in the archive, below the top directory, sorted."""
    ext = binary_ext(target)
    names = [f"{b}{ext}" for b in BINARIES] + list(DOC_FILES) + [NOTICES_NAME]
    return sorted(names)


# ---------------------------------------------------------------- version


def read_version(manifest: Path) -> str:
    try:
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise ReleaseError(f"{manifest} not found") from None
    workspace = data.get("workspace", {}).get("package", {}).get("version")
    package = data.get("package", {}).get("version")
    if not isinstance(workspace, str):
        raise ReleaseError(f"{manifest} has no [workspace.package] version")
    if not isinstance(package, str):
        raise ReleaseError(f"{manifest} has no [package] version")
    if workspace != package:
        raise ReleaseError(
            f"{manifest}: [workspace.package] version {workspace} and "
            f"[package] version {package} disagree"
        )
    return workspace


def git(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        capture_output=True,
        text=True,
        check=False,
    )


def check_tag(version: str, tag: str) -> None:
    if not TAG_RE.match(tag):
        raise ReleaseError(f"tag {tag}: release tags look like v0.MINOR.PATCH; coppa stays pre-1.0")
    if tag != f"v{version}":
        raise ReleaseError(f"tag {tag} does not match Cargo.toml version {version}")


def check_on_main(repo: Path, tag: str, main_ref: str) -> None:
    tag_commit = git(repo, "rev-parse", "--verify", "--quiet", f"refs/tags/{tag}^{{commit}}")
    if tag_commit.returncode != 0:
        raise ReleaseError(f"cannot judge whether {tag} is on {main_ref}: tag {tag} not found", 2)
    main_commit = git(repo, "rev-parse", "--verify", "--quiet", f"{main_ref}^{{commit}}")
    if main_commit.returncode != 0:
        raise ReleaseError(
            f"cannot judge whether {tag} is on {main_ref}: {main_ref} not found "
            "(fetch it with full history first)",
            2,
        )
    sha = tag_commit.stdout.strip()
    ancestor = git(repo, "merge-base", "--is-ancestor", sha, main_commit.stdout.strip())
    if ancestor.returncode == 1:
        raise ReleaseError(f"tag {tag} points at {sha}, which is not on {main_ref}")
    if ancestor.returncode != 0:
        raise ReleaseError(
            f"cannot judge whether {tag} is on {main_ref}: {ancestor.stderr.strip()}", 2
        )


def cmd_version(args: argparse.Namespace) -> None:
    manifest = Path(args.manifest).resolve()
    version = read_version(manifest)
    if args.require_on_main and not args.tag:
        raise ReleaseError("--require-on-main needs --tag")
    if args.tag:
        check_tag(version, args.tag)
        if args.require_on_main:
            check_on_main(manifest.parent, args.tag, args.require_on_main)
    print(version)


# ---------------------------------------------------------------- asset-name


def cmd_asset_name(args: argparse.Namespace) -> None:
    print(asset_name(args.version, args.target))


# ---------------------------------------------------------------- package


def source_date_epoch() -> int:
    raw = os.environ.get("SOURCE_DATE_EPOCH")
    if raw is None or raw == "":
        return int(time.time())
    try:
        value = int(raw)
    except ValueError:
        raise ReleaseError(f"SOURCE_DATE_EPOCH={raw!r} is not an integer") from None
    if value < 0:
        raise ReleaseError(f"SOURCE_DATE_EPOCH={raw!r} is negative")
    return value


def collect_inputs(
    version: str, target: str, bin_dir: Path, notices: Path, repo_root: Path
) -> list[tuple[str, Path, int]]:
    """(path in archive below the top dir, source file, mode), sorted by path."""
    ext = binary_ext(target)
    inputs: list[tuple[str, Path, int]] = []
    for b in BINARIES:
        inputs.append((f"{b}{ext}", bin_dir / f"{b}{ext}", 0o755))
    for rel in DOC_FILES:
        inputs.append((rel, repo_root / rel, 0o644))
    inputs.append((NOTICES_NAME, notices, 0o644))
    missing = [str(src) for _, src, _ in inputs if not src.is_file()]
    if missing:
        raise ReleaseError("missing archive input: " + ", ".join(missing))
    inputs.sort(key=lambda item: item[0])
    assert [name for name, _, _ in inputs] == archive_members(version, target)
    return inputs


def write_tar_gz(path: Path, root: str, inputs: list[tuple[str, Path, int]], mtime: int) -> None:
    def info(name: str, mode: int, size: int, kind: bytes) -> tarfile.TarInfo:
        ti = tarfile.TarInfo(name)
        ti.mode = mode
        ti.size = size
        ti.mtime = mtime
        ti.uid = ti.gid = 0
        ti.uname = ti.gname = ""
        ti.type = kind
        return ti

    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.PAX_FORMAT) as tar:
        tar.addfile(info(f"{root}/", 0o755, 0, tarfile.DIRTYPE))
        for name, src, mode in inputs:
            data = src.read_bytes()
            tar.addfile(info(f"{root}/{name}", mode, len(data), tarfile.REGTYPE), io.BytesIO(data))
    with path.open("wb") as raw, gzip.GzipFile(
        filename="", mode="wb", fileobj=raw, mtime=mtime, compresslevel=9
    ) as gz:
        gz.write(buf.getvalue())


def write_zip(path: Path, root: str, inputs: list[tuple[str, Path, int]], mtime: int) -> None:
    # ZIP timestamps start in 1980 and have two-second resolution.
    stamp = time.gmtime(max(mtime, 315532800))
    date_time = (stamp.tm_year, stamp.tm_mon, stamp.tm_mday, stamp.tm_hour, stamp.tm_min,
                 stamp.tm_sec - stamp.tm_sec % 2)

    def info(name: str, mode: int) -> zipfile.ZipInfo:
        zi = zipfile.ZipInfo(name, date_time=date_time)
        zi.create_system = 3  # Unix, so the mode bits below are honoured.
        zi.external_attr = mode << 16
        zi.compress_type = zipfile.ZIP_DEFLATED
        return zi

    with zipfile.ZipFile(path, "w") as zf:
        top = info(f"{root}/", 0o40755)
        top.external_attr |= 0x10  # MS-DOS directory flag
        top.compress_type = zipfile.ZIP_STORED
        zf.writestr(top, b"")
        for name, src, mode in inputs:
            zf.writestr(info(f"{root}/{name}", 0o100000 | mode), src.read_bytes())


def cmd_package(args: argparse.Namespace) -> None:
    name = asset_name(args.version, args.target)
    inputs = collect_inputs(
        args.version, args.target, Path(args.bin_dir), Path(args.notices), Path(args.repo_root)
    )
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    path = out / name
    root = top_dir(args.version, args.target)
    mtime = source_date_epoch()
    if args.target == WINDOWS_TARGET:
        write_zip(path, root, inputs, mtime)
    else:
        write_tar_gz(path, root, inputs, mtime)
    print(path)


# ---------------------------------------------------------------- sums


def is_archive(name: str) -> bool:
    return name.startswith("coppa-") and (name.endswith(".tar.gz") or name.endswith(".zip"))


def cmd_sums(args: argparse.Namespace) -> None:
    directory = Path(args.dir)
    names = sorted(p.name for p in directory.iterdir() if p.is_file() and is_archive(p.name))
    if not names:
        raise ReleaseError(f"no coppa-*.tar.gz or coppa-*.zip archives in {directory}")
    lines = []
    for name in names:
        digest = hashlib.sha256((directory / name).read_bytes()).hexdigest()
        lines.append(f"{digest}  {name}\n")
    (directory / "SHA256SUMS").write_text("".join(lines), encoding="utf-8")
    sys.stdout.write("".join(lines))


# ---------------------------------------------------------------- notes


def render_notes(version: str, first: bool, template: Path = NOTES_TEMPLATE) -> str:
    check_version_string(version)
    text = template.read_text(encoding="utf-8")
    if first:
        text = text.rstrip("\n") + "\n\n" + FIRST_RELEASE_PARAGRAPH
    text = text.replace("{version}", version)
    left = sorted(set(PLACEHOLDER_RE.findall(text)))
    if left:
        raise ReleaseError(f"{template}: unfilled placeholders: {', '.join(left)}")
    return text


def cmd_notes(args: argparse.Namespace) -> None:
    sys.stdout.write(render_notes(args.version, args.first, Path(args.template)))


# ---------------------------------------------------------------- main


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="command", required=True)

    v = sub.add_parser("version", help="print the workspace version and check a tag")
    v.add_argument("--manifest", default=str(REPO_ROOT / "Cargo.toml"))
    v.add_argument("--tag", help="release tag to check against the version")
    v.add_argument("--require-on-main", metavar="REF", help="fail unless the tag's commit is on REF")
    v.set_defaults(func=cmd_version)

    a = sub.add_parser("asset-name", help="print the archive name")
    a.add_argument("--version", required=True)
    a.add_argument("--target", required=True)
    a.set_defaults(func=cmd_asset_name)

    k = sub.add_parser("package", help="build one release archive")
    k.add_argument("--version", required=True)
    k.add_argument("--target", required=True)
    k.add_argument("--bin-dir", required=True, help="directory holding coppa and coppad")
    k.add_argument("--notices", required=True, help="third-party licence file")
    k.add_argument("--out", required=True, help="directory to write the archive into")
    k.add_argument("--repo-root", default=str(REPO_ROOT), help=argparse.SUPPRESS)
    k.set_defaults(func=cmd_package)

    s = sub.add_parser("sums", help="write SHA256SUMS for the archives in DIR")
    s.add_argument("dir")
    s.set_defaults(func=cmd_sums)

    n = sub.add_parser("notes", help="render the release notes")
    n.add_argument("--version", required=True)
    n.add_argument("--first", action="store_true", help="first release: link the full history")
    n.add_argument("--template", default=str(NOTES_TEMPLATE), help=argparse.SUPPRESS)
    n.set_defaults(func=cmd_notes)
    return p


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        args.func(args)
    except ReleaseError as err:
        print(f"error: {err}", file=sys.stderr)
        return err.code
    return 0


if __name__ == "__main__":
    sys.exit(main())
