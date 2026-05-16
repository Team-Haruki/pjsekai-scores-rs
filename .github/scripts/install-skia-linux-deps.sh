#!/usr/bin/env bash
set -euo pipefail

target="${1:-x86_64}"

install_rpm_deps() {
  local manager="$1"

  "$manager" install -y fontconfig-devel freetype-devel

  if ! command -v pkg-config >/dev/null 2>&1; then
    "$manager" install -y pkgconfig || "$manager" install -y pkgconf-pkg-config
  fi
}

install_apt_deps() {
  case "$target" in
    x86_64 | aarch64)
      apt-get update
      apt-get install -y libfontconfig1-dev libfreetype6-dev pkg-config
      ;;
    *)
      echo "Unsupported Skia Linux target: $target" >&2
      exit 1
      ;;
  esac
}

if command -v yum >/dev/null 2>&1; then
  install_rpm_deps yum
elif command -v dnf >/dev/null 2>&1; then
  install_rpm_deps dnf
elif command -v microdnf >/dev/null 2>&1; then
  install_rpm_deps microdnf
elif command -v apt-get >/dev/null 2>&1; then
  install_apt_deps
else
  echo "No supported package manager found for Skia Linux dependencies" >&2
  exit 1
fi

if ! command -v pkg-config >/dev/null 2>&1; then
  echo "pkg-config was not installed by the detected package manager" >&2
  exit 1
fi
