# goat-herdr

A [Herdr](https://herdr.dev) plugin that alerts you when an agent needs you. Telegram and ntfy, with a seam for more.

Status: scaffold. The event hook runs and prints alerts to the plugin log. Telegram and ntfy are next. See [docs/PLAN.md](docs/PLAN.md).

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
statuses = ["blocked", "done"]
host_label = "mac-mini"

[[sinks]]
type = "stdout"
```

## License

MIT
