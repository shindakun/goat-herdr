# goat-herdr

A Herdr plugin that alerts you when an agent needs you. Rust. One binary. Sinks are modules. Telegram and ntfy ship together in 0.1: Telegram is the rich path, ntfy proves a plain HTTP webhook sink.

## What it does

1. Herdr fires `pane.agent_status_changed`. Herdr runs `goat-herdr notify`.
2. `notify` reads `HERDR_PLUGIN_EVENT_JSON` and `HERDR_PLUGIN_CONTEXT_JSON`, builds one `Alert`, and hands it to every configured sink. The envelope's `event` field is the snake_case kind (`pane_agent_status_changed`); the dotted name is only in `HERDR_PLUGIN_EVENT`.
3. A `blocked` alert carries the last N lines of the pane (`herdr agent read --lines N`) so you can see the question.
4. Optional bridge daemon: reply in Telegram, the text goes to that agent via `herdr agent prompt`.

## Identity: which agent, which machine

Every alert answers three questions in its first line: which host, which workspace, which agent.

```text
🟥 BLOCKED claude · goat-herdr · mac-mini
pane w3:p3
```

Telegram routing uses forum topics. The chat is a supergroup with Topics on; the bot is an admin with Manage Topics. One topic per `(host, workspace, agent)`, or per `(host, workspace)` with `topics = "per-workspace"`. The workspace part drops Herdr's `[n]` ordinal prefix so a reorder does not split a project across topics. The plugin creates topics lazily with `createForumTopic`, caches `message_thread_id` in state, closes the topic when the last pane behind it closes, and reopens it when the same key returns. Telegram lets an admin bot post into a closed topic without error, so the plugin records which topics it closed and reopens them explicitly. Every `sendMessage` sets `message_thread_id`. The header line is always present, so a plain private chat works as well.

Two ways to run several hosts:

- Alerts only: one bot token shared by all hosts. Host name in the topic and header tells them apart.
- Bridge on: one bot token per host. Telegram allows one `getUpdates` consumer per token; a second host gets 409. The bot's name then names the host.

## Config

`$(herdr plugin config-dir shindakun.goat-herdr)/config.toml`:

```toml
[alerts]
statuses = ["blocked", "done"]   # which transitions fire
debounce_secs = 5                # drop flaps: same pane, same status, inside window
tail_lines = 30                  # pane output attached to blocked alerts
host_label = "mac-mini"          # default: hostname

[[sinks]]
type = "telegram"
bot_token_env = "TELEGRAM_BOT_TOKEN"   # or bot_token = "..."
chat_id = -1001234567890
topics = "per-agent"                   # per-agent | per-workspace | none
allowed_user_ids = [12345678]          # bridge accepts input only from these
bridge = true

[[sinks]]
type = "ntfy"
url = "https://ntfy.sh/steve-agents"
token_env = "NTFY_TOKEN"               # optional, Bearer auth for self-hosted
```

Secrets live in `config.toml` or `.env` in the config dir. A `*_env` name is looked up in `.env` first, then the process environment.

## Code layout

```text
goat-herdr/
  herdr-plugin.toml
  Cargo.toml
  Makefile              # check: fmt clippy test
  src/
    main.rs             # subcommands: notify, bridge, test, toggle
    herdr.rs            # env + JSON parsing; wrapper over HERDR_BIN_PATH
    alert.rs            # Alert { host, workspace, agent, pane_id, status, tail, at }
    config.rs
    state.rs            # STATE_DIR json: debounce table, pause flag, topic map, poll offset; std File::lock
    http.rs             # blocking client over ureq; test mock server
    sink/mod.rs         # trait Sink, trait Bridge, registry by `type`
    sink/telegram.rs
    sink/ntfy.rs
    sink/slack.rs
    sink/stdout.rs      # for tests and dry runs
  tests/fixtures/       # real HERDR_PLUGIN_EVENT_JSON captures
```

The sink contract:

```rust
pub trait Sink {
    fn name(&self) -> &str;
    /// Returns Sent or Skipped. A pane close arrives as Status::Closed;
    /// sinks with per-agent state clean up, the rest skip it.
    fn send(&self, alert: &Alert) -> Result<Delivery>;
}

pub trait Bridge {
    fn poll(&self, state: &mut State) -> Result<Vec<Inbound>>;  // blocks up to 30s
    fn ack(&self, inbound: &Inbound, reply: &str) -> Result<()>;
}
```

Adding a sink means one file and one match arm in `sink/mod.rs`. ntfy is a single HTTP POST and is the reference for every webhook-style sink after it.

Dependencies: `ureq` (rustls, json), `serde`, `serde_json`, `toml`. File locks use `std::fs::File::lock`; the hostname comes from `hostname(1)`. All I/O is blocking. Each hook is a short-lived process.

## Manifest

```toml
id = "shindakun.goat-herdr"
name = "Goat Herdr"
version = "0.1.0"
min_herdr_version = "0.9.0"
platforms = ["linux", "macos"]

[[build]]
command = ["cargo", "build", "--release"]

[[events]]
on = "pane.agent_status_changed"
command = ["./target/release/goat-herdr", "notify"]

[[events]]
on = "pane.closed"
command = ["./target/release/goat-herdr", "notify"]

[[startup]]
command = ["./target/release/goat-herdr", "bridge", "--detach"]

[[actions]]
id = "test"
title = "Send test alert"
command = ["./target/release/goat-herdr", "test"]

[[actions]]
id = "toggle"
title = "Toggle alerts"
command = ["./target/release/goat-herdr", "toggle"]
```

## Telegram details

- Endpoint `https://api.telegram.org/bot<token>/<method>`, JSON body.
- `sendMessage`: `chat_id`, `text`, `message_thread_id`, `parse_mode = "HTML"`, `reply_markup`. Escape `<`, `>`, `&` in agent output. Tail goes in `<pre>`.
- Limit 4096 chars per message. Truncate the tail from the top.
- 429 returns `parameters.retry_after`. Sleep it, retry once, then log and give up. One message per second per chat; 20 per minute per group.
- `createForumTopic(chat_id, name)` returns `message_thread_id`. Name limit 128 chars. `closeForumTopic`, `reopenForumTopic` for pane lifecycle; `TOPIC_NOT_MODIFIED` means already in that state and counts as success. `message thread not found` means the topic was deleted; forget it and create again.
- `getMe` at `test` time to confirm the token.
- Blocked alerts carry an inline keyboard: `y`, `n`, `Enter`, `Tail`. A `callback_query` maps to `herdr agent send-keys` or `herdr agent read`. `answerCallbackQuery` closes the spinner.

## ntfy details

- `POST <url>` with the body as the message text. Headers: `Title` (the header line), `Priority` (`urgent` for blocked, `default` for done), `Tags` (status emoji), `Authorization: Bearer <token>` when set.
- Topic name is the URL. Host and agent identity ride in `Title` and the first body line, same text as Telegram.
- One-way. ntfy has no reply path back to Herdr.

## Bridge daemon

`goat-herdr bridge --detach` forks, writes a pidfile in `STATE_DIR`, and exits so the startup hook returns. A second start finds the live pid and exits. The daemon long-polls `getUpdates` with `timeout = 30`, persists `offset`, and rejects any sender not in `allowed_user_ids`.

| Input in a topic | Action |
|---|---|
| plain text | `herdr agent prompt <pane> "<text>"` |
| `/tail [n]` | reply with `herdr agent read --lines n` |
| `/keys <k>` | `herdr agent send-keys <pane> <k>` |
| `/agents` (General topic) | `herdr agent list` |

Topic to pane: state maps `message_thread_id` to `AgentKey`, then `AgentKey` to the current pane id by matching `herdr agent list` on workspace label and agent name. The key survives restarts.

The daemon dies with the Herdr server. Startup hooks run again on a new server, so it comes back.

## Milestones

1. Done. Scaffold, `notify` with the stdout sink, hook verified in a real Herdr, fixtures captured under `tests/fixtures/`.
2. Done. Telegram sink without topics, ntfy sink, `test` sends to every sink, `toggle` pauses, debounce, tail on blocked alerts. Both sinks verified with a real blocked event and the message read back.
3. Done. Topics: lazy create, state cache, close when the last pane exits, explicit reopen. Verified against a real forum supergroup.
4. Bridge. Reply, `/tail`, inline keyboard.
5. Slack webhook sink.
6. README, CI (fmt, clippy, test, build on three OSes).

## Checks before each milestone closes

- Unit tests parse the captured fixtures.
- Telegram sink tests run against a local `TcpListener` mock and assert the request body.
- `cargo build --release` before any real-Herdr run. The linked manifest executes `target/release/goat-herdr`; `make check` builds debug only, so a stale release binary passes every test and still runs old code.
- The real path is exercised: linked plugin, real agent, real message on a phone. For each sink, the message is read back from the service, not inferred from the plugin log.

## Release checklist

Run every line. A line that cannot be run blocks the release.

1. `make check` and `cargo build --release` on a clean tree; CI green on `main`.
2. `herdr plugin unlink shindakun.goat-herdr`, then `herdr plugin install shindakun/goat-herdr --yes` from the commit being released. The install preview shows the manifest, the build runs, and `herdr plugin list` shows the plugin enabled.
3. `herdr plugin action invoke shindakun.goat-herdr.test` with every sink in the plan configured. Each message is read back from its service (Telegram `getUpdates` or the phone, ntfy `json?poll=1`).
4. A real agent, not `report-agent`: start `claude` in a pane, give it a prompt that asks a question, confirm the blocked alert arrives with the pane tail.
5. Restart the Herdr server and confirm the startup hook logs a success in `herdr plugin log list`.
6. README matches the config the release accepts: every key in `config.rs` is documented, no key in the docs is unimplemented.
7. `CHANGELOG.md` has the version and date. `version` in `Cargo.toml` and `herdr-plugin.toml` match.
8. Tag `vX.Y.Z`, push the tag, confirm the marketplace card shows the new version within an hour.

## Windows

0.1 declares `platforms = ["linux", "macos"]`. On Windows, Herdr passes a relative manifest argv to `CreateProcessW`, which resolves it against Herdr's own directory and appends no `.exe`. Windows support means a PowerShell launcher that finds the plugin root via `herdr plugin list --json` and runs the binary by absolute path (the pattern in `smarzban/herdr-file-viewer`).
