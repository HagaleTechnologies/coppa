"""Guard wiki/manifest.json against stale page entries (COP-114 CI failure).

ci-full.yml lints wiki/ with sunto's wiki-lint, which requires each page's
manifest entry to carry the sha256 of the page body (after the closing ---
of the frontmatter, leading newlines stripped) and a copy of the page's
verified and sources fields. Editing a wiki page without syncing its
manifest entry turns the required "CI" check red. This test reproduces that
contract offline so the drift is caught before the push.
"""
import hashlib
import json
import pathlib
import unittest

WIKI = pathlib.Path(__file__).resolve().parents[2] / "wiki"


def split_page(path):
    text = path.read_text(encoding="utf-8")
    _, fm, body = text.split("---\n", 2)
    return fm, body.lstrip("\n")


def parse_frontmatter(fm):
    fields = {}
    sources = []
    in_sources = False
    for line in fm.splitlines():
        if in_sources:
            if line.startswith("  - "):
                sources.append(line[4:].strip())
                continue
            in_sources = False
        stripped = line.strip()
        if stripped.endswith(":"):
            in_sources = stripped == "sources:"
            continue
        if ":" in stripped:
            key, value = stripped.split(":", 1)
            fields[key.strip()] = value.strip()
    fields["sources"] = sources
    return fields


class ManifestTests(unittest.TestCase):
    def test_every_entry_matches_its_page(self):
        manifest = json.loads((WIKI / "manifest.json").read_text(encoding="utf-8"))
        for entry in manifest["pages"]:
            path = WIKI / entry["path"]
            fm, body = split_page(path)
            body_hash = "sha256:" + hashlib.sha256(body.encode()).hexdigest()
            fields = parse_frontmatter(fm)
            with self.subTest(page=entry["id"]):
                self.assertEqual(
                    body_hash,
                    entry["hash"],
                    "manifest hash is stale; sync wiki/manifest.json after editing "
                    + entry["path"],
                )
                self.assertEqual(
                    entry["verified_commit"],
                    fields["commit"],
                    "manifest verified_commit does not match the page frontmatter",
                )
                self.assertEqual(
                    entry["verified_date"],
                    fields["date"],
                    "manifest verified_date does not match the page frontmatter",
                )
                self.assertEqual(
                    entry["sources"],
                    fields["sources"],
                    "manifest sources do not match the page frontmatter",
                )


if __name__ == "__main__":
    unittest.main()
