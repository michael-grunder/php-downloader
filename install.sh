#!/usr/bin/env bash
set -euo pipefail

OWNER="michael-grunder"
REPO="php-downloader"
PROG="php-downloader"

: "${BINDIR:=$HOME/.local/bin}"
: "${VERSION:=latest}"   # "latest" or "vX.Y.Z"
: "${ASSET:=}"           # override if desired

need_cmd() { command -v "$1" >/dev/null 2>&1; }

download() {
  local url="$1" out="$2"

  if need_cmd curl; then
    curl -fL "$url" -o "$out"
    return
  fi

  if need_cmd wget; then
    wget -qO "$out" "$url"
    return
  fi

  echo "error: need curl or wget" >&2
  exit 1
}

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"

case "$os" in
  linux)  os="linux" ;;
  darwin) os="macos" ;;
  *)
    echo "error: unsupported OS: $(uname -s)" >&2
    exit 1
    ;;
esac

case "$arch" in
  x86_64|amd64)   arch="x86_64" ;;
  aarch64|arm64)  arch="arm64" ;;
  *)
    echo "error: unsupported arch: $(uname -m)" >&2
    exit 1
    ;;
esac

if [[ -z "$ASSET" ]]; then
  ASSET="${PROG}-${os}-${arch}"
fi

mkdir -p "$BINDIR"

if [[ "$VERSION" == "latest" ]]; then
  url="https://github.com/${OWNER}/${REPO}/releases/latest/download/${ASSET}"
else
  url="https://github.com/${OWNER}/${REPO}/releases/download/${VERSION}/${ASSET}"
fi

tmp="$(mktemp "${BINDIR}/.${PROG}.XXXXXX")"
trap 'rm -f "$tmp"' EXIT

echo "Downloading: $url"
download "$url" "$tmp"

chmod 0755 "$tmp"
mv -f "$tmp" "${BINDIR}/${PROG}"
trap - EXIT

echo "Installed: ${BINDIR}/${PROG}"

if [[ ":$PATH:" != *":$BINDIR:"* ]]; then
  echo "Note: ${BINDIR} is not on PATH."
  echo "Example: export PATH=\"${BINDIR}:\$PATH\""
fi
