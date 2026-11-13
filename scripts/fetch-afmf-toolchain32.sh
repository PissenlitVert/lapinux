#!/usr/bin/env bash
# Prépare la chaîne de compilation 32 bits de la couche afmf-linux sans toucher
# au système : les paquets i686 sont téléchargés puis extraits dans un dossier du
# dépôt, et le compilateur s'en sert par `-isystem` et `-nostdlib`.
#
# Nécessaire pour les jeux 32 bits (DXVK i386) : sans ces en-têtes, `gcc -m32`
# ne trouve ni gnu/stubs-32.h ni crti.o.
#
# Usage : scripts/fetch-afmf-toolchain32.sh
#   AFMF_TOOLCHAIN32 remplace target/afmf-toolchain32.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
TOOLCHAIN="${AFMF_TOOLCHAIN32:-$ROOT/target/afmf-toolchain32}"
PACKAGES=(glibc-devel.i686 glibc.i686 libgcc.i686)

mkdir -p "$TOOLCHAIN/rpm" "$TOOLCHAIN/root"
if ! command -v dnf >/dev/null 2>&1; then
    echo "dnf introuvable : la couche 32 bits ne peut pas être préparée ici" >&2
    exit 1
fi
# `dnf download` n'installe rien : il ne récupère que les paquets.
dnf download --destdir "$TOOLCHAIN/rpm" "${PACKAGES[@]}"
for package in "$TOOLCHAIN"/rpm/*.rpm; do
    rpm2cpio "$package" | (cd "$TOOLCHAIN/root" && cpio -idm --quiet)
done
# Les paquets placent leurs fichiers sous usr/ ; le compilateur les attend à la
# racine du dossier d'extraction.
if [ -d "$TOOLCHAIN/root/usr" ]; then
    cp -a "$TOOLCHAIN/root/usr/." "$TOOLCHAIN/root/"
    rm -rf "$TOOLCHAIN/root/usr"
fi
for file in include/gnu/stubs-32.h lib/crti.o lib/libc_nonshared.a; do
    if [ ! -e "$TOOLCHAIN/root/$file" ]; then
        echo "chaîne 32 bits incomplète : $file manquant" >&2
        exit 1
    fi
done
echo "chaîne 32 bits prête : $TOOLCHAIN/root"
