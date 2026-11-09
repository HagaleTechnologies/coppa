"""Unit tests for tools/check_ffi_abi.py (COP-178).

Run: python3 -m unittest discover -s tools/tests -p 'test_check_ffi_abi.py'
Needs the `libclang` Python package and a C compiler; a missing dependency
is an error, not a skip.
"""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(TOOLS))

import check_ffi_abi as abi  # noqa: E402

BASE_HEADER = """
#include <stdint.h>
#define COPPA_OK 0
#define COPPA_ERR_NULL_ARG -1
#define COPPA_STATE_IDLE 0
#define COPPA_ABI_VERSION_MAJOR {major}
#define COPPA_ABI_VERSION_MINOR {minor}
typedef struct coppa_engine_t coppa_engine_t;
typedef struct coppa_cfg_t {{
  uint32_t struct_size;
  const char *name;
  uint8_t level;
}} coppa_cfg_t;
typedef enum coppa_kind_t {{ COPPA_KIND_A = 1, COPPA_KIND_B = 2 }} coppa_kind_t;
{extra_decls}
int32_t coppa_send(struct coppa_engine_t *h, const uint8_t *data, uintptr_t len);
const char *coppa_name(void);
{extra_funcs}
"""


def header(major=1, minor=0, extra_decls="", extra_funcs="", replace=None):
    text = BASE_HEADER.format(major=major, minor=minor, extra_decls=extra_decls, extra_funcs=extra_funcs)
    for old, new in (replace or {}).items():
        assert old in text, old
        text = text.replace(old, new)
    return text


class ManifestCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.n = 0

    def tearDown(self):
        self._tmp.cleanup()

    def manifest(self, text, exports="declared"):
        self.n += 1
        d = self.tmp / f"h{self.n}"
        d.mkdir()
        p = d / "coppa.h"
        p.write_text(text)
        m = abi.build_manifest(p, None)
        m["exports"] = sorted(m["functions"]) if exports == "declared" else exports
        return m

    def verdict(self, base_text, cur_text, cur_exports="declared"):
        b = self.manifest(base_text)
        c = self.manifest(cur_text, cur_exports)
        changes = abi.compare(b, c)
        return abi.classify(b["abi_version"], c["abi_version"], changes), changes

    def assertFails(self, base_text, cur_text, needle, **kw):
        v, _ = self.verdict(base_text, cur_text, **kw)
        self.assertFalse(v.ok, v)
        self.assertTrue(any(needle in p for p in v.problems), v.problems)
        return v


class ExtractorTest(ManifestCase):
    def test_real_header(self):
        m = abi.build_manifest(abi.REPO_ROOT / abi.HEADER_PATH, None)
        self.assertIn("coppa_abi_version", m["functions"])
        self.assertIn("coppa_version", m["functions"])
        cfg = m["records"]["coppa_link_config_t"]
        self.assertFalse(cfg["opaque"])
        self.assertEqual(cfg["fields"][0], {"name": "struct_size", "type": "unsigned int", "offset": 0})
        self.assertTrue(m["records"]["coppa_engine_t"]["opaque"])
        self.assertEqual(m["constants"]["COPPA_ERR_NULL_ARG"], -1)
        self.assertNotIn(abi.ABI_MAJOR, m["constants"])
        self.assertEqual(m["abi_version"][0], 1)

    def test_synthetic_header(self):
        m = self.manifest(header())
        self.assertEqual(m["functions"]["coppa_send"]["params"],
                         ["struct coppa_engine_t *", "const unsigned char *", "unsigned long"])
        self.assertEqual(m["enums"]["coppa_kind_t"], {"COPPA_KIND_A": 1, "COPPA_KIND_B": 2})
        self.assertEqual(m["records"]["coppa_cfg_t"]["size"], 24)

    def test_unevaluated_constant_fails(self):
        with self.assertRaises(abi.AbiToolError):
            self.manifest(header(extra_decls='#define COPPA_NAME "x"'))

    def test_unknown_declaration_fails(self):
        with self.assertRaises(abi.AbiToolError):
            self.manifest(header(extra_decls="extern int coppa_global;"))

    def test_missing_version_metadata_fails(self):
        with self.assertRaises(abi.AbiToolError):
            self.manifest(header().replace("#define COPPA_ABI_VERSION_MINOR 0\n", ""))

    def test_missing_compiler_fails(self):
        p = self.tmp / "coppa.h"
        p.write_text(header())
        with self.assertRaises(abi.AbiToolError):
            abi.build_manifest(p, None, cc="no-such-cc-coppa")


class CompareTest(ManifestCase):
    def test_unchanged(self):
        v, changes = self.verdict(header(), header())
        self.assertTrue(v.ok)
        self.assertEqual(v.summary, "PASS (unchanged)")
        self.assertEqual(changes, [])

    def test_removed_function(self):
        self.assertFails(header(), header(replace={"const char *coppa_name(void);\n": ""}),
                         "function coppa_name removed")

    def test_argument_type_change(self):
        self.assertFails(header(), header(replace={"uintptr_t len": "uint32_t len"}),
                         "function coppa_send changed")

    def test_return_type_change(self):
        self.assertFails(header(), header(replace={"int32_t coppa_send": "int64_t coppa_send"}),
                         "function coppa_send changed")

    def test_pointer_constness(self):
        self.assertFails(header(), header(replace={"const uint8_t *data": "uint8_t *data"}),
                         "function coppa_send changed")

    def test_struct_reorder(self):
        self.assertFails(header(), header(replace={
            "  const char *name;\n  uint8_t level;": "  uint8_t level;\n  const char *name;"}),
            "struct coppa_cfg_t changed")

    def test_struct_field_type(self):
        self.assertFails(header(), header(replace={"uint8_t level;": "uint16_t level;"}),
                         "struct coppa_cfg_t changed")

    def test_struct_size_growth(self):
        v = self.assertFails(header(), header(minor=1, replace={"uint8_t level;": "uint8_t level;\n  uint32_t extra;"}),
                             "struct coppa_cfg_t changed")
        self.assertTrue(any("requires COPPA_ABI_VERSION_MAJOR 2" in p for p in v.problems))

    def test_struct_offset_change(self):
        self.assertFails(header(), header(replace={"typedef struct coppa_cfg_t {": "typedef struct __attribute__((packed)) coppa_cfg_t {"}),
                         "struct coppa_cfg_t changed")

    def test_enum_value_change(self):
        self.assertFails(header(), header(replace={"COPPA_KIND_B = 2": "COPPA_KIND_B = 3"}),
                         "enum coppa_kind_t value COPPA_KIND_B changed")

    def test_macro_value_change(self):
        self.assertFails(header(), header(replace={"#define COPPA_ERR_NULL_ARG -1": "#define COPPA_ERR_NULL_ARG -9"}),
                         "constant COPPA_ERR_NULL_ARG changed")

    def test_removed_constant(self):
        self.assertFails(header(), header(replace={"#define COPPA_STATE_IDLE 0\n": ""}),
                         "constant COPPA_STATE_IDLE removed")

    def test_missing_export(self):
        self.assertFails(header(), header(), "coppa_send is declared in coppa.h but not exported",
                         cur_exports=["coppa_name"])

    def test_removed_export_detected_even_with_stale_header(self):
        b = self.manifest(header())
        b["exports"] = ["coppa_hidden", "coppa_name", "coppa_send"]
        c = self.manifest(header())
        changes = abi.compare(b, c)
        v = abi.classify(b["abi_version"], c["abi_version"], changes)
        self.assertFalse(v.ok)
        self.assertTrue(any("export coppa_hidden removed" in p for p in v.problems))

    def test_addition_with_minor_bump_passes(self):
        v, _ = self.verdict(header(), header(minor=1, extra_decls="#define COPPA_NEW 7",
                                             extra_funcs="uint32_t coppa_new(void);"))
        self.assertTrue(v.ok, v.problems)
        self.assertEqual(v.summary, "PASS (compatible additions)")

    def test_addition_without_minor_bump_fails(self):
        self.assertFails(header(), header(extra_funcs="uint32_t coppa_new(void);"),
                         "requires COPPA_ABI_VERSION_MINOR > 0")

    def test_incompatible_with_minor_only_bump_fails(self):
        self.assertFails(header(), header(minor=1, replace={"COPPA_KIND_B = 2": "COPPA_KIND_B = 3"}),
                         "requires COPPA_ABI_VERSION_MAJOR 2 and COPPA_ABI_VERSION_MINOR 0")

    def test_major_bump_with_minor_reset_passes(self):
        v, _ = self.verdict(header(minor=3), header(major=2, minor=0, replace={"const char *coppa_name(void);\n": ""}))
        self.assertTrue(v.ok, v.problems)
        self.assertEqual(v.summary, "PASS (major bump)")

    def test_major_bump_without_minor_reset_fails(self):
        self.assertFails(header(minor=3), header(major=2, minor=1), "must reset COPPA_ABI_VERSION_MINOR to 0")

    def test_rollback_fails(self):
        self.assertFails(header(minor=2), header(minor=1), "rolled back")
        self.assertFails(header(major=2), header(major=1, minor=5), "rolled back")

    def test_tagged_release_break_needs_major(self):
        """A tagged release baseline: a removed symbol, a changed layout and a
        changed enum value fail with an unchanged major and pass only after a
        valid major bump."""
        broken = dict(replace={
            "const char *coppa_name(void);\n": "",
            "uint8_t level;": "uint16_t level;",
            "COPPA_KIND_B = 2": "COPPA_KIND_B = 9",
        })
        v = self.assertFails(header(minor=4), header(minor=5, **broken), "function coppa_name removed")
        self.assertTrue(any("struct coppa_cfg_t changed" in p for p in v.problems))
        self.assertTrue(any("COPPA_KIND_B changed" in p for p in v.problems))
        ok, _ = self.verdict(header(minor=4), header(major=2, minor=0, **broken))
        self.assertTrue(ok.ok, ok.problems)


def git(repo, *args):
    subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, text=True)


class BaselineTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self._tmp.name) / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.email", "t@example.com")
        git(self.repo, "config", "user.name", "t")
        git(self.repo, "config", "commit.gpgsign", "false")
        git(self.repo, "config", "tag.gpgsign", "false")
        self.bootstrap = self.commit("0.11.0", header())

    def tearDown(self):
        self._tmp.cleanup()

    def commit(self, version, hdr, msg="c"):
        (self.repo / "Cargo.toml").write_text(
            f'[workspace]\nmembers = []\n\n[workspace.package]\nversion = "{version}"\n')
        h = self.repo / "crates" / "coppa-ffi"
        h.mkdir(parents=True, exist_ok=True)
        if hdr is None:
            (h / "coppa.h").unlink(missing_ok=True)
        else:
            (h / "coppa.h").write_text(hdr)
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "--allow-empty", "-m", msg)
        return subprocess.run(["git", "-C", str(self.repo), "rev-parse", "HEAD"], check=True,
                              capture_output=True, text=True).stdout.strip()

    def select(self, base="main"):
        return abi.select_baseline(self.repo, base, self.bootstrap)

    def test_no_tags_uses_bootstrap(self):
        b = self.select()
        self.assertEqual(b.ref, self.bootstrap)
        self.assertIn("no release tags", b.label)

    def test_highest_reachable_stable_tag(self):
        self.commit("0.12.0", header(minor=1))
        git(self.repo, "tag", "v0.12.0")
        self.commit("0.13.0", header(minor=2))
        git(self.repo, "tag", "v0.13.0")
        git(self.repo, "tag", "v0.13.0-rc.1")
        git(self.repo, "tag", "nightly")
        git(self.repo, "tag", "v9.9.9")  # package version does not match: not a release
        self.commit("0.14.0", header(minor=3), "pr commit after a release")
        b = self.select()
        self.assertEqual(b.ref, "v0.13.0")
        self.assertEqual(b.label, "release v0.13.0")

    def test_tag_on_other_branch_ignored(self):
        git(self.repo, "checkout", "-q", "-b", "side")
        self.commit("0.20.0", header(minor=1))
        git(self.repo, "tag", "v0.20.0")
        git(self.repo, "checkout", "-q", "main")
        self.assertEqual(self.select().ref, self.bootstrap)

    def test_missing_header_at_selected_release_fails(self):
        self.commit("0.12.0", None)
        git(self.repo, "tag", "v0.12.0")
        b = self.select()
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(abi.AbiToolError) as cm:
                abi.materialize_baseline(self.repo, b.ref, Path(d))
        self.assertIn("missing ABI metadata", str(cm.exception))

    def test_corrupt_header_at_selected_release_fails(self):
        self.commit("0.12.0", "this is not C {")
        git(self.repo, "tag", "v0.12.0")
        with tempfile.TemporaryDirectory() as d:
            p = abi.materialize_baseline(self.repo, self.select().ref, Path(d))
            with self.assertRaises(abi.AbiToolError):
                abi.build_manifest(p, None)

    def test_unknown_base_ref_fails(self):
        with self.assertRaises(abi.AbiToolError):
            self.select("origin/does-not-exist")

    def test_failed_fetch_is_an_error(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "tools").mkdir()
            (Path(d) / "tools" / "ffi-abi-baseline.json").write_text(json.dumps({"ref": self.bootstrap}))
            rc = abi.main(["--repo", str(self.repo), "--base-ref", "main", "--fetch-tags",
                           "--lib", str(Path(d) / "none.so")])
        self.assertEqual(rc, 2)  # no `origin` remote: the fetch fails, never "no tags"

    def test_bootstrap_file_validated(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "b.json"
            p.write_text(json.dumps({"ref": "main"}))
            with self.assertRaises(abi.AbiToolError):
                abi.load_bootstrap(p)
        data = abi.load_bootstrap()
        self.assertEqual(data["ref"], "42372733033dcac0908fe1b05a622fb17f34a2ee")

    def test_missing_library_fails(self):
        with self.assertRaises(abi.AbiToolError):
            abi.read_exports(Path(self._tmp.name) / "libcoppa_ffi.so")


class WorkflowTest(unittest.TestCase):
    """The CI job exists, is required by the `ci` fan-in and its watchdog."""

    def test_ci_full_wires_c_abi_job(self):
        wf = (abi.REPO_ROOT / ".github" / "workflows" / "ci-full.yml").read_text()
        self.assertIn("  ffi-abi:\n    name: C ABI compatibility\n", wf)
        self.assertIn('"C ABI compatibility"', wf)  # watchdog expected names
        self.assertRegex(wf, r"needs:\s*\[[^\]]*\bffi-abi,")
        self.assertIn("FFI_ABI: ${{ needs.ffi-abi.result }}", wf)
        self.assertIn('"$FFI_ABI"', wf)  # the failure loop
        self.assertIn("tools/check_ffi_abi.py", wf)
        self.assertIn("fetch-depth: 0", wf)


if __name__ == "__main__":
    unittest.main()
