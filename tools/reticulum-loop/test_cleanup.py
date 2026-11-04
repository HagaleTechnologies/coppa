"""Exercise the EXIT trap without starting radios, audio, or Reticulum."""
import subprocess
import unittest
from pathlib import Path


class CleanupTest(unittest.TestCase):
    def run_cleanup(self, kill_status=0, wait_status=143, original_status=0):
        script = Path(__file__).with_name("run.sh").read_text()
        cleanup = script[script.index("cleanup() {"):script.index("trap cleanup EXIT")]
        return subprocess.run(
            ["bash", "-c", f"""
set -euo pipefail
PIDS=(123)
KEEP=1
kill() {{ return {kill_status}; }}
wait() {{ return {wait_status}; }}
{cleanup}
trap cleanup EXIT
exit {original_status}
"""], capture_output=True, text=True, check=False,
        )

    def test_expected_sigterm_is_successful_cleanup(self):
        self.assertEqual(self.run_cleanup().returncode, 0)

    def test_prior_failure_survives_cleanup(self):
        self.assertEqual(self.run_cleanup(original_status=7).returncode, 7)

    def test_kill_failure_is_reported_and_fails(self):
        result = self.run_cleanup(kill_status=1, wait_status=0)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("could not terminate child 123", result.stderr)

    def test_unexpected_child_failure_is_reported_and_fails(self):
        result = self.run_cleanup(wait_status=2)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("child 123 exited with status 2", result.stderr)


if __name__ == "__main__":
    unittest.main()
