# Preview stress test

The stress harness runs the real Workbench TUI with six synthetic producers on
its own tmux server. Workbench is inside the same session, so the live client
must suppress output delivery from sibling panes and Workbench's own pane.
It does not access user sessions, change user configuration, use real agents,
or make paid calls. Panes and fixture files are removed on normal completion
and handled interruptions. Reports contain measurements, not pane transcripts.

## Run

Requires Unix, Python 3, tmux, `ps`, `pgrep`, and `/usr/bin/time`. Build before
measuring; compilation or competing workloads can distort CPU measurements.

```sh
cargo build --release --locked -p workbench-tui
python3 scripts/stress-preview.py --duration 600 --report target/preview-stress.json
```

The report path must be new and its parent directory must exist. Progress is
printed every 30 seconds. A failed check exits nonzero but still writes the
completed report. An infrastructure failure, such as a process crashing before
measurement completes, exits nonzero with an error instead of a partial success.
Only the final `result` event and report, after cleanup, establish success. The
earlier `checks_complete` event has not yet verified teardown. The harness drains
its dummy terminal during teardown; requiring forced shutdown fails the run.

For a quick check, use `--duration 60`. The full run is needed to exercise every
lifecycle action and make the post-warm-up memory check meaningful. Use
`--scene idle`, `--scene active`, `--scene siblings`, or `--scene bursts` to
isolate a workload. The idle and
busy-sibling workloads still inject occasional latency probes, so they are not
zero-output benchmarks.

To compare an explicitly archived polling binary, pass `--binary /path/to/binary`
and `--expect-polling` with the same scene and duration. Do not compare binaries
concurrently. CPU includes Workbench's waited children, the tmux server, and the
display client, relative to one core. Synthetic producer, harness, and terminal
app/GPU CPU are excluded. The server measurements include fixture queries, and
the test adapter has small command-logging overhead. Results include startup
and teardown. These are workload comparisons, not whole-machine utilization or
performance guarantees.

## Checks

The ten-minute run divides time across active output, a quiet selection with
busy siblings, Unicode/tab/long-line bursts, navigation/resizing, and lifecycle
faults. Markers start in a blank preview row and are read from actual TUI output,
not a terminal emulator. Their delay includes fixture dispatch and rendering.

The checks are fixed before a run:

- No polling invocations, unexpected live reconnections, or missing observer
  during normal load.
- No stable duplicate observers, leaked observers after leaving WORK, unexpected
  agent input, or unintended focus or agent-pane dimension changes.
- Busy background producers must keep advancing while delivery is paused.
- Marker p95 at most 350 ms; an outstanding marker must appear within 1500 ms.
- Workbench RSS median growth at most 16 MiB after warm-up. This coarse soak check
  is not proof of leak freedom; tmux's own history memory is separate.
- Controlled observer detachment, copy mode, producer respawn, and a deleted
  selected pane must leave the UI usable. A healthy selection must reconnect.
- Workbench must quit successfully, and private resources must be cleaned up.

CPU is reported, not automatically gated. Promotion also requires interpreting
matching CPU measurements and operational recovery; a passing short scene alone
is not a promotion decision. New reports fingerprint the binary with SHA-256.

Controlled lifecycle faults are distinguished from normal-load failures. Observer
detachment, copy mode, and producer respawn must recover automatically without
reselection. A deleted selected pane must clear its stale preview; selecting a
healthy registration must open a fresh live connection. Every phase must have
zero polling captures.

Server disconnection is also covered by the Rust test below. Killing the tmux
server that hosts a TUI necessarily destroys its terminal; the Rust test checks
the transport error independently of that host-terminal behavior.

```sh
python3 -B scripts/test-preview-stress.py
cargo test -p workbench-core preview::tests::test_tmux_preview -- --ignored --nocapture
```

The core-only diagnostic helper reports snapshot and transient-error counts
without retaining pane text. It intentionally adds another observer while the
TUI draws, so its CPU is not a comparable stress measurement:

```sh
cargo build --release --locked -p workbench-core --example preview_probe
python3 scripts/stress-preview.py --duration 20 --scene bursts \
  --probe target/release/examples/preview_probe
```

## Latest result

The live-only build passed the full 600-second macOS arm64 run with tmux 3.7c
and Tokio 1.53.3. Its frozen binary SHA-256 was
`449816c8ce92bc9fcaece811fa7a578ff6f93f3fe45400bd3b1e95793106543d`.

- Zero polling captures and zero unexpected reconnects.
- 179 probes: p95 130 ms, maximum 194 ms.
- Peak Workbench RSS 11056 KiB; post-warm-up median growth 464 KiB.
- No detected observer leaks, unintended input, focus changes, or pane-size changes.
- Background producers continued advancing while their delivery was paused.
- Three observer detachments, three copy-mode transitions, and three producer
  restarts recovered automatically. One deleted selected pane cleared correctly.
- Cleanup completed without forcing the dummy terminal client to exit.

An earlier qualification attempt completed its application checks but failed
fixture teardown because the dummy terminal was not drained. It was not counted
as a pass. A regression test now fills a dummy PTY with 200000 bytes and verifies
normal shutdown; the complete soak above was rerun with the corrected harness.

Separate sequential 20-second pairs compared this binary with an archived
five-FPS-active/two-FPS-idle polling build, using Tokio 1.53.3 in both. Binary
order varied between scenarios. Combined Workbench/children + tmux + display
client CPU, as percentages of one core:

| Workload | Archived polling | Live-only |
| --- | --- | --- |
| Quiet, with latency probes | 8.2% | 3.8% |
| Quiet selection, busy siblings | 12.3% | 7.4% |
| Selected pane active | 14.6% | 14.6% |

Active probe p95 was 218 ms polling versus 107 ms live. These short pairs are
local observations, not stable CPU or latency guarantees. The long soak's mixed
workload averaged 13.8% combined CPU; do not compare that average directly with
the individual short scenes.

The previous unfiltered prototype exceeded the traffic cap during long-line
bursts. The replacement pauses delivery from unrelated panes only for its own
client and keeps the existing safety limits. Enabling output and pausing must
be one command: tmux resets per-pane offsets when `no-output` changes. Current
runtime behavior is described in [status detection](status-detection.md#live-pane-preview).
Polling, its fallback path, and the opt-in feature flag have been removed.
