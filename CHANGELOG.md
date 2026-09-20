# Changelog

## Unreleased

- Pushover sink (application token plus user key, priority by state).
- Pushbullet sink (access token).

## 0.2.0 (2026-09-19)

- Discord sink: channel webhook, or bot token plus channel id. Posting only; the bridge design is in `TODO.md`.

## 0.1.1 (2026-09-19)

- `scripts/release.sh` bumps the version, checks, tags, pushes, and publishes the GitHub release.

## 0.1.0 (2026-09-19)

First release.

- Alerts on `pane.agent_status_changed` for the configured states, with host, workspace, agent, pane, and the last lines of the pane on blocked alerts. Debounce per pane and state. Pause and resume with the `toggle` action.
- Sinks: Telegram (HTML, forum topics per agent pane or per workspace), ntfy, Slack incoming webhook, generic JSON webhook, stdout.
- Telegram bridge: a detached daemon that turns topic replies into `herdr agent prompt`, dialog options into buttons, and `/tail`, `/keys`, `/status`, `/agents` into Herdr calls. Input is accepted only from `allowed_user_ids` in the configured chat.
- Secrets in `.env` beside `config.toml`; error text never includes a URL path.
- Linux and macOS.
