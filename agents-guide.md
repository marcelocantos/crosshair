# crosshair — agent guide

Crosshair is the convergence executor daemon for [bullseye](https://github.com/marcelocantos/bullseye) targets. It reads `bullseye.yaml` files, finds targets that carry a `strategy` block, and runs each strategy's command on a tick loop. State (last attempt, last success, consecutive failures, cooldown) is persisted in SQLite.

## When to use it

Reach for crosshair when a bullseye target's desired state can be reached by mechanical action — the kind of work a script can do, repeatedly, without human direction. Examples:

- Sync local commits to a remote (yadm dotfile push, repo mirroring).
- Re-run a deploy step until it succeeds.
- Rotate logs / clean caches on schedule.
- Anything currently driven by a launchd/cron job whose retry, timeout, and notification story you'd rather not maintain by hand.

If the target requires human judgement — design work, code review, bug investigation — it does not belong on a strategy. Leave those for `/cv` to recommend.

## Strategy block

Bullseye 0.25.0+ ships a `strategy:` field on every target. Crosshair reads (subset only — extra fields are ignored on load):

```yaml
targets:
  example:
    name: A target whose desired state can be mechanically restored
    status: identified         # achieved | set_aside skip the runner entirely
    strategy:
      command: /path/to/script # run via `sh -c` (shell idioms work)
      trigger: "cron:*/30 * * * *" # five-field cron expression
      timeout: 2m              # per-attempt ceiling; default 5m
      retry:
        max_attempts: 5        # advisory in v0.1; cooldown ladder is fixed
        backoff: exponential
```

The `command` is dispatched through `sh -c`, so pipes, env interpolation, and absolute paths all work. The strategy is responsible for being idempotent — when the target's desired state is already in place, the command should exit 0 quickly (a heartbeat, not a full re-run).

Supported triggers are:

- `cron:<five-field expression>` — run once in each matching minute.
- `every:<duration>` — run at most once per duration, measured from the prior attempt.
- `manual` — never run automatically; invoke the command outside crosshair when appropriate.

## CLI

```
crosshair run     -c <bullseye.yaml> [-c <another.yaml>] [--tick 30s] [--once]
crosshair status  -c <bullseye.yaml>
```

- `--config` / `-c` — repeatable; each YAML's strategy-bearing targets are merged.
- `--tick` — loop interval (default `30s`). It determines how often crosshair checks whether each strategy's trigger is due.
- `--once` — run a single tick and exit. Useful for cron-driven setups and tests.
- `--state` — SQLite path. Defaults to `$HOME/.local/state/crosshair/state.db`.

`crosshair status` prints one row per strategy-bearing target, joining the YAML view with the persisted state so targets that have never run yet still appear (with `—` in the timestamp columns).

## Tick loop

Each tick:

1. Reload all configured YAML files (so target edits take effect without restart).
2. Skip terminal-status targets (`achieved`, `set_aside`) and strategies whose trigger is not due.
3. Skip targets whose `cooldown_until` is in the future.
4. For the rest, run `command` under the per-attempt timeout. Capture exit, stdout, stderr.
5. Update SQLite. On failure, set the next cooldown from the backoff ladder.

Backoff ladder (indexed by *new* consecutive-failure count): 30m → 2h → 6h → 24h. A successful attempt clears `cooldown_until`; the strategy still waits for its next scheduled cron minute or interval.

## Running as a launchd KeepAlive job

Drop this at `~/Library/LaunchAgents/com.<you>.crosshair.plist`, then `launchctl bootstrap gui/$(id -u) <plist>`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.you.crosshair</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/you/.cargo/bin/crosshair</string>
        <string>run</string>
        <string>-c</string><string>/Users/you/.config/crosshair/targets.yaml</string>
        <string>--tick</string><string>30m</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>WorkingDirectory</key><string>/Users/you</string>
    <key>StandardOutPath</key><string>/Users/you/.local/var/log/crosshair.log</string>
    <key>StandardErrorPath</key><string>/Users/you/.local/var/log/crosshair.log</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
        <key>RUST_LOG</key><string>info</string>
    </dict>
</dict>
</plist>
```

`KeepAlive: true` makes restarts transparent — kill the daemon and launchd respawns it with a fresh pid. The strategy's per-attempt timeout bounds how long any single hung command can block subsequent ticks.

## State key

The SQLite store keys on the *canonical absolute path* of the YAML file plus the target ID. Moving a YAML file invalidates the prior state — crosshair will start fresh against the new path. For stable identity across renames, keep a single canonical YAML file per host or use symlinks pointing at one canonical location.

## Limits in v0.1

- **Satisfaction check is trust-the-status-field.** `achieved` / `set_aside` skip; due non-terminal strategies run their command and let it decide. A pluggable check is planned.
- **Triggers are deliberately narrow.** Only five-field `cron:`, duration-based `every:`, and `manual` are supported. Unsupported trigger kinds fail the tick rather than silently running at the wrong cadence.
- **Backoff ladder is hard-coded.** `retry.backoff` is parsed but the schedule is fixed at 30m → 2h → 6h → 24h.
- **No notifications, no agent escalation, no MCP surface.** Failures show up in `crosshair status` and the launchd log; everything past that is for later releases.

## Gotchas

- The strategy command is a shell string (`sh -c "..."`), not an `argv` list. Multi-word arguments need to be quoted inside the YAML string the same way you'd quote them at the shell.
- Absolute paths beat `$PATH` — launchd's default `PATH` is minimal. Either set `EnvironmentVariables.PATH` in the plist, or use absolute paths in the strategy command.
- The state DB and the launchd log are *not* in the same directory. State: `~/.local/state/crosshair/state.db`. Logs: wherever your plist points `StandardOutPath` (the recommended setup uses `~/.local/var/log/crosshair.log`).
- `crosshair status` reads the same `--config` files as `run`. If a target was deleted from the YAML, it disappears from `status` even if there's still a state row in SQLite.
- An attempt owns its own Unix process group. Crosshair kills that group after a timeout and when the shell exits, so commands must not background or daemonize work they expect to survive the attempt.
