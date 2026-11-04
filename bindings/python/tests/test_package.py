"""The binding imports only the standard library."""

import ast
import sys
import unittest
from pathlib import Path

PKG_ROOT = Path(__file__).resolve().parents[1]


class PackageTests(unittest.TestCase):
    def test_imports_are_stdlib_only(self):
        allowed = set(sys.stdlib_module_names) | {"coppa"}
        files = sorted(PKG_ROOT.rglob("*.py"))
        self.assertTrue(files)
        for path in files:
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
            for node in ast.walk(tree):
                if isinstance(node, ast.Import):
                    names = [alias.name for alias in node.names]
                elif isinstance(node, ast.ImportFrom):
                    if node.level:  # relative import inside the package
                        continue
                    names = [node.module]
                else:
                    continue
                for name in names:
                    top = name.split(".")[0]
                    self.assertIn(top, allowed, f"{path}: imports non-stdlib module {name}")


if __name__ == "__main__":
    unittest.main()
