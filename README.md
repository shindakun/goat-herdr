# goat-herdr

A [Herdr](https://herdr.dev) plugin that alerts you when an agent needs you. Telegram and ntfy, with a seam for more.

Status: alerts work. Telegram (one chat, no topics yet) and ntfy. Two-way bridge and Telegram topics are next. See [docs/PLAN.md](docs/PLAN.md).

## Install

```sh
herdr plugin install shindakun/goat-herdr
```

Requires `cargo`; the install step builds the binary.

## Develop

```sh
git clone https://github.com/shindakun/goat-herdr
cd goat-herdr
cargo build --release
herdr plugin link .
herdr plugin action invoke shindakun.goat-herdr.test
herdr plugin log list --plugin shindakun.goat-herdr
```

`make check` runs fmt, clippy, tests, and markdownlint. CI runs the same on Linux and macOS.

## Configure

`$(herdr plugin config-dir shindakun.goat-herdr)/config.toml`. With no file, alerts print to the plugin log.

```toml
[alerts]
statuses = ["blocked", "done"]   # which agent states fire
debounce_secs = 5                # drop a repeat of the same pane and state inside this window
tail_lines = 30                  # pane output attached to blocked alerts, 0 to disable
host_label = "mac-mini"          # default: hostname

[[sinks]]
type = "telegram"
bot_token_env = "TELEGRAM_BOT_TOKEN"   # or bot_token = "123:abc"
chat_id = -1001234567890

[[sinks]]
type = "ntfy"
url = "https://ntfy.sh/your-topic"
token_env = "NTFY_TOKEN"               # optional
```

Put secrets in `.env` next to `config.toml`:

```sh
TELEGRAM_BOT_TOKEN=123456:abc...
```

Telegram setup: create a bot with @BotFather, start a chat with it (or add it to a group), then get the chat id from `https://api.telegram.org/bot<token>/getUpdates` after sending it a message.

Every alert leads with host, workspace, and agent, so one chat can carry several machines:

```text
🟥 BLOCKED claude · goat-herdr · mac-mini
pane w3:p3
<last 30 lines of the pane>
```

Actions: `shindakun.goat-herdr.test` sends a test alert to every sink; `shindakun.goat-herdr.toggle` pauses and resumes alerts. Bind either with a `plugin_action` key in your Herdr config.

## License

MIT
