#!/usr/bin/env python3
"""Exercise real Workbench previews using six synthetic panes on a private server.

No user panes, configuration, workspaces, authentication, or paid calls are used.
Only standard-library modules, tmux, ps, and /usr/bin/time are required (Unix).
Build binaries before running; compilation during a run distorts CPU results.
"""

import argparse
import fcntl
import hashlib
import json
import math
import os
import pty
import re
import select
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import tempfile
import termios
import time
import tty
from pathlib import Path

HERE = Path(__file__).resolve()
MARKER = re.compile(rb"AWBSTRESS_(\d+)_(\d+)_(\d+)_END")
CSI = re.compile(rb"\x1b\[[0-?]*[ -/]*[@-~]")
SCENES = ("active", "siblings", "bursts", "navigation", "lifecycle")


def fingerprint(path):
    digest = hashlib.sha256()
    with path.open("rb") as binary:
        while block := binary.read(65536):
            digest.update(block)
    return digest.hexdigest()


def cpu_seconds(value):
    """Accept macOS m:ss.ss and Linux [days-]hh:mm:ss CPU times."""
    days, sep, rest = value.partition("-")
    value = rest if sep else days
    total = 0.0
    for component in value.split(":"):
        total = total * 60 + float(component)
    return total + (int(days) * 86400 if sep else 0)


def percentile(values, fraction):
    return (
        sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]
        if values
        else None
    )


def emit(event, **fields):
    print(json.dumps(dict(event=event, **fields), sort_keys=True), flush=True)


def write_scene(root, scene):
    temporary = root / "scene.next"
    temporary.write_text(json.dumps(scene))
    temporary.replace(root / "scene.json")


def feed(root, index):
    """Cursor-addressed output keeps history bounded and leaves a marker row."""
    tty.setcbreak(0)
    tick = 0
    last_frame = 0.0
    last_progress = 0.0
    previous = None
    while True:
        if select.select([0], [], [], 0)[0]:
            received = os.read(0, 4096)
            if received:
                with (root / "unexpected-input").open("ab") as log:
                    log.write(received)
        config = json.loads((root / "scene.json").read_text())
        now = time.monotonic()
        mode = config["mode"]
        rate = (10, 2, 5, 10, 20, 5)[index]
        if mode == "idle" or mode == "siblings" and index == 0:
            rate = 0
        if mode == "bursts":
            rate = 20 if int(now) % 6 < 2 else 2
        marker = ""
        pulse = config.get("pulse", 0)
        if (
            config["selected"] == index
            and 0 <= time.monotonic_ns() - pulse < 800_000_000
        ):
            marker = f"AWBSTRESS_{index}_{config['sequence']}_{pulse}_END"
        state = (mode, marker)
        if tick == 0 or state != previous or rate and now - last_frame >= 1 / rate:
            long = "λ🙂界\t" * 100 if mode == "bursts" else "λ🙂界 text " * 8
            rows = [
                f"\x1b[38;2;137;180;250mfixture {index}, row {row:02}, update {tick:08}: {long}\x1b[0m"
                for row in range(16)
            ]
            # Blank marker -> new marker forces a complete Ratatui cell run, so
            # latency can be read from real output without a terminal emulator.
            frame = "\x1b[H\x1b[2J" + "\r\n".join(rows + [marker, "fixture footer"])
            sys.stdout.write(frame)
            sys.stdout.flush()
            previous = state
            last_frame = now
            tick += 1
            if now - last_progress >= 1:
                progress = root / f"progress-{index}.next"
                progress.write_text(str(time.monotonic()))
                progress.replace(root / f"progress-{index}")
                last_progress = now
        time.sleep(0.02)


def launch(root, binary):
    """Executed only inside the disposable Workbench pane."""
    os.environ.update(
        PATH=str(root / "bin") + os.pathsep + os.environ["PATH"],
        AWB_STRESS_SOCKET=str(root / "tmux.sock"),
        AWB_STRESS_LOG=str(root / "transport.log"),
        AWB_STRESS_TMUX=(root / "real-tmux").read_text(),
        COLORTERM="truecolor",
    )
    output = os.open(root / "time.log", os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    os.dup2(output, 2)
    os.close(output)
    os.execv(
        "/usr/bin/time",
        ["time", "-p", binary, "--state-file", str(root / "state.json")],
    )


class Fixture:
    def __init__(self, root, binary, expected_live):
        self.root = root
        self.binary = str(Path(binary).resolve())
        self.expected_live = expected_live
        self.real_tmux = shutil.which("tmux")
        self.env = dict(os.environ, TERM="xterm-256color")
        for key in ("TMUX", "TMUX_PANE", "AGENT_WORKBENCH_STATE_FILE"):
            self.env.pop(key, None)
        self.client = None
        self.master = None
        self.panes = []
        self.app_pid = None
        self.failures = []
        self.failure_details = []
        self.latencies = []
        self.pending = {}
        self.observed = set()
        self.rolling = b""
        self.selected = 0
        self.in_work = True
        self.changed_at = 0.0
        self.samples = []
        self.max_observers = 0
        self.last_log = 0
        self.polls = 0
        self.streams = 0
        self.expected_starts = 1
        self.unexpected_reconnects = 0
        self.faults = {}
        self.closed_pane = False
        self.config = {"mode": "active", "selected": 0, "sequence": 0, "pulse": 0}

    def tmux(self, *args, check=True):
        result = subprocess.run(
            [self.real_tmux, "-S", str(self.root / "tmux.sock"), *args],
            env=self.env,
            capture_output=True,
            timeout=5,
            check=False,
        )
        if check and result.returncode:
            raise RuntimeError(
                f"fixture tmux command {args[0]} failed: {result.stderr!r}"
            )
        return result.stdout.decode("utf-8", errors="replace").strip()

    def setup(self):
        if not self.real_tmux or not Path(self.binary).is_file():
            raise RuntimeError("tmux and a prebuilt Workbench binary are required")
        self.binary_hash = fingerprint(Path(self.binary))
        (self.root / "bin").mkdir()
        wrapper = self.root / "bin" / "tmux"
        wrapper.symlink_to(HERE.with_name("preview-tmux.sh"))
        (self.root / "real-tmux").write_text(self.real_tmux)
        write_scene(self.root, self.config)
        bootstrap = self.tmux(
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "fixture",
            "-x",
            "200",
            "-y",
            "50",
            "-P",
            "-F",
            "#{pane_id}",
            "sleep",
            "900",
        )
        self.tmux("set-option", "-g", "remain-on-exit", "on")
        for index in range(6):
            command = [sys.executable, str(HERE), "--feed", str(self.root), str(index)]
            args = ["new-window", "-d", "-t", "fixture"]
            pane = self.tmux(*args, "-P", "-F", "#{pane_id}", *command)
            self.panes.append(pane)
            self.tmux("set-option", "-w", "-t", pane, "window-size", "manual")
            subprocess.run(
                [
                    self.binary,
                    "register",
                    "--id",
                    f"fixture-{index}",
                    "--kind",
                    "implementation",
                    "--repository",
                    str(self.root),
                    "--workspace",
                    str(self.root),
                    "--pane",
                    pane,
                    "--state-file",
                    str(self.root / "state.json"),
                ],
                env=self.env,
                check=True,
                capture_output=True,
                timeout=5,
            )
        self.tmux("set-option", "-g", "status", "off")
        self.tmux("set-option", "-g", "remain-on-exit", "on")
        self.server_pid = int(self.tmux("display-message", "-p", "#{pid}"))
        self.server_start = self.stats([self.server_pid])[self.server_pid][1]
        self.launched_at = time.monotonic()
        metadata = self.tmux(
            "new-window",
            "-d",
            "-t",
            "fixture",
            "-n",
            "workbench",
            "-P",
            "-F",
            "#{pane_id}:#{pane_pid}",
            sys.executable,
            str(HERE),
            "--launch",
            str(self.root),
            self.binary,
        )
        self.workbench, parent = metadata.split(":")
        self.parent_pid = int(parent)
        self.tmux("set-option", "-w", "-t", self.workbench, "window-size", "manual")
        self.tmux("resize-window", "-t", self.workbench, "-x", "240", "-y", "60")
        self.tmux("select-window", "-t", self.workbench)
        self.tmux("kill-pane", "-t", bootstrap)
        self.focus = self.tmux(
            "display-message", "-p", "-t", "fixture", "#{window_id}:#{pane_id}"
        )
        self.sizes = self.tmux(
            "list-panes", "-a", "-F", "#{pane_id}:#{pane_width}:#{pane_height}"
        ).splitlines()[:6]
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 60, 240, 0, 0))
        self.client = subprocess.Popen(
            [
                self.real_tmux,
                "-S",
                str(self.root / "tmux.sock"),
                "attach-session",
                "-f",
                "ignore-size",
                "-t",
                "fixture",
            ],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=self.env,
            start_new_session=True,
        )
        os.close(slave)
        self.pump(2)
        for pane in self.panes:
            if self.tmux("display-message", "-p", "-t", pane, "#{pane_dead}") == "1":
                text = self.tmux("capture-pane", "-p", "-t", pane)
                raise RuntimeError(f"synthetic producer exited: {text[-2000:]!r}")
        children = subprocess.check_output(
            ["pgrep", "-P", str(self.parent_pid)], text=True
        ).split()
        if len(children) != 1:
            raise RuntimeError("could not identify the fixture Workbench process")
        self.app_pid = int(children[0])
        self.client_start = 0.0

    def fail(self, message):
        if message not in self.failures:
            self.failures.append(message)
            self.failure_details.append(
                {"message": message, "mode": self.config["mode"]}
            )

    def keys(self, text):
        os.write(self.master, text.encode())

    def choose(self, index):
        distance = abs(index - self.selected)
        key = "j" if index > self.selected else "k"
        self.expected_starts += distance
        self.selected = index
        self.config.update(selected=index, pulse=0)
        write_scene(self.root, self.config)
        self.pending.clear()
        for _ in range(distance):
            self.keys(key)
            self.pump(0.15)
        self.changed_at = time.monotonic()

    def scene(self, mode):
        self.config.update(mode=mode, pulse=0)
        self.pending.clear()
        write_scene(self.root, self.config)

    def pulse(self):
        if not self.in_work or time.monotonic() - self.changed_at < 1:
            return
        self.config.update(
            sequence=self.config["sequence"] + 1, pulse=time.monotonic_ns()
        )
        self.pending[self.config["sequence"]] = self.config["pulse"]
        write_scene(self.root, self.config)

    def pump(self, seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if self.client and self.client.poll() is not None:
                raise RuntimeError("fixture display client exited unexpectedly")
            if select.select(
                [self.master], [], [], min(0.02, max(0, deadline - time.monotonic()))
            )[0]:
                data = os.read(self.master, 65536)
                if b"\x1b[6n" in data:
                    self.keys("\x1b[1;1R")
                self.rolling = (self.rolling + data)[-32768:]
                for match in MARKER.finditer(CSI.sub(b"", self.rolling)):
                    index, sequence, sent = map(int, match.groups())
                    if (
                        sequence in self.pending
                        and index == self.selected
                        and sequence not in self.observed
                    ):
                        self.latencies.append((time.monotonic_ns() - sent) / 1_000_000)
                        self.observed.add(sequence)
                        del self.pending[sequence]
            for sequence, sent in list(self.pending.items()):
                if time.monotonic_ns() - sent > 1_500_000_000:
                    self.fail("preview did not display a fixture pulse within 1500 ms")
                    del self.pending[sequence]

    def stats(self, pids):
        result = subprocess.run(
            ["ps", "-p", ",".join(map(str, pids)), "-o", "pid=,rss=,time="],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
        rows = {}
        for row in result.stdout.splitlines():
            pid, rss, cpu = row.split()
            rows[int(pid)] = (int(rss), cpu_seconds(cpu))
        return rows

    def observers(self):
        output = self.tmux(
            "list-clients", "-F", "#{client_pid}:#{client_control_mode}:#{client_name}"
        )
        return [
            (int(row.split(":", 2)[0]), row.split(":", 2)[2])
            for row in output.splitlines()
            if row.split(":", 2)[1] == "1"
        ]

    def sample(self, elapsed, expected_fault=False):
        observers = self.observers()
        self.max_observers = max(self.max_observers, len(observers))
        if len(observers) > 1:
            self.pump(0.25)
            observers = self.observers()
            if len(observers) > 1:
                self.fail("more than one preview observer remained attached")
        rows = self.stats(
            [self.app_pid, self.server_pid, self.client.pid]
            + [pid for pid, _ in observers]
        )
        if self.app_pid not in rows:
            raise RuntimeError("fixture Workbench process exited")
        self.samples.append((elapsed, rows[self.app_pid][0], rows[self.server_pid][0]))
        if (
            self.tmux(
                "display-message", "-p", "-t", "fixture", "#{window_id}:#{pane_id}"
            )
            != self.focus
        ):
            self.fail("preview changed the fixture session's focus")
        sizes = self.tmux(
            "list-panes", "-a", "-F", "#{pane_id}:#{pane_width}:#{pane_height}"
        ).splitlines()[:6]
        if not expected_fault and sizes != self.sizes:
            self.fail("preview changed an agent pane's dimensions")
        log = self.root / "transport.log"
        lines = log.read_text().splitlines() if log.exists() else []
        fresh = lines[self.last_log :]
        self.last_log = len(lines)
        polls = fresh.count("poll")
        self.polls += polls
        self.streams += fresh.count("stream")
        if self.expected_live and polls:
            self.fail("live preview invoked polling")
        if self.expected_live and not expected_fault:
            self.unexpected_reconnects = max(
                self.unexpected_reconnects, self.streams - self.expected_starts
            )
            if self.unexpected_reconnects:
                self.fail("live preview reconnected unexpectedly during normal load")
        if (
            self.expected_live
            and self.in_work
            and not expected_fault
            and len(observers) != 1
            and time.monotonic() - self.changed_at > 0.6
        ):
            self.fail("live preview observer missing during normal load")
        if (self.root / "unexpected-input").exists():
            self.fail("fixture agent received unexpected input")
        if not expected_fault:
            mode = self.config["mode"]
            for index in range(6):
                if mode == "idle" or mode == "siblings" and index == 0:
                    continue
                path = self.root / f"progress-{index}"
                if not path.exists() or time.monotonic() - float(path.read_text()) > 3:
                    self.fail("background producer stopped advancing")

    def detach(self):
        self.keys("s")
        self.in_work = False
        self.pending.clear()
        self.pump(0.6)
        if self.observers():
            self.fail("observer leaked after leaving WORK")

    def resume(self):
        self.expected_starts += 1
        self.keys("w")
        self.in_work = True
        self.changed_at = time.monotonic()
        self.pump(0.5)

    def fault(self, index):
        self.pending.clear()
        action = ("detach", "copy_mode", "respawn", "missing_pane")[index % 4]
        if action == "detach":
            for _, name in self.observers():
                self.tmux("detach-client", "-t", name)
            self.pump(0.5)
        elif action == "copy_mode":
            self.tmux("copy-mode", "-t", self.panes[0])
            self.pump(0.5)
            self.tmux("send-keys", "-t", self.panes[0], "-X", "cancel")
        elif action == "respawn":
            self.tmux(
                "respawn-pane",
                "-k",
                "-t",
                self.panes[0],
                sys.executable,
                str(HERE),
                "--feed",
                str(self.root),
                "0",
            )
            self.pump(0.5)
        elif not self.closed_pane:
            self.choose(5)
            self.pump(0.5)
            self.tmux("kill-pane", "-t", self.panes[5])
            self.closed_pane = True
            self.pump(2.5)
            screen = self.tmux("capture-pane", "-p", "-t", self.workbench)
            if "missing" not in screen.lower() or "Pane preview: fixture-5" in screen:
                self.fail("missing pane retained a stale preview")
        else:
            return
        self.faults[action] = self.faults.get(action, 0) + 1
        if action == "missing_pane":
            self.choose(0)
        deadline = time.monotonic() + 5
        while self.expected_live:
            self.pump(0.1)
            screen = self.tmux("capture-pane", "-p", "-t", self.workbench)
            if (
                len(self.observers()) == 1
                and "Pane preview: fixture-0" in screen
                and "Preview unavailable" not in screen
                and "Loading pane preview" not in screen
            ):
                break
            if time.monotonic() > deadline:
                self.fail("live preview did not recover automatically")
                break

    def run(self, duration, scene="soak"):
        started = time.monotonic()
        next_sample, next_pulse, next_progress, next_action = 0, 1, 30, 5
        phase = None
        fault_index = 0
        while time.monotonic() - started < duration:
            elapsed = time.monotonic() - started
            mode = (
                SCENES[min(4, int(elapsed / duration * 5))]
                if scene == "soak"
                else scene
            )
            if mode != phase:
                self.choose(0)
                self.scene("active" if mode in ("navigation", "lifecycle") else mode)
                phase = mode
                emit("phase", mode=mode, seconds=round(elapsed, 1))
            if elapsed >= next_action and mode == "navigation":
                if int(next_action / 5) % 3 == 0:
                    self.detach()
                    self.resume()
                elif int(next_action / 5) % 3 == 1:
                    self.choose((self.selected + 1) % 6)
                else:
                    width, height = (100, 40) if int(next_action / 5) % 2 else (240, 60)
                    self.tmux(
                        "resize-window",
                        "-t",
                        self.workbench,
                        "-x",
                        str(width),
                        "-y",
                        str(height),
                    )
                    self.changed_at = time.monotonic()
                next_action = elapsed + 5
            elif elapsed >= next_action and mode == "lifecycle":
                self.fault(fault_index)
                fault_index += 1
                next_action = elapsed + 10
            if elapsed >= next_pulse:
                self.pulse()
                next_pulse = elapsed + 3
            if elapsed >= next_sample:
                self.sample(elapsed, expected_fault=mode == "lifecycle")
                next_sample = elapsed + 5
            if elapsed >= next_progress:
                emit(
                    "progress",
                    seconds=round(elapsed, 1),
                    pulses=len(self.latencies),
                    failures=self.failures,
                )
                next_progress = elapsed + 30
            self.pump(0.05)
        self.pending.clear()
        self.detach()
        # Measure the separate server/client before quitting its application.
        rows = self.stats([self.server_pid, self.client.pid])
        server_cpu = rows[self.server_pid][1] - self.server_start
        display_cpu = rows[self.client.pid][1] - self.client_start
        self.keys("q")
        deadline = time.monotonic() + 5
        while (
            self.tmux("display-message", "-p", "-t", self.workbench, "#{pane_dead}")
            != "1"
        ):
            if time.monotonic() > deadline:
                raise RuntimeError("Workbench did not quit")
            self.pump(0.05)
        measured = time.monotonic() - started
        elapsed = time.monotonic() - self.launched_at
        timing = (self.root / "time.log").read_text()
        cpu = {
            name: float(value)
            for name, value in re.findall(
                r"^\s*(user|sys)\s+([\d.]+)\s*$", timing, re.MULTILINE
            )
        }
        if set(cpu) != {"user", "sys"}:
            raise RuntimeError(f"missing CPU accounting: {timing!r}")
        if (
            self.tmux(
                "display-message", "-p", "-t", self.workbench, "#{pane_dead_status}"
            )
            != "0"
        ):
            self.fail("Workbench exited with an error")
        warm = [row[1] for row in self.samples if row[0] >= min(30, duration / 3)]
        growth = (
            max(0, statistics.median(warm[-3:]) - statistics.median(warm[:3]))
            if len(warm) >= 6
            else None
        )
        if growth is not None and growth > 16 * 1024:
            self.fail("Workbench RSS grew more than 16 MiB after warm-up")
        latency = percentile(self.latencies, 0.95)
        if self.expected_live and latency is not None and latency > 350:
            self.fail("preview p95 pulse latency exceeded 350 ms")
        if not self.latencies:
            self.fail("no preview pulse was observed")
        app_cpu = 100 * (cpu["user"] + cpu["sys"]) / elapsed
        result = {
            "binary": self.binary,
            "binary_sha256": self.binary_hash,
            "scene": scene,
            "elapsed_seconds": round(elapsed, 2),
            "measured_seconds": round(measured, 2),
            "app_children_cpu_percent": round(app_cpu, 2),
            "tmux_cpu_percent": round(100 * server_cpu / elapsed, 2),
            "display_client_cpu_percent": round(100 * display_cpu / elapsed, 2),
            "combined_cpu_percent": round(
                app_cpu + 100 * (server_cpu + display_cpu) / elapsed, 2
            ),
            "peak_app_rss_kib": max(row[1] for row in self.samples),
            "peak_server_rss_kib": max(row[2] for row in self.samples),
            "rss_growth_kib": growth,
            "pulse_count": len(self.latencies),
            "latency_p50_ms": percentile(self.latencies, 0.5),
            "latency_p95_ms": latency,
            "latency_max_ms": max(self.latencies, default=None),
            "stream_starts": self.streams,
            "unexpected_reconnects": self.unexpected_reconnects,
            "polling_captures": self.polls,
            "max_observers": self.max_observers,
            "fault_counts": self.faults,
            "failures": self.failures,
            "failure_details": self.failure_details,
            "passed": not self.failures,
        }
        return result

    def diagnose(self, probe, duration, scene):
        # Keep the TUI drawing: a core-only idle display misses notifications
        # generated by Workbench's own pane. This diagnostic adds a second
        # observer intentionally; do not use its CPU as a stress measurement.
        self.scene(scene)
        env = dict(
            self.env,
            TMUX=f"{self.root / 'tmux.sock'},{self.server_pid},0",
            PATH=str(self.root / "bin") + os.pathsep + self.env["PATH"],
            AWB_STRESS_TMUX=self.real_tmux,
            AWB_STRESS_SOCKET=str(self.root / "tmux.sock"),
            AWB_STRESS_LOG=str(self.root / "transport.log"),
        )
        child = subprocess.Popen(
            [
                str(probe.resolve()),
                str(self.root / "state.json"),
                "fixture-0",
                str(int(duration)),
            ],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            deadline = time.monotonic() + duration + 5
            while child.poll() is None:
                if time.monotonic() > deadline:
                    raise RuntimeError("transport probe did not stop")
                self.pump(0.05)
            output, error = child.communicate(timeout=3)
            if child.returncode:
                raise RuntimeError(f"transport probe failed: {error!r}")
            result = json.loads(output)
            return {
                "scene": scene,
                "diagnostic": result,
                "passed": result["error"] is None
                and result["errors"] == 0
                and result["captures"] > 0,
            }
        finally:
            if child.poll() is None:
                child.terminate()
                child.wait(timeout=3)

    def close(self):
        forced = False
        try:
            if self.real_tmux:
                self.tmux("kill-server", check=False)
        finally:
            try:
                if self.client:
                    deadline = time.monotonic() + 3
                    # tmux flushes its terminal before exiting. Waiting without
                    # draining a full PTY can block even after the server exits.
                    while self.client.poll() is None and time.monotonic() < deadline:
                        if (
                            self.master is not None
                            and select.select([self.master], [], [], 0.02)[0]
                        ):
                            try:
                                os.read(self.master, 65536)
                            except OSError:
                                time.sleep(0.01)
                        else:
                            time.sleep(0.01)
                    if self.client.poll() is None:
                        forced = True
                        self.client.kill()
                    self.client.wait(timeout=3)
            finally:
                if self.master is not None:
                    os.close(self.master)
                    self.master = None
        return forced


def interrupted(_signal, _frame):
    raise KeyboardInterrupt


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "--feed":
        feed(Path(sys.argv[2]), int(sys.argv[3]))
        return
    if len(sys.argv) > 1 and sys.argv[1] == "--launch":
        launch(Path(sys.argv[2]), sys.argv[3])
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary", type=Path, default=HERE.parent.parent / "target/release/workbench"
    )
    parser.add_argument(
        "--duration", type=float, default=600, help="measured seconds, default 600"
    )
    parser.add_argument(
        "--scene",
        choices=("soak", "idle", "active", "siblings", "bursts"),
        default="soak",
    )
    parser.add_argument(
        "--expect-polling",
        action="store_true",
        help="measure an archived polling baseline",
    )
    parser.add_argument(
        "--report", type=Path, help="optional JSON report, must not already exist"
    )
    parser.add_argument(
        "--probe",
        type=Path,
        help="run the core-only transport diagnostic instead of UI stress",
    )
    args = parser.parse_args()
    if not math.isfinite(args.duration) or args.duration < 10:
        parser.error("--duration must be finite and at least 10 seconds")
    if args.report and args.report.exists():
        parser.error("--report must not already exist")
    if args.report and not args.report.parent.is_dir():
        parser.error("--report parent directory must exist")
    signal.signal(signal.SIGTERM, interrupted)
    with tempfile.TemporaryDirectory(prefix="awb-preview-") as directory:
        fixture = Fixture(Path(directory), args.binary, not args.expect_polling)
        result = None
        try:
            fixture.setup()
            result = (
                fixture.diagnose(
                    args.probe,
                    args.duration,
                    "bursts" if args.scene == "soak" else args.scene,
                )
                if args.probe
                else fixture.run(args.duration, args.scene)
            )
            emit("checks_complete", cleanup_complete=False, **result)
        finally:
            forced = fixture.close()
            if result is not None:
                result["display_client_forced_exit"] = forced
                result["cleanup_complete"] = True
                if forced:
                    result["passed"] = False
                    result.setdefault("failures", []).append(
                        "fixture display client required forced shutdown"
                    )
    emit("result", **result)
    if args.report:
        with args.report.open("x") as report:
            json.dump(result, report, indent=2)
            report.write("\n")
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
