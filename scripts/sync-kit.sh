#!/usr/bin/env bash
# Recopie la caisse commune depuis plugins/morph-kit (source de vérité locale).
set -euo pipefail
SRC="${1:-../morph-kit}"
rm -rf vendor/morph-kit
mkdir -p vendor/morph-kit
cp -r "$SRC/src" "$SRC/python" vendor/morph-kit/
# Le dépôt hôte est déjà la racine du workspace : la copie n'en déclare pas.
sed '/^\[workspace\]/d' "$SRC/Cargo.toml" > vendor/morph-kit/Cargo.toml
echo "morph-kit synchronisé depuis $SRC"
