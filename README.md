# morph-browser

Contrôle **tout** le navigateur — pas seulement l'onglet ouvert : liste des fenêtres et onglets, retrouver un onglet déjà ouvert, l'activer, en ouvrir ou en fermer, lire, cliquer, saisir, faire défiler, capturer, fouiller l'historique.

Navigateurs : Chrome, Edge, Brave, Arc, Vivaldi, Opera, Comet (Chromium ≥ 116) et Firefox (≥ 128). Safari n'est pas pris en charge.

## Installer en une commande

```powershell
irm https://raw.githubusercontent.com/Locaryn/morph-browser/main/install.ps1 | iex
```

Linux / macOS : `curl -fsSL https://raw.githubusercontent.com/Locaryn/morph-browser/main/install.sh | bash`

Télécharge la dernière release, vérifie son empreinte SHA-256, l'installe sans droits administrateur et affiche le bloc MCP à coller dans Freebuff (`+ MCP`), Antigravity ou Claude. Détails et réglages de sécurité : [docs/INSTALL-AUTRES-OUTILS-IA.md](docs/INSTALL-AUTRES-OUTILS-IA.md).


## Ce que voit l'utilisateur

- **Un cadre animé** fait le tour de l'onglet contrôlé, avec une pastille « Locaryn contrôle cet onglet » et un bouton **Stop**.
- **Dans la barre d'onglets** : le titre de l'onglet est précédé de `◉` et son icône porte une pastille verte ; l'icône de l'extension affiche `◉` sur cet onglet. Option : regrouper les onglets contrôlés dans un groupe « Locaryn ».
- **Un curseur** se déplace dans la page et émet une onde à chaque clic (gauche ou droit).
- **Stop** coupe le contrôle de l'onglet immédiatement ; l'onglet reste bloqué jusqu'à ce que l'utilisateur le réautorise depuis l'icône de l'extension.
- Le cadre reste affiché pendant toute la tâche ; `browser_release` le retire, et un filet de sécurité l'efface après 3 min sans commande.
- **Barre latérale** (clic sur l'icône de l'extension) : un chat pour donner une consigne à Locaryn sans quitter la page. La consigne part dans la conversation ouverte de Locaryn (l'application doit être ouverte), avec l'onglet d'où elle vient ; la réponse revient dans la barre. Les réglages (code, port, onglets arrêtés) sont derrière `⋯`.
- **Clic droit › « Demander à Locaryn »** : ouvre la barre latérale et joint la sélection (ou le lien, l'image, la page).
- **Sélecteur de modèle** en bas de la barre latérale : le modèle de la conversation ouverte dans Locaryn (Locaryn après la 0.4.74).
- **Plusieurs navigateurs** peuvent être reliés en même temps (Comet, Opera, Chrome…). Chacun est nommé d'après le programme qui tient la connexion (`comet`, `opera`, `chrome`, `chrome 2` pour un second profil) ; les outils prennent `browser` pour viser l'un d'eux, sinon le navigateur des réglages, sinon le dernier relié. Le bouton **Navigateurs reliés** du panneau du morph les liste avec la version de leur extension.

## Installation

1. Installer le morph. Il lance un serveur local (`127.0.0.1`, port 17421 par défaut).
2. Dans le navigateur :
   - **Chrome, Edge, Brave, Arc, Vivaldi, Opera, Comet** : `chrome://extensions` (ou `edge://extensions`…) → mode développeur → **Charger l'extension non empaquetée** → dossier `extension/` du morph.
   - **Firefox** : `about:debugging#/runtime/this-firefox` → **Charger un module complémentaire temporaire** → fichier `extension-firefox/manifest.json`. Puis, dans `about:addons` → l'extension → Autorisations, accordez l'accès à **tous les sites** : Firefox ne l'accorde pas à l'installation, et sans lui l'extension ne peut lire aucune page. Un module temporaire disparaît à la fermeture de Firefox ; un module permanent exige une extension signée (publication sur addons.mozilla.org, pas encore faite).
3. Relier : dans Locaryn, Réglages de l'extension → **Relier au navigateur** (ou demandez « relie mon navigateur » dans le chat). Le navigateur choisi — « auto » : un navigateur déjà ouvert, sinon celui de l'ordinateur — ouvre un onglet de guide (dans la fenêtre déjà ouverte, sans nouvelle fenêtre) qui donne l'adresse de sa page des extensions et le chemin du dossier, chacun avec un bouton Copier ; le chemin du dossier est déjà dans le presse-papiers. Les navigateurs Chromium refusent qu'un autre programme ouvre leurs pages internes : d'où l'adresse à coller. Une fois l'extension chargée, elle se relie seule dans les deux minutes, sans code à recopier. Le code d'appairage reste dans `pairing-token` (`%APPDATA%\locaryn\morph-browser\` sous Windows) pour un appairage à la main depuis l'icône de l'extension.

Le pont refuse toute origine qui n'est pas une extension de navigateur et tout client sans le code d'appairage.

### Mettre à jour l'extension

L'extension est chargée « non empaquetée » depuis le dossier `extension/` du morph : une mise à jour du morph remplace ce dossier, mais le navigateur garde l'ancien code tant qu'il ne l'a pas relu.

L'extension porte la version du morph : morph `0.1.0-beta.17` → extension `0.1.0.17` (Chrome n'accepte que des chiffres), affichée « Extension 0.1.0-beta.17 » dans la barre latérale. `node scripts/sync-version.mjs` aligne les manifestes sur `morph.json` ; la CI refuse une release désalignée.

- **Depuis l'extension 0.2.0** : rien à faire. À la connexion, le serveur compare la version de l'extension à celle du dossier et lui demande de se recharger (une fois par version).
- **Extension 0.1.x (une seule fois)** : ouvrez `chrome://extensions` (Comet : la même adresse), repérez « Locaryn — contrôle du navigateur » et cliquez sur la flèche circulaire **Recharger**. Si l'extension avait été chargée depuis un autre dossier, retirez-la et rechargez le dossier `extension/` du morph installé (`~/.lochor/plugins/morph-browser/extension`).
- **Firefox** : un module temporaire se recharge depuis `about:debugging` → **Recharger**.

## Laya

[Laya](https://github.com/NandhaKishorM/laya) est un classifieur non autorégressif : il répond à des questions fermées en un seul passage (~50 ms sur GPU mesuré ici, RTX 4050). Il **ne planifie pas** : le modèle de conversation garde la conduite de la tâche. Laya sert à :

| Outil | Question posée à Laya |
| --- | --- |
| `browser_find_element` | Quel élément de la page sert cet objectif ? |
| `browser_find_tab` | Lequel des onglets ouverts correspond à cette description ? |
| garde-fou de `browser_click_element` / `browser_type_text` | Cette action est-elle irréversible (achat, suppression, envoi) ? |
| `browser_goal_reached` | L'objectif est-il atteint d'après le texte de la page ? |

Sans Laya, `find_element` et `find_tab` retombent sur un classement par mots et le signalent (`engine: lexical`) ; le garde-fou garde son repli lexical.

Installation de Laya : `python -m pip install laya` (avec un PyTorch CUDA pour la vitesse). Le premier chargement dure une à deux minutes ; il démarre en arrière-plan dès le premier outil utilisé.

Réglages (fichier de configuration de l'extension) : `port`, `confirm_risky`, `laya_risk_check`, `laya_checkpoint` (`multilingual` par défaut, `english`, `typed-decisions`, `router`), `laya_device`.

## Limites connues

- Pas encore publiée sur le Chrome Web Store ni addons.mozilla.org : l'installation passe par le mode développeur. Publier supprimerait cette étape, mais demande un compte éditeur et une validation.
- Firefox : testée par analyse statique (`web-ext lint`, 0 erreur) mais pas encore sur un Firefox réel ; pas de regroupement d'onglets (Firefox n'a pas cette API).
- Les clics sont des événements DOM (`isTrusted: false`) : quelques sites les ignorent.
- Le snapshot lit la page principale et les shadow DOM ouverts, pas le contenu des `iframe`.
- Les pages internes du navigateur (`chrome://…`, Chrome Web Store) ne sont pas pilotables.
- `browser_evaluate` est bloqué par la CSP de certains sites.
- Laya départage des candidats ; sa confiance est une aide, pas une garantie.
