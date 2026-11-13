#!/usr/bin/env bash
# Reconstruit les runtimes vkBasalt embarqués dans assets/vkbasalt.
# Nécessite podman ou docker. Ubuntu 20.04 + GCC 10 garde des symboles glibc
# 2.29, compatibles avec les Steam Linux Runtime utilisés par Proton.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
VERSION=0.3.2.10
URL="https://github.com/DadSchoorse/vkBasalt/releases/download/v${VERSION}/vkBasalt-${VERSION}.tar.gz"
HASH="eb196ff446fa36ec0ca99c4406d753c1fa210afddeec5e7a76e1c2e74ed605e3"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
curl -L --fail --max-time 120 -o "$WORK/vkbasalt.tar.gz" "$URL"
echo "$HASH  $WORK/vkbasalt.tar.gz" | sha256sum -c -
mkdir "$WORK/src"
tar -xzf "$WORK/vkbasalt.tar.gz" -C "$WORK/src" --strip-components=1
ENGINE=$(command -v podman || command -v docker)

"$ENGINE" run --rm -v "$WORK/src:/src:Z" -w /src ubuntu:20.04 bash -lc '
  set -e
  apt-get update >/dev/null
  DEBIAN_FRONTEND=noninteractive apt-get install -y \
    meson ninja-build g++-10 libx11-dev libvulkan-dev glslang-tools \
    pkg-config spirv-headers >/dev/null
  rm -rf build64
  CC=gcc-10 CXX=g++-10 meson setup build64 --buildtype=release \
    --prefix=/usr --libdir=lib/x86_64-linux-gnu
  ninja -C build64
'

"$ENGINE" run --rm -v "$WORK/src:/src:Z" -w /src ubuntu:20.04 bash -lc '
  set -e
  dpkg --add-architecture i386
  apt-get update >/dev/null
  DEBIAN_FRONTEND=noninteractive apt-get install -y \
    meson ninja-build g++-10 gcc-10 g++-10-multilib gcc-10-multilib \
    libc6-dev-i386 libx11-dev:i386 libvulkan-dev:i386 glslang-tools \
    pkg-config spirv-headers >/dev/null
  rm -rf build32
  CC=gcc-10 CXX=g++-10 \
    PKG_CONFIG_LIBDIR=/usr/lib/i386-linux-gnu/pkgconfig:/usr/share/pkgconfig \
    meson setup build32 --buildtype=release --prefix=/usr \
      --libdir=lib/i386-linux-gnu \
      -Dc_args=-m32 -Dcpp_args=-m32 -Dc_link_args=-m32 -Dcpp_link_args=-m32
  ninja -C build32
'

for arch in x86_64 i686; do
  mkdir -p "$ROOT/assets/vkbasalt/$arch"
  if [ "$arch" = x86_64 ]; then build=build64; else build=build32; fi
  install -m 755 "$WORK/src/$build/src/libvkbasalt.so" \
    "$ROOT/assets/vkbasalt/$arch/libvkbasalt.so"
  strip --strip-unneeded "$ROOT/assets/vkbasalt/$arch/libvkbasalt.so"
done
cp "$WORK/src/LICENSE" "$ROOT/assets/vkbasalt/LICENSE"
echo "Runtimes vkBasalt reconstruits sous $ROOT/assets/vkbasalt"
