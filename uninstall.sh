#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="mihomot"
INSTALL_DIR="${MIHOMOT_INSTALL_DIR:-/usr/local/bin}"
CONFIG_PATH="${MIHOMOT_CONFIG:-/etc/mihomo/config.yaml}"
SERVICE_NAME="${MIHOMOT_SERVICE_NAME:-mihomot}"
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

if [ "$PURGE" = true ] && has_cmd systemctl && [ -d /run/systemd/system ]; then
  info "removing systemd-resolved mihomot DNS drop-in"
  as_root rm -f /etc/systemd/resolved.conf.d/mihomot.conf
  as_root rmdir /etc/systemd/resolved.conf.d >/dev/null 2>&1 || true
  as_root systemctl restart systemd-resolved.service >/dev/null 2>&1 || true
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
  rm -rf "${HOME}/.config/mihomot"
else
  info "keeping config: $CONFIG_PATH"
  info "run with --purge to remove config and ~/.config/mihomot"
fi

info "mihomot uninstalled"
