#!/usr/bin/env python3
"""Offline smoke test: real Codex TUI/hooks + private tmux + fake loopback provider.

No user config, existing panes, authentication, or paid model calls are touched.
Requires codex-cli 0.159.3-compatible hooks, tmux, and a built workbench binary.
Trust bypass is ONLY for the generated fixture hooks in its disposable CODEX_HOME.
"""
import http.server
import fcntl
import json
import os
import pty
from pathlib import Path
import shutil
import select
import struct
import subprocess
import sys
import tempfile
import threading
import termios
import time


class Provider(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        with self.server.lock:
            if request.get("tools"):
                self.server.step += 1
                step = self.server.step
            else:
                step = 0  # Background title generation.
        if self.server.case == "question" and step == 1:
            output = [{"type": "function_call", "id": "fc_question", "call_id": "question_1",
                       "name": "request_user_input", "arguments": json.dumps({"questions": [{
                           "id": "strategy", "header": "Tests", "question": "Which test strategy?",
                           "options": [{"label": "Unit tests", "description": "Fast isolated tests."},
                                       {"label": "Integration", "description": "Test the full flow."}]}]})}]
        elif self.server.case.startswith("permission") and step == 1:
            output = [{"type": "function_call", "id": "fc_permission", "call_id": "permission_1",
                       "name": "exec_command", "arguments": json.dumps({
                           "cmd": "printf 'synthetic approval test\\n'", "sandbox_permissions": "require_escalated",
                           "justification": "Synthetic approval test"})}]
        elif self.server.case == "scroll" and step == 1:
            output = [{"type": "message", "id": "msg_history", "role": "assistant", "status": "completed",
                       "content": [{"type": "output_text", "text": "\n".join(
                           f"Synthetic history row {n}" for n in range(80)), "annotations": []}]},
                      {"type": "function_call", "id": "fc_tool", "call_id": "tool_1", "name": "exec_command",
                       "arguments": json.dumps({"cmd": "printf 'test\\n'", "login": False})}]
        else:
            output = [{"type": "message", "id": "msg_done", "role": "assistant", "status": "completed",
                       "content": [{"type": "output_text", "text": "Synthetic turn finished.", "annotations": []}]}]
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        events = [{"type": "response.created", "response": {"id": f"resp_{step}", "status": "in_progress"}}]
        events += [{"type": "response.output_item.done", "output_index": i, "item": item}
                   for i, item in enumerate(output)]
        events += [{"type": "response.completed", "response": {"id": f"resp_{step}", "status": "completed",
                   "output": output, "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}}]
        for event in events:
            self.wfile.write(("event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n").encode())
            self.wfile.flush()
            if step == 2 and event["type"] == "response.created":
                time.sleep(12)  # Quiet active turn, including after a denied approval.


def run(case, binary, codex):
    with tempfile.TemporaryDirectory(prefix="awb-codex-signals-") as temp:
        root = Path(temp).resolve()
        home = root / "home"
        home.mkdir()
        state = root / "work-items.json"
        signal_file = Path(str(state) + ".codex-signals.json")
        callback = f"'{binary}' codex-hook --state-file '{state}' 2>>'{root / 'callback-errors'}'"
        events = ["SessionStart", "SessionEnd", "UserPromptSubmit", "PreToolUse", "PostToolUse",
                  "PermissionRequest", "Stop", "Interrupt"]
        (home / "hooks.json").write_text(json.dumps({"hooks": {event: [{"hooks": [{
            "type": "command", "command": callback, "timeout": 3}]}] for event in events}}))
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
        server.case, server.step, server.lock = case, 0, threading.Lock()
        threading.Thread(target=server.serve_forever, daemon=True).start()
        socket = root / "tmux.sock"
        shadow_socket = root / "shadow.sock"
        def tmux(*args):
            return subprocess.check_output(["tmux", "-S", str(socket), *args], text=True).strip()
        env = dict(os.environ, CODEX_HOME=str(home), TERM="xterm-256color")
        # Never inherit the invoking user's server/pane identity into this fixture.
        env.pop("TMUX", None)
        env.pop("TMUX_PANE", None)
        args = [codex, "--no-daemon", "--dangerously-bypass-hook-trust", "--sandbox", "read-only",
                "--ask-for-approval", "on-request"]
        client = None
        master = None
        settings = {
            "model": '"gpt-5.4"', "model_provider": '"probe"', "web_search": '"disabled"',
            "features.hooks": "true", "features.code_mode": "false", "tui.show_tooltips": "false",
            "model_providers.probe.name": '"Offline test fixture"',
            "model_providers.probe.base_url": json.dumps(f"http://127.0.0.1:{server.server_port}/v1"),
            "model_providers.probe.wire_api": '"responses"',
            "model_providers.probe.requires_openai_auth": "false",
            "model_providers.probe.supports_websockets": "false",
            f'projects."{root}".trust_level': '"trusted"',
            "notify": json.dumps([str(binary), "codex-notify", "--state-file", str(state)]),
        }
        for key, value in settings.items():
            args.extend(["-c", key + "=" + value])
        if case != "question":
            args.append("Synthetic test")
        try:
            pane = subprocess.check_output(["tmux", "-S", str(socket), "-f", "/dev/null", "new-session",
                "-d", "-s", "fixture", "-x", "120", "-y", "35", "-c", str(root), "-P", "-F", "#{pane_id}",
                *args], env=env, text=True).strip()
            tmux("set-option", "-g", "remain-on-exit", "on")
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 120, 0, 0))
            client = subprocess.Popen(["tmux", "-S", str(socket), "attach-session", "-t", "fixture"],
                stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True)
            os.close(slave)
            def drain_client():
                while client.poll() is None:
                    if select.select([master], [], [], 0.1)[0]:
                        try:
                            data = os.read(master, 65536)
                            if b"\x1b[6n" in data:
                                os.write(master, b"\x1b[1;1R")
                        except OSError:
                            return
            threading.Thread(target=drain_client, daemon=True).start()
            def screen():
                return tmux("capture-pane", "-p", "-t", pane)
            def wait(test, label, seconds=20):
                deadline = time.monotonic() + seconds
                while time.monotonic() < deadline:
                    if "Trust this folder?" in screen():
                        tmux("send-keys", "-t", pane, "1", "Enter")  # This disposable folder only.
                        time.sleep(0.2)
                    if test():
                        return
                    time.sleep(0.1)
                errors = root / "callback-errors"
                print("CALLBACK ERRORS", errors.read_text() if errors.exists() else "none", flush=True)
                print("PANE IDENTITY", tmux("display-message", "-p", "-t", pane,
                      "#{socket_path}|#{pid}:#{start_time}|#{pane_id}|#{pane_pid}|#{pane_dead}|#{pane_current_command}"), flush=True)
                raise AssertionError(label + "\n" + screen())
            wait(lambda: signal_file.exists() or "? for shortcuts" in screen() or "Trust this folder?" in screen(), "No fixture startup")
            time.sleep(0.6)  # Folder trust can arrive after the initial composer.
            if "Trust this folder?" in screen():
                tmux("send-keys", "-t", pane, "1", "Enter")
                time.sleep(0.3)
            subprocess.check_call([str(binary), "register", "--id", "probe", "--kind", "implementation",
                "--repository", str(root), "--workspace", str(root), "--pane", pane, "--state-file", str(state)])
            list_env = dict(env, TMUX=f"{socket},{tmux('display-message', '-p', '#{pid}')},0", TMUX_PANE=pane)
            def observed():
                return subprocess.check_output([str(binary), "list", "--state-file", str(state)],
                                               env=list_env, text=True)
            def expect(status):
                text = observed()
                assert text.splitlines()[0].split("\t")[1] == status, text
                return text
            if case == "question":
                wait(lambda: "? for shortcuts" in screen(), "No fixture composer")
                tmux("send-keys", "-t", pane, "-l", "/plan")
                time.sleep(0.3)
                tmux("send-keys", "-t", pane, "Enter")
                wait(lambda: "Plan mode" in screen() or "Plan ·" in screen(), "Plan mode not entered")
                tmux("send-keys", "-t", pane, "-l", "Synthetic test")
                time.sleep(0.3)
                tmux("send-keys", "-t", pane, "Enter")
            wait(lambda: signal_file.exists(), "No bound SessionStart signal")
            wait(lambda: server.step >= 1, "No model request")
            if case == "question":
                wait(lambda: "Which test strategy?" in screen(), "No question dialog")
                print("QUESTION SCREEN\n" + screen(), flush=True)
                wait(lambda: "WAITING" in observed().splitlines()[0], "Question not detected")
                tmux("send-keys", "-t", pane, "Enter")
            elif case.startswith("permission"):
                wait(lambda: "Would you like" in screen(), "No approval dialog")
                wait(lambda: "WAITING" in observed().splitlines()[0], "Approval not detected")
                if case == "permission-interrupt":
                    tmux("send-keys", "-t", pane, "Escape")
                    wait(lambda: '"phase":"Interrupted"' in signal_file.read_text(), "No interruption signal")
                    expect("IDLE")
                    print("PASS permission-interrupt: pending approval cleared without false completion", flush=True)
                    return
                tmux("send-keys", "-t", pane, "y")
            wait(lambda: server.step >= 2, "No quiet resumed generation")
            expect("RUNNING")
            tmux("send-keys", "-t", pane, "draft first line", "S-Enter", "draft second line")
            expect("RUNNING")
            tmux("send-keys", "-t", pane, "PageUp")
            expect("RUNNING")
            tmux("resize-window", "-t", "fixture", "-x", "100", "-y", "28")
            expect("RUNNING")
            tmux("copy-mode", "-t", pane)
            expect("RUNNING")
            tmux("send-keys", "-t", pane, "-X", "cancel")
            wait(lambda: '"phase":"Complete"' in signal_file.read_text(), "No completion notification")
            expect("TURN FINISHED")
            tmux("send-keys", "-t", pane, "PageUp")
            expect("TURN FINISHED")
            # A newer callback from another server with the same %0 pane ID must
            # not shadow the first server's valid record in the shared sidecar.
            shadow_args = args if case != "question" else args + ["Synthetic shadow turn"]
            shadow = subprocess.check_output(["tmux", "-S", str(shadow_socket), "-f", "/dev/null",
                "new-session", "-d", "-s", "shadow", "-c", str(root), "-P", "-F", "#{pane_id}",
                *shadow_args], env=env, text=True).strip()
            assert shadow == pane, "Fixture must reuse the same pane ID on its second server"
            def shadow_finished():
                return any(r["binding"]["socket"] == str(shadow_socket) and r["phase"] == "Complete"
                           for r in json.loads(signal_file.read_text()))
            wait(shadow_finished, "No second-server completion signal")
            assert "Codex confirmed turn completion" in expect("TURN FINISHED")
            print(f"PASS {case}: newer signals from another server cannot shadow the bound pane", flush=True)
            print(f"PASS {case}: native callbacks, waiting/resolution, quiet generation, draft, scroll, resize, copy mode, completion", flush=True)
            tmux("respawn-pane", "-k", "-t", pane, "sleep", "60")
            expect("UNKNOWN")
            print(f"PASS {case}: replaced agent in reused pane cannot inherit native completion", flush=True)
        finally:
            subprocess.run(["tmux", "-S", str(socket), "kill-server"], capture_output=True)
            subprocess.run(["tmux", "-S", str(shadow_socket), "kill-server"], capture_output=True)
            if client is not None:
                try:
                    client.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    client.terminate()
                    client.wait(timeout=3)
            if master is not None:
                os.close(master)
            server.shutdown()


if __name__ == "__main__":
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/workbench").resolve()
    codex = shutil.which("codex")
    if not codex or not shutil.which("tmux") or not binary.exists():
        raise SystemExit("Build workbench and install Codex/tmux before running this optional test.")
    for case in sys.argv[2:] or ["scroll", "question", "permission", "permission-interrupt"]:
        run(case, binary, codex)
