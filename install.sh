#!/bin/sh
# ticketsplease installer — downloads a prebuilt static binary from GitHub Releases.
#
#   curl -fsSL https://raw.githubusercontent.com/moderately-ai/ticketsplease/main/install.sh | sh
#
# Environment overrides:
#   TICKETSPLEASE_VERSION   tag to install (default: latest), e.g. v0.1.0
#   BIN_DIR                 install directory (default: ~/.local/bin)
#   ALIAS                   short alias to symlink (default: tkt; empty to skip)
set -eu

REPO="moderately-ai/ticketsplease"
BIN_NAME="ticketsplease"
: "${TICKETSPLEASE_VERSION:=latest}"
: "${BIN_DIR:=${HOME}/.local/bin}"
: "${ALIAS:=tkt}"

err() { printf '%s\n' "$*" >&2; }

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Darwin) os_part="apple-darwin" ;;
    Linux) os_part="unknown-linux-musl" ;;
    *) err "unsupported OS: $os"; exit 1 ;;
  esac
  case "$arch" in
    x86_64 | amd64) arch_part="x86_64" ;;
    arm64 | aarch64) arch_part="aarch64" ;;
    *) err "unsupported architecture: $arch"; exit 1 ;;
  esac
  printf '%s-%s' "$arch_part" "$os_part"
}

download() {
  # download <url> <dest>
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    err "need curl or wget"; exit 1
  fi
}

verify() {
  # verify <file> <sha256-file>
  expected="$(awk '{print $1}' <"$2")"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$1" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$1" | awk '{print $1}')"
  else
    err "warning: no sha256 tool found; skipping verification"; return 0
  fi
  if [ "$expected" != "$actual" ]; then
    err "checksum mismatch: expected $expected, got $actual"; exit 1
  fi
}

main() {
  target="$(detect_target)"
  asset="${BIN_NAME}-${target}.tar.gz"
  if [ "$TICKETSPLEASE_VERSION" = "latest" ]; then
    base="https://github.com/${REPO}/releases/latest/download"
  else
    base="https://github.com/${REPO}/releases/download/${TICKETSPLEASE_VERSION}"
  fi
  url="${base}/${asset}"

  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT

  err "Downloading ${url}"
  download "$url" "${tmp}/${asset}"
  if download "${url}.sha256" "${tmp}/${asset}.sha256" 2>/dev/null; then
    verify "${tmp}/${asset}" "${tmp}/${asset}.sha256"
  else
    err "warning: no published checksum; skipping verification"
  fi

  tar -xzf "${tmp}/${asset}" -C "$tmp"
  mkdir -p "$BIN_DIR"
  install -m 0755 "${tmp}/${BIN_NAME}" "${BIN_DIR}/${BIN_NAME}"
  if [ -n "$ALIAS" ]; then
    ln -sf "${BIN_DIR}/${BIN_NAME}" "${BIN_DIR}/${ALIAS}"
  fi
  err "Installed ${BIN_NAME}${ALIAS:+ and ${ALIAS}} to ${BIN_DIR}"

  case ":${PATH}:" in
    *":${BIN_DIR}:"*) ;;
    *) err "Note: add ${BIN_DIR} to your PATH:  export PATH=\"${BIN_DIR}:\$PATH\"" ;;
  esac
}

main "$@"
