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

rank_github_prefixes() {
  local probe_url="$1"
  local prefix
  local url
  local result_file
  local total

  result_file="$TMP_DIR/github-prefix-speed.txt"
  : > "$result_file"

  while IFS= read -r prefix; do
    url="$(proxied_url "$prefix" "$probe_url")"
    total="$(
      curl -fL --connect-timeout 4 --max-time 12 \
        -o /dev/null \
        -w '%{time_total}' \
        "$url" 2>/dev/null || true
    )"

    if [ -n "$total" ]; then
      printf '%s\t%s\n' "$total" "$prefix" >> "$result_file"
      info "GitHub source reachable in ${total}s: ${url}"
    else
      warn "GitHub source probe failed: $url"
    fi
  done < <(github_prefixes)

  if [ -s "$result_file" ]; then
    sort -n "$result_file" | awk -F '\t' '{print $2}'
  else
    github_prefixes
  fi
}

download_with_ranked_prefixes() {
  local source_url="$1"
  local output="$2"
  local probe_url="$3"
  local prefix
  local url

  while IFS= read -r prefix; do
    url="$(proxied_url "$prefix" "$source_url")"
    info "downloading: $url"

    if curl -fL --retry 1 --connect-timeout 10 --speed-time 20 --speed-limit 10240 --max-time 300 -o "$output" "$url"; then
      return
    fi

    warn "download failed or too slow, trying next source"
  done < <(rank_github_prefixes "$probe_url")

  die "failed to download $source_url"
}

install_systemd_service_if_present() {
  if ! has_cmd systemctl || [ ! -d /run/systemd/system ]; then
    warn "systemd is not available; skipping service update"
    return
  fi

  local service_file
  local region_env
  local existing_service

  service_file="$TMP_DIR/${SERVICE_NAME}.service"
  existing_service="/etc/systemd/system/${SERVICE_NAME}.service"

  # Preserve MIHOMOT_REGION from the existing service file if present
  region_env=""
  if as_root test -f "$existing_service"; then
    region_env="$(as_root grep -E '^Environment="?MIHOMOT_REGION=' "$existing_service" 2>/dev/null | head -1 || true)"
  fi

  {
    cat <<EOF
[Unit]
Description=mihomot API server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EOF
    [ -n "$region_env" ] && printf '%s\n' "$region_env"
    cat <<EOF
ExecStart=${INSTALL_DIR}/${BIN_NAME} serve --config ${CONFIG_PATH}
Restart=on-failure
RestartSec=3
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
EOF
  } > "$service_file"

  info "updating systemd service: ${SERVICE_NAME}.service"
  as_root install -m 644 "$service_file" "/etc/systemd/system/${SERVICE_NAME}.service"
  as_root systemctl daemon-reload
  as_root systemctl enable "${SERVICE_NAME}.service" >/dev/null 2>&1 || true
  as_root systemctl restart "${SERVICE_NAME}.service"
}

install_tui_settings() {
  if ! as_root test -f "$CONFIG_PATH"; then
    warn "mihomo config not found at ${CONFIG_PATH}; skipping TUI settings"
    return
  fi

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

  secret="$(as_root awk '/^secret:/ {sub(/^secret:[[:space:]]*/, ""); sub(/[[:space:]]+#.*/, ""); sub(/[[:space:]]+$/, ""); print; exit}' "$CONFIG_PATH" | tr -d '\"' || true)"
  controller="$(as_root awk '/^external-controller:/ {sub(/^external-controller:[[:space:]]*/, ""); sub(/[[:space:]]+#.*/, ""); sub(/[[:space:]]+$/, ""); print; exit}' "$CONFIG_PATH" | tr -d '\"' || true)"
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

main() {
  [ "$(uname -s)" = "Linux" ] || die "upgrade.sh only supports Linux"

  need_cmd curl
  need_cmd tar
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
    info "upgrading ${BIN_NAME} to ${MIHOMOT_VERSION} for ${target}"
  else
    package="${BIN_NAME}-${target}"
    release_base="https://github.com/${REPO}/releases/latest/download"
    info "upgrading ${BIN_NAME} to latest release for ${target}"
  fi

  archive="$TMP_DIR/${package}.tar.gz"
  checksum="$TMP_DIR/${package}.tar.gz.sha256"

  download_with_ranked_prefixes \
    "${release_base}/${package}.tar.gz" \
    "$archive" \
    "${release_base}/${package}.tar.gz.sha256"
  download_with_ranked_prefixes \
    "${release_base}/${package}.tar.gz.sha256" \
    "$checksum" \
    "${release_base}/${package}.tar.gz.sha256"

  info "verifying checksum"
  (cd "$TMP_DIR" && sha256sum -c "${package}.tar.gz.sha256")

  tar -xzf "$archive" -C "$TMP_DIR"
  binary_path="$(find "$TMP_DIR" -type f -path "*/${BIN_NAME}" -perm -u+x | head -n 1)"
  [ -n "$binary_path" ] || die "downloaded archive did not contain an executable ${BIN_NAME}"

  if as_root test -x "${INSTALL_DIR}/${BIN_NAME}"; then
    info "current version: $(${INSTALL_DIR}/${BIN_NAME} --version 2>/dev/null || true)"
  fi

  info "installing binary to ${INSTALL_DIR}/${BIN_NAME}"
  as_root mkdir -p "$INSTALL_DIR"
  as_root install -m 755 "$binary_path" "${INSTALL_DIR}/${BIN_NAME}"
  info "new version: $(${INSTALL_DIR}/${BIN_NAME} --version 2>/dev/null || true)"

  install_systemd_service_if_present
  install_tui_settings

  info "mihomot upgraded successfully"
  printf '\nConfig preserved: %s\n' "$CONFIG_PATH"
  printf 'Check status: systemctl status %s\n' "$SERVICE_NAME"
}

main "$@"
