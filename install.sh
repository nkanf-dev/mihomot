#!/usr/bin/env bash
set -euo pipefail

REPO="${MIHOMOT_REPO:-nkanf-dev/mihomot}"
BIN_NAME="mihomot"
INSTALL_DIR="${MIHOMOT_INSTALL_DIR:-/usr/local/bin}"
CONFIG_PATH="${MIHOMOT_CONFIG:-/etc/mihomo/config.yaml}"
SERVICE_NAME="${MIHOMOT_SERVICE_NAME:-mihomot}"
DETECTED_REGION="${MIHOMOT_REGION:-}"
STATE_DIR="${MIHOMOT_STATE_DIR:-/etc/mihomot}"
RESOLVED_BACKUP_DIR="${STATE_DIR}/resolved-backup"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

info() {
  printf '\033[1;34m==>\033[0m %s\n' "$*" >&2
}

warn() {
  printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2
}

die() {
  printf '\033[1;31merror:\033[0m %s\n' "$*" >&2
  exit 1
}

has_cmd() {
  command -v "$1" >/dev/null 2>&1
}

as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
  elif has_cmd sudo; then
    sudo "$@"
  else
    die "this step needs root; please install sudo or run the script as root"
  fi
}

need_cmd() {
  has_cmd "$1" || die "missing required command: $1"
}

detect_target() {
  case "$(uname -m)" in
    x86_64 | amd64)
      printf '%s\n' "x86_64-unknown-linux-gnu"
      ;;
    aarch64 | arm64)
      printf '%s\n' "aarch64-unknown-linux-gnu"
      ;;
    *)
      die "unsupported architecture: $(uname -m)"
      ;;
  esac
}

is_likely_cn() {
  case "${MIHOMOT_REGION:-}" in
    cn | CN | china | China)
      return 0
      ;;
    global | GLOBAL | direct | DIRECT)
      return 1
      ;;
  esac

  case "${TZ:-}" in
    *Shanghai* | *Chongqing* | *PRC*)
      return 0
      ;;
  esac

  case "${LANG:-}${LC_ALL:-}${LC_CTYPE:-}" in
    *zh_CN*)
      return 0
      ;;
  esac

  return 1
}

github_direct_ok() {
  curl -fsSIL --connect-timeout 4 --max-time 8 https://github.com/ >/dev/null 2>&1
}

github_prefixes() {
  if [ -n "${MIHOMOT_GITHUB_PROXY:-}" ]; then
    if [ "$MIHOMOT_GITHUB_PROXY" = "direct" ]; then
      printf '%s\n' ""
    else
      printf '%s\n' "$MIHOMOT_GITHUB_PROXY"
      printf '%s\n' ""
    fi
    return
  fi

  printf '%s\n' "https://gh-proxy.com/"
  printf '%s\n' "https://gh.jasonzeng.dev/"
  printf '%s\n' "https://ghfast.top/"
  printf '%s\n' "https://gh.llkk.cc/"
  printf '%s\n' ""
}

proxied_url() {
  local prefix="$1"
  local url="$2"

  if [ -z "$prefix" ]; then
    printf '%s\n' "$url"
  else
    printf '%s%s\n' "$prefix" "$url"
  fi
}

download_first() {
  local source_url="$1"
  local output="$2"
  local prefix
  local url

  while IFS= read -r prefix; do
    url="$(proxied_url "$prefix" "$source_url")"
    info "downloading: $url"

    if curl -fL --retry 2 --connect-timeout 10 --max-time 300 -o "$output" "$url"; then
      if [ -n "$prefix" ] && [ -z "${MIHOMOT_REGION:-}" ]; then
        DETECTED_REGION="cn"
      fi
      return
    fi

    warn "download failed, trying next source"
  done < <(github_prefixes)

  die "failed to download $source_url"
}

rank_github_prefixes() {
  local probe_url="$1"
  local output_file="$2"
  local prefix
  local url
  local total

  : > "$output_file"

  while IFS= read -r prefix; do
    url="$(proxied_url "$prefix" "$probe_url")"
    total="$(
      curl -fL --connect-timeout 4 --max-time 12 \
        -o /dev/null \
        -w '%{time_total}' \
        "$url" 2>/dev/null || true
    )"

    if [ -n "$total" ]; then
      printf '%s\t%s\n' "$total" "$prefix" >> "$output_file"
      info "GitHub source reachable in ${total}s: ${url}"
      if [ -n "$prefix" ] && [ -z "${MIHOMOT_REGION:-}" ]; then
        DETECTED_REGION="cn"
      fi
    else
      warn "GitHub source probe failed: $url"
    fi
  done < <(github_prefixes)

  if [ -s "$output_file" ]; then
    sort -n "$output_file" | awk -F '\t' '{print $2}' > "${output_file}.ranked"
    mv "${output_file}.ranked" "$output_file"
  else
    github_prefixes > "$output_file"
  fi
}

download_with_ranked_prefixes() {
  local source_url="$1"
  local output="$2"
  local ranked_file="$3"
  local prefix
  local url

  while IFS= read -r prefix; do
    url="$(proxied_url "$prefix" "$source_url")"
    info "downloading: $url"

    if curl -fL --retry 1 --connect-timeout 10 --speed-time 20 --speed-limit 10240 --max-time 300 -o "$output" "$url"; then
      return
    fi

    warn "download failed or too slow, trying next source"
  done < "$ranked_file"

  die "failed to download $source_url"
}

generate_secret() {
  if has_cmd openssl; then
    openssl rand -hex 16
  elif [ -r /proc/sys/kernel/random/uuid ]; then
    tr -d '-' < /proc/sys/kernel/random/uuid
  else
    date +%s%N | sha256sum | awk '{print $1}'
  fi
}

install_default_config() {
  if as_root test -f "$CONFIG_PATH"; then
    info "mihomo config already exists: $CONFIG_PATH"
    return
  fi

  local config_dir
  local secret
  local tmp_config

  config_dir="$(dirname "$CONFIG_PATH")"
  secret="$(generate_secret)"
  tmp_config="$TMP_DIR/config.yaml"

  cat > "$tmp_config" <<EOF
log-level: error
external-controller: 0.0.0.0:9090
secret: ${secret}
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

proxy-groups:
  - name: Proxy
    type: select
    proxies:
      - DIRECT

rules:
  - GEOIP,LAN,DIRECT,no-resolve
  - GEOIP,CN,DIRECT,no-resolve
  - MATCH,Proxy
EOF

  info "creating default mihomo config: $CONFIG_PATH"
  as_root mkdir -p "$config_dir"
  as_root install -m 600 "$tmp_config" "$CONFIG_PATH"
}

download_geoip_database() {
  local config_dir
  local output
  local tmp_geoip
  local urls
  local url

  config_dir="$(dirname "$CONFIG_PATH")"
  output="${config_dir}/Country.mmdb"
  tmp_geoip="$TMP_DIR/Country.mmdb"
  urls="
https://testingcf.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/country.mmdb
https://cdn.jsdelivr.net/gh/MetaCubeX/meta-rules-dat@release/country.mmdb
https://gh-proxy.com/https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/country.mmdb
https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/country.mmdb
"

  if as_root test -s "$output"; then
    info "GeoIP database already exists: $output"
    return
  fi

  while IFS= read -r url; do
    [ -n "$url" ] || continue
    info "downloading GeoIP database: $url"
    if curl -fL --retry 2 --connect-timeout 10 --max-time 180 -o "$tmp_geoip" "$url"; then
      as_root mkdir -p "$config_dir"
      as_root install -m 644 "$tmp_geoip" "$output"
      info "GeoIP database installed: $output"
      return
    fi
    warn "GeoIP database download failed, trying next source"
  done <<EOF
$urls
EOF

  warn "failed to download GeoIP database; GEOIP,CN rules may fail until mihomo can download MMDB"
}

fix_legacy_cn_geoip_rule() {
  if ! is_likely_cn && [ "$DETECTED_REGION" != "cn" ]; then
    return
  fi

  if ! as_root test -f "$CONFIG_PATH"; then
    return
  fi

  if ! as_root grep -q '^[[:space:]]*-[[:space:]]*GEOIP,CN,DIRECT[[:space:]]*$' "$CONFIG_PATH"; then
    return
  fi

  if as_root test -s "$(dirname "$CONFIG_PATH")/Country.mmdb"; then
    info "keeping GEOIP,CN,DIRECT because GeoIP database is available"
    return
  fi

  local backup_path
  backup_path="${CONFIG_PATH}.bak.$(date +%Y%m%d%H%M%S)"

  warn "removing GEOIP,CN,DIRECT from ${CONFIG_PATH}; it can block first start when MMDB download is unavailable"
  as_root cp "$CONFIG_PATH" "$backup_path"
  as_root sed -i '/^[[:space:]]*-[[:space:]]*GEOIP,CN,DIRECT[[:space:]]*$/d' "$CONFIG_PATH"
  info "backup saved to ${backup_path}"
}

install_resolved_dns() {
  if [ "${MIHOMOT_NO_RESOLVED:-}" = "1" ]; then
    info "skipping systemd-resolved DNS setup because MIHOMOT_NO_RESOLVED=1"
    return
  fi

  if ! has_cmd systemctl || [ ! -d /run/systemd/system ]; then
    warn "systemd is not available; skipping systemd-resolved DNS setup"
    return
  fi

  if ! systemctl list-unit-files systemd-resolved.service --no-legend 2>/dev/null \
    | awk '{print $1}' \
    | grep -qx 'systemd-resolved.service'; then
    warn "systemd-resolved is not installed; skipping DNS setup"
    return
  fi

  local resolved_dir
  local resolved_file
  local tmp_resolved

  resolved_dir="/etc/systemd/resolved.conf.d"
  resolved_file="${resolved_dir}/mihomot.conf"
  tmp_resolved="$TMP_DIR/mihomot-resolved.conf"

  if ! as_root test -e "${RESOLVED_BACKUP_DIR}/.mihomot-backup"; then
    info "backing up systemd-resolved drop-ins to ${RESOLVED_BACKUP_DIR}"
    as_root rm -rf "$RESOLVED_BACKUP_DIR"
    as_root mkdir -p "$RESOLVED_BACKUP_DIR"
    if as_root test -d "$resolved_dir"; then
      as_root sh -c "cp -a '${resolved_dir}/.' '${RESOLVED_BACKUP_DIR}/' 2>/dev/null || true"
    fi
    as_root sh -c "date -u +%Y-%m-%dT%H:%M:%SZ > '${RESOLVED_BACKUP_DIR}/.mihomot-backup'"
  fi

  cat > "$tmp_resolved" <<EOF
[Resolve]
DNS=127.0.0.1
Domains=~.
DNSStubListener=yes
EOF

  info "configuring systemd-resolved to send DNS through mihomo"
  as_root mkdir -p "$resolved_dir"
  as_root install -m 644 "$tmp_resolved" "$resolved_file"
  as_root systemctl enable --now systemd-resolved.service >/dev/null 2>&1 || true
  as_root systemctl restart systemd-resolved.service
}

install_systemd_service() {
  if ! has_cmd systemctl || [ ! -d /run/systemd/system ]; then
    warn "systemd is not available; start manually with: ${INSTALL_DIR}/${BIN_NAME} serve --config ${CONFIG_PATH}"
    return
  fi

  local service_file
  local region_env

  service_file="$TMP_DIR/${SERVICE_NAME}.service"
  region_env=""
  if is_likely_cn || [ "$DETECTED_REGION" = "cn" ]; then
    region_env="Environment=MIHOMOT_REGION=cn"
  fi

  cat > "$service_file" <<EOF
[Unit]
Description=mihomot API server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
${region_env}
ExecStart=${INSTALL_DIR}/${BIN_NAME} serve --config ${CONFIG_PATH}
Restart=on-failure
RestartSec=3
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
EOF

  info "installing systemd service: ${SERVICE_NAME}.service"
  as_root install -m 644 "$service_file" "/etc/systemd/system/${SERVICE_NAME}.service"
  as_root systemctl daemon-reload
  as_root systemctl enable --now "${SERVICE_NAME}.service"
}

install_tui_settings() {
  local target_user
  local target_home
  local settings_dir
  local settings_file
  local tmp_settings
  local secret
  local controller
  local port

  target_user="${SUDO_USER:-$(id -un)}"
  if [ "$target_user" = "root" ]; then
    target_home="/root"
  else
    target_home="$(getent passwd "$target_user" 2>/dev/null | awk -F: '{print $6}')"
  fi
  [ -n "$target_home" ] || target_home="${HOME:-/root}"

  secret="$(as_root awk -F ': *' '/^secret:/ {print $2; exit}' "$CONFIG_PATH" | tr -d '\"' || true)"
  controller="$(as_root awk -F ': *' '/^external-controller:/ {print $2; exit}' "$CONFIG_PATH" | tr -d '\"' || true)"
  port="${controller##*:}"
  case "$port" in
    '' | *[!0-9]*)
      port="9090"
      ;;
  esac

  settings_dir="${target_home}/.config/mihomot"
  settings_file="${settings_dir}/settings.json"
  tmp_settings="$TMP_DIR/settings.json"

  cat > "$tmp_settings" <<EOF
{
  "base_url": "http://127.0.0.1:${port}",
  "api_secret": "${secret}",
  "test_url": "https://www.google.com",
  "test_timeout": 3000
}
EOF

  info "writing TUI settings for ${target_user}: ${settings_file}"
  as_root mkdir -p "$settings_dir"
  as_root install -m 600 "$tmp_settings" "$settings_file"
  if [ "$target_user" != "root" ]; then
    as_root chown -R "${target_user}:${target_user}" "$settings_dir" 2>/dev/null || true
  fi
}

print_agent_instructions() {
  if ! has_cmd journalctl || ! has_cmd systemctl || [ ! -d /run/systemd/system ]; then
    return
  fi

  info "waiting for mihomot agent instructions"
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    if as_root journalctl -u "${SERVICE_NAME}.service" --since "2 minutes ago" -n 200 --no-pager \
      | awk '
        /━━━━━━━━/ && !capture {last_sep=$0}
        /把这段话发给你的 AI agent:/ {
          block = ""
          if (last_sep) block = last_sep "\n"
          capture=1
          token=0
        }
        capture {block = block $0 "\n"}
        capture && /token:/ {token=1}
        capture && token && /━━━━━━━━/ {
          last_block=block
          seen=1
          capture=0
          token=0
          block=""
        }
        END {
          if (seen) {
            printf "%s", last_block
            exit 0
          }
          exit 1
        }
      '; then
      return
    fi
    sleep 1
  done

  warn "could not find agent instructions in journal yet"
  printf 'View them with: sudo journalctl -u %s -n 120 --no-pager\n' "$SERVICE_NAME"
}

main() {
  [ "$(uname -s)" = "Linux" ] || die "install.sh only supports Linux"

  need_cmd curl
  need_cmd tar
  need_cmd sed
  need_cmd awk
  need_cmd sha256sum

  local target
  local package
  local release_base
  local archive
  local checksum
  local binary_path
  local ranked_prefixes

  target="$(detect_target)"
  if [ -n "${MIHOMOT_VERSION:-}" ]; then
    package="${BIN_NAME}-${MIHOMOT_VERSION}-${target}"
    release_base="https://github.com/${REPO}/releases/download/${MIHOMOT_VERSION}"
  else
    package="${BIN_NAME}-${target}"
    release_base="https://github.com/${REPO}/releases/latest/download"
  fi
  archive="$TMP_DIR/${package}.tar.gz"
  checksum="$TMP_DIR/${package}.tar.gz.sha256"
  ranked_prefixes="$TMP_DIR/github-prefixes-ranked.txt"

  if [ -n "${MIHOMOT_VERSION:-}" ]; then
    info "installing ${BIN_NAME} ${MIHOMOT_VERSION} for ${target}"
  else
    info "installing latest ${BIN_NAME} for ${target}"
  fi

  rank_github_prefixes "${release_base}/${package}.tar.gz.sha256" "$ranked_prefixes"
  download_with_ranked_prefixes "${release_base}/${package}.tar.gz" "$archive" "$ranked_prefixes"
  download_with_ranked_prefixes "${release_base}/${package}.tar.gz.sha256" "$checksum" "$ranked_prefixes"

  info "verifying checksum"
  (cd "$TMP_DIR" && sha256sum -c "${package}.tar.gz.sha256")

  info "installing binary to ${INSTALL_DIR}/${BIN_NAME}"
  tar -xzf "$archive" -C "$TMP_DIR"
  binary_path="$(find "$TMP_DIR" -type f -path "*/${BIN_NAME}" -perm -u+x | head -n 1)"
  [ -n "$binary_path" ] || die "downloaded archive did not contain an executable ${BIN_NAME}"
  as_root mkdir -p "$INSTALL_DIR"
  as_root install -m 755 "$binary_path" "${INSTALL_DIR}/${BIN_NAME}"

  install_default_config
  download_geoip_database
  fix_legacy_cn_geoip_rule
  install_systemd_service
  install_resolved_dns
  install_tui_settings

  info "mihomot installed successfully"
  print_agent_instructions
  printf '\nLocal TUI: mihomot tui\n'
  printf 'Temporary tunnel: sudo mihomot tunnel\n'
  printf '  Use this when TCP 9091 is not open; the trycloudflare endpoint is temporary.\n'
  printf '\nStatus: systemctl status %s\n' "$SERVICE_NAME"
}

main "$@"
