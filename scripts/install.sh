#!/usr/bin/env bash

set -euo pipefail

REPO="${REMIPN_REPO:-rhslack/remipn}"
BINARY_NAME="${REMIPN_BINARY:-remipn}"
INSTALL_DIR_DEFAULT="${HOME}/.local/bin"
INSTALL_DIR="${REMIPN_INSTALL_DIR:-$INSTALL_DIR_DEFAULT}"

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Error: required command '$1' is not installed." >&2
    exit 1
  fi
}

detect_os() {
  case "$(uname -s)" in
    Darwin) echo "macos" ;;
    Linux) echo "linux" ;;
    MINGW*|MSYS*|CYGWIN*) echo "windows" ;;
    *)
      echo "Error: unsupported operating system: $(uname -s)" >&2
      exit 1
      ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64) echo "x86_64" ;;
    arm64|aarch64) echo "aarch64" ;;
    *)
      echo "Error: unsupported architecture: $(uname -m)" >&2
      exit 1
      ;;
  esac
}

fetch_latest_tag() {
  local api_url="https://api.github.com/repos/${REPO}/releases/latest"
  local tag
  tag="$({ curl -fsSL "$api_url" || true; } | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"

  if [ -z "$tag" ]; then
    echo "Error: unable to fetch latest release tag from ${api_url}" >&2
    exit 1
  fi

  printf '%s\n' "$tag"
}

main() {
  require_cmd curl

  local os arch asset_name tag download_url tmp_file
  os="$(detect_os)"
  arch="$(detect_arch)"

  if [ "$os" = "windows" ] && [ "$arch" != "x86_64" ]; then
    echo "Error: no published Windows build for architecture '${arch}'." >&2
    exit 1
  fi

  asset_name="${BINARY_NAME}-${os}-${arch}"
  if [ "$os" = "windows" ]; then
    asset_name="${asset_name}.exe"
  fi

  tag="$(fetch_latest_tag)"
  download_url="https://github.com/${REPO}/releases/download/${tag}/${asset_name}"

  if ! curl -fsI "$download_url" >/dev/null 2>&1; then
    echo "Error: no compatible asset found for ${os}/${arch} in release ${tag}." >&2
    echo "Tried: ${download_url}" >&2
    exit 1
  fi

  mkdir -p "$INSTALL_DIR"
  tmp_file="$(mktemp "${TMPDIR:-/tmp}/${BINARY_NAME}.XXXXXX")"

  echo "Downloading ${asset_name} (${tag})..."
  curl -fL "$download_url" -o "$tmp_file"

  if [ "$os" = "windows" ]; then
    mv "$tmp_file" "${INSTALL_DIR}/${BINARY_NAME}.exe"
    echo "Installed ${BINARY_NAME}.exe to ${INSTALL_DIR}"
  else
    chmod +x "$tmp_file"
    mv "$tmp_file" "${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
    echo "Installed ${BINARY_NAME} to ${INSTALL_DIR}"
  fi

  case ":$PATH:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
      echo
      echo "Add this directory to your PATH if needed:"
      echo "  ${INSTALL_DIR}"
      ;;
  esac
}

main "$@"
