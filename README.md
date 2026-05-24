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

The installer downloads the latest GitHub Release binary for your Linux architecture, verifies the `.sha256` checksum, installs `mihomot` to `/usr/local/bin`, creates `/etc/mihomo/config.yaml` if missing, configures systemd-resolved to send DNS through mihomo, and starts `mihomot.service` on systemd systems. It does not compile from source.

Set `MIHOMOT_NO_RESOLVED=1` during installation to skip the systemd-resolved DNS drop-in.

After installation, view the token with:

```bash
sudo journalctl -u mihomot -f
```

To install a specific release:

```bash
curl -fsSL https://raw.githubusercontent.com/nkanf-dev/mihomot/main/install.sh | MIHOMOT_VERSION=v0.1.0 bash
```

To upgrade an existing server install while preserving `/etc/mihomo/config.yaml`:

```bash
curl -fsSL https://raw.githubusercontent.com/nkanf-dev/mihomot/main/upgrade.sh | bash
```

For mainland China networks, use the script through a GitHub mirror. The upgrade script still probes release mirrors before downloading the larger archive:

```bash
curl -fsSL https://gh-proxy.com/https://raw.githubusercontent.com/nkanf-dev/mihomot/main/upgrade.sh | MIHOMOT_REGION=cn bash
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
ipv6: false
geox-url:
  mmdb: "https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/country.mmdb"

tun:
  enable: true
  stack: system
  auto-route: true
  auto-detect-interface: true
  dns-hijack: ["any:53"]
  route-exclude-address:
    - 0.0.0.0/8
    - 10.0.0.0/8
    - 100.64.0.0/10
    - 127.0.0.0/8
    - 169.254.0.0/16
    - 172.16.0.0/12
    - 192.168.0.0/16
    - 224.0.0.0/4
    - 240.0.0.0/4

dns:
  enable: true
  listen: 127.0.0.1:53
  enhanced-mode: fake-ip
  fake-ip-range: 198.18.0.1/16
  nameserver:
    - 223.5.5.5
    - 119.29.29.29

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
  - GEOIP,LAN,DIRECT,no-resolve
  - GEOIP,CN,DIRECT,no-resolve
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
| `-c, --config` | mihomo config.yaml path | `MIHOMOT_CONFIG`, `/etc/mihomo/config.yaml`, then `~/.config/mihomo/config.yaml` |
| `--listen` | mihomot API listen address | `0.0.0.0:9091` |
| `-p, --port` | port (overrides --listen) | `9091` |

### TUI Mode

```bash
mihomot tui                              # auto-detect from config
mihomot tui -p 9090                      # mihomo API port
mihomot tui -U http://1.2.3.4:9090 -S s  # manual endpoint & secret
mihomot tui -c /etc/mihomo/config.yaml   # server install config
```

| Flag | Description | Default |
|------|-------------|---------|
| `-U, --url` | mihomo external-controller URL | auto-detect from config |
| `-S, --secret` | mihomo API secret | auto-detect from config |
| `-c, --config` | mihomo config.yaml path | `~/.config/mihomo/config.yaml` |
| `-p, --port` | mihomo API port | - |

The install and upgrade scripts write `~/.config/mihomot/settings.json` for the user running the script (or `SUDO_USER`) with the local API URL and secret, so `mihomot tui` works even when `/etc/mihomo/config.yaml` is root-readable only.

### Temporary Tunnel

If opening TCP `9091` is inconvenient, start a temporary Cloudflare Tunnel:

```bash
sudo mihomot tunnel
```

`sudo mihomot tunnel` installs `cloudflared` into root's mihomot state directory when it is not already available, starts it in the background, reuses an existing tunnel on the next run, and prints an agent message with a `https://*.trycloudflare.com` endpoint. Use `sudo` so the command can read the root-readable `/etc/mihomo/config.yaml` secret. The endpoint is temporary: it remains useful only while the background `cloudflared` process is alive.

## HTTP API

mihomot exposes an HTTP API for config.yaml operations that mihomo's native API doesn't cover.

All `/mhmt/` endpoints require auth: `Authorization: Bearer {secret}` (same secret as mihomo).

| Method | Path | Description |
|--------|------|-------------|
| GET | `/mhmt/config/list` | List switchable mihomo YAML configs beside the active config |
| POST | `/mhmt/config/switch` | Switch active config and reload mihomo |
| GET | `/mhmt/config/raw` | Return full config.yaml |
| POST | `/mhmt/config/raw` | Replace config.yaml and reload |
| GET | `/mhmt/config/backup` | Create and return a timestamped backup |
| POST | `/mhmt/reload` | Reload mihomo and return connectivity result |
| GET | `/mhmt/status` | Status: version, mode, connection count |
| GET | `/skill.md` | AI agent skill document |

For multi-subscription setups, keep switchable mihomo YAML files in the
same directory as the active config. For example, if mihomot is running with
`/etc/mihomo/config.yaml`, place alternatives such as `/etc/mihomo/us.yaml`
and `/etc/mihomo/hk.yaml` beside it. `/mhmt/config/list` only scans that
active config directory and includes files that look like mihomo main configs,
for example YAML files containing `external-controller`, `mixed-port`,
`proxies`, `proxy-groups`, or `rules`. `/mhmt/config/switch` rejects paths
outside the active config directory, non-mihomo YAML, and configs whose
`secret` or `external-controller` are incompatible with the running mihomot
service.

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
Set `MIHOMOT_NO_RESOLVED=1` to leave systemd-resolved unchanged during installation.
Set `MIHOMOT_GITHUB_PROXY=<prefix>` to force a GitHub proxy prefix, or `MIHOMOT_GITHUB_PROXY=direct` to skip proxies.

When `MIHOMOT_GITHUB_PROXY` is not set, the installer probes the release checksum through each GitHub source first and downloads the larger release archive from the fastest reachable source.

The generated default config enables TUN, listens for DNS on `127.0.0.1:53`, routes LAN/CN traffic directly, and sends other traffic through the `Proxy` group. It excludes local, private, carrier-grade NAT, link-local, multicast, and reserved address ranges from TUN routing so SSH and server management traffic remain direct.

The installer backs up existing systemd-resolved drop-ins to `/etc/mihomot/resolved-backup` before writing `/etc/systemd/resolved.conf.d/mihomot.conf`, so system DNS queries go through mihomo and fake-ip routing can work by default. It also pre-downloads `Country.mmdb` into the mihomo config directory so the default `GEOIP,CN,DIRECT,no-resolve` rule can work on first start.

`uninstall.sh --purge` restores the saved systemd-resolved drop-ins. If no backup is available, it installs a fallback DNS drop-in with `223.5.5.5`, `119.29.29.29`, `8.8.8.8`, and `1.1.1.1` so uninstall does not leave the host unable to resolve names.

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
