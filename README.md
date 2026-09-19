# goat-herdr

A [Herdr](https://herdr.dev) plugin that alerts you when an agent needs you. Telegram, ntfy, and Slack, with a seam for more.

Status: alerts and the two-way Telegram bridge work. Telegram with forum topics, ntfy, and Slack incoming webhooks. See [docs/PLAN.md](docs/PLAN.md).

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
topics = "per-agent"                   # none | per-agent | per-workspace
bridge = true                          # two-way: reply in a topic to prompt that agent
allowed_user_ids = [123456789]         # your Telegram user id; required for the bridge

[[sinks]]
type = "ntfy"
url = "https://ntfy.sh/your-topic"
token_env = "NTFY_TOKEN"               # optional

[[sinks]]
type = "slack"
webhook_url_env = "SLACK_WEBHOOK_URL"  # or webhook_url = "https://hooks.slack.com/services/..."
```

Put secrets in `.env` next to `config.toml`:

```sh
TELEGRAM_BOT_TOKEN=123456:abc...
SLACK_WEBHOOK_URL=https://hooks.slack.com/services/...
```

Slack setup: at api.slack.com/apps create an app from a manifest with the `incoming-webhook` bot scope, open Incoming Webhooks, add a webhook to the channel you want, and copy its URL. The URL is the credential. Slack is one-way; there is no bridge.

Telegram setup: create a bot with @BotFather, start a chat with it (or add it to a group), then get the chat id from `https://api.telegram.org/bot<token>/getUpdates` after sending it a message.

Topics need a supergroup with Topics turned on and the bot as an admin with Manage Topics. `per-agent` gives one topic per agent pane (`claude · goat-herdr · mac-mini · w3:p16`), so a reply in a topic reaches exactly that agent; `per-workspace` one per host and workspace, shared by every agent in it. The plugin creates a topic on the first alert, closes it when the pane closes, and reopens it if the same pane alerts again. `none` posts everything to the chat root.

Every alert leads with host, workspace, and agent, so one chat can carry several machines:

```text
🟥 BLOCKED claude · goat-herdr · mac-mini
pane w3:p3
<last 30 lines of the pane>
```

## Bridge

With `bridge = true` and your Telegram user id in `allowed_user_ids`, the startup hook runs a small daemon that polls the bot. In an agent's topic:

| You send | It does |
|---|---|
| plain text | prompts the agent; if the agent is waiting on a dialog, types it into the dialog's text field |
| a button under a blocked alert | picks that numbered option, or `Enter`, `Esc`, or posts the pane `Tail` |
| `/tail [n]` | posts the last n lines of the pane |
| `/keys y Enter` | sends key presses |
| `/status`, `/agents` | agent state |

The bridge log is `bridge.log` in the plugin's state directory.

## Security

The bridge lets a chat message drive a terminal on your machine, so it is strict about who it listens to.

- Only Telegram user ids listed in `allowed_user_ids` are accepted. Everyone else is logged as `ignored user <id>` and dropped: nothing reaches Herdr, nothing is replied. An empty list accepts nobody.
- Only updates from the configured `chat_id` are read. Other chats are dropped.
- Telegram user ids are not secret, so this is "only this account", not a password. Keep the group private and add only people you would hand a shell to.
- The bot token is the real credential. Anyone with it can read the group and post as the bot, and with the bot's admin rights create and close topics. Keep it in `.env` in the plugin config directory (mode 600), never in the plugin root or the repo. The plugin never writes the token to its logs or error messages; if it ever leaks, revoke it in @BotFather.
- Blocked alerts include the last lines of the agent's terminal. Whatever is on screen goes to the chat, so do not point the plugin at a group you would not paste your terminal into.
- Alerts and the bridge go over HTTPS to `api.telegram.org` and your ntfy server. Nothing else is contacted.

Actions: `shindakun.goat-herdr.test` sends a test alert to every sink; `shindakun.goat-herdr.toggle` pauses and resumes alerts; `shindakun.goat-herdr.bridge` restarts the bridge. Bind any with a `plugin_action` key in your Herdr config.

## License

MIT
