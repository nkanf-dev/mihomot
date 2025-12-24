# mihomot

A TUI client for Mihomo (Clash Meta) built with Rust and Ratatui.

## Installation

### From Source

```bash
cargo install --path .
```

## Usage

Ensure Mihomo is running with external controller enabled.

```bash
mihomot
```

## Configuration

App settings are stored in `~/.config/mihomot/settings.json`.

Default configuration:
```json
{
  "base_url": "http://127.0.0.1:9090",
  "api_secret": "mihomo"
}
```

These can be configured within the application Settings view.

## Keybindings

**General**
- `q`: Quit
- `j` / `Down`: Next item
- `k` / `Up`: Previous item
- `s`: Open Settings
- `r`: Refresh data

**Main View**
- `h` / `Left`: Focus Groups list
- `l` / `Right`: Focus Proxies list
- `Enter`: Select group (in Groups) or Select proxy (in Proxies)
- `t`: Test Latency (Google)
- `i`: Show Proxy Info popup

**Settings View**
- `Esc` / `q` / `s`: Close Settings
- `Enter`: Edit value or Toggle option

**Editing**
- `Enter`: Save
- `Esc`: Cancel
