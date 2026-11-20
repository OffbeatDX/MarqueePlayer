#!/bin/bash
# Builds PipeWire 0.3.65's library and headers into a folder of their own, to build the app
# against on a system whose PipeWire is older (Ubuntu 22.04 has 0.3.48): the PipeWire bindings
# (libspa 0.10) use SPA headers of 0.3.65, Debian 12's version. Only for building: the AppImage
# never carries this library, it uses the system's, which needs only the functions the app
# calls (0.3.50 and newer have them; docs/releases.md, "Linux AppImage").
#
#   packaging/linux/pipewire-sdk.sh [prefix]     # default ~/.local/share/mp-build/pipewire-0.3.65
#
# Needs meson, ninja, a C compiler, pkg-config, libdbus-1-dev, curl. No root: it installs
# into the prefix only. `cargo xtask release` finds it there (or MP_PIPEWIRE_PREFIX).
set -euo pipefail
VER=0.3.65
PREFIX=${1:-$HOME/.local/share/mp-build/pipewire-$VER}
if PKG_CONFIG_PATH="$PREFIX/lib/x86_64-linux-gnu/pkgconfig:$PREFIX/lib/pkgconfig" pkg-config --exact-version=$VER libpipewire-0.3 2>/dev/null; then
    echo "PipeWire $VER is already in $PREFIX"
    exit 0
fi
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"
echo "Downloading PipeWire $VER (gitlab.freedesktop.org)..."
curl -sSfL -o pw.tar.gz "https://gitlab.freedesktop.org/pipewire/pipewire/-/archive/$VER/pipewire-$VER.tar.gz"
tar xzf pw.tar.gz
cd "pipewire-$VER"
# What building against it needs is the library and the headers; the rest is turned off where
# meson allows it (no session manager, devices, tools or docs). Nothing goes outside the prefix.
meson setup build --prefix="$PREFIX" --buildtype=release \
    -Dsession-managers=[] -Dexamples=disabled -Dtests=disabled -Dinstalled_tests=disabled \
    -Ddocs=disabled -Dman=disabled -Dgstreamer=disabled -Dgstreamer-device-provider=disabled \
    -Dsystemd=disabled -Dsystemd-system-service=disabled -Dsystemd-user-service=disabled \
    -Dpipewire-alsa=disabled -Dpipewire-jack=disabled -Dpipewire-v4l2=disabled -Djack-devel=false \
    -Dalsa=disabled -Daudiomixer=disabled \
    -Dbluez5=disabled -Dcontrol=disabled -Daudiotestsrc=disabled -Dffmpeg=disabled -Djack=disabled \
    -Devl=disabled -Dtest=disabled -Dv4l2=disabled -Dlibcamera=disabled \
    -Dvideoconvert=disabled -Dvideotestsrc=disabled -Dvolume=disabled -Dvulkan=disabled \
    -Dpw-cat=disabled -Dudev=disabled -Dsdl2=disabled -Dsndfile=disabled -Dlibpulse=disabled \
    -Droc=disabled -Davahi=disabled -Decho-cancel-webrtc=disabled -Dlibusb=disabled \
    -Draop=disabled -Dlv2=disabled -Dx11=disabled -Dx11-xfixes=disabled -Dlibcanberra=disabled \
    -Dlegacy-rtkit=false -Dflatpak=disabled -Dgsettings=disabled -Dreadline=disabled -Davb=disabled \
    -Dcompress-offload=disabled -Dpw-cat-ffmpeg=disabled \
    > meson.log 2>&1 || { tail -30 meson.log; exit 1; }
ninja -C build > ninja.log 2>&1 || { tail -30 ninja.log; exit 1; }
ninja -C build install > install.log 2>&1 || { tail -30 install.log; exit 1; }
PKG_CONFIG_PATH="$PREFIX/lib/x86_64-linux-gnu/pkgconfig:$PREFIX/lib/pkgconfig" pkg-config --modversion libpipewire-0.3
echo "PipeWire $VER installed in $PREFIX"
