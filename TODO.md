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

## Other sinks

Gotify, Pushover, Pushbullet, Mattermost, Microsoft Teams, Matrix, a desktop notifier, `apprise`. Each is one file; `ntfy.rs` is the template.
