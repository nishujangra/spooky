#!/usr/bin/env bash
set -euo pipefail

# ---------------------------------------------------------------------------
# make-deb.sh — build a Debian .deb package for spooky
#
# Usage:
#   ./make-deb.sh [--version <ver>] [--arch <arch>] [--skip-build]
#
#   --version     override package version (default: read from Cargo.toml)
#   --arch        target architecture: amd64 | arm64  (default: host arch)
#   --skip-build  skip `cargo build --release`, use existing target/release/spooky
# ---------------------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

# ---- defaults --------------------------------------------------------------
PKG_NAME="spooky"
PKG_VERSION="0.4.0-beta"
PKG_ARCH="amd64"
SKIP_BUILD=0

# ---- parse args ------------------------------------------------------------
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) PKG_VERSION="$2"; shift 2 ;;
    --arch)    PKG_ARCH="$2";    shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done

# ---- resolve version -------------------------------------------------------
if [[ -z "$PKG_VERSION" ]]; then
  PKG_VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/version *= *"\(.*\)"/\1/')
fi

# ---- resolve architecture --------------------------------------------------
if [[ -z "$PKG_ARCH" ]]; then
  case "$(uname -m)" in
    x86_64)  PKG_ARCH="amd64" ;;
    aarch64) PKG_ARCH="arm64" ;;
    *)       PKG_ARCH="$(uname -m)" ;;
  esac
fi

PKG_FULL="${PKG_NAME}_${PKG_VERSION}_${PKG_ARCH}"
BUILD_DIR="/tmp/${PKG_FULL}"
BINARY_SRC="target/release/spooky"

echo "==> Building package: ${PKG_FULL}.deb"

# ---- build binary ----------------------------------------------------------
if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "==> cargo build --release"
  cargo build --release
fi

if [[ ! -f "$BINARY_SRC" ]]; then
  echo "ERROR: binary not found at $BINARY_SRC — run without --skip-build or build manually" >&2
  exit 1
fi

# ---- prepare package tree --------------------------------------------------
rm -rf "$BUILD_DIR"
mkdir -p \
  "$BUILD_DIR/DEBIAN" \
  "$BUILD_DIR/usr/bin" \
  "$BUILD_DIR/etc/spooky/certs" \
  "$BUILD_DIR/var/log/spooky" \
  "$BUILD_DIR/lib/systemd/system"

# binary
install -m 0755 "$BINARY_SRC" "$BUILD_DIR/usr/bin/${PKG_NAME}"

# default config (marked as conffile so dpkg won't clobber on upgrade)
install -m 0640 "packaging/deb/debian/config.yaml" "$BUILD_DIR/etc/spooky/config.yaml"

# systemd unit
install -m 0644 "packaging/deb/debian/spooky.service" "$BUILD_DIR/lib/systemd/system/spooky.service"

# ---- DEBIAN/control --------------------------------------------------------
cat > "$BUILD_DIR/DEBIAN/control" <<EOF
Package: ${PKG_NAME}
Version: ${PKG_VERSION}
Architecture: ${PKG_ARCH}
Maintainer: Supernova Labs <noreply@supernova-labs.dev>
Section: net
Priority: optional
Description: Spooky QUIC/HTTP3 reverse proxy and load balancer
 A high-performance HTTP/3 and QUIC reverse proxy with adaptive load balancing,
 circuit breaking, and observability built in.
EOF

# ---- DEBIAN/conffiles ------------------------------------------------------
cat > "$BUILD_DIR/DEBIAN/conffiles" <<EOF
/etc/spooky/config.yaml
EOF

# ---- DEBIAN/postinst -------------------------------------------------------
cat > "$BUILD_DIR/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e

# create system user/group if missing
if ! getent group spooky > /dev/null 2>&1; then
  groupadd --system spooky
fi
if ! getent passwd spooky > /dev/null 2>&1; then
  useradd --system --gid spooky --no-create-home \
          --home-dir /etc/spooky --shell /usr/sbin/nologin \
          --comment "Spooky reverse proxy" spooky
fi

# ownership
chown -R spooky:spooky /etc/spooky
chmod 750 /etc/spooky
chmod 750 /etc/spooky/certs

chown -R spooky:spooky /var/log/spooky
chmod 750 /var/log/spooky

# enable + start service
if command -v systemctl > /dev/null 2>&1 && systemctl is-system-running --quiet 2>/dev/null; then
  systemctl daemon-reload
  systemctl enable spooky.service
  systemctl restart spooky.service || true
fi
EOF
chmod 0755 "$BUILD_DIR/DEBIAN/postinst"

# ---- DEBIAN/prerm ----------------------------------------------------------
cat > "$BUILD_DIR/DEBIAN/prerm" <<'EOF'
#!/bin/sh
set -e

if command -v systemctl > /dev/null 2>&1; then
  systemctl stop spooky.service  2>/dev/null || true
  systemctl disable spooky.service 2>/dev/null || true
fi
EOF
chmod 0755 "$BUILD_DIR/DEBIAN/prerm"

# ---- DEBIAN/postrm ---------------------------------------------------------
cat > "$BUILD_DIR/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e

if [ "$1" = "purge" ]; then
  rm -rf /etc/spooky
  rm -rf /var/log/spooky

  if getent passwd spooky > /dev/null 2>&1; then
    userdel spooky
  fi
  if getent group spooky > /dev/null 2>&1; then
    groupdel spooky
  fi
fi

if command -v systemctl > /dev/null 2>&1; then
  systemctl daemon-reload || true
fi
EOF
chmod 0755 "$BUILD_DIR/DEBIAN/postrm"

# ---- build .deb ------------------------------------------------------------
echo "==> dpkg-deb --build ${PKG_FULL}"
dpkg-deb --root-owner-group --build "$BUILD_DIR" "${PKG_FULL}.deb"

rm -rf "$BUILD_DIR"

echo ""
echo "Done: ${PWD}/${PKG_FULL}.deb"
echo ""
echo "Install with:"
echo "  sudo dpkg -i ${PKG_FULL}.deb"
echo ""
echo "After install, place your TLS certs at:"
echo "  /etc/spooky/certs/fullchain.pem"
echo "  /etc/spooky/certs/privkey.pem"
echo "Then edit /etc/spooky/config.yaml and: sudo systemctl restart spooky"
