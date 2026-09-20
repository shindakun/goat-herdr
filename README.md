# goat-herdr

A [Herdr](https://herdr.dev) plugin that alerts you when an agent needs you. Telegram, Discord, ntfy, Slack, or any JSON webhook. From Telegram you can answer the agent.

- An agent goes `blocked` or `done`: one message, with the host, workspace, agent, pane, and the last lines of its terminal.
- Telegram forum topics give each agent pane its own thread.
- Reply in that thread and the text goes to the agent. Buttons under a blocked alert answer its dialog.

## Install

```sh
herdr plugin install shindakun/goat-herdr
```

Needs `cargo`; the install step builds the binary. Linux and macOS.

## Configure

Config lives in `$(herdr plugin config-dir shindakun.goat-herdr)/config.toml`. With no file, alerts print to the plugin log (`herdr plugin log list --plugin shindakun.goat-herdr`).

```toml
[alerts]
statuses = ["blocked", "done"]   # agent states that fire; also idle, working, unknown
debounce_secs = 5                # drop a repeat of the same pane and state inside this window
tail_lines = 30                  # pane output attached to blocked alerts; 0 disables
host_label = "mac-mini"          # default: hostname

[[sinks]]
type = "telegram"
bot_token_env = "TELEGRAM_BOT_TOKEN"   # or bot_token = "123:abc"
chat_id = -1001234567890
topics = "per-agent"                   # none | per-agent | per-workspace
bridge = true                          # two-way; needs allowed_user_ids
allowed_user_ids = [123456789]         # Telegram user ids the bridge obeys

[[sinks]]
type = "ntfy"
url = "https://ntfy.sh/your-topic"
token_env = "NTFY_TOKEN"               # optional Bearer token

[[sinks]]
type = "slack"
webhook_url_env = "SLACK_WEBHOOK_URL"  # or webhook_url = "https://hooks.slack.com/services/..."

[[sinks]]
type = "webhook"
url = "https://example.com/hook"       # or url_env = "MY_HOOK_URL"

[[sinks]]
type = "discord"
webhook_url_env = "DISCORD_WEBHOOK_URL"   # or bot_token_env = "DISCORD_BOT_TOKEN" with channel_id = "..."
```

Any `*_env` key names a variable that is read from `.env` next to `config.toml` first, then from the environment. Keep secrets there:

```sh
TELEGRAM_BOT_TOKEN=123456:abc...
SLACK_WEBHOOK_URL=https://hooks.slack.com/services/...
DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/...
```

Every alert starts with the same line, so one chat can carry several machines:

```text
🟥 BLOCKED claude · goat-herdr · mac-mini
pane w3:p3
<last 30 lines of the pane>
```

## Sinks

### Telegram

Create a bot with @BotFather and put its token in `.env`. Send the bot a message, or add it to a group and post there, then read `chat.id` from `https://api.telegram.org/bot<token>/getUpdates`.

`topics` needs a supergroup with Topics turned on and the bot as an admin with Manage Topics.

- `per-agent`: one topic per agent pane, named `claude · goat-herdr · mac-mini · w3:p16`. A reply in the topic reaches that pane and no other. The topic is created on the first alert, closed when the pane closes, and reopened if the pane alerts again.
- `per-workspace`: one topic per host and workspace, shared by every agent in it.
- `none`: everything in the chat root.

`api_url` overrides `https://api.telegram.org` for a self-hosted Bot API server.

### Discord

One message per alert in a channel: bold headline, pane line, tail in a code block, within Discord's 2,000-character limit. Two ways in:

- Webhook: channel settings → Integrations → Webhooks → New Webhook → Copy Webhook URL. `webhook_url` or `webhook_url_env`. No bot needed.
- Bot: create an application at discord.com/developers, open Bot, reset the token, and put it in `.env` as `bot_token_env` names it. Invite the bot to your server with the `bot` scope and the permissions it needs (View Channel, Send Messages, Send Messages in Threads):

  ```text
  https://discord.com/oauth2/authorize?client_id=<APPLICATION_ID>&scope=bot&permissions=309237648384
  ```

  `<APPLICATION_ID>` is on the app's General Information page. Adding the app from the Developer Portal without that URL installs it with no bot member, and every post fails with `403 Missing Access`. Then set `channel_id` to the channel's id (Copy Channel ID in Discord with Developer Mode on, or the last number in the channel's URL).

Posting only. A Discord bridge is on the [TODO](TODO.md).

### ntfy

One POST per alert to the topic URL. `Title` is the headline, `Priority` is `urgent` for blocked and `default` for done, `Tags` is the status square. `token` or `token_env` adds `Authorization: Bearer` for a protected topic. One-way.

### Slack

At api.slack.com/apps create an app from a manifest with the `incoming-webhook` bot scope, open Incoming Webhooks, add a webhook to a channel, and copy the URL. One POST per alert in mrkdwn with the tail in a code block. The URL is the credential. One-way.

### Generic webhook

One JSON POST per alert, `Content-Type: application/json`. Any 2xx is delivered. Pane closes are not sent. No headers or auth; a secret rides in the URL through `url_env`. One-way.

```json
{
  "source": "goat-herdr",
  "status": "blocked",
  "host": "mac-mini",
  "workspace": "goat-herdr",
  "agent": "claude",
  "pane_id": "w3:p18",
  "headline": "BLOCKED claude · goat-herdr · mac-mini",
  "tail": "last lines of the pane, or null",
  "text": "🟥 BLOCKED claude · goat-herdr · mac-mini\npane w3:p18\n---\nlast lines of the pane"
}
```

`status` is `blocked`, `done`, `idle`, `working`, or `unknown`. `tail` is set on blocked alerts when `tail_lines` is above zero. `text` is the plain rendering ntfy and the plugin log use.

### Adding one

A sink is one file under `src/sink/` with a `name` and a `send`, plus one match arm in `src/sink/mod.rs` and a config struct. `ntfy.rs` is the template. Services with the same shape, an HTTP POST and a token: Mattermost (Slack-compatible webhook), Microsoft Teams (workflow webhook), Gotify, Pushover, Pushbullet, Matrix (a room webhook bot), and a desktop notifier (`osascript` on macOS, `notify-send` on Linux). Anything the `apprise` CLI covers can be reached by shelling out to it.

## Bridge

With `bridge = true` and your Telegram user id in `allowed_user_ids`, the startup hook runs a daemon that polls the bot. In an agent's topic:

| You send | It does |
|---|---|
| plain text | prompts the agent; when the agent is waiting on a dialog, types it into the dialog's text field |
| a button under a blocked alert | picks that numbered option, or sends `Enter` or `Esc`, or posts the pane `Tail` |
| `/tail [n]` | posts the last n lines of the pane |
| `/keys y Enter` | sends key presses |
| `/status` | this agent's row from `herdr agent list` |
| `/agents` | every live agent (works in any topic) |
| `/help` | the command list |

Blocked alerts show the dialog's numbered options as buttons. A free-text option (`Type something`, `Other`) selects the field; your next text reply fills it.

Every input and every dropped update is one line in `bridge.log` in the plugin's state directory.

## Actions

| Action | Does |
|---|---|
| `shindakun.goat-herdr.test` | sends a test alert to every sink |
| `shindakun.goat-herdr.toggle` | pauses or resumes alerts |
| `shindakun.goat-herdr.bridge` | restarts the bridge daemon |

Bind one in your Herdr config:

```toml
[[keys.command]]
key = "prefix+shift+a"
type = "plugin_action"
command = "shindakun.goat-herdr.toggle"
description = "toggle agent alerts"
```

## Security

The bridge lets a chat message drive a terminal on your machine.

- Only Telegram user ids in `allowed_user_ids` are accepted. Everyone else is logged as `ignored user <id>` and dropped: nothing reaches Herdr, nothing is replied. An empty list accepts nobody.
- Only updates from the configured `chat_id` are read.
- Telegram user ids are not secret. This is "only this account", not a password. Keep the group private and add only people you would hand a shell to.
- The bot token is the credential. Anyone with it can read the group, post as the bot, and use the bot's admin rights. Keep it in `.env` in the plugin config directory, mode 600, never in the plugin root or a repo. The plugin does not write it to logs or error messages. If it leaks, revoke it in @BotFather.
- Slack webhook URLs and secret webhook URLs are credentials in the same way. Errors name the host only, never the path.
- Blocked alerts carry the last lines of the agent's terminal. Whatever is on screen goes to every configured sink.
- Network destinations are the configured sinks: `api.telegram.org`, `discord.com`, your ntfy server, `hooks.slack.com`, your webhook URL. Nothing else.

## Develop

```sh
git clone https://github.com/shindakun/goat-herdr
cd goat-herdr
cargo build --release
herdr plugin link .
herdr plugin action invoke shindakun.goat-herdr.test
herdr plugin log list --plugin shindakun.goat-herdr
```

`make check` runs fmt, clippy, tests, `cargo audit`, and markdownlint. CI runs the same on Linux and macOS. The linked plugin executes `target/release/goat-herdr`, so rebuild release before testing in Herdr. Design and milestones: [docs/PLAN.md](docs/PLAN.md).

To release: add a `## X.Y.Z (date)` section to `CHANGELOG.md`, commit it, then `scripts/release.sh X.Y.Z`. The script bumps `Cargo.toml`, `Cargo.lock`, and `herdr-plugin.toml`, runs `make check` and a release build, commits, tags `vX.Y.Z`, pushes, and publishes the GitHub release with the changelog section as notes. The marketplace picks up the new version from the tag within about 30 minutes.

## License

MIT
