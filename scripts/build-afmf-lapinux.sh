#!/usr/bin/env bash
# Construit la couche afmf-linux de ce dépôt et l'installe comme version
# « local » de Lapinux, avec ses propres empreintes.
#
# La version locale n'est pas une publication : `lapinux-local.json` ne dit pas
# d'où vient le binaire, seulement que les fichiers chargés sont ceux qui ont
# été construits et installés ici. C'est ce que vérifie le daemon avant de
# laisser Vulkan charger la couche.
#
# Usage : scripts/build-afmf-lapinux.sh
#   AFMF_SOURCE_DIR, AFMF_BUILD_DIR, AFMF_INSTALL_DIR remplacent les chemins.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
SOURCE="${AFMF_SOURCE_DIR:-$ROOT/target/afmf-linux-mission}"
BUILD="${AFMF_BUILD_DIR:-$ROOT/target/afmf-linux-mission-build}"
CMAKE="${CMAKE:-$ROOT/target/afmf-build-tools/bin/cmake}"
DEST="${AFMF_INSTALL_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/lapinux/tools/frame-generation/afmf-linux/local}"

if [ ! -f "$SOURCE/CMakeLists.txt" ]; then
    echo "source absente : $SOURCE" >&2
    exit 1
fi
if [ ! -x "$CMAKE" ]; then
    CMAKE=$(command -v cmake || true)
fi
if [ -z "$CMAKE" ]; then
    echo "cmake introuvable" >&2
    exit 1
fi

if [ ! -f "$BUILD/CMakeCache.txt" ]; then
    "$CMAKE" -S "$SOURCE" -B "$BUILD" -G Ninja -DCMAKE_BUILD_TYPE=Release
fi
"$CMAKE" --build "$BUILD" -j "$(nproc 2>/dev/null || echo 4)"

LAYER="$BUILD/layer/libafmf-linux.so"
MANIFEST="$BUILD/layer/afmf-linux.json"
if [ ! -f "$LAYER" ] || [ ! -f "$MANIFEST" ]; then
    echo "couche non construite : $LAYER" >&2
    exit 1
fi

# Les jeux 32 bits (Proton i386, DXVK 32 bits) chargent une couche 32 bits :
# même source, autre cible. Elle n'est construite que si la chaîne 32 bits a
# été préparée par scripts/fetch-afmf-toolchain32.sh ; sinon la version locale
# reste 64 bits et les jeux 32 bits ne reçoivent rien.
TOOLCHAIN32="${AFMF_TOOLCHAIN32:-$ROOT/target/afmf-toolchain32}"
BUILD32="${AFMF_BUILD_DIR32:-$ROOT/target/afmf-linux-mission-build32}"
HEADERS32="$TOOLCHAIN32/root/include"
LAYER32=""
if [ -e "$HEADERS32/gnu/stubs-32.h" ]; then
    # Sans libc liée (les symboles viennent du processus hôte), les divisions 64 bits sur
    # 32 bits demandent quand même l'aide de libgcc.
    LIBGCC32=$(dirname "$(gcc -m32 -print-libgcc-file-name 2>/dev/null || echo /usr/lib/gcc/x86_64-redhat-linux/16/32/libgcc.a)")
    if [ ! -f "$BUILD32/CMakeCache.txt" ]; then
        NINJA=$(command -v ninja || echo "$ROOT/target/afmf-build-tools/bin/ninja")
        "$CMAKE" -S "$SOURCE" -B "$BUILD32" -G Ninja -DCMAKE_MAKE_PROGRAM="$NINJA" \
            -DCMAKE_BUILD_TYPE=Release \
            -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY \
            -DCMAKE_C_FLAGS="-m32 -isystem $HEADERS32 -Wno-error=conversion -Wno-error=sign-conversion -Wno-error=cast-align -Wno-error=pointer-arith" \
            -DCMAKE_SHARED_LINKER_FLAGS="-nostdlib -L$LIBGCC32" \
            -DCMAKE_C_STANDARD_LIBRARIES=-lgcc \
            -DVulkan_INCLUDE_DIR="$ROOT/target/vulkan-headers-mission/include" \
            -DVulkan_LIBRARY=/usr/lib64/libvulkan.so.1
    fi
    "$CMAKE" --build "$BUILD32" --target libafmf-linux.so -j "$(nproc 2>/dev/null || echo 4)"
    if [ -f "$BUILD32/layer/libafmf-linux.so" ]; then
        LAYER32="$BUILD32/layer/libafmf-linux.so"
    fi
else
    echo "couche 32 bits ignorée : lance scripts/fetch-afmf-toolchain32.sh pour couvrir les jeux 32 bits" >&2
fi

STAGE=$(mktemp -d "${TMPDIR:-/tmp}/afmf-local.XXXXXX")
trap 'rm -rf "$STAGE"' EXIT
install -m 0644 "$LAYER" "$STAGE/libafmf-linux.so"
[ -n "$LAYER32" ] && install -m 0644 "$LAYER32" "$STAGE/libafmf-linux32.so"
[ -f "$SOURCE/LICENSE" ] && install -m 0644 "$SOURCE/LICENSE" "$STAGE/LICENSE"
[ -f "$SOURCE/CHANGELOG.md" ] && install -m 0644 "$SOURCE/CHANGELOG.md" "$STAGE/CHANGELOG.md"

# Le manifeste de la couche est réécrit avec un chemin relatif : le dossier
# peut être déplacé, et aucune couche système ne prend sa place.
API_VERSION=$(sed -n 's/.*"api_version": *"\([^"]*\)".*/\1/p' "$MANIFEST" | head -1)
IMPLEMENTATION=$(sed -n 's/.*"implementation_version": *"\([^"]*\)".*/\1/p' "$MANIFEST" | head -1)
cat > "$STAGE/afmf-linux.json" <<EOF
{
  "file_format_version": "1.0.0",
  "layer": {
    "name": "VK_LAYER_AFMF",
    "type": "GLOBAL",
    "library_path": "./libafmf-linux.so",
    "api_version": "${API_VERSION:-1.4.341}",
    "implementation_version": "${IMPLEMENTATION:-5}",
    "description": "afmf-linux (couche locale Lapinux) : génération d'images ×2/×3/×4, contrôle à chaud",
    "functions": {
      "vkNegotiateLoaderLayerInterfaceVersion": "vkNegotiateLoaderLayerInterfaceVersion"
    },
    "enable_environment": {
      "AFMF_ENABLE": "1"
    },
    "disable_environment": {
      "DISABLE_AFMF": "1"
    }
  }
}
EOF

if [ -n "$LAYER32" ]; then
    # Second manifeste, réservé aux processus 32 bits : le chargeur Vulkan ne
    # distingue pas les architectures, il tente les deux et garde celle qui se
    # charge. Le nom diffère pour que les deux couches puissent coexister.
    cat > "$STAGE/afmf-linux32.json" <<EOF
{
  "file_format_version": "1.0.0",
  "layer": {
    "name": "VK_LAYER_AFMF32",
    "type": "GLOBAL",
    "library_path": "./libafmf-linux32.so",
    "api_version": "${API_VERSION:-1.4.341}",
    "implementation_version": "${IMPLEMENTATION:-5}",
    "description": "afmf-linux (couche locale Lapinux 32 bits) : génération d'images ×2/×3/×4, contrôle à chaud",
    "functions": {
      "vkNegotiateLoaderLayerInterfaceVersion": "vkNegotiateLoaderLayerInterfaceVersion"
    },
    "enable_environment": {
      "AFMF_ENABLE": "1"
    },
    "disable_environment": {
      "DISABLE_AFMF": "1"
    }
  }
}
EOF
fi

COMMIT=$(git -C "$SOURCE" rev-parse HEAD 2>/dev/null || echo "inconnu")
python3 - "$STAGE" "$COMMIT" <<'PY'
import hashlib, json, os, sys, time

stage, commit = sys.argv[1], sys.argv[2]
files = {}
for name in sorted(os.listdir(stage)):
    path = os.path.join(stage, name)
    if name == "lapinux-local.json" or not os.path.isfile(path):
        continue
    with open(path, "rb") as handle:
        files[name] = hashlib.sha256(handle.read()).hexdigest()
manifest = {
    "kind": "lapinux-afmf-local",
    "source_commit": commit,
    "built_at": int(time.time()),
    "files": files,
}
with open(os.path.join(stage, "lapinux-local.json"), "w", encoding="utf-8") as handle:
    json.dump(manifest, handle, indent=2, sort_keys=True)
    handle.write("\n")
PY

# Remplacement complet du dossier : l'ancienne couche locale n'a plus de
# raison de rester, et un fichier ajouté à la main ne doit pas être chargé.
PARENT=$(dirname "$DEST")
mkdir -p "$PARENT"
rm -rf "$DEST.new"
mv "$STAGE" "$DEST.new"
trap - EXIT
rm -rf "$DEST"
mv "$DEST.new" "$DEST"

echo "couche locale installée : $DEST"
ls -l "$DEST"
