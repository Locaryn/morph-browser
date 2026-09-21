# morph-browser

Contrôle **tout** le navigateur — pas seulement l'onglet ouvert : liste des fenêtres et onglets, retrouver un onglet déjà ouvert, l'activer, en ouvrir ou en fermer, lire, cliquer, saisir, faire défiler, capturer, fouiller l'historique.

Navigateurs : Chrome, Edge, Brave, Arc, Vivaldi (Chromium ≥ 116). Firefox n'est pas pris en charge.

## Ce que voit l'utilisateur

- **Un cadre animé** fait le tour de l'onglet contrôlé, avec une pastille « Locaryn contrôle cet onglet » et un bouton **Stop**.
- **Dans la barre d'onglets** : le titre de l'onglet est précédé de `◉` et son icône porte une pastille verte ; l'icône de l'extension affiche `◉` sur cet onglet. Option : regrouper les onglets contrôlés dans un groupe « Locaryn ».
- **Un curseur** se déplace dans la page et émet une onde à chaque clic (gauche ou droit).
- **Stop** coupe le contrôle de l'onglet immédiatement ; l'onglet reste bloqué jusqu'à ce que l'utilisateur le réautorise depuis l'icône de l'extension.
- Le cadre disparaît seul après 10 s sans commande.

## Installation

1. Installer le morph. Il lance un serveur local (`127.0.0.1`, port 17421 par défaut).
2. Dans le navigateur : `chrome://extensions` → mode développeur → **Charger l'extension non empaquetée** → dossier `extension/` du morph.
3. Demander à Locaryn `browser_status` : il rend le code d'appairage. Le coller dans l'icône de l'extension, **Enregistrer**.

Le pont refuse toute origine qui n'est pas une extension de navigateur et tout client sans le code d'appairage.

## Laya

[Laya](https://github.com/NandhaKishorM/laya) est un classifieur non autorégressif : il répond à des questions fermées en un seul passage (~50 ms sur GPU mesuré ici, RTX 4050). Il **ne planifie pas** : le modèle de conversation garde la conduite de la tâche. Laya sert à :

| Outil | Question posée à Laya |
| --- | --- |
| `browser_find_element` | Quel élément de la page sert cet objectif ? |
| `browser_find_tab` | Lequel des onglets ouverts correspond à cette description ? |
| garde-fou de `browser_click` / `browser_type` | Cette action est-elle irréversible (achat, suppression, envoi) ? |
| `browser_goal_reached` | L'objectif est-il atteint d'après le texte de la page ? |

Sans Laya, `find_element` et `find_tab` retombent sur un classement par mots et le signalent (`engine: lexical`) ; le garde-fou garde son repli lexical.

Installation de Laya : `python -m pip install laya` (avec un PyTorch CUDA pour la vitesse). Le premier chargement dure une à deux minutes ; il démarre en arrière-plan dès le premier outil utilisé.

Réglages (fichier de configuration de l'extension) : `port`, `confirm_risky`, `laya_risk_check`, `laya_checkpoint` (`multilingual` par défaut, `english`, `typed-decisions`, `router`), `laya_device`.

## Limites connues

- Les clics sont des événements DOM (`isTrusted: false`) : quelques sites les ignorent.
- Le snapshot lit la page principale et les shadow DOM ouverts, pas le contenu des `iframe`.
- Les pages internes du navigateur (`chrome://…`, Chrome Web Store) ne sont pas pilotables.
- `browser_evaluate` est bloqué par la CSP de certains sites.
- Laya départage des candidats ; sa confiance est une aide, pas une garantie.
