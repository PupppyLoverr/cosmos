#!/usr/bin/env bash
# Cosmos Linux packaging — produces branded, distributable artifacts from the
# release binary:
#   artifacts/linux/Cosmos-linux-$ARCH.tar.gz   (portable: binary + icon + .desktop + installer)
#   artifacts/linux/Cosmos-linux-$ARCH.deb      (system package, install/uninstall via apt/dpkg)
#   artifacts/linux/Cosmos-linux-$ARCH.AppImage (single-file portable app; best-effort)
#
# Usage: scripts/package-linux-cosmos.sh
# Env:   COSMOS_BIN (default target/release/cosmos), SKIP_APPIMAGE=1 to skip,
#        COSMOS_REQUIRE_INSTALLERS=1 to fail if .deb or AppImage cannot be built.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
ARCH="$(uname -m)"
REQUIRE_INSTALLERS="${COSMOS_REQUIRE_INSTALLERS:-0}"
BIN="${COSMOS_BIN:-$ROOT/target/release/cosmos}"
OUT="$ROOT/artifacts/linux"
ICON="$ROOT/dist/zeron.png"   # the Cosmos app icon (dist filename predates rename)
SVG="$ROOT/dist/zeron.svg"
FONT_LICENSES="$ROOT/crates/ui/assets/fonts/licenses"

[[ -x "$BIN" ]] || { echo "error: release binary not found at $BIN (run: cargo build --release -p zeron)"; exit 1; }
mkdir -p "$OUT"
echo "Packaging Cosmos $VERSION ($ARCH) from $BIN"

# ── shared .desktop entry (StartupWMClass matches the Linux app_id "cosmos") ──
make_desktop() { # make_desktop <exec-name> <out-file>
  cat >"$2" <<EOF
[Desktop Entry]
Type=Application
Name=Cosmos
GenericName=Coding Agent Controller
Comment=Multi-device controller for coding agents
Exec=$1 %u
TryExec=$1
Icon=cosmos
Terminal=false
Categories=Development;Utility;
Keywords=agent;claude;codex;cursor;ai;coding;cosmos;
StartupWMClass=cosmos
MimeType=x-scheme-handler/cosmos;x-scheme-handler/zeron;
EOF
}

# ── 1. tar.gz (portable) ──────────────────────────────────────────────────────
STAGE="$OUT/Cosmos-$VERSION-linux-$ARCH"
TARBALL="$OUT/Cosmos-linux-$ARCH.tar.gz"
rm -rf "$STAGE"; mkdir -p "$STAGE/licenses/fonts"
install -m 755 "$BIN" "$STAGE/cosmos"
install -m 644 "$ICON" "$STAGE/cosmos.png"
[[ -f "$SVG" ]] && install -m 644 "$SVG" "$STAGE/cosmos.svg" || true
make_desktop cosmos "$STAGE/cosmos.desktop"
cp "$FONT_LICENSES/"* "$STAGE/licenses/fonts/" 2>/dev/null || true
echo "$VERSION" > "$STAGE/VERSION"
cat >"$STAGE/install.sh" <<'INSTALL'
#!/usr/bin/env bash
# Install Cosmos into ~/.local (no root needed).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
install -Dm755 "$HERE/cosmos" "$HOME/.local/bin/cosmos"
install -Dm644 "$HERE/cosmos.png" "$HOME/.local/share/icons/hicolor/512x512/apps/cosmos.png"
sed "s|^Exec=.*|Exec=$HOME/.local/bin/cosmos %u|; s|^TryExec=.*|TryExec=$HOME/.local/bin/cosmos|" \
  "$HERE/cosmos.desktop" > "$HOME/.local/share/applications/cosmos.desktop"
command -v update-desktop-database >/dev/null 2>&1 \
  && update-desktop-database "$HOME/.local/share/applications" || true
echo "Installed Cosmos to ~/.local/bin/cosmos — ensure ~/.local/bin is on PATH."
INSTALL
chmod 755 "$STAGE/install.sh"
cat >"$STAGE/uninstall.sh" <<'UNINSTALL'
#!/usr/bin/env bash
set -euo pipefail
rm -f "$HOME/.local/bin/cosmos" \
      "$HOME/.local/share/icons/hicolor/512x512/apps/cosmos.png" \
      "$HOME/.local/share/applications/cosmos.desktop"
echo "Removed Cosmos from ~/.local."
UNINSTALL
chmod 755 "$STAGE/uninstall.sh"
rm -f "$TARBALL"
tar -czf "$TARBALL" -C "$OUT" "$(basename "$STAGE")"
echo "  ✓ $TARBALL"

# ── 2. .deb ───────────────────────────────────────────────────────────────────
DEB="$OUT/Cosmos-linux-$ARCH.deb"
DEBARCH="amd64"; [[ "$ARCH" == "aarch64" ]] && DEBARCH="arm64"
PKG="$OUT/deb-root"
rm -rf "$PKG"
mkdir -p "$PKG/DEBIAN" "$PKG/usr/bin" \
         "$PKG/usr/share/applications" \
         "$PKG/usr/share/icons/hicolor/512x512/apps" \
         "$PKG/usr/share/icons/hicolor/scalable/apps" \
         "$PKG/usr/share/doc/cosmos"
install -m 755 "$BIN" "$PKG/usr/bin/cosmos"
install -m 644 "$ICON" "$PKG/usr/share/icons/hicolor/512x512/apps/cosmos.png"
[[ -f "$SVG" ]] && install -m 644 "$SVG" "$PKG/usr/share/icons/hicolor/scalable/apps/cosmos.svg" || true
make_desktop cosmos "$PKG/usr/share/applications/cosmos.desktop"
[[ -f "$ROOT/LICENSE" ]] && install -m 644 "$ROOT/LICENSE" "$PKG/usr/share/doc/cosmos/copyright" || true
INSTALLED_KB=$(du -ks "$PKG/usr" | cut -f1)
cat >"$PKG/DEBIAN/control" <<EOF
Package: cosmos
Version: $VERSION
Section: devel
Priority: optional
Architecture: $DEBARCH
Maintainer: Cosmos <noreply@zeron.sh>
Installed-Size: $INSTALLED_KB
Depends: libc6, libgcc-s1, libxkbcommon0, libxkbcommon-x11-0, libxcb1, libwayland-client0, libfontconfig1, libfreetype6
Recommends: libvulkan1, libwebkit2gtk-4.1-0, libasound2t64 | libasound2
Description: Cosmos — multi-device controller for coding agents
 Control your coding agents (Claude Code, Codex, Cursor, Devin, Amp, OpenCode,
 Grok, Hermes, Pi and more) locally by default, with optional multi-device sync.
 Native Rust + GPUI desktop application.
EOF
cat >"$PKG/DEBIAN/postinst" <<'POSTINST'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t /usr/share/icons/hicolor || true
fi
exit 0
POSTINST
chmod 755 "$PKG/DEBIAN/postinst"
rm -f "$DEB"
if command -v dpkg-deb >/dev/null 2>&1; then
  dpkg-deb --build --root-owner-group "$PKG" "$DEB" >/dev/null
  echo "  ✓ $DEB"
else
  if [[ "$REQUIRE_INSTALLERS" == "1" ]]; then
    echo "error: dpkg-deb not found; cannot build required .deb installer" >&2
    exit 1
  fi
  echo "  ! dpkg-deb not found; skipping .deb"
fi

# ── 3. AppImage (best-effort) ─────────────────────────────────────────────────
if [[ "${SKIP_APPIMAGE:-0}" == "1" ]]; then
  if [[ "$REQUIRE_INSTALLERS" == "1" ]]; then
    echo "error: SKIP_APPIMAGE=1 conflicts with COSMOS_REQUIRE_INSTALLERS=1" >&2
    exit 1
  fi
  echo "  · SKIP_APPIMAGE=1 — skipping AppImage"
  exit 0
fi
APPIMAGE="$OUT/Cosmos-linux-$ARCH.AppImage"
APPDIR="$OUT/Cosmos.AppDir"
TOOL="$OUT/appimagetool-$ARCH.AppImage"
rm -rf "$APPDIR"; mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/icons/hicolor/512x512/apps"
install -m 755 "$BIN" "$APPDIR/usr/bin/cosmos"
install -m 644 "$ICON" "$APPDIR/cosmos.png"
install -m 644 "$ICON" "$APPDIR/usr/share/icons/hicolor/512x512/apps/cosmos.png"
make_desktop cosmos "$APPDIR/cosmos.desktop"
cat >"$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/bin:$PATH"
exec "$HERE/usr/bin/cosmos" "$@"
APPRUN
chmod 755 "$APPDIR/AppRun"

if [[ ! -f "$TOOL" ]]; then
  echo "  · downloading appimagetool"
  if curl -fsSL -o "$TOOL" \
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"; then
    chmod +x "$TOOL" 2>/dev/null || true
  else
    rm -f "$TOOL"
    if [[ "$REQUIRE_INSTALLERS" == "1" ]]; then
      echo "error: failed to download required appimagetool for $ARCH" >&2
      exit 1
    fi
  fi
fi
rm -f "$APPIMAGE"
if [[ -x "$TOOL" ]]; then
  # No FUSE in most CI/VMs: extract-and-run. ARCH env is required by the tool.
  if ARCH="$ARCH" "$TOOL" --appimage-extract-and-run "$APPDIR" "$APPIMAGE" 2>"$OUT/appimage-build.log"; then
    if [[ "$REQUIRE_INSTALLERS" == "1" && ! -f "$APPIMAGE" ]]; then
      echo "error: appimagetool produced no AppImage; build log tail:" >&2
      tail -n 40 "$OUT/appimage-build.log" >&2
      exit 1
    fi
    chmod +x "$APPIMAGE"
    echo "  ✓ $APPIMAGE"
  else
    if [[ "$REQUIRE_INSTALLERS" == "1" ]]; then
      echo "error: appimagetool failed; build log tail:" >&2
      tail -n 40 "$OUT/appimage-build.log" >&2
      exit 1
    fi
    echo "  ! appimagetool failed (see $OUT/appimage-build.log); tar.gz and .deb still produced"
  fi
else
  if [[ "$REQUIRE_INSTALLERS" == "1" ]]; then
    echo "error: appimagetool unavailable; cannot build required AppImage" >&2
    exit 1
  fi
  echo "  ! appimagetool unavailable; tar.gz and .deb still produced"
fi
