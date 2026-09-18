# Cosmos for Linux (x86_64) — v0.2.72

Downloadable installers for the native Rust + GPUI Cosmos desktop app.
Pick whichever fits the target machine. Verify integrity with `SHA256SUMS`:

```sh
sha256sum -c SHA256SUMS
```

## AppImage (any distro, no root)
Single self-contained file — the easiest "anyone can run" option.

```sh
chmod +x Cosmos-linux-x86_64.AppImage
./Cosmos-linux-x86_64.AppImage
```

Runs without installation. To integrate it into menus, use an AppImage
manager (e.g. Gear Lever) or move it somewhere on your PATH.

## Debian / Ubuntu (.deb)
System package with menu entry and icon.

```sh
sudo apt install ./Cosmos-linux-x86_64.deb     # resolves dependencies
# or: sudo dpkg -i Cosmos-linux-x86_64.deb && sudo apt -f install
cosmos                                          # launch
sudo apt remove cosmos                          # uninstall
```

Runtime deps: `libc6, libgcc-s1, libxkbcommon0, libxkbcommon-x11-0, libxcb1,
libwayland-client0, libfontconfig1, libfreetype6` (Recommends: `libvulkan1,
libwebkit2gtk-4.1-0, libasound2t64`). GPU is optional — Cosmos falls back to
software rendering (llvmpipe).

## Portable tarball (any distro)
Binary + icon + `.desktop`, with a user-level installer (no root).

```sh
tar xzf Cosmos-linux-x86_64.tar.gz
cd Cosmos-*-linux-x86_64
./cosmos              # run in place, or:
./install.sh          # install into ~/.local (adds menu entry + icon)
./uninstall.sh        # remove
```

## Notes
- x86_64 only. Rebuild for other arches with `scripts/package-linux-cosmos.sh`.
- First launch starts local-only (no account/network required).
