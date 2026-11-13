//! Pont WebSocket entre le serveur MCP et les extensions des navigateurs.
//!
//! Le serveur n'écoute que sur 127.0.0.1, n'accepte qu'une origine
//! d'extension, et exige un code d'appairage : sans cela, n'importe quelle
//! page web ouverte pourrait s'adresser au pont.
//!
//! Plusieurs navigateurs peuvent être reliés en même temps (Comet, Opera,
//! Chrome…) : chacun est une liaison nommée, et une commande vise celui que
//! donne `params.browser`, sinon le navigateur préféré, sinon le dernier relié.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;

const DELAI_APPAIRAGE: Duration = Duration::from_secs(5);
const DELAI_COMMANDE: Duration = Duration::from_secs(45);

/// Au-delà, les plus anciennes consignes non relevées sont oubliées :
/// Locaryn est fermé, personne ne les lira.
const MAX_PANNEAU: usize = 20;

/// Le rythme auquel une instance sans port réessaie de le prendre.
const REPRISE: Duration = Duration::from_secs(5);

/// Le refus envoyé à une extension qui n'a pas encore de code : elle
/// réessaie en silence, le journal n'a pas à s'en remplir.
const SANS_CODE: &str = "en attente d'appairage";

type Attente = oneshot::Sender<Result<Value, String>>;

/// Un navigateur relié.
struct Liaison {
    /// `comet`, `opera`, `chrome`… ; unique parmi les liaisons ouvertes.
    nom: String,
    tx: mpsc::UnboundedSender<String>,
    hello: Value,
    /// Onglets sur lesquels la personne a pressé Stop.
    bloques: Vec<u64>,
}

/// La fenêtre ouverte par « Relier » : jusqu'à `fin`, une extension sans
/// code reçoit le code. `vise` : le navigateur choisi, s'il est connu.
struct Appairage {
    fin: Instant,
    vise: Option<String>,
}

pub struct Bridge {
    pub port: u16,
    token: String,
    liaisons: Mutex<HashMap<u64, Liaison>>,
    /// Commandes en attente de réponse, avec la liaison qui doit répondre.
    pending: Mutex<HashMap<u64, (u64, Attente)>>,
    next_id: AtomicU64,
    next_conn: AtomicU64,
    bind_error: Mutex<Option<String>>,
    appairage: Mutex<Option<Appairage>>,
    /// Les consignes tapées dans les barres latérales, en attente d'être
    /// relevées par le panneau du morph dans Locaryn.
    panneau: Mutex<Vec<Value>>,
    /// Les versions d'extension à qui l'on a déjà demandé de se recharger :
    /// une seule demande par version, sinon une copie chargée ailleurs que
    /// dans le dossier du morph se rechargerait en boucle.
    rechargements: Mutex<Vec<String>>,
    /// Le navigateur visé par défaut (réglage « Votre navigateur »).
    prefere: Mutex<String>,
}

impl Bridge {
    /// Démarre l'écoute. Un port pris n'empêche pas le serveur MCP de vivre :
    /// une autre instance du morph le tient, on passe par elle.
    pub async fn start(port: u16, token: String) -> Arc<Self> {
        let bridge = Arc::new(Self {
            port,
            token,
            liaisons: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            next_conn: AtomicU64::new(1),
            bind_error: Mutex::new(None),
            appairage: Mutex::new(None),
            panneau: Mutex::new(Vec::new()),
            rechargements: Mutex::new(Vec::new()),
            prefere: Mutex::new(String::new()),
        });
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => {
                let b = bridge.clone();
                tokio::spawn(async move { b.accept_loop(listener).await });
            }
            Err(e) => {
                *bridge.bind_error.lock().await = Some(format!(
                    "port {port} tenu par une autre instance du morph ({e}) : les commandes passent par elle"
                ));
                let b = bridge.clone();
                tokio::spawn(async move { b.reprendre_le_port().await });
            }
        }
        bridge
    }

    /// Le navigateur visé quand une commande n'en nomme aucun.
    pub async fn preferer(&self, nom: &str) {
        *self.prefere.lock().await = nom.trim().to_ascii_lowercase();
    }

    /// Réessaie d'écouter tant qu'une autre instance tient le port.
    async fn reprendre_le_port(self: Arc<Self>) {
        loop {
            tokio::time::sleep(REPRISE).await;
            if let Ok(listener) = TcpListener::bind(("127.0.0.1", self.port)).await {
                eprintln!("pont navigateur : port {} repris", self.port);
                *self.bind_error.lock().await = None;
                self.clone().accept_loop(listener).await;
                return;
            }
        }
    }

    /// Vrai quand une autre instance du morph tient le port.
    async fn en_relais(&self) -> bool {
        self.bind_error.lock().await.is_some()
    }

    async fn relayer(&self, action: &str, params: Value) -> Result<Value, String> {
        crate::relais::demander(
            self.port,
            &self.token,
            action,
            params,
            DELAI_COMMANDE + Duration::from_secs(5),
        )
        .await
    }

    pub async fn bind_error(&self) -> Option<String> {
        self.bind_error.lock().await.clone()
    }

    // ── Appairage ───────────────────────────────────────────────────────────

    /// Ouvre la fenêtre d'appairage : la première extension sans code qui se
    /// présente dans ce délai le reçoit — celle du navigateur `vise`, s'il est
    /// donné et reconnaissable. Le code ne passe jamais par le modèle de
    /// conversation ni par l'écran.
    pub async fn ouvrir_appairage(&self, duree: Duration, vise: Option<&str>) {
        if self.en_relais().await {
            let params = json!({ "seconds": duree.as_secs(), "browser": vise });
            if let Err(e) = self.relayer("open_pairing", params).await {
                eprintln!("pont navigateur : appairage non ouvert : {e}");
            }
            return;
        }
        *self.appairage.lock().await = Some(Appairage {
            fin: Instant::now() + duree,
            vise: vise.map(str::to_string),
        });
    }

    /// Prend la fenêtre si elle est encore ouverte et vise ce navigateur :
    /// un seul appairage par ouverture.
    async fn prendre_appairage(&self, nom: &str) -> bool {
        let mut fenetre = self.appairage.lock().await;
        let ok = match fenetre.as_ref() {
            Some(a) if Instant::now() < a.fin => a.vise.as_deref().is_none_or(|v| v == nom),
            _ => false,
        };
        if ok {
            *fenetre = None;
        }
        ok
    }

    // ── État ────────────────────────────────────────────────────────────────

    /// Les navigateurs reliés, avec la version de leur extension.
    pub async fn navigateurs(&self) -> Vec<Value> {
        if self.en_relais().await {
            return match self.relayer("status", json!({})).await {
                Ok(v) => serde_json::from_value(v["browsers"].clone()).unwrap_or_default(),
                Err(e) => {
                    eprintln!("pont navigateur : {e}");
                    Vec::new()
                }
            };
        }
        self.navigateurs_ici().await
    }

    async fn navigateurs_ici(&self) -> Vec<Value> {
        let liaisons = self.liaisons.lock().await;
        let mut v: Vec<(u64, Value)> = liaisons
            .iter()
            .map(|(conn, l)| {
                (
                    *conn,
                    json!({
                        "browser": l.nom,
                        "extension_version": l.hello.get("version_name")
                            .or_else(|| l.hello.get("version"))
                            .cloned()
                            .unwrap_or(Value::Null),
                        "tabs_stopped_by_user": l.bloques,
                    }),
                )
            })
            .collect();
        v.sort_by_key(|(conn, _)| *conn);
        v.into_iter().map(|(_, x)| x).collect()
    }

    pub async fn is_connected(&self) -> bool {
        !self.navigateurs().await.is_empty()
    }

    /// Ce navigateur est-il relié ?
    pub async fn est_relie(&self, nom: &str) -> bool {
        self.navigateurs()
            .await
            .iter()
            .any(|n| n["browser"].as_str() == Some(nom))
    }

    // ── Barre latérale ──────────────────────────────────────────────────────

    /// Prend les consignes des barres latérales, et dit à chaque extension
    /// que les siennes sont arrivées dans Locaryn.
    pub async fn prendre_messages_panneau(&self) -> Vec<Value> {
        if self.en_relais().await {
            return match self.relayer("panel_take", json!({})).await {
                Ok(Value::Array(pris)) => pris,
                Ok(_) => Vec::new(),
                Err(e) => {
                    eprintln!("pont navigateur : {e}");
                    Vec::new()
                }
            };
        }
        let pris: Vec<Value> = std::mem::take(&mut *self.panneau.lock().await);
        let mut par_navigateur: HashMap<String, Vec<Value>> = HashMap::new();
        for m in &pris {
            let nom = m["browser"].as_str().unwrap_or("").to_string();
            par_navigateur.entry(nom).or_default().push(m["id"].clone());
        }
        for (nom, ids) in par_navigateur {
            let accuse = json!({ "type": "panel_ack", "ids": ids });
            if let Err(e) = self.pousser(accuse, Some(&nom)).await {
                eprintln!("pont navigateur : accusé de réception non transmis : {e}");
            }
        }
        pris
    }

    /// Envoie un message sans attendre de réponse : au navigateur `cible`, ou
    /// à tous.
    pub async fn pousser(&self, message: Value, cible: Option<&str>) -> Result<(), String> {
        if self.en_relais().await {
            return self
                .relayer("push", json!({ "message": message, "browser": cible }))
                .await
                .map(|_| ());
        }
        let liaisons = self.liaisons.lock().await;
        let mut envoye = false;
        for l in liaisons.values() {
            if cible.is_none_or(|c| c.is_empty() || c == l.nom) {
                envoye |= l.tx.send(message.to_string()).is_ok();
            }
        }
        if envoye {
            Ok(())
        } else {
            Err(match cible {
                Some(c) if !c.is_empty() => format!("Le navigateur « {c} » n'est pas relié."),
                _ => "Aucun navigateur relié à Locaryn.".to_string(),
            })
        }
    }

    /// Onglets sur lesquels la personne a pressé Stop, navigateur par navigateur.
    pub async fn blocked_tabs(&self) -> Vec<Value> {
        self.navigateurs()
            .await
            .into_iter()
            .flat_map(|n| {
                let nom = n["browser"].clone();
                n["tabs_stopped_by_user"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |id| json!({ "browser": nom, "tab_id": id }))
            })
            .collect()
    }

    // ── Commandes ───────────────────────────────────────────────────────────

    /// Envoie une commande à un navigateur et attend sa réponse. Le navigateur
    /// est `params.browser`, sinon le préféré, sinon le dernier relié ; la
    /// liste des onglets sans navigateur nommé les réunit tous.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        if self.en_relais().await {
            return self
                .relayer("call", json!({ "method": method, "params": params }))
                .await;
        }
        let nomme = params["browser"]
            .as_str()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty());
        if method == "tabs_list" && nomme.is_none() {
            return self.tous_les_onglets(params).await;
        }
        let (conn, nom) = self.choisir(nomme.as_deref()).await?;
        let res = self.call_sur(conn, method, params).await?;
        Ok(annoter(res, &nom))
    }

    /// Les onglets de tous les navigateurs reliés, chacun avec son navigateur.
    async fn tous_les_onglets(&self, params: Value) -> Result<Value, String> {
        let cibles: Vec<(u64, String)> = {
            let liaisons = self.liaisons.lock().await;
            liaisons.iter().map(|(c, l)| (*c, l.nom.clone())).collect()
        };
        if cibles.is_empty() {
            return Err(PAS_RELIE.to_string());
        }
        // Tous à la fois : un navigateur lent ne retarde pas les autres.
        let reponses = futures_util::future::join_all(cibles.into_iter().map(|(conn, nom)| {
            let params = params.clone();
            async move { (nom, self.call_sur(conn, "tabs_list", params).await) }
        }))
        .await;
        let mut onglets = Vec::new();
        let mut erreurs = Vec::new();
        for (nom, reponse) in reponses {
            match reponse {
                Ok(r) => {
                    let liste = r["tabs"].as_array().cloned().unwrap_or_default();
                    onglets.extend(liste.into_iter().map(|t| annoter(t, &nom)));
                }
                Err(e) => erreurs.push(json!({ "browser": nom, "error": e })),
            }
        }
        Ok(json!({ "tabs": onglets, "errors": erreurs }))
    }

    /// La liaison visée.
    async fn choisir(&self, nomme: Option<&str>) -> Result<(u64, String), String> {
        let liaisons = self.liaisons.lock().await;
        if liaisons.is_empty() {
            return Err(PAS_RELIE.to_string());
        }
        let trouver = |nom: &str| {
            liaisons
                .iter()
                .find(|(_, l)| l.nom == nom)
                .map(|(c, l)| (*c, l.nom.clone()))
        };
        if let Some(nom) = nomme {
            return trouver(nom).ok_or_else(|| {
                // Dans l'ordre où ils ont été reliés.
                let mut tries: Vec<(&u64, &Liaison)> = liaisons.iter().collect();
                tries.sort_by_key(|(c, _)| **c);
                let relies: Vec<&str> = tries.iter().map(|(_, l)| l.nom.as_str()).collect();
                format!(
                    "Le navigateur « {nom} » n'est pas relié. Navigateurs reliés : {}.",
                    relies.join(", ")
                )
            });
        }
        let prefere = self.prefere.lock().await.clone();
        if let Some(t) = Some(prefere.as_str())
            .filter(|p| !p.is_empty() && *p != "auto")
            .and_then(trouver)
        {
            return Ok(t);
        }
        liaisons
            .iter()
            .max_by_key(|(c, _)| **c)
            .map(|(c, l)| (*c, l.nom.clone()))
            .ok_or_else(|| PAS_RELIE.to_string())
    }

    async fn call_sur(&self, conn: u64, method: &str, params: Value) -> Result<Value, String> {
        let sender = self
            .liaisons
            .lock()
            .await
            .get(&conn)
            .map(|l| l.tx.clone())
            .ok_or("La connexion avec l'extension vient de tomber.")?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (rep_tx, rep_rx) = oneshot::channel();
        self.pending.lock().await.insert(id, (conn, rep_tx));
        let msg = json!({ "id": id, "method": method, "params": params }).to_string();
        if sender.send(msg).is_err() {
            self.pending.lock().await.remove(&id);
            return Err("La connexion avec l'extension vient de tomber.".into());
        }
        match timeout(DELAI_COMMANDE, rep_rx).await {
            Ok(Ok(res)) => res,
            Ok(Err(_)) => {
                Err("La connexion avec l'extension s'est fermée pendant la commande.".into())
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(format!(
                    "L'extension n'a pas répondu à « {method} » en {} s.",
                    DELAI_COMMANDE.as_secs()
                ))
            }
        }
    }

    // ── Instance sœur ───────────────────────────────────────────────────────

    /// Exécute ici l'action demandée par une instance sœur sans port.
    async fn servir_soeur(&self, texte: &str) -> Result<Value, String> {
        let demande: Value =
            serde_json::from_str(texte).map_err(|e| format!("relais : demande illisible : {e}"))?;
        let params = demande.get("params").cloned().unwrap_or(Value::Null);
        let navigateur = params["browser"].as_str().map(str::to_string);
        match demande.get("action").and_then(Value::as_str).unwrap_or("") {
            "status" => Ok(json!({ "browsers": self.navigateurs_ici().await })),
            "open_pairing" => {
                let secondes = params["seconds"].as_u64().unwrap_or(120).min(600);
                self.ouvrir_appairage(Duration::from_secs(secondes), navigateur.as_deref())
                    .await;
                Ok(json!(true))
            }
            "call" => {
                let methode = params["method"].as_str().unwrap_or("").to_string();
                self.call(&methode, params["params"].clone()).await
            }
            "panel_take" => Ok(Value::Array(self.prendre_messages_panneau().await)),
            "push" => self
                .pousser(params["message"].clone(), navigateur.as_deref())
                .await
                .map(|_| json!(true)),
            autre => Err(format!("relais : action inconnue « {autre} »")),
        }
    }

    // ── Connexions ──────────────────────────────────────────────────────────

    async fn accept_loop(self: Arc<Self>, listener: TcpListener) {
        loop {
            match listener.accept().await {
                Ok((stream, pair)) => {
                    let b = self.clone();
                    tokio::spawn(async move {
                        match b.handle(stream, pair.port()).await {
                            Err(e) if e != SANS_CODE => eprintln!("pont navigateur : {e}"),
                            _ => {}
                        }
                    });
                }
                Err(e) => eprintln!("pont navigateur : accept : {e}"),
            }
        }
    }

    async fn handle(self: Arc<Self>, stream: TcpStream, port_pair: u16) -> Result<(), String> {
        let soeur = Arc::new(AtomicBool::new(false));
        let soeur_vue = soeur.clone();
        let jeton_attendu = self.token.clone();
        let verifier = move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
            let entete = |nom: &str| {
                req.headers()
                    .get(nom)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string()
            };
            let origine = entete("origin");
            if origine_autorisee(&origine) {
                return Ok(resp);
            }
            // Une instance sœur du morph : jamais d'origine (une page web en
            // envoie toujours une), et le code d'appairage.
            let jeton = entete(crate::relais::EN_TETE);
            if origine.is_empty() && jeton_de_soeur_valide(&jeton, &jeton_attendu) {
                soeur_vue.store(true, Ordering::Relaxed);
                return Ok(resp);
            }
            let mut refus = ErrorResponse::new(Some("origine refusée".into()));
            *refus.status_mut() = StatusCode::FORBIDDEN;
            Err(refus)
        };
        let ws = tokio_tungstenite::accept_hdr_async(stream, verifier)
            .await
            .map_err(|e| format!("poignée de main refusée : {e}"))?;
        let (mut sink, mut flux) = ws.split();
        if soeur.load(Ordering::Relaxed) {
            let demande = timeout(DELAI_APPAIRAGE, flux.next())
                .await
                .map_err(|_| "relais : aucune demande".to_string())?
                .ok_or("relais : connexion fermée")?
                .map_err(|e| format!("relais : lecture : {e}"))?;
            let reponse = match self.servir_soeur(demande.to_text().unwrap_or("")).await {
                Ok(result) => json!({ "result": result }),
                Err(e) => json!({ "error": e }),
            };
            return sink
                .send(Message::text(reponse.to_string()))
                .await
                .map_err(|e| format!("relais : réponse : {e}"));
        }

        let premier = timeout(DELAI_APPAIRAGE, flux.next())
            .await
            .map_err(|_| "aucun message d'appairage".to_string())?
            .ok_or("connexion fermée avant l'appairage")?
            .map_err(|e| format!("lecture : {e}"))?;
        let hello: Value = serde_json::from_str(premier.to_text().unwrap_or(""))
            .map_err(|e| format!("appairage illisible : {e}"))?;
        let est_hello = hello.get("type") == Some(&json!("hello"));
        let jeton = hello.get("token").and_then(Value::as_str).unwrap_or("");
        let jeton_ok = est_hello && jeton == self.token.as_str();
        // Le nom ne se cherche (programme au bout de la connexion) qu'une
        // fois l'extension admise ou une fenêtre d'appairage ouverte : une
        // extension sans code se représente toutes les trois secondes.
        let fenetre_ouverte = self.appairage.lock().await.is_some();
        if !(jeton_ok || (est_hello && fenetre_ouverte)) {
            return self.refuser(&mut sink, jeton).await;
        }
        let nom = nommer(&hello, port_pair, self.port).await;
        if !jeton_ok {
            if !self.prendre_appairage(&nom).await {
                return self.refuser(&mut sink, jeton).await;
            }
            // Relié depuis Locaryn : l'extension garde ce code et
            // l'utilisera seule ensuite.
            sink.send(Message::text(
                json!({ "type": "paired", "token": self.token, "label": nom }).to_string(),
            ))
            .await
            .map_err(|e| format!("appairage : {e}"))?;
        }

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let conn = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let nom = {
            let mut liaisons = self.liaisons.lock().await;
            let nom = nom_libre(&nom, liaisons.values().map(|l| l.nom.as_str()));
            liaisons.insert(
                conn,
                Liaison {
                    nom: nom.clone(),
                    tx: tx.clone(),
                    hello: hello.clone(),
                    bloques: Vec::new(),
                },
            );
            nom
        };
        sink.send(Message::text(
            json!({ "type": "welcome", "label": nom }).to_string(),
        ))
        .await
        .map_err(|e| format!("accueil : {e}"))?;
        self.recharger_si_perimee(&hello, &tx).await;

        let ecrivain = tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                if sink.send(Message::text(m)).await.is_err() {
                    break;
                }
            }
        });
        while let Some(Ok(msg)) = flux.next().await {
            // Les trames de contrôle (ping, pong) n'ont rien à lire.
            if let Message::Text(texte) = msg {
                self.on_message(conn, &texte).await;
            }
        }
        ecrivain.abort();
        self.liaisons.lock().await.remove(&conn);
        self.fail_pending(conn, "La connexion avec l'extension s'est fermée.")
            .await;
        Ok(())
    }

    async fn refuser<S>(&self, sink: &mut S, jeton: &str) -> Result<(), String>
    where
        S: SinkExt<Message> + Unpin,
    {
        let raison = if jeton.is_empty() {
            SANS_CODE
        } else {
            "code d'appairage invalide"
        };
        let refus = json!({ "type": "refused", "reason": raison }).to_string();
        if sink.send(Message::text(refus)).await.is_err() {
            eprintln!("pont navigateur : refus non transmis");
        }
        Err(raison.into())
    }

    /// Une extension d'une autre version que celle du dossier du morph : on
    /// lui demande de se recharger, ce qui relit ce dossier. Une extension non
    /// empaquetée ne le fait jamais d'elle-même — la personne gardait l'ancien
    /// code après chaque mise à jour du morph.
    async fn recharger_si_perimee(&self, hello: &Value, tx: &mpsc::UnboundedSender<String>) {
        let Some(version) = hello.get("version").and_then(Value::as_str) else {
            return;
        };
        let firefox = hello
            .get("browser")
            .and_then(Value::as_str)
            .is_some_and(|b| b.contains("Firefox/"));
        let dossier = if firefox {
            "extension-firefox"
        } else {
            "extension"
        };
        let Some(livree) = crate::liaison::dossier_extension(dossier)
            .and_then(|d| crate::liaison::version_du_dossier(&d))
        else {
            return;
        };
        // « Différente » et pas « plus ancienne » : la numérotation a changé
        // (0.2.0 → 0.1.0.17, alignée sur le morph).
        if livree == version {
            return;
        }
        {
            let mut deja = self.rechargements.lock().await;
            if deja.iter().any(|v| v == version) {
                return;
            }
            deja.push(version.to_string());
        }
        eprintln!("pont navigateur : extension {version} rechargée vers {livree}");
        let message = json!({ "type": "reload", "version": livree }).to_string();
        if tx.send(message).is_err() {
            eprintln!("pont navigateur : rechargement non demandé");
        }
    }

    async fn on_message(&self, conn: u64, texte: &str) {
        let Ok(v) = serde_json::from_str::<Value>(texte) else {
            eprintln!("pont navigateur : message illisible");
            return;
        };
        if v.get("type") == Some(&json!("event")) {
            let mut liaisons = self.liaisons.lock().await;
            let Some(liaison) = liaisons.get_mut(&conn) else {
                return;
            };
            if v.get("event") == Some(&json!("user_stop")) {
                if let Some(id) = v.get("tab_id").and_then(Value::as_u64) {
                    liaison.bloques.push(id);
                }
            }
            if v.get("event") == Some(&json!("panel_message")) {
                let message = annoter(
                    v.get("message").cloned().unwrap_or(Value::Null),
                    &liaison.nom,
                );
                let mut file = self.panneau.lock().await;
                file.push(message);
                let trop = file.len().saturating_sub(MAX_PANNEAU);
                file.drain(..trop);
            }
            return;
        }
        let Some(id) = v.get("id").and_then(Value::as_u64) else {
            return;
        };
        let Some((_, attente)) = self.pending.lock().await.remove(&id) else {
            return;
        };
        let res = match v.get("error").and_then(Value::as_str) {
            Some(e) => Err(e.to_string()),
            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
        };
        if attente.send(res).is_err() {
            eprintln!("pont navigateur : réponse {id} sans destinataire");
        }
    }

    async fn fail_pending(&self, conn: u64, raison: &str) {
        let mut p = self.pending.lock().await;
        let ids: Vec<u64> = p
            .iter()
            .filter(|(_, (c, _))| *c == conn)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some((_, attente)) = p.remove(&id) {
                if attente.send(Err(raison.to_string())).is_err() {
                    eprintln!("pont navigateur : attente abandonnée");
                }
            }
        }
    }
}

const PAS_RELIE: &str =
    "Aucun navigateur relié à Locaryn. Appelez browser_status pour les étapes d'installation.";

/// Ajoute `browser` à un objet JSON (résultat de commande, onglet, consigne).
fn annoter(mut v: Value, nom: &str) -> Value {
    if let Value::Object(o) = &mut v {
        o.insert("browser".into(), json!(nom));
    }
    v
}

/// Le nom d'un navigateur relié : le programme au bout de la connexion s'il
/// est reconnu, sinon le nom donné à l'appairage, sinon son User-Agent.
async fn nommer(hello: &Value, port_pair: u16, port_pont: u16) -> String {
    let programme = tokio::task::spawn_blocking(move || {
        crate::processus::programme_de_la_connexion(port_pair, port_pont)
    })
    .await
    .unwrap_or_else(|e| {
        eprintln!("pont navigateur : programme de la connexion : {e}");
        None
    });
    if let Some(n) = programme.and_then(|p| crate::liaison::Navigateur::depuis(&p)) {
        return n.nom.to_string();
    }
    if let Some(label) = hello
        .get("label")
        .and_then(Value::as_str)
        .filter(|l| !l.trim().is_empty())
    {
        return label.trim().to_ascii_lowercase();
    }
    nom_depuis_user_agent(hello.get("browser").and_then(Value::as_str).unwrap_or(""))
}

fn nom_depuis_user_agent(ua: &str) -> String {
    let nom = if ua.contains("OPR/") {
        "opera"
    } else if ua.contains("Edg/") {
        "edge"
    } else if ua.contains("Firefox/") {
        "firefox"
    } else if ua.contains("Vivaldi") {
        "vivaldi"
    } else {
        "chrome"
    };
    nom.to_string()
}

/// `chrome`, puis `chrome 2` si un autre profil de Chrome est déjà relié.
fn nom_libre<'a>(nom: &str, pris: impl Iterator<Item = &'a str> + Clone) -> String {
    if !pris.clone().any(|p| p == nom) {
        return nom.to_string();
    }
    (2..)
        .map(|i| format!("{nom} {i}"))
        .find(|candidat| !pris.clone().any(|p| p == candidat))
        .unwrap_or_else(|| nom.to_string())
}

/// Le code présenté par une instance sœur : le nôtre, ou celui du fichier si
/// les deux instances l'ont créé en même temps au premier lancement.
fn jeton_de_soeur_valide(jeton: &str, le_notre: &str) -> bool {
    if jeton.len() < 32 {
        return false;
    }
    if jeton == le_notre {
        return true;
    }
    std::fs::read_to_string(data_dir().join("pairing-token"))
        .map(|t| t.trim() == jeton)
        .unwrap_or(false)
}

/// Seules les extensions de navigateur peuvent parler au pont.
fn origine_autorisee(origine: &str) -> bool {
    origine.starts_with("chrome-extension://") || origine.starts_with("moz-extension://")
}

/// Le code d'appairage, créé au premier lancement et conservé ensuite.
pub fn load_or_create_token() -> Result<String, String> {
    let dossier = data_dir();
    std::fs::create_dir_all(&dossier).map_err(|e| format!("dossier de données : {e}"))?;
    let chemin = dossier.join("pairing-token");
    if let Ok(t) = std::fs::read_to_string(&chemin) {
        let t = t.trim().to_string();
        if t.len() >= 32 {
            return Ok(t);
        }
    }
    let mut octets = [0u8; 20];
    getrandom::getrandom(&mut octets).map_err(|e| format!("aléa système : {e}"))?;
    let token: String = octets.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(&chemin, &token).map_err(|e| format!("écriture du code : {e}"))?;
    reserve_au_proprietaire(&chemin)?;
    Ok(token)
}

/// Le code ouvre le navigateur à qui le connaît : sous Unix, lui seul le lit.
/// (Sous Windows, le dossier de profil de l'utilisateur fait déjà ce travail.)
#[cfg(unix)]
fn reserve_au_proprietaire(chemin: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(chemin, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("droits du code d'appairage : {e}"))
}

#[cfg(not(unix))]
fn reserve_au_proprietaire(_chemin: &std::path::Path) -> Result<(), String> {
    Ok(())
}

fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("LOCARYN_EXTENSION_DATA_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("locaryn").join("morph-browser")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    type Ws = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    fn port_libre() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// Une extension qui se présente ; rend la connexion et la première réponse.
    async fn extension(port: u16, jeton: &str, ua: &str) -> (Ws, Value) {
        let mut req = format!("ws://127.0.0.1:{port}")
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("origin", "chrome-extension://essai".parse().unwrap());
        let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
        ws.send(Message::text(
            json!({"type":"hello","token":jeton,"browser":ua,"version":"0.1.0.17"}).to_string(),
        ))
        .await
        .unwrap();
        let rep = ws.next().await.unwrap().unwrap();
        let v = serde_json::from_str::<Value>(rep.to_text().unwrap()).unwrap();
        (ws, v)
    }

    /// La prochaine commande reçue par une extension (l'accueil et la demande
    /// de rechargement passent avant).
    async fn prochaine_commande(ws: &mut Ws) -> Value {
        loop {
            let m = ws.next().await.unwrap().unwrap();
            let v: Value = serde_json::from_str(m.to_text().unwrap()).unwrap();
            if v.get("method").is_some() {
                return v;
            }
        }
    }

    #[test]
    fn seules_les_origines_d_extension_passent() {
        assert!(origine_autorisee("chrome-extension://abcdef"));
        assert!(!origine_autorisee("https://evil.example"));
        assert!(!origine_autorisee(""));
    }

    #[test]
    fn deux_profils_du_meme_navigateur_gardent_des_noms_distincts() {
        assert_eq!(nom_libre("chrome", ["opera"].into_iter()), "chrome");
        assert_eq!(nom_libre("chrome", ["chrome"].into_iter()), "chrome 2");
        assert_eq!(
            nom_libre("chrome", ["chrome", "chrome 2"].into_iter()),
            "chrome 3"
        );
        assert_eq!(
            nom_depuis_user_agent("Mozilla/5.0 … Chrome/131 OPR/116"),
            "opera"
        );
    }

    #[tokio::test]
    async fn sans_extension_la_commande_dit_quoi_faire() {
        let b = Bridge::start(0, "t".repeat(40)).await;
        let e = b.call("tabs_list", json!({})).await.unwrap_err();
        assert!(e.contains("browser_status"), "{e}");
    }

    /// Une extension sans code est refusée tant que personne n'a cliqué
    /// « Relier », puis reçoit le code une seule fois.
    #[tokio::test]
    async fn la_fenetre_d_appairage_donne_le_code_une_fois() {
        let port = port_libre();
        let code = "c".repeat(40);
        let b = Bridge::start(port, code.clone()).await;
        let (_, refus) = extension(port, "", "Chrome/131").await;
        assert_eq!(refus["type"], "refused");
        assert_eq!(refus["reason"], SANS_CODE);

        b.ouvrir_appairage(Duration::from_secs(30), None).await;
        let (_ws, relie) = extension(port, "", "Chrome/131").await;
        assert_eq!(relie["type"], "paired");
        assert_eq!(relie["token"], code.as_str());

        // Une seule extension par ouverture.
        let (_, encore) = extension(port, "", "Chrome/131").await;
        assert_eq!(encore["type"], "refused");
    }

    /// Plusieurs navigateurs reliés en même temps ; une commande vise celui
    /// qu'on nomme, et la liste des onglets les réunit.
    #[tokio::test]
    async fn plusieurs_navigateurs_relies_a_la_fois() {
        let port = port_libre();
        let code = "a".repeat(40);
        let b = Bridge::start(port, code.clone()).await;
        let (mut chrome, _) = extension(port, &code, "Chrome/131").await;
        let (mut opera, _) = extension(port, &code, "Chrome/131 OPR/116").await;
        let noms: Vec<String> = b
            .navigateurs()
            .await
            .iter()
            .map(|n| n["browser"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(noms, vec!["chrome", "opera"]);

        // Une commande nommée ne va qu'à Opera.
        let commande = tokio::spawn({
            let b = b.clone();
            async move { b.call("snapshot", json!({"browser": "opera"})).await }
        });
        let recu: Value = prochaine_commande(&mut opera).await;
        assert_eq!(recu["method"], "snapshot");
        opera
            .send(Message::text(
                json!({"id": recu["id"], "result": {"title": "x"}}).to_string(),
            ))
            .await
            .unwrap();
        let r = commande.await.unwrap().unwrap();
        assert_eq!(r["browser"], "opera");

        // La liste des onglets interroge les deux.
        let liste = tokio::spawn({
            let b = b.clone();
            async move { b.call("tabs_list", json!({})).await }
        });
        for ws in [&mut chrome, &mut opera] {
            let recu: Value = prochaine_commande(ws).await;
            ws.send(Message::text(
                json!({"id": recu["id"], "result": {"tabs": [{"tab_id": 1}]}}).to_string(),
            ))
            .await
            .unwrap();
        }
        let onglets = liste.await.unwrap().unwrap();
        let navs: Vec<&str> = onglets["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["browser"].as_str().unwrap())
            .collect();
        assert!(
            navs.contains(&"chrome") && navs.contains(&"opera"),
            "{navs:?}"
        );

        let e = b
            .call("snapshot", json!({"browser": "comet"}))
            .await
            .unwrap_err();
        assert!(e.contains("chrome, opera"), "{e}");
    }

    /// Deux serveurs du morph (bureau et service) pour un port : « Relier »
    /// sur celui qui n'a pas le port apparie quand même l'extension, et ses
    /// commandes passent par l'autre.
    #[tokio::test]
    async fn l_instance_sans_port_passe_par_celle_qui_l_a() {
        let port = port_libre();
        let code = "d".repeat(40);
        let proprietaire = Bridge::start(port, code.clone()).await;
        let soeur = Bridge::start(port, code.clone()).await;
        assert!(proprietaire.bind_error().await.is_none());
        assert!(soeur.bind_error().await.is_some());
        assert!(!soeur.is_connected().await);

        soeur.ouvrir_appairage(Duration::from_secs(30), None).await;
        let (mut ws, paire) = extension(port, "", "Chrome/131").await;
        assert_eq!(paire["type"], "paired");
        let _accueil = ws.next().await.unwrap().unwrap();
        assert!(soeur.is_connected().await);

        // Une commande de la sœur arrive à l'extension et revient.
        let commande = tokio::spawn({
            let soeur = soeur.clone();
            async move { soeur.call("snapshot", json!({})).await }
        });
        let recu: Value = prochaine_commande(&mut ws).await;
        assert_eq!(recu["method"], "snapshot");
        ws.send(Message::text(
            json!({"id": recu["id"], "result": {"title": "x"}}).to_string(),
        ))
        .await
        .unwrap();
        assert_eq!(commande.await.unwrap().unwrap()["title"], "x");
    }

    #[tokio::test]
    async fn un_faux_code_de_soeur_est_refuse() {
        let port = port_libre();
        let _proprietaire = Bridge::start(port, "e".repeat(40)).await;
        let r = crate::relais::demander(
            port,
            &"f".repeat(40),
            "status",
            json!({}),
            Duration::from_secs(2),
        )
        .await;
        assert!(r.is_err());
    }
}
