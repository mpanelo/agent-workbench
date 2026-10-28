# Preview stress test

The stress harness runs the real Workbench TUI with six synthetic producers on
its own tmux server. Workbench is inside the same session, so the live client
also encounters notifications from sibling panes and Workbench's output.
It does not access user sessions, change user configuration, use real agents,
or make paid calls. Panes and fixture files are removed on normal completion
and handled interruptions. Reports contain measurements, not pane transcripts.

## Run

Requires Unix, Python 3, tmux, `ps`, `pgrep`, and `/usr/bin/time`. Build before
measuring; compilation or competing workloads can distort CPU measurements.

```sh
cargo build --release --locked -p workbench-tui --features live-preview
python3 scripts/stress-preview.py --duration 600 --report target/preview-stress.json
```

The report path must be new and its parent directory must exist. Progress is
printed every 30 seconds. A failed check exits nonzero but still writes the
completed report. An infrastructure failure, such as a process crashing before
measurement completes, exits nonzero with an error instead of a partial success.

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

- No unexpected polling fallback or missing observer during normal live load.
- No stable duplicate observers, leaked observers after leaving WORK, unexpected
  agent input, or unintended focus or agent-pane dimension changes.
- Marker p95 at most 350 ms; an outstanding marker must appear within 1500 ms.
- Workbench RSS median growth at most 16 MiB after warm-up. This coarse soak check
  is not proof of leak freedom; tmux's own history memory is separate.
- Controlled observer detachment, copy mode, producer respawn, and a deleted
  selected pane must leave the UI usable. A healthy selection must reconnect.
- Workbench must quit successfully, and private resources must be cleaned up.

CPU is reported, not automatically gated. Promotion also requires interpreting
matching CPU measurements and operational recovery; a passing short scene alone
is not a promotion decision. New reports fingerprint the binary with SHA-256.

Controlled lifecycle faults may trigger fallback in the prototype; those
intervals are distinguished from normal-load failures. Reselection is used to
exercise its current recovery behavior. Removing polling would require replacing
that recovery path and testing it, not just deleting a fallback branch.

Server disconnection is also covered by the Rust test below. Killing the tmux
server that hosts a TUI necessarily destroys its terminal; the Rust test checks
the transport error independently of that host-terminal behavior.

```sh
python3 -B scripts/test-preview-stress.py
cargo test -p workbench-core preview::tests::test_tmux_preview -- --ignored --nocapture
```

The core-only diagnostic helper reports snapshot counts and transport errors
without retaining pane text. It intentionally adds another observer while the
TUI draws, so its CPU is not a comparable stress measurement:

```sh
cargo build --release --locked -p workbench-core --example preview_probe
python3 scripts/stress-preview.py --duration 20 --scene bursts \
  --probe target/release/examples/preview_probe
```

## Latest result

The full 600-second local macOS arm64 run failed the promotion gate: long-line
bursts exceeded the live connection's 4 MiB/sec traffic limit, causing unexpected
polling fallback. A separate diagnostic build recorded the exact limit error.
Normal active and busy-sibling phases had no unexpected fallback.

The run observed 174 latency probes, p95 205 ms, maximum 429 ms, peak Workbench
RSS 9552 KiB, and post-warm-up RSS growth 672 KiB. It found no observer leaks,
unexpected input, focus changes, or agent size changes. Lifecycle coverage
included three detachments, three copy-mode transitions, three producer respawns,
and one deleted selected pane. The run recorded 607 polling captures, including
normal-load fallback and deliberate fault intervals; its latency figures therefore
do not describe an uninterrupted live stream.

The long run used the original Tokio 1.53.1 binary. A shorter diagnostic run
with patched Tokio 1.53.3 reproduced the same traffic-limit failure. The Tokio
security update does not establish that the preview failure is fixed.

Separate sequential 20-second pairs used patched Tokio 1.53.3 in both builds.
The following are observed combined Workbench/children + tmux + display-client
CPU percentages of one core:

| Workload | Polling | Live |
| --- | --- | --- |
| Quiet, with latency probes | 8.1% | 2.5% |
| Quiet selection, busy siblings | 13.0% | 6.3% |
| Selected pane active | 12.6% | 11.8% |

The quiet pair was rerun after compiler/test jobs finished. Binary order varied
between scenarios. These short pairs show potential, not a stable CPU guarantee;
the burst reliability failure blocks promotion regardless of these numbers.

Live preview remains opt-in, and polling is retained. A candidate follow-up is
client-local `refresh-client -A pane:pause` for unrelated panes, including the
Workbench pane, followed by another full run and CPU comparison. The
[tmux implementation](https://github.com/tmux/tmux/blob/3.7c/control.c) distinguishes
client-local paused delivery from an off pane that can affect producer reads.
That optimization is not implemented or validated here. Safety limits are
unchanged.
