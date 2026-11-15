#!/usr/bin/env bash
# Garde i18n : parité fr/en, clés appelées, littéraux UI visibles.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test -p lapinux-ui --test i18n_guard
cargo test -p lapinux-core i18n::tests
