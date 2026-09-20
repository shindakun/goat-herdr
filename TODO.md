# TODO

## Discord bridge

Alerts to Discord work today through a webhook or a bot token. A two-way bridge like Telegram's is possible with the bot token; the webhook cannot receive anything.

What Discord needs that Telegram does not:

- No long polling. A bot receives messages over the Gateway, a WebSocket with a heartbeat, or through an Interactions endpoint, which needs a public HTTPS URL. The Gateway is the fit for a daemon on a laptop. `tungstenite` gives a blocking WebSocket without tokio; one thread reads, one sends heartbeats at the interval the `HELLO` frame states.
- Reading message text is a privileged intent. `MESSAGE_CONTENT` has to be switched on for the app in the Developer Portal, and `IDENTIFY` has to request `GUILD_MESSAGES | MESSAGE_CONTENT`.
- Buttons are message components. The bot attaches them with `components` on the message and receives presses as `INTERACTION_CREATE` over the same Gateway. Each press must be answered within 3 s with an interaction response (type 6, deferred update) or Discord shows "interaction failed".
- Threads replace topics. `POST /channels/{id}/threads` on the alert message gives one thread per agent pane; replies in the thread carry `channel_id` of the thread, which maps back to the pane the same way `message_thread_id` does now. Threads auto-archive; posting into an archived thread unarchives it.
- Reconnect. The Gateway closes sessions; the daemon has to resume with the last sequence number or re-identify, and respect the 1000 identifies per day limit.
- Authorization is the sender's user id, same rule as `allowed_user_ids`.

Shape of the work: a `Bridge` impl for Discord in `sink/discord.rs`, a Gateway client of about 300 lines, `components` on blocked alerts, thread creation in `send`, and a `discord` cursor (session id plus sequence) in state. The dispatch in `bridge.rs` does not change.

## Slack bridge

The incoming webhook cannot receive anything. A bridge needs a Slack app with a bot token, and Socket Mode so the daemon needs no public URL.

What Slack needs:

- Two tokens. A bot token (`xoxb-`, scopes `chat:write`, `channels:history` or `groups:history` for the channel, `channels:read`) and an app-level token (`xapp-`, scope `connections:write`) for Socket Mode. Event Subscriptions on, subscribed to `message.channels` (or `message.groups` for a private channel). Interactivity on.
- Socket Mode. `apps.connections.open` with the app-level token returns a WebSocket URL. The daemon holds that socket (`tungstenite`, blocking, a ping every few seconds as Slack sends them) and reads envelopes. Every envelope must be acknowledged within 3 s by sending `{"envelope_id": "..."}` back, or Slack redelivers it. The URL expires; on `disconnect` the daemon opens a new one.
- Alerts go through `chat.postMessage` instead of the webhook, so the response carries the message `ts`. That `ts` is the thread: replies arrive as `message` events with `thread_ts` equal to it, which maps to the pane the way `message_thread_id` does now. The bridge answers with `chat.postMessage` and the same `thread_ts`.
- Buttons are a Block Kit `actions` block on the alert. A press arrives as an `interactive` envelope of type `block_actions` with the button's `action_id` and `value`; the same `k|<pane>|<key>` and `t|<pane>` values work. Acknowledge the envelope, then post the result in the thread.
- Slack sends the bot its own messages; drop events where `bot_id` is set or `user` is the bot's user id, or the daemon answers itself.
- Authorization is the sender's Slack user id (`U...`), the same rule as `allowed_user_ids`.
- Rate limits: `chat.postMessage` is about one per second per channel; 429 carries `Retry-After` in seconds.

Shape of the work: a `Bridge` impl in `sink/slack.rs` behind `bot_token_env` and `app_token_env`, a Socket Mode client of about 200 lines (open, read, ack, reconnect), `chat.postMessage` with blocks in `send`, and a `slack` entry in state mapping `thread_ts` to the pane. The dispatch in `bridge.rs` does not change.

## Other sinks

Gotify, Mattermost, Microsoft Teams, Matrix, `apprise`. Each is one file; `ntfy.rs` is the template.
