#!/usr/bin/env python3
"""C ABI compatibility check for coppa-ffi (COP-178).

Compares the C contract of the current tree (crates/coppa-ffi/coppa.h plus the
symbols the built libcoppa_ffi.so exports) with a baseline: the newest stable
release tag reachable from --base-ref, or the pinned bootstrap commit in
tools/ffi-abi-baseline.json while no release tag exists.

Any removed or changed function signature, struct/union layout, typedef, enum
value or public COPPA_* numeric constant is incompatible and needs a larger
COPPA_ABI_VERSION_MAJOR with COPPA_ABI_VERSION_MINOR reset to 0. Additions need
the same major and a larger minor (or a valid major bump). Every infrastructure
problem (no compiler, no libclang, unreadable baseline, failed build, failed
tag fetch) exits nonzero; nothing here turns into a passing skip.

Declarations are read from the Clang AST (the `libclang` Python package);
numeric values, sizeof, alignof and offsetof come from a probe program the C
compiler builds against each header. See docs/testing/c-abi-check.md.

Exit codes: 0 compatible, 1 incompatible change or bad version metadata,
2 infrastructure error.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
HEADER_PATH = "crates/coppa-ffi/coppa.h"
OLD_CALLER_PATH = "crates/coppa-ffi/tests/c/error_strings.c"
BASELINE_FILE = REPO_ROOT / "tools" / "ffi-abi-baseline.json"
SYMBOL_PREFIX = "coppa_"
CONSTANT_PREFIX = "COPPA_"
ABI_MAJOR = "COPPA_ABI_VERSION_MAJOR"
ABI_MINOR = "COPPA_ABI_VERSION_MINOR"
VERSION_MACROS = {ABI_MAJOR, ABI_MINOR}
# Object-like macros in the header that are not numeric constants. Anything
# else named COPPA_* must evaluate to an integer or extraction fails.
NON_CONSTANT_MACROS = {"COPPA_H", "COPPA_API"}
RELEASE_TAG = re.compile(r"^v(\d+)\.(\d+)\.(\d+)$")


class AbiToolError(Exception):
    """An infrastructure failure: the comparison could not be made."""


# --------------------------------------------------------------------------
# Extraction
# --------------------------------------------------------------------------


def _cindex():
    try:
        import clang.cindex as cindex  # type: ignore
    except ImportError as e:  # pragma: no cover - environment dependent
        raise AbiToolError(
            "libclang Python bindings are missing: `python3 -m pip install libclang` "
            f"({e})"
        ) from e
    return cindex


def _run(cmd, cwd=None, env=None, what="command"):
    try:
        proc = subprocess.run(
            cmd, cwd=cwd, env=env, capture_output=True, text=True, check=False
        )
    except FileNotFoundError as e:
        raise AbiToolError(f"{what}: {cmd[0]} not found ({e})") from e
    if proc.returncode != 0:
        raise AbiToolError(
            f"{what} failed (exit {proc.returncode}): {' '.join(map(str, cmd))}\n"
            f"{proc.stdout}{proc.stderr}"
        )
    return proc.stdout


def _canon(t) -> str:
    return t.get_canonical().spelling


def _is_function_like_macro(cursor) -> bool:
    tokens = list(cursor.get_tokens())
    if len(tokens) < 2:
        return False
    name, nxt = tokens[0], tokens[1]
    return nxt.spelling == "(" and nxt.extent.start.offset == name.extent.end.offset


def _macro_body(cursor) -> list[str]:
    return [t.spelling for t in list(cursor.get_tokens())[1:]]


def system_include_dirs(cc: str = "cc") -> list[str]:
    """The C compiler's include search path. The pip `libclang` wheel ships
    no builtin headers (stdarg.h, stddef.h), so the parse borrows them."""
    if shutil.which(cc) is None:
        raise AbiToolError(f"C compiler {cc!r} not found")
    proc = subprocess.run(
        [cc, "-E", "-Wp,-v", "-x", "c", "-"], input="", capture_output=True, text=True, check=False
    )
    dirs, inside = [], False
    for line in proc.stderr.splitlines():
        if line.startswith("#include <...> search starts here"):
            inside = True
        elif line.startswith("End of search list"):
            break
        elif inside:
            dirs.append(line.strip())
    if proc.returncode != 0 or not dirs:
        raise AbiToolError(f"cannot read {cc}'s include search path:\n{proc.stderr}")
    return dirs


def extract_declarations(header: Path, cc: str = "cc") -> dict:
    """Read every public declaration in `header` from the Clang AST.

    Returns functions, records (struct/union), enums, typedefs and the names of
    object-like COPPA_* macros (values come from the probe). Fails on any
    parse error and on any declaration kind it does not know how to compare.
    """
    cindex = _cindex()
    header = Path(header).resolve()
    if not header.is_file():
        raise AbiToolError(f"header not found: {header}")
    index = cindex.Index.create()
    tu = index.parse(
        str(header),
        args=["-x", "c", "-std=c11"] + [f"-isystem{d}" for d in system_include_dirs(cc)],
        options=cindex.TranslationUnit.PARSE_DETAILED_PROCESSING_RECORD,
    )
    errors = [
        d.spelling
        for d in tu.diagnostics
        if d.severity >= cindex.Diagnostic.Error
    ]
    if errors:
        raise AbiToolError(f"clang could not parse {header}: {errors}")

    K = cindex.CursorKind
    functions: dict = {}
    records: dict = {}
    enums: dict = {}
    typedefs: dict = {}
    macros: list = []
    unsupported: list = []

    def in_header(c) -> bool:
        return c.location.file is not None and Path(c.location.file.name).resolve() == header

    def record(c):
        kind = "union" if c.kind == K.UNION_DECL else "struct"
        name = c.spelling
        if not name or c.is_anonymous():
            unsupported.append(f"anonymous {kind} at line {c.location.line}")
            return
        fields = []
        if c.is_definition():
            for f in c.get_children():
                if f.kind == K.FIELD_DECL:
                    fields.append({"name": f.spelling, "type": _canon(f.type)})
                elif f.kind in (K.STRUCT_DECL, K.UNION_DECL, K.ENUM_DECL):
                    unsupported.append(f"nested {f.kind.name} in {name}")
            prev = records.get(name)
            records[name] = {"kind": kind, "opaque": False, "fields": fields}
            if prev and not prev["opaque"] and prev["fields"] != fields:
                unsupported.append(f"conflicting definitions of {name}")
        elif name not in records:
            records[name] = {"kind": kind, "opaque": True, "fields": []}

    for c in tu.cursor.get_children():
        if not in_header(c):
            continue
        if c.kind == K.FUNCTION_DECL:
            ftype = c.type
            functions[c.spelling] = {
                "return": _canon(ftype.get_result()),
                "params": [_canon(a.type) for a in c.get_arguments()],
                "variadic": ftype.kind == cindex.TypeKind.FUNCTIONPROTO
                and ftype.is_function_variadic(),
            }
        elif c.kind in (K.STRUCT_DECL, K.UNION_DECL):
            record(c)
        elif c.kind == K.ENUM_DECL:
            if not c.spelling or c.is_anonymous():
                values = {e.spelling: e.enum_value for e in c.get_children()}
                enums.setdefault("<anonymous>", {}).update(values)
            else:
                enums[c.spelling] = {e.spelling: e.enum_value for e in c.get_children()}
        elif c.kind == K.TYPEDEF_DECL:
            underlying = c.underlying_typedef_type
            for child in c.get_children():
                if child.kind in (K.STRUCT_DECL, K.UNION_DECL):
                    record(child)
                elif child.kind == K.ENUM_DECL:
                    enums[child.spelling or c.spelling] = {
                        e.spelling: e.enum_value for e in child.get_children()
                    }
            typedefs[c.spelling] = _canon(underlying)
        elif c.kind == K.MACRO_DEFINITION:
            name = c.spelling
            if not name.startswith(CONSTANT_PREFIX):
                unsupported.append(f"macro {name} outside the {CONSTANT_PREFIX} namespace")
                continue
            if _is_function_like_macro(c) or name in NON_CONSTANT_MACROS:
                continue
            if not _macro_body(c):
                unsupported.append(f"empty macro {name}")
                continue
            macros.append(name)
        elif c.kind in (K.INCLUSION_DIRECTIVE, K.MACRO_INSTANTIATION):
            continue
        else:
            unsupported.append(f"{c.kind.name} {c.spelling!r} at line {c.location.line}")

    if unsupported:
        raise AbiToolError(
            f"unsupported declarations in {header} (extend the checker rather than "
            f"skipping them): {unsupported}"
        )
    return {
        "functions": functions,
        "records": records,
        "enums": enums,
        "typedefs": typedefs,
        "macros": sorted(macros),
    }


def _probe_source(header: Path, decls: dict) -> str:
    lines = [
        "#include <stddef.h>",
        "#include <stdio.h>",
        f'#include "{header.name}"',
    ]
    # A file-scope array bound must be an integer constant expression, so a
    # macro that is a string, float or runtime expression fails to compile.
    for name in decls["macros"]:
        lines.append(f"typedef char coppa_abi_probe_{name}[({name}) * 0 + 1];")
    lines.append("int main(void) {")
    for name in decls["macros"]:
        lines.append(
            f'  printf("const {name} %lld\\n", (long long)({name}));'
        )
    for name, rec in sorted(decls["records"].items()):
        if rec["opaque"]:
            continue
        t = f"{rec['kind']} {name}"
        lines.append(
            f'  printf("size {name} %zu %zu\\n", sizeof({t}), _Alignof({t}));'
        )
        for f in rec["fields"]:
            lines.append(
                f'  printf("offset {name} {f["name"]} %zu\\n", offsetof({t}, {f["name"]}));'
            )
    for ename, values in sorted(decls["enums"].items()):
        for const in values:
            lines.append(f'  printf("enum {const} %lld\\n", (long long)({const}));')
    lines += ["  return 0;", "}"]
    return "\n".join(lines) + "\n"


def probe_values(header: Path, decls: dict, cc: str = "cc") -> dict:
    """Compile and run a probe for constants, sizes, alignments and offsets."""
    header = Path(header).resolve()
    if shutil.which(cc) is None:
        raise AbiToolError(f"C compiler {cc!r} not found")
    with tempfile.TemporaryDirectory(prefix="coppa-abi-probe-") as tmp:
        src = Path(tmp) / "probe.c"
        exe = Path(tmp) / "probe"
        src.write_text(_probe_source(header, decls))
        _run(
            [cc, "-std=c11", "-Wall", "-Werror", f"-I{header.parent}", str(src), "-o", str(exe)],
            what="compiling the layout/constant probe",
        )
        out = _run([str(exe)], what="running the layout/constant probe")
    result = {"constants": {}, "sizes": {}, "offsets": {}, "enum_values": {}}
    for line in out.splitlines():
        parts = line.split()
        if parts[0] == "const":
            result["constants"][parts[1]] = int(parts[2])
        elif parts[0] == "size":
            result["sizes"][parts[1]] = {"size": int(parts[2]), "align": int(parts[3])}
        elif parts[0] == "offset":
            result["offsets"].setdefault(parts[1], {})[parts[2]] = int(parts[3])
        elif parts[0] == "enum":
            result["enum_values"][parts[1]] = int(parts[2])
        else:
            raise AbiToolError(f"unexpected probe output: {line!r}")
    return result


def read_exports(lib: Path) -> list[str]:
    """Defined dynamic `coppa_*` function symbols of a shared library."""
    lib = Path(lib)
    if not lib.is_file():
        raise AbiToolError(f"library not found: {lib} (build coppa-ffi first)")
    out = _run(["nm", "-D", "--defined-only", str(lib)], what="reading exports with nm")
    names = set()
    for line in out.splitlines():
        parts = line.split()
        if len(parts) >= 3 and parts[1] in ("T", "W", "i") and parts[2].startswith(SYMBOL_PREFIX):
            names.add(parts[2])
    if not names:
        raise AbiToolError(f"{lib} exports no {SYMBOL_PREFIX}* functions")
    return sorted(names)


def build_manifest(header: Path, exports: list[str] | None, cc: str = "cc") -> dict:
    """The normalized C contract of one header (and optionally its library)."""
    decls = extract_declarations(header, cc)
    probe = probe_values(header, decls, cc)
    records = {}
    for name, rec in decls["records"].items():
        entry = {"kind": rec["kind"], "opaque": rec["opaque"]}
        if not rec["opaque"]:
            offsets = probe["offsets"].get(name, {})
            entry["size"] = probe["sizes"][name]["size"]
            entry["align"] = probe["sizes"][name]["align"]
            entry["fields"] = [
                {"name": f["name"], "type": f["type"], "offset": offsets[f["name"]]}
                for f in rec["fields"]
            ]
        records[name] = entry
    constants = dict(sorted(probe["constants"].items()))
    for v in VERSION_MACROS:
        if v not in constants:
            raise AbiToolError(f"{header} has no {v}: missing ABI version metadata")
    return {
        "abi_version": [constants[ABI_MAJOR], constants[ABI_MINOR]],
        "functions": dict(sorted(decls["functions"].items())),
        "records": dict(sorted(records.items())),
        "enums": {k: dict(sorted(v.items())) for k, v in sorted(decls["enums"].items())},
        "typedefs": dict(sorted(decls["typedefs"].items())),
        "constants": {k: v for k, v in constants.items() if k not in VERSION_MACROS},
        "exports": sorted(exports) if exports is not None else None,
    }


# --------------------------------------------------------------------------
# Comparison
# --------------------------------------------------------------------------


@dataclass
class Change:
    kind: str  # "incompatible" | "addition" | "error"
    subject: str
    old: object = None
    new: object = None

    def describe(self) -> str:
        return f"{self.subject}: {self.old!r} -> {self.new!r}"


def _diff_map(changes, label, base: dict, cur: dict):
    for name in sorted(base.keys() | cur.keys()):
        if name not in cur:
            changes.append(Change("incompatible", f"{label} {name} removed", base[name], None))
        elif name not in base:
            changes.append(Change("addition", f"{label} {name} added", None, cur[name]))
        elif base[name] != cur[name]:
            changes.append(Change("incompatible", f"{label} {name} changed", base[name], cur[name]))


def check_exports(manifest: dict) -> list[Change]:
    """Every function the header declares must be exported by the library."""
    exports = manifest.get("exports")
    if exports is None:
        return []
    missing = sorted(set(manifest["functions"]) - set(exports))
    return [
        Change("error", f"function {n} is declared in coppa.h but not exported by the library", "declared", None)
        for n in missing
    ]


def compare(base: dict, cur: dict) -> list[Change]:
    changes: list[Change] = []
    _diff_map(changes, "function", base["functions"], cur["functions"])
    _diff_map(changes, "struct", base["records"], cur["records"])
    _diff_map(changes, "typedef", base["typedefs"], cur["typedefs"])
    _diff_map(changes, "constant", base["constants"], cur["constants"])
    for ename in sorted(base["enums"].keys() | cur["enums"].keys()):
        b = base["enums"].get(ename)
        c = cur["enums"].get(ename)
        if c is None:
            changes.append(Change("incompatible", f"enum {ename} removed", b, None))
        elif b is None:
            changes.append(Change("addition", f"enum {ename} added", None, c))
        else:
            _diff_map(changes, f"enum {ename} value", b, c)
    if base.get("exports") is not None and cur.get("exports") is not None:
        b, c = set(base["exports"]), set(cur["exports"])
        for n in sorted(b - c):
            changes.append(Change("incompatible", f"export {n} removed", "exported", None))
        declared = set(cur["functions"])
        for n in sorted(c - b - declared):
            changes.append(Change("addition", f"export {n} added", None, "exported"))
    changes.extend(check_exports(cur))
    return changes


@dataclass
class Verdict:
    ok: bool
    summary: str
    problems: list[str] = field(default_factory=list)


def classify(base_version, cur_version, changes: list[Change]) -> Verdict:
    """Apply the ABI version rules to a list of changes."""
    bM, bm = base_version
    cM, cm = cur_version
    incompatible = [c for c in changes if c.kind == "incompatible"]
    additions = [c for c in changes if c.kind == "addition"]
    errors = [c for c in changes if c.kind == "error"]
    problems = [f"{c.subject}" for c in errors]
    if (cM, cm) < (bM, bm):
        problems.append(
            f"ABI version rolled back from {bM}.{bm} to {cM}.{cm}; it may only increase"
        )
    elif cM > bM:
        if cm != 0:
            problems.append(
                f"ABI major bump {bM}.{bm} -> {cM}.{cm} must reset {ABI_MINOR} to 0"
            )
    else:
        for c in incompatible:
            problems.append(
                f"{c.describe()} [incompatible: requires {ABI_MAJOR} {bM + 1} and {ABI_MINOR} 0]"
            )
        if additions and cm <= bm:
            for c in additions:
                problems.append(
                    f"{c.describe()} [addition: requires {ABI_MINOR} > {bm} with {ABI_MAJOR} {bM}]"
                )
    if problems:
        return Verdict(False, "FAIL", problems)
    if cM > bM:
        return Verdict(True, "PASS (major bump)")
    if additions:
        return Verdict(True, "PASS (compatible additions)")
    return Verdict(True, "PASS (unchanged)")


# --------------------------------------------------------------------------
# Baseline selection
# --------------------------------------------------------------------------


@dataclass
class Baseline:
    ref: str
    label: str


def _git(repo: Path, *args, what=None) -> str:
    return _run(["git", "-C", str(repo), *args], what=what or f"git {' '.join(args)}")


def _workspace_version(repo: Path, ref: str) -> str | None:
    try:
        text = _git(repo, "show", f"{ref}:Cargo.toml")
    except AbiToolError:
        return None
    section = None
    for line in text.splitlines():
        s = line.strip()
        if s.startswith("["):
            section = s
            continue
        m = re.match(r'^version\s*=\s*"([^"]+)"', s)
        if m and section in ("[workspace.package]", "[package]"):
            return m.group(1)
    return None


def load_bootstrap(path: Path = BASELINE_FILE) -> dict:
    try:
        data = json.loads(Path(path).read_text())
    except (OSError, ValueError) as e:
        raise AbiToolError(f"cannot read bootstrap baseline {path}: {e}") from e
    if not isinstance(data.get("ref"), str) or not re.fullmatch(r"[0-9a-f]{40}", data["ref"]):
        raise AbiToolError(f"{path}: `ref` must be a full commit sha")
    return data


def select_baseline(repo: Path, base_ref: str, bootstrap_ref: str) -> Baseline:
    """Highest stable vX.Y.Z tag reachable from base_ref whose workspace version
    matches the tag, else the bootstrap commit."""
    _git(repo, "rev-parse", "--verify", f"{base_ref}^{{commit}}", what=f"resolving base ref {base_ref}")
    tags = _git(repo, "tag", "--merged", base_ref, "--list", "v*").split()
    releases = []
    for tag in tags:
        m = RELEASE_TAG.match(tag)
        if not m:
            continue
        version = ".".join(m.groups())
        if _workspace_version(repo, tag) != version:
            continue
        releases.append((tuple(int(x) for x in m.groups()), tag))
    if releases:
        releases.sort()
        tag = releases[-1][1]
        return Baseline(tag, f"release {tag}")
    _git(repo, "rev-parse", "--verify", f"{bootstrap_ref}^{{commit}}", what="resolving the bootstrap baseline commit")
    return Baseline(bootstrap_ref, f"bootstrap {bootstrap_ref} (no release tags)")


def materialize_baseline(repo: Path, ref: str, dest: Path, path: str = HEADER_PATH) -> Path:
    """Write `path` as of `ref` into `dest`; a missing file is an error."""
    try:
        text = _git(repo, "show", f"{ref}:{path}")
    except AbiToolError as e:
        raise AbiToolError(f"baseline {ref} has no {path}: missing ABI metadata\n{e}") from e
    out = Path(dest) / Path(path).name
    out.write_text(text)
    return out


def build_baseline_library(repo: Path, ref: str, tmp: Path) -> Path:
    """Build coppa-ffi at `ref` in a temporary worktree and return its .so."""
    wt = Path(tmp) / "baseline-worktree"
    _git(repo, "worktree", "add", "--detach", str(wt), ref, what=f"creating a worktree at {ref}")
    try:
        _run(
            ["cargo", "build", "-p", "coppa-ffi", "--target-dir", str(Path(tmp) / "target")],
            cwd=wt,
            what=f"building coppa-ffi at baseline {ref}",
        )
    finally:
        subprocess.run(["git", "-C", str(repo), "worktree", "remove", "--force", str(wt)], check=False)
    lib = Path(tmp) / "target" / "debug" / "libcoppa_ffi.so"
    if not lib.is_file():
        raise AbiToolError(f"baseline build at {ref} produced no {lib}")
    return lib


# --------------------------------------------------------------------------
# Smoke checks
# --------------------------------------------------------------------------


def old_caller_smoke(repo: Path, ref: str, lib: Path, cc: str = "cc", cxx: str = "c++") -> None:
    """Build the baseline's C caller against the baseline header and run it
    against the current library (an ABI-major-1 program keeps working)."""
    lib = Path(lib).resolve()
    with tempfile.TemporaryDirectory(prefix="coppa-abi-old-caller-") as tmp:
        tmpd = Path(tmp)
        materialize_baseline(repo, ref, tmpd)
        src = materialize_baseline(repo, ref, tmpd, OLD_CALLER_PATH)
        env = dict(os.environ, LD_LIBRARY_PATH=str(lib.parent))
        for compiler, flags, name in (
            (cc, ["-std=c11"], "old_caller"),
            (cxx, ["-x", "c++", "-std=c++17"], "old_caller_cpp"),
        ):
            exe = tmpd / name
            _run(
                [compiler, *flags, "-Wall", "-Wextra", "-Wpedantic", "-Werror", f"-I{tmpd}", str(src),
                 f"-L{lib.parent}", "-lcoppa_ffi", "-o", str(exe)],
                what=f"compiling the baseline caller with {compiler}",
            )
            _run([str(exe)], env=env, what=f"running the baseline caller {name} against the current library")


def header_smoke(header: Path, lib: Path, cc: str = "cc", cxx: str = "c++") -> None:
    """The current header compiles as C11 and C++17, and the loaded library's
    coppa_abi_version() equals the header's version macros."""
    header = Path(header).resolve()
    lib = Path(lib).resolve()
    src_text = (
        '#include <stdio.h>\n#include "coppa.h"\n'
        "int main(void) {\n"
        "  unsigned want = ((unsigned)COPPA_ABI_VERSION_MAJOR << 16) | (unsigned)COPPA_ABI_VERSION_MINOR;\n"
        "  unsigned got = coppa_abi_version();\n"
        '  printf("coppa_abi_version 0x%08x header 0x%08x\\n", got, want);\n'
        "  return got == want ? 0 : 1;\n"
        "}\n"
    )
    with tempfile.TemporaryDirectory(prefix="coppa-abi-header-") as tmp:
        tmpd = Path(tmp)
        src = tmpd / "header_smoke.c"
        src.write_text(src_text)
        env = dict(os.environ, LD_LIBRARY_PATH=str(lib.parent))
        for compiler, flags, name in (
            (cc, ["-std=c11"], "header_c11"),
            (cxx, ["-x", "c++", "-std=c++17"], "header_cpp17"),
        ):
            exe = tmpd / name
            _run(
                [compiler, *flags, "-Wall", "-Wextra", "-Wpedantic", "-Werror", f"-I{header.parent}", str(src),
                 f"-L{lib.parent}", "-lcoppa_ffi", "-o", str(exe)],
                what=f"compiling the current header with {compiler}",
            )
            _run([str(exe)], env=env, what=f"checking coppa_abi_version() ({name})")


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--repo", type=Path, default=REPO_ROOT)
    p.add_argument("--base-ref", default="origin/main",
                   help="history to take release tags from (PR: target branch or merge base; push: HEAD)")
    p.add_argument("--header", type=Path, default=None, help=f"current header (default {HEADER_PATH})")
    p.add_argument("--lib", type=Path, default=None,
                   help="current libcoppa_ffi.so (default target/debug/libcoppa_ffi.so)")
    p.add_argument("--baseline-exports", choices=["header", "build"], default="header",
                   help="take baseline exports from the baseline header, or build the baseline library")
    p.add_argument("--fetch-tags", action="store_true",
                   help="run `git fetch --tags origin` first; a failed fetch fails the check")
    p.add_argument("--smoke", action="store_true",
                   help="also build the baseline C caller and the current header (C11, C++17) and run them")
    p.add_argument("--write-manifests", type=Path, default=None, help="directory for baseline/current/diff JSON")
    p.add_argument("--cc", default=os.environ.get("CC", "cc"))
    p.add_argument("--cxx", default=os.environ.get("CXX", "c++"))
    args = p.parse_args(argv)

    repo = args.repo.resolve()
    header = (args.header or repo / HEADER_PATH).resolve()
    lib = (args.lib or repo / "target" / "debug" / "libcoppa_ffi.so").resolve()
    try:
        if args.fetch_tags:
            _git(repo, "fetch", "--tags", "--force", "origin", what="fetching release tags")
        bootstrap = load_bootstrap(repo / "tools" / "ffi-abi-baseline.json")
        baseline = select_baseline(repo, args.base_ref, bootstrap["ref"])
        print(f"C ABI baseline: {baseline.label}")
        with tempfile.TemporaryDirectory(prefix="coppa-abi-") as tmp:
            base_header = materialize_baseline(repo, baseline.ref, Path(tmp))
            if args.baseline_exports == "build":
                base_exports = read_exports(build_baseline_library(repo, baseline.ref, Path(tmp)))
                base_manifest = build_manifest(base_header, base_exports, args.cc)
            else:
                base_manifest = build_manifest(base_header, None, args.cc)
                base_manifest["exports"] = sorted(base_manifest["functions"])
        cur_manifest = build_manifest(header, read_exports(lib), args.cc)
        if args.baseline_exports == "header":
            # Header-derived baseline exports only know declared functions, so
            # compare exports over declared functions only: an undeclared
            # coppa_* export the baseline library also had is not an addition.
            declared = set(base_manifest["functions"]) | set(cur_manifest["functions"])
            cur_cmp = dict(cur_manifest, exports=sorted(set(cur_manifest["exports"]) & declared))
        else:
            cur_cmp = cur_manifest
        changes = compare(base_manifest, cur_cmp)
        bv, cv = base_manifest["abi_version"], cur_manifest["abi_version"]
        print(f"C ABI: {bv[0]}.{bv[1]} -> {cv[0]}.{cv[1]}")
        verdict = classify(bv, cv, changes)
        if args.write_manifests:
            out = args.write_manifests
            out.mkdir(parents=True, exist_ok=True)
            (out / "baseline.json").write_text(json.dumps(base_manifest, indent=2) + "\n")
            (out / "current.json").write_text(json.dumps(cur_manifest, indent=2) + "\n")
            (out / "diff.json").write_text(json.dumps(
                {"baseline": baseline.label, "verdict": verdict.summary, "problems": verdict.problems,
                 "changes": [c.__dict__ for c in changes]}, indent=2, default=str) + "\n")
        for c in changes:
            print(f"  {c.kind}: {c.describe()}")
        if args.smoke:
            if cv[0] > bv[0]:
                # A major bump is allowed to break old callers, so the
                # baseline caller is not expected to run against this library.
                print(f"C ABI smoke: baseline caller skipped (ABI major {bv[0]} -> {cv[0]})")
            else:
                old_caller_smoke(repo, baseline.ref, lib, args.cc, args.cxx)
            header_smoke(header, lib, args.cc, args.cxx)
            print("C ABI smoke: baseline caller and current header (C11, C++17) run against the current library")
    except AbiToolError as e:
        print(f"C ABI compatibility: ERROR\n{e}", file=sys.stderr)
        return 2
    except Exception as e:  # any other tool failure is infrastructure, never an ABI verdict
        print(f"C ABI compatibility: ERROR\ninternal checker error: {type(e).__name__}: {e}", file=sys.stderr)
        return 2
    if not verdict.ok:
        for prob in verdict.problems:
            print(f"  {prob}", file=sys.stderr)
        print("C ABI compatibility: FAIL", file=sys.stderr)
        return 1
    print(f"C ABI compatibility: {verdict.summary}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
