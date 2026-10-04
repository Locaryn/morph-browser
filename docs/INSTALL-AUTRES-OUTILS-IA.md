# Utiliser morph-browser depuis un autre outil d'IA

`morph-browser` est un serveur **MCP** (stdio) accompagné d'une **extension de navigateur**. Il fonctionne sans l'application Locaryn : tout client MCP peut le lancer et donner à son modèle la main sur vos onglets (lister, retrouver, lire, cliquer, saisir, naviguer, capturer, historique).

Clients couverts : Claude Desktop, Claude Code, Antigravity, Freebuff, et tout autre client MCP. Navigateurs : Chrome, Edge, Brave, Arc, Vivaldi, Opera, Firefox (≥ 128).

## Ce que vous autorisez vraiment

Le modèle agit **dans les sessions déjà ouvertes** de votre navigateur : si vous êtes connecté à votre messagerie, votre banque ou un compte d'entreprise, il peut y lire et y cliquer comme vous. L'extension demande l'accès à tous les sites (c'est ce qui permet de piloter n'importe quel onglet).

Ce qui limite le risque :

1. **Votre client** demande votre accord avant d'appeler un outil. Ne l'éteignez pas pour ces outils.
2. **L'extension** : un cadre animé et une pastille « Locaryn contrôle cet onglet » avec un bouton **Stop** s'affichent dans chaque onglet contrôlé, son titre est précédé de `◉`. Stop bloque l'onglet jusqu'à ce que vous le réautorisiez depuis l'icône de l'extension.
3. **Le garde-fou du serveur** : un clic ou une saisie jugés irréversibles (achat, suppression, envoi) ne s'exécutent pas ; le modèle doit vous demander votre accord.
4. **Le pont local** : le serveur n'écoute que sur `127.0.0.1`, refuse toute origine qui n'est pas une extension de navigateur et exige un code d'appairage. Une page web ordinaire ne peut donc pas lui parler.

À ne pas faire : piloter un navigateur où sont ouverts des comptes sensibles ; laisser tourner sans surveillance ; installer l'extension dans votre profil principal si vous pouvez créer un **profil dédié** (c'est le meilleur réglage : le modèle n'y voit que ce que vous y connectez) ; croire qu'une page web est digne de confiance (elle peut contenir des consignes destinées au modèle).

## Installation en une commande

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/Locaryn/morph-browser/main/install.ps1 | iex
```

**Linux / macOS**

```bash
curl -fsSL https://raw.githubusercontent.com/Locaryn/morph-browser/main/install.sh | bash
```

Le script télécharge la dernière release, **vérifie son empreinte SHA-256**, l'installe dans votre dossier utilisateur (aucun droit administrateur), crée le code d'appairage, puis affiche — et copie dans le presse-papiers — le bloc MCP à coller :

- **Freebuff** : `+ MCP`, collez le bloc.
- **Antigravity** : menu des serveurs MCP → configuration brute → collez le bloc.
- **Claude Desktop / Claude Code** : voir plus bas, ou, sous Windows, `-Client claude` (ajoute l'entrée après sauvegarde du fichier).

Il reste **une seule étape manuelle**, imposée par les navigateurs : charger l'extension (le script affiche le dossier exact et le code à coller dans son icône). Vous pouvez lire les scripts avant de les lancer : [install.ps1](https://github.com/Locaryn/morph-browser/blob/main/install.ps1), [install.sh](https://github.com/Locaryn/morph-browser/blob/main/install.sh).

Le reste de cette page détaille chaque étape, pour qui préfère tout faire à la main.

## 1. Télécharger et vérifier

1. Téléchargez l'archive de votre système (`morph-browser-v<version>-windows-x86_64.zip`, `…-linux-x86_64.zip`, `…-macos-aarch64.zip` ou `…-macos-x86_64.zip`) depuis <https://github.com/Locaryn/morph-browser/releases>, et nulle part ailleurs.
2. Comparez son empreinte à celle de `SHA256SUMS.txt`, sur la même page :
   - Windows : `(Get-FileHash .\<archive>.zip -Algorithm SHA256).Hash`
   - Linux / macOS : `shasum -a 256 <archive>.zip`
   Si les valeurs diffèrent, n'installez rien.
3. Extrayez dans un dossier dont vous êtes propriétaire. Vous y trouvez `bin/` (le serveur), `extension/` (navigateurs Chromium) et `extension-firefox/` (Firefox).
4. Les binaires ne sont pas signés : Windows SmartScreen ou macOS Gatekeeper peuvent avertir au premier lancement. Pour ne faire confiance qu'à du code lu : `cargo build --release --locked` dans le dépôt.

## 2. Configurer votre client

Remplacez le chemin par le vôtre ; dans un fichier JSON, chaque `\` s'écrit `\\`.

### Claude Desktop

Fichier `%APPDATA%\Claude\claude_desktop_config.json` (macOS : `~/Library/Application Support/Claude/claude_desktop_config.json`). Ajoutez sous `mcpServers`, sans toucher aux autres entrées :

```json
{
  "mcpServers": {
    "morph-browser": {
      "command": "C:\\Users\\<vous>\\AppData\\Local\\Locaryn\\morph-browser\\bin\\locaryn-browser-mcp.exe",
      "args": []
    }
  }
}
```

Quittez complètement Claude Desktop, relancez-le. Préférez « autoriser une fois » à « toujours autoriser » pour `browser_click`, `browser_type` et `browser_evaluate`.

### Claude Code

```bash
claude mcp add morph-browser -- "C:\Users\<vous>\AppData\Local\Locaryn\morph-browser\bin\locaryn-browser-mcp.exe"
```

(`claude mcp add --help` donne la syntaxe de votre version.) N'utilisez pas la portée « projet partagé » : elle écrirait votre chemin dans un fichier versionné. Dans `/permissions`, ne mettez pas `mcp__morph-browser` en entier dans « autorisé ».

### Antigravity

Fichier `%USERPROFILE%\.gemini\antigravity\mcp_config.json`, même forme :

```json
{
  "mcpServers": {
    "morph-browser": {
      "command": "C:\\Users\\<vous>\\AppData\\Local\\Locaryn\\morph-browser\\bin\\locaryn-browser-mcp.exe",
      "args": []
    }
  }
}
```

Ajoutez l'entrée aux existantes, rechargez la liste des serveurs, laissez la validation des actions sur « demander ».

### Freebuff

Dans Freebuff : `+ MCP`, puis collez le bloc affiché par le script (ou la configuration JSON de Claude Desktop ci-dessus). Même forme, aucun autre réglage.

### Un autre client MCP

Commande = chemin de `locaryn-browser-mcp(.exe)`, aucun argument.

## 3. Installer l'extension et l'appairer

1. Lancez une fois le client avec le serveur configuré (il crée alors le **code d'appairage**, propre à votre machine), puis lisez-le vous-même dans le fichier :
   - Windows : `Get-Content $env:APPDATA\locaryn\morph-browser\pairing-token`
   - Linux / macOS : `cat "$HOME/locaryn/morph-browser/pairing-token"`

   L'outil `browser_status` rend aussi ce code, mais **sa réponse passe par le modèle, donc par votre fournisseur d'IA** : préférez le fichier. Le port est 17421 par défaut.
2. Installez l'extension :
   - **Chrome, Edge, Brave, Arc, Vivaldi, Opera** : page des extensions → mode développeur → *Charger l'extension non empaquetée* → dossier `extension/`.
   - **Firefox** : `about:debugging#/runtime/this-firefox` → *Charger un module complémentaire temporaire* → `extension-firefox/manifest.json`, puis `about:addons` → l'extension → Autorisations → accès à **tous les sites**. Un module temporaire disparaît à la fermeture de Firefox.
3. Cliquez l'icône de l'extension, collez le code, *Enregistrer*. Le voyant passe à « Connecté à Locaryn ».
4. Gardez le code **secret** : quiconque l'a et peut lancer un programme sur votre machine peut piloter le navigateur apparié. Il est stocké dans le fichier ci-dessus ; sous Linux et macOS, il n'est lisible que par vous.

## 4. Vérifier

Demandez `browser_list_tabs` : vos onglets doivent apparaître. Puis `browser_snapshot` sur un onglet de test : le cadre vert et la pastille doivent s'afficher dessus. Sinon, ne continuez pas.

## 5. Laya (facultatif)

Sans lui, tout marche (classement par mots et liste de mots à la place). Avec : `python -m pip install laya` — il télécharge un modèle depuis Hugging Face, premier chargement d'une à deux minutes. À ne faire que si vous l'acceptez.

## 6. Couper, retirer

- Bouton **Stop** dans l'onglet, ou retirez l'extension.
- Retirer l'accès du modèle : supprimez l'entrée `morph-browser` du client.
- Changer le code d'appairage : supprimez le fichier `pairing-token` ; un nouveau est créé au prochain lancement, à recoller dans l'extension.
