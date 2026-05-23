#!/usr/bin/env bash
set -euo pipefail

REPO="${MIHOMOT_REPO:-nkanf-dev/mihomot}"
BIN_NAME="mihomot"
INSTALL_DIR="${MIHOMOT_INSTALL_DIR:-/usr/local/bin}"
CONFIG_PATH="${MIHOMOT_CONFIG:-/etc/mihomo/config.yaml}"
SERVICE_NAME="${MIHOMOT_SERVICE_NAME:-mihomot}"

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

  if is_likely_cn || ! github_direct_ok; then
    printf '%s\n' "https://gh-proxy.com/"
    printf '%s\n' "https://gh.jasonzeng.dev/"
    printf '%s\n' "https://ghfast.top/"
    printf '%s\n' "https://gh.llkk.cc/"
    printf '%s\n' ""
  else
    printf '%s\n' ""
    printf '%s\n' "https://gh-proxy.com/"
    printf '%s\n' "https://gh.jasonzeng.dev/"
    printf '%s\n' "https://ghfast.top/"
    printf '%s\n' "https://gh.llkk.cc/"
  fi
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
      return
    fi

    warn "download failed, trying next source"
  done < <(github_prefixes)

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

proxy-groups:
  - name: Proxy
    type: select
    proxies:
      - DIRECT

rules:
  - GEOIP,CN,DIRECT
  - MATCH,Proxy
EOF

  info "creating default mihomo config: $CONFIG_PATH"
  as_root mkdir -p "$config_dir"
  as_root install -m 600 "$tmp_config" "$CONFIG_PATH"
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
  if is_likely_cn; then
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

  if [ -n "${MIHOMOT_VERSION:-}" ]; then
    info "installing ${BIN_NAME} ${MIHOMOT_VERSION} for ${target}"
  else
    info "installing latest ${BIN_NAME} for ${target}"
  fi

  download_first "${release_base}/${package}.tar.gz" "$archive"
  download_first "${release_base}/${package}.tar.gz.sha256" "$checksum"

  info "verifying checksum"
  (cd "$TMP_DIR" && sha256sum -c "${package}.tar.gz.sha256")

  info "installing binary to ${INSTALL_DIR}/${BIN_NAME}"
  tar -xzf "$archive" -C "$TMP_DIR"
  binary_path="$(find "$TMP_DIR" -type f -path "*/${BIN_NAME}" -perm -u+x | head -n 1)"
  [ -n "$binary_path" ] || die "downloaded archive did not contain an executable ${BIN_NAME}"
  as_root mkdir -p "$INSTALL_DIR"
  as_root install -m 755 "$binary_path" "${INSTALL_DIR}/${BIN_NAME}"

  install_default_config
  install_systemd_service

  info "mihomot installed successfully"
  printf '\nNext steps:\n'
  printf '  systemctl status %s\n' "$SERVICE_NAME"
  printf '  sudo journalctl -u %s -f\n' "$SERVICE_NAME"
  printf '\nIf mihomot is still pulling the mihomo Docker image, wait until it finishes.\n'
  printf 'Then copy the token printed in the logs and send it to your AI agent.\n'
}

main "$@"
