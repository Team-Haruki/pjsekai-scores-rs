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
  local host_arch
  host_arch="$(dpkg --print-architecture)"

  case "$target" in
    x86_64)
      apt-get update
      apt-get install -y libfontconfig1-dev libfreetype6-dev pkg-config
      ;;
    aarch64)
      if [[ "$host_arch" == "arm64" ]]; then
        apt-get update
        apt-get install -y libfontconfig1-dev libfreetype6-dev pkg-config
      else
        for source_file in /etc/apt/sources.list /etc/apt/sources.list.d/*.list; do
          [[ -f "$source_file" ]] || continue
          sed -i -E 's|^deb (\[[^]]+\] )?http|deb [arch=amd64] http|' "$source_file"
        done
        dpkg --add-architecture arm64
        cat >/etc/apt/sources.list.d/arm64-ports.list <<'EOF'
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports jammy main restricted universe multiverse
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports jammy-updates main restricted universe multiverse
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports jammy-security main restricted universe multiverse
deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports jammy-backports main restricted universe multiverse
EOF
        apt-get update
        apt-get install -y --no-install-recommends \
          libfontconfig-dev:arm64 \
          libfreetype-dev:arm64 \
          pkg-config
        local target_lib_dir="/usr/aarch64-unknown-linux-gnu/aarch64-unknown-linux-gnu/sysroot/usr/lib"
        mkdir -p "$target_lib_dir" .cargo
        ln -sf /usr/lib/aarch64-linux-gnu/libfontconfig.so \
          "$target_lib_dir/libfontconfig.so"
        ln -sf /usr/lib/aarch64-linux-gnu/libfontconfig.so.1 \
          "$target_lib_dir/libfontconfig.so.1"
        ln -sf /usr/lib/aarch64-linux-gnu/libfreetype.so \
          "$target_lib_dir/libfreetype.so"
        ln -sf /usr/lib/aarch64-linux-gnu/libfreetype.so.6 \
          "$target_lib_dir/libfreetype.so.6"
        cat >>.cargo/config.toml <<EOF

[target.aarch64-unknown-linux-gnu]
rustflags = [
  "-L", "native=$target_lib_dir",
  "-L", "native=/usr/lib/aarch64-linux-gnu",
]
EOF
      fi
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
