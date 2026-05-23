# mihomot

An AI native manager for Mihomo (Clash Meta) built with Rust.

## Installation

### One-line Install (Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/nkanf-dev/mihomot/main/install.sh | bash
```

For mainland China networks, use the same script through a GitHub mirror:

```bash
curl -fsSL https://gh-proxy.com/https://raw.githubusercontent.com/nkanf-dev/mihomot/main/install.sh | MIHOMOT_REGION=cn bash
```

The installer downloads the latest GitHub Release binary for your Linux architecture, verifies the `.sha256` checksum, installs `mihomot` to `/usr/local/bin`, creates `/etc/mihomo/config.yaml` if missing, and starts `mihomot.service` on systemd systems. It does not compile from source.

After installation, view the token with:

```bash
sudo journalctl -u mihomot -f
```

To install a specific release:

```bash
curl -fsSL https://raw.githubusercontent.com/nkanf-dev/mihomot/main/install.sh | MIHOMOT_VERSION=v0.1.0 bash
```

To uninstall:

```bash
curl -fsSL https://raw.githubusercontent.com/nkanf-dev/mihomot/main/uninstall.sh | bash
```

To uninstall and remove generated config/state:

```bash
curl -fsSL https://raw.githubusercontent.com/nkanf-dev/mihomot/main/uninstall.sh | bash -s -- --purge
```

### From Source

```bash
cargo install --path .
```

## Quick Start

Ensure you have a mihomo `config.yaml` (default: `~/.config/mihomo/config.yaml`).

If you don't have one, here is an example:

```yaml
log-level: error
external-controller: 0.0.0.0:9090
secret: mihomo
mixed-port: 7890
mode: rule

tun:
  enable: true
  stack: system
  auto-route: true
  auto-detect-interface: true
  dns-hijack: ["any:53"]

dns:
  enable: true
  enhanced-mode: fake-ip
  nameserver: [223.5.5.5, 119.29.29.29]

proxy-providers:
  MyProxies:
    type: http
    url: "https://www.example.com"
    interval: 3600
    health-check:
      enable: true
      url: http://www.gstatic.com/generate_204

proxy-groups:
  - name: Proxy
    type: select
    use:
      - MyProxies

rules:
  - MATCH,Proxy
```

Then start mihomot:

```bash
mihomot
```

mihomot will auto-detect or install the mihomo kernel, start it, generate a token, and print a message you can copy to your AI agent.

## CLI

```
mihomot [COMMAND]

Commands:
  serve  Start the mihomot API server (default)
  tui    Launch the TUI client
```

### Server Mode (default)

```bash
mihomot                                # listen on 0.0.0.0:9091
mihomot serve -p 8080                  # custom port
mihomot serve --listen 127.0.0.1:9091  # custom bind address
mihomot serve -c /path/to/config.yaml  # custom config path
```

| Flag | Description | Default |
|------|-------------|---------|
| `-c, --config` | mihomo config.yaml path | `~/.config/mihomo/config.yaml` |
| `--listen` | mihomot API listen address | `0.0.0.0:9091` |
| `-p, --port` | port (overrides --listen) | `9091` |

### TUI Mode

```bash
mihomot tui                              # auto-detect from config
mihomot tui -p 9090                      # mihomo API port
mihomot tui -U http://1.2.3.4:9090 -S s  # manual endpoint & secret
```

| Flag | Description | Default |
|------|-------------|---------|
| `-U, --url` | mihomo external-controller URL | auto-detect from config |
| `-S, --secret` | mihomo API secret | auto-detect from config |
| `-c, --config` | mihomo config.yaml path | `~/.config/mihomo/config.yaml` |
| `-p, --port` | mihomo API port | - |

## HTTP API

mihomot exposes an HTTP API for config.yaml operations that mihomo's native API doesn't cover.

All `/mhmt/` endpoints require auth: `Authorization: Bearer {secret}` (same secret as mihomo).

| Method | Path | Description |
|--------|------|-------------|
| GET | `/mhmt/config/raw` | Return full config.yaml |
| POST | `/mhmt/config/raw` | Replace config.yaml and reload |
| GET | `/mhmt/config/backup` | Create and return a timestamped backup |
| POST | `/mhmt/reload` | Reload mihomo and return connectivity result |
| GET | `/mhmt/status` | Status: version, mode, connection count |
| GET | `/skill.md` | AI agent skill document |

Mihomo native API endpoints (`/proxies`, `/rules`, `/configs`, etc.) can be called through the same mihomot address. Unknown non-`/mhmt/` paths are proxied to mihomo with the same bearer token.

## Token Format

```
mhmt_{hostname}_{base64(secret)}
```

- `mhmt_` prefix for reliable agent detection
- `{hostname}`: server hostname, used as alias
- `base64(secret)`: mihomo's API secret

The token does not include the endpoint address (the server may be behind NAT). The agent derives the endpoint from the mihomot server address it connects to.

## Mihomo Kernel Management

On startup, mihomot auto-detects or installs the mihomo kernel:

1. **Docker** — pulls `metacubex/mihomo:latest` and manages the container lifecycle. In mainland China mode, it tries verified Docker Hub mirror prefixes first, then Docker Hub direct.
2. **Local binary** — `~/.config/mihomot/mihomo` or `mihomo` in PATH
3. **Auto-download** — tries GitHub release mirrors first (for CN users), then GitHub direct. Falls back to manual download instructions on failure.

Set `MIHOMOT_REGION=cn` to force mirror proxy.
Set `MIHOMOT_MIHOMO_IMAGE=<image>` to force a specific mihomo Docker image.

The generated default config avoids `GEOIP,CN,DIRECT` because mihomo may try to download MMDB before the proxy is ready. Add GEOIP rules later after MMDB/geodata is available.

## Multi-Server

Run mihomot on each server. Each instance prints its own token. Send tokens to your agent one by one — it appends them to `~/.mihomot/servers.json` automatically.

## TUI Keybindings

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
- `t`: Test Latency
- `i`: Show Proxy Info popup

**Settings View**
- `Esc` / `q` / `s`: Close Settings
- `Enter`: Edit value or Toggle option

**Editing**
- `Enter`: Save
- `Esc`: Cancel

## Configuration

App settings are stored in `~/.config/mihomot/settings.json`.

```json
{
  "base_url": "http://127.0.0.1:9090",
  "api_secret": "mihomo",
  "test_url": "https://www.google.com",
  "test_timeout": 3000
}
```

These can be configured within the TUI Settings view.

## License

MIT
