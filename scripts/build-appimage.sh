#!/usr/bin/env bash
# Construit l'AppImage portable de Lapinux.
#
#   ./scripts/build-appimage.sh
#
# Le résultat sort dans dist/Lapinux-<version>-x86_64.AppImage.
#
# L'AppImage contient les deux binaires et le pont Steamworks natif expérimental.
# Les bibliothèques système restent celles de la distribution : la compatibilité
# dépend notamment de la glibc utilisée à la compilation. Le SHA-256 attendu du
# runtime doit être vérifié auprès de sa source et fourni explicitement via
# LAPINUX_APPIMAGE_RUNTIME_SHA256.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[ -n "$version" ] || version=0.1.0

runtime=${LAPINUX_APPIMAGE_RUNTIME:-"$root/dist/appimage/runtime-x86_64"}
out=${LAPINUX_APPIMAGE_OUTPUT:-"$root/dist/Lapinux-$version-x86_64.AppImage"}
profile=${LAPINUX_APPIMAGE_PROFILE:-dist}
expected=${LAPINUX_APPIMAGE_RUNTIME_SHA256:-}
if [[ ! "$expected" =~ ^[[:xdigit:]]{64}$ ]]; then
    echo "Fournir LAPINUX_APPIMAGE_RUNTIME_SHA256 : empreinte vérifiée du runtime AppImage." >&2
    exit 1
fi
if [ ! -f "$runtime" ]; then
    echo "Runtime absent : $runtime ; téléchargez une version vérifiée avant le build." >&2
    exit 1
fi
printf '%s  %s\n' "$expected" "$runtime" | sha256sum --check --status || {
    echo "Empreinte du runtime AppImage incorrecte : $runtime" >&2
    exit 1
}
if [ -e "$out" ]; then
    echo "AppImage déjà présente, non écrasée : $out" >&2
    exit 1
fi
mkdir -p "$(dirname "$out")"
# Le lien final doit être sur le même système de fichiers que la sortie.
work=$(mktemp -d "$(dirname "$out")/.lapinux-appimage-build.XXXXXXXX")
trap 'rm -rf -- "$work"' EXIT
appdir="$work/Lapinux.AppDir"

echo "Compilation (profil $profile)…"
cargo build --profile "$profile" -p lapinux --bins
cargo build --profile "$profile" -p lapinux-steamshim
build_profile=$profile
[ "$profile" = dev ] && build_profile=debug
build_dir="${CARGO_TARGET_DIR:-$root/target}/$build_profile"

mkdir -p \
    "$appdir/usr/bin" \
    "$appdir/usr/share/applications" \
    "$appdir/usr/share/icons/hicolor/256x256/apps" \
    "$appdir/usr/share/icons/hicolor/512x512/apps" \
    "$appdir/usr/share/icons/hicolor/scalable/apps"

install -m 755 "$build_dir/lapinux" "$appdir/usr/bin/lapinux"
install -m 755 "$build_dir/lapinux-probe" "$appdir/usr/bin/lapinux-probe"
install -m 755 "$build_dir/libsteam_api.so" "$appdir/usr/bin/libsteam_api.so"

# Notices connues des ressources embarquées ; l'inventaire des droits et la
# licence générale du projet restent à valider avant toute distribution.
notices="$appdir/usr/share/licenses/lapinux"
mkdir -p "$notices"
install -m 644 assets/fonts/OFL-Inter.txt "$notices/"
install -m 644 assets/fonts/LICENSE-RemixIcon.txt "$notices/"
install -m 644 assets/graphics/shaders/LICENSE.SweetFX "$notices/"
install -m 644 assets/vkbasalt/LICENSE "$notices/LICENSE-vkbasalt"
install -m 644 vendor/egui-0.29.1/LICENSE-MIT "$notices/LICENSE-egui-MIT"
install -m 644 vendor/egui-0.29.1/LICENSE-APACHE "$notices/LICENSE-egui-APACHE"

cat > "$appdir/AppRun" <<'RUN'
#!/bin/sh
# Point d'entrée de l'AppImage : les binaires vivent dans usr/bin, et
# lapinux-probe est appelé par nom par la sonde Steam.
HERE=$(dirname "$(readlink -f "$0")")
export PATH="$HERE/usr/bin:$PATH"
export LAPINUX_APPIMAGE=1
exec "$HERE/usr/bin/lapinux" "$@"
RUN
chmod 755 "$appdir/AppRun"

sed 's/^TryExec=lapinux$/TryExec=lapinux/' packaging/lapinux.desktop \
    > "$appdir/lapinux.desktop"
install -m 644 "$appdir/lapinux.desktop" \
    "$appdir/usr/share/applications/lapinux.desktop"

# Icône de l'application : un PNG racine (obligatoire), .DirIcon et les tailles
# hicolor pour les bureaux qui ne lisent que le thème.
convert assets/lapinux-icon.png -resize 256x256 "$appdir/lapinux.png"
cp "$appdir/lapinux.png" "$appdir/.DirIcon"
install -m 644 "$appdir/lapinux.png" \
    "$appdir/usr/share/icons/hicolor/256x256/apps/lapinux.png"
convert assets/lapinux-icon.png -resize 512x512 "$appdir/lapinux-512.png"
install -m 644 "$appdir/lapinux-512.png" \
    "$appdir/usr/share/icons/hicolor/512x512/apps/lapinux.png"
rm -f "$appdir/lapinux-512.png"

# La compression gzip (zlib) est prise en charge par les runtimes AppImage
# type 2, notamment le runtime officiel 20251108 qui ne prend pas en charge xz.
mksquashfs "$appdir" "$work/Lapinux.squashfs" \
    -root-owned -noappend -no-xattrs -comp gzip \
    >/dev/null

# Publier sans écraser une image créée pendant la compilation.
cat "$runtime" "$work/Lapinux.squashfs" > "$work/Lapinux.AppImage"
chmod 755 "$work/Lapinux.AppImage"
ln "$work/Lapinux.AppImage" "$out" || { echo "Sortie déjà présente ou impossible : $out" >&2; exit 1; }
sha256sum "$out"

size=$(du -h "$out" | cut -f1)
echo "AppImage prête : $out ($size)"
