# mihomot

A TUI client for Mihomo (Clash Meta) built with Rust and Ratatui.

## Installation

### From Source

```bash
cargo install --path .
```

## Usage

Ensure Mihomo is running with external controller enabled.

If you don't have a mihomo core running, you can use docker to quick start one.

Here is an example `docker-compose.yml` file to start a mihomo core using tun mode. 

```yaml
services:
  mihomo:
    image: metacubex/mihomo:latest
    container_name: mihomo
    restart: always
    network_mode: host
    volumes:
      - ./config.yaml:/root/.config/mihomo/config.yaml
    cap_add:
      - NET_ADMIN
    devices:
      - /dev/net/tun
```

And you could edit `config.yaml` to add some proxy providers or rules.

Here is also an example:

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
    path: ./proxies/glados.yaml
    health-check:
      enable: true
      interval: 3600
      url: http://www.gstatic.com/generate_204

proxy-groups:
  - name: 🚀 Proxy
    type: select
    use:
      - MyProxies

rules:
  - GEOIP,CN,DIRECT
  - MATCH,🚀 Proxy
```

To run mihomot, just type this command to open the tui if you have installed.

```bash
mihomot
```

## Configuration

App settings are stored in `~/.config/mihomot/settings.json`.

Default configuration:
```json
{
  "base_url": "http://127.0.0.1:9090",
  "api_secret": "mihomo",
  "test_url": "https://www.google.com",
  "test_timeout": 3000
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
