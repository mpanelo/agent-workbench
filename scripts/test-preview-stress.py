#!/usr/bin/env python3
"""Test stress-harness calculations without starting tmux."""

import hashlib
import importlib.util
import os
import pty
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "stress_preview", Path(__file__).with_name("stress-preview.py")
)
stress = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stress)


class Calculations(unittest.TestCase):
    def test_cleanup(self):
        fixture = stress.Fixture.__new__(stress.Fixture)
        fixture.real_tmux = None
        fixture.master, slave = pty.openpty()
        fixture.client = subprocess.Popen(
            [
                sys.executable,
                "-c",
                "import sys; sys.stdout.buffer.write(b'x' * 200000); sys.stdout.flush()",
            ],
            stdin=slave,
            stdout=slave,
            stderr=slave,
        )
        os.close(slave)
        self.assertFalse(fixture.close())
        self.assertEqual(fixture.client.returncode, 0)
        self.assertIsNone(fixture.master)

    def test_fingerprint(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "binary"
            content = b"x" * 70000
            path.write_bytes(content)
            self.assertEqual(
                stress.fingerprint(path), hashlib.sha256(content).hexdigest()
            )

    def test_cpu(self):
        self.assertEqual(stress.cpu_seconds("1:02.50"), 62.5)
        self.assertEqual(stress.cpu_seconds("00:01:02"), 62)
        self.assertEqual(stress.cpu_seconds("1-02:03:04"), 93784)

    def test_percentile(self):
        self.assertIsNone(stress.percentile([], 0.95))
        self.assertEqual(stress.percentile([3, 1, 2], 0.5), 2)
        self.assertEqual(stress.percentile(list(range(100)), 0.95), 94)

    def test_marker(self):
        raw = b"\x1b[12;8H\x1b[32mAWBSTRESS_2_17_123456_END\x1b[0m"
        match = stress.MARKER.search(stress.CSI.sub(b"", raw))
        self.assertEqual(tuple(map(int, match.groups())), (2, 17, 123456))
        self.assertIsNone(stress.MARKER.search(b"AWBSTRESS_2_17_123456"))

    def test_duration(self):
        for value in ("nan", "inf", "-1", "9"):
            result = subprocess.run(
                [sys.executable, str(stress.HERE), "--duration", value],
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn(b"must be finite", result.stderr)


if __name__ == "__main__":
    unittest.main()
