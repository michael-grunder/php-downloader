#!/usr/bin/env bash
set -euo pipefail

OWNER="michael-grunder"
REPO="php-downloader"
PROG="php-downloader"

: "${BINDIR:=$HOME/.local/bin}"
: "${VERSION:=latest}"   # "latest" or "vX.Y.Z"
: "${ASSET:=}"           # override if desired
: "${CHANNEL:=release}"  # "release" or "nightly"

usage() {
  cat <<'EOF'
Usage: install.sh [latest|nightly|VERSION]

Without arguments the script downloads the latest published release.
Pass an explicit VERSION (for example v0.2.0) to pin that release
or use "nightly" to grab the artifact from the main branch build.
EOF
}

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

extract_from_zip() {
  local archive="$1" entry="$2" dest="$3"

  if need_cmd unzip; then
    unzip -p "$archive" "$entry" >"$dest"
    return
  fi

  if need_cmd python3; then
    python3 - "$archive" "$entry" "$dest" <<'PY'
import sys
from zipfile import ZipFile

archive, entry, dest = sys.argv[1:4]
with ZipFile(archive) as zf:
    with zf.open(entry) as src, open(dest, "wb") as dst:
        dst.write(src.read())
PY
    return
  fi

  echo "error: need unzip or python3 to extract nightly artifact" >&2
  exit 1
}

if [[ $# -gt 0 ]]; then
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    nightly)
      CHANNEL="nightly"
      shift
      ;;
    latest)
      VERSION="latest"
      shift
      ;;
    v*|[0-9]*)
      VERSION="$1"
      shift
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
fi

if [[ $# -gt 0 ]]; then
  echo "error: too many arguments" >&2
  usage
  exit 1
fi

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

tmp="$(mktemp "${BINDIR}/.${PROG}.XXXXXX")"
tmp_zip="${tmp}.zip"
trap 'rm -f "$tmp" "$tmp_zip"' EXIT

if [[ "$CHANNEL" == "nightly" ]]; then
  nightly_zip="${ASSET}.zip"
  url="https://nightly.link/${OWNER}/${REPO}/workflows/release.yml/main/${nightly_zip}"
  echo "Downloading: $url"
  download "$url" "$tmp_zip"
  extract_from_zip "$tmp_zip" "$ASSET" "$tmp"
else
  if [[ "$VERSION" == "latest" ]]; then
    url="https://github.com/${OWNER}/${REPO}/releases/latest/download/${ASSET}"
  else
    url="https://github.com/${OWNER}/${REPO}/releases/download/${VERSION}/${ASSET}"
  fi

  echo "Downloading: $url"
  download "$url" "$tmp"
fi

chmod 0755 "$tmp"
mv -f "$tmp" "${BINDIR}/${PROG}"
trap - EXIT

echo "Installed: ${BINDIR}/${PROG}"

if [[ ":$PATH:" != *":$BINDIR:"* ]]; then
  echo "Note: ${BINDIR} is not on PATH."
  echo "Example: export PATH=\"${BINDIR}:\$PATH\""
fi
