#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="mihomot"
INSTALL_DIR="${MIHOMOT_INSTALL_DIR:-/usr/local/bin}"
CONFIG_PATH="${MIHOMOT_CONFIG:-/etc/mihomo/config.yaml}"
SERVICE_NAME="${MIHOMOT_SERVICE_NAME:-mihomot}"
STATE_DIR="${MIHOMOT_STATE_DIR:-/etc/mihomot}"
RESOLVED_BACKUP_DIR="${STATE_DIR}/resolved-backup"
PURGE=false

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

restore_resolved_dns() {
  if ! has_cmd systemctl || [ ! -d /run/systemd/system ]; then
    return
  fi

  if ! systemctl list-unit-files systemd-resolved.service --no-legend 2>/dev/null \
    | awk '{print $1}' \
    | grep -qx 'systemd-resolved.service'; then
    return
  fi

  local resolved_dir
  local fallback_file
  local tmp_fallback

  resolved_dir="/etc/systemd/resolved.conf.d"
  fallback_file="${resolved_dir}/fallback-dns.conf"
  tmp_fallback="$(mktemp)"

  if as_root test -e "${RESOLVED_BACKUP_DIR}/.mihomot-backup"; then
    info "restoring systemd-resolved drop-ins from ${RESOLVED_BACKUP_DIR}"
    as_root rm -rf "$resolved_dir"
    as_root mkdir -p "$resolved_dir"
    as_root sh -c "cp -a '${RESOLVED_BACKUP_DIR}/.' '${resolved_dir}/' 2>/dev/null || true"
    as_root rm -f "${resolved_dir}/.mihomot-backup" "${resolved_dir}/mihomot.conf"
    as_root rm -rf "$RESOLVED_BACKUP_DIR"
  else
    warn "no mihomot DNS backup found; installing fallback systemd-resolved DNS"
    cat > "$tmp_fallback" <<EOF
[Resolve]
DNS=223.5.5.5 119.29.29.29 8.8.8.8 1.1.1.1
FallbackDNS=223.5.5.5 119.29.29.29 8.8.8.8 1.1.1.1
DNSStubListener=yes
EOF
    as_root mkdir -p "$resolved_dir"
    as_root install -m 644 "$tmp_fallback" "$fallback_file"
  fi

  rm -f "$tmp_fallback"
  as_root rm -f "${resolved_dir}/mihomot.conf"
  as_root systemctl restart systemd-resolved.service >/dev/null 2>&1 || true
  if has_cmd resolvectl; then
    as_root resolvectl flush-caches >/dev/null 2>&1 || true
  fi
}

usage() {
  cat <<EOF
Usage: uninstall.sh [--purge]

Options:
  --purge    Also remove ${CONFIG_PATH} and ~/.config/mihomot.
  -h, --help Show this help.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --purge)
      PURGE=true
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
  shift
done

if has_cmd systemctl && [ -d /run/systemd/system ]; then
  info "stopping ${SERVICE_NAME}.service"
  as_root systemctl disable --now "${SERVICE_NAME}.service" >/dev/null 2>&1 || true

  info "removing systemd service"
  as_root rm -f "/etc/systemd/system/${SERVICE_NAME}.service"
  as_root systemctl daemon-reload
  as_root systemctl reset-failed "${SERVICE_NAME}.service" >/dev/null 2>&1 || true
else
  warn "systemd is not available; skipping service removal"
fi

if [ "$PURGE" = true ]; then
  restore_resolved_dns
fi

info "removing ${INSTALL_DIR}/${BIN_NAME}"
as_root rm -f "${INSTALL_DIR}/${BIN_NAME}"

if has_cmd docker; then
  info "removing mihomo docker container if it exists"
  as_root docker rm -f mihomo >/dev/null 2>&1 || true
fi

if [ "$PURGE" = true ]; then
  info "purging config and mihomot state"
  as_root rm -f "$CONFIG_PATH"
  as_root rmdir "$(dirname "$CONFIG_PATH")" >/dev/null 2>&1 || true
  as_root rmdir "$STATE_DIR" >/dev/null 2>&1 || true
  rm -rf "${HOME}/.config/mihomot"
else
  info "keeping config: $CONFIG_PATH"
  info "run with --purge to remove config and ~/.config/mihomot"
fi

info "mihomot uninstalled"
