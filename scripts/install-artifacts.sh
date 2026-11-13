#!/usr/bin/env bash
# Copie les artefacts compilés dans un répertoire explicite, sans les écraser.
set -euo pipefail
if [ "$#" -ne 2 ]; then
  echo "Usage: bash scripts/install-artifacts.sh <dossier-build> <dossier-installation>" >&2
  exit 2
fi
build=$1
install_dir=$2
for name in lapinux lapinux-probe libsteam_api.so; do
  if [ ! -f "$build/$name" ]; then
    echo "Artefact manquant : $build/$name" >&2
    exit 1
  fi
done
# Un nouveau répertoire seulement : pas d'écrasement d'une installation existante.
if ! mkdir "$install_dir"; then
  echo "Le dossier d'installation doit être nouveau : $install_dir" >&2
  exit 1
fi
install -m 755 "$build/lapinux" "$install_dir/lapinux"
install -m 755 "$build/lapinux-probe" "$install_dir/lapinux-probe"
install -m 755 "$build/libsteam_api.so" "$install_dir/libsteam_api.so"
