#!/usr/bin/env bash
# Installe morph-browser (serveur MCP de controle du navigateur + extension) sous Linux et macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/Locaryn/morph-browser/main/install.sh | bash
#
# 1. telecharge la derniere release depuis github.com/Locaryn/morph-browser ;
# 2. verifie son empreinte SHA-256 (SHA256SUMS.txt publie avec la release) ;
# 3. l'installe dans ~/.local/share/locaryn/morph-browser (aucun droit administrateur) ;
# 4. cree le code d'appairage de l'extension (lisible par vous seul) ;
# 5. affiche le bloc MCP a coller dans votre outil, et les etapes pour l'extension.
#
# Rien n'est modifie en dehors de ces deux dossiers.
set -euo pipefail

REPO="Locaryn/morph-browser"
NAME="morph-browser"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/share/locaryn/morph-browser}"
DATA_DIR="${LOCARYN_EXTENSION_DATA_DIR:-$HOME/locaryn/morph-browser}"
VERSION="${VERSION:-}"

step() { printf '\033[32m-> %s\033[0m\n' "$1"; }
die() { printf 'Erreur : %s\n' "$1" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "'$1' est requis et introuvable."; }

need curl; need unzip
if command -v sha256sum >/dev/null 2>&1; then SHA="sha256sum"; else need shasum; SHA="shasum -a 256"; fi

# -- Systeme ---------------------------------------------------------------
if [ -n "${TARGET:-}" ]; then
  : # cible imposee (essais)
else
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) TARGET="linux-x86_64" ;;
    Darwin-arm64) TARGET="macos-aarch64" ;;
    Darwin-x86_64) TARGET="macos-x86_64" ;;
    *) die "systeme non pris en charge : $(uname -s) $(uname -m) (Linux x86_64, macOS Intel ou Apple)." ;;
  esac
fi

# -- La release -----------------------------------------------------------
step "Recherche de la derniere version"
if [ -n "$VERSION" ]; then
  API="https://api.github.com/repos/$REPO/releases/tags/$VERSION"
else
  # /releases/latest ignorerait les prereleases : toutes les versions actuelles en sont.
  API="https://api.github.com/repos/$REPO/releases?per_page=1"
fi
JSON="$(curl -fsSL -H 'User-Agent: locaryn-install' "$API")"
urls() { printf '%s' "$JSON" | grep -o '"browser_download_url": *"[^"]*"' | sed 's/.*: *"//; s/"$//'; }
ZIP_URL="$(urls | grep -- "-$TARGET\.zip\$" | head -n 1 || true)"
SUMS_URL="$(urls | grep -- '/SHA256SUMS\.txt$' | head -n 1 || true)"
[ -n "$ZIP_URL" ] || die "aucune archive $TARGET dans la derniere release."
[ -n "$SUMS_URL" ] || die "la release ne publie pas SHA256SUMS.txt : installation refusee, impossible de verifier le telechargement."
ZIP_NAME="${ZIP_URL##*/}"
echo "   $ZIP_NAME"

# -- Telechargement et verification ---------------------------------------
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
step "Telechargement"
curl -fsSL -o "$TMP/$ZIP_NAME" "$ZIP_URL"
curl -fsSL -o "$TMP/SHA256SUMS.txt" "$SUMS_URL"

step "Verification SHA-256"
EXPECTED="$(grep -F "$ZIP_NAME" "$TMP/SHA256SUMS.txt" | head -n 1 | awk '{print tolower($1)}')"
[ -n "$EXPECTED" ] || die "SHA256SUMS.txt ne mentionne pas $ZIP_NAME."
ACTUAL="$($SHA "$TMP/$ZIP_NAME" | awk '{print tolower($1)}')"
[ "$EXPECTED" = "$ACTUAL" ] || die "EMPREINTE DIFFERENTE : attendue $EXPECTED, recue $ACTUAL. Rien n'a ete installe."
echo "   $ACTUAL  (conforme)"

# -- Installation ---------------------------------------------------------
step "Installation dans $INSTALL_DIR"
mkdir -p "$INSTALL_DIR"
unzip -q -o "$TMP/$ZIP_NAME" -d "$INSTALL_DIR"
EXE="$INSTALL_DIR/bin/locaryn-browser-mcp"
[ -f "$EXE" ] || EXE="$EXE.exe"
[ -f "$EXE" ] || die "installation incomplete : le serveur manque."
chmod +x "$EXE" 2>/dev/null || true
[ -f "$INSTALL_DIR/extension/manifest.json" ] || die "installation incomplete : l'extension manque."

# -- Code d'appairage (jamais ecrase s'il existe deja) --------------------
mkdir -p "$DATA_DIR"
TOKEN_FILE="$DATA_DIR/pairing-token"
if [ ! -s "$TOKEN_FILE" ] || [ "$(wc -c < "$TOKEN_FILE")" -lt 32 ]; then
  ( umask 077; od -An -N20 -tx1 /dev/urandom | tr -d ' \n' > "$TOKEN_FILE" )
fi
chmod 600 "$TOKEN_FILE"
CODE="$(tr -d ' \n\r' < "$TOKEN_FILE")"

# -- Le bloc MCP ----------------------------------------------------------
ENV_PART=""
if [ "$DATA_DIR" != "$HOME/locaryn/morph-browser" ]; then
  ENV_PART=",
      \"env\": { \"LOCARYN_EXTENSION_DATA_DIR\": \"$DATA_DIR\" }"
fi
BLOC="{
  \"mcpServers\": {
    \"$NAME\": {
      \"command\": \"$EXE\",
      \"args\": []$ENV_PART
    }
  }
}"
if command -v pbcopy >/dev/null 2>&1; then printf '%s' "$BLOC" | pbcopy
elif command -v xclip >/dev/null 2>&1; then printf '%s' "$BLOC" | xclip -selection clipboard
elif command -v wl-copy >/dev/null 2>&1; then printf '%s' "$BLOC" | wl-copy; fi

printf '\n\033[32mInstalle.\033[0m\n\nBloc MCP a coller dans votre outil (« + MCP » dans Freebuff, configuration brute d'\''Antigravity ou de Claude) :\n\n%s\n\n' "$BLOC"
printf '\033[33mETAPE SUIVANTE - installer l'\''extension dans le navigateur (une seule fois) :\033[0m\n'
printf "  Chrome / Edge / Brave / Opera / Vivaldi : page des extensions -> mode developpeur -> 'Charger l'extension non empaquetee' ->\n      %s/extension\n" "$INSTALL_DIR"
printf "  Firefox : about:debugging#/runtime/this-firefox -> 'Charger un module complementaire temporaire' ->\n      %s/extension-firefox/manifest.json   (puis about:addons -> Autorisations -> tous les sites)\n" "$INSTALL_DIR"
printf "  Puis cliquez l'icone de l'extension, collez ce code d'appairage et 'Enregistrer' :\n      \033[36m%s\033[0m\n\n" "$CODE"
printf "Securite : le modele agit dans vos onglets deja connectes (messagerie, banque...). Un profil de navigateur\ndedie est le meilleur reglage. Cadre anime + pastille + bouton Stop sur chaque onglet controle.\n"
printf "Guide complet : https://github.com/%s/blob/main/docs/INSTALL-AUTRES-OUTILS-IA.md\n" "$REPO"
