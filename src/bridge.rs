//! Pont WebSocket entre le serveur MCP et l'extension du navigateur.
//!
//! Le serveur n'écoute que sur 127.0.0.1, n'accepte qu'une origine
//! d'extension, et exige un code d'appairage : sans cela, n'importe quelle
//! page web ouverte pourrait s'adresser au pont.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;

const DELAI_APPAIRAGE: Duration = Duration::from_secs(5);
const DELAI_COMMANDE: Duration = Duration::from_secs(45);

type Attente = oneshot::Sender<Result<Value, String>>;

pub struct Bridge {
    pub port: u16,
    token: String,
    tx: Mutex<Option<(u64, mpsc::UnboundedSender<String>)>>,
    pending: Mutex<HashMap<u64, Attente>>,
    next_id: AtomicU64,
    next_conn: AtomicU64,
    hello: Mutex<Option<Value>>,
    blocked: Mutex<Vec<u64>>,
    bind_error: Mutex<Option<String>>,
    /// Fin de la fenêtre d'appairage ouverte par `browser_link` : jusque-là,
    /// une extension sans code reçoit le code au lieu d'être refusée.
    appairage: Mutex<Option<std::time::Instant>>,
    /// Les consignes tapées dans la barre latérale, en attente d'être
    /// relevées par le panneau du morph dans Locaryn.
    panneau: Mutex<Vec<Value>>,
    /// Les versions d'extension à qui l'on a déjà demandé de se recharger :
    /// une seule demande par version, sinon une copie ancienne chargée
    /// ailleurs que dans le dossier du morph se rechargerait en boucle.
    rechargements: Mutex<Vec<String>>,
}

/// Au-delà, les plus anciennes consignes non relevées sont oubliées :
/// Locaryn est fermé, personne ne les lira.
const MAX_PANNEAU: usize = 20;

/// Le refus envoyé à une extension qui n'a pas encore de code : elle
/// réessaie en silence, le journal n'a pas à s'en remplir.
const SANS_CODE: &str = "en attente d'appairage";

impl Bridge {
    /// Démarre l'écoute. Un port pris n'empêche pas le serveur MCP de vivre :
    /// l'erreur est remontée par `status`.
    pub async fn start(port: u16, token: String) -> Arc<Self> {
        let bridge = Arc::new(Self {
            port,
            token,
            tx: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            next_conn: AtomicU64::new(1),
            hello: Mutex::new(None),
            blocked: Mutex::new(Vec::new()),
            bind_error: Mutex::new(None),
            appairage: Mutex::new(None),
            panneau: Mutex::new(Vec::new()),
            rechargements: Mutex::new(Vec::new()),
        });
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => {
                let b = bridge.clone();
                tokio::spawn(async move { b.accept_loop(listener).await });
            }
            Err(e) => {
                *bridge.bind_error.lock().await = Some(format!(
                    "port {port} indisponible ({e}) : une autre instance l'occupe peut-être"
                ));
            }
        }
        bridge
    }

    /// Ouvre la fenêtre d'appairage : la première extension qui se présente
    /// sans code dans ce délai le reçoit. Le code ne passe jamais par le
    /// modèle de conversation ni par l'écran.
    pub async fn ouvrir_appairage(&self, duree: Duration) {
        *self.appairage.lock().await = Some(std::time::Instant::now() + duree);
    }

    /// Prend la fenêtre si elle est encore ouverte : un seul appairage par
    /// ouverture.
    async fn prendre_appairage(&self) -> bool {
        let mut fenetre = self.appairage.lock().await;
        match *fenetre {
            Some(fin) if std::time::Instant::now() < fin => {
                *fenetre = None;
                true
            }
            _ => false,
        }
    }

    pub async fn bind_error(&self) -> Option<String> {
        self.bind_error.lock().await.clone()
    }

    pub async fn is_connected(&self) -> bool {
        self.tx.lock().await.is_some()
    }

    pub async fn hello(&self) -> Option<Value> {
        self.hello.lock().await.clone()
    }

    /// Prend les consignes de la barre latérale, et dit à l'extension
    /// qu'elles sont arrivées dans Locaryn.
    pub async fn prendre_messages_panneau(&self) -> Vec<Value> {
        let pris: Vec<Value> = std::mem::take(&mut *self.panneau.lock().await);
        if !pris.is_empty() {
            let ids: Vec<Value> = pris.iter().filter_map(|m| m.get("id").cloned()).collect();
            if let Err(e) = self
                .pousser(json!({ "type": "panel_ack", "ids": ids }))
                .await
            {
                eprintln!("pont navigateur : accusé de réception non transmis : {e}");
            }
        }
        pris
    }

    /// Envoie un message à l'extension sans attendre de réponse.
    pub async fn pousser(&self, message: Value) -> Result<(), String> {
        let tx = self.tx.lock().await;
        let (_, sender) = tx
            .as_ref()
            .ok_or("L'extension Locaryn n'est pas connectée au navigateur.")?;
        sender
            .send(message.to_string())
            .map_err(|_| "La connexion avec l'extension vient de tomber.".to_string())
    }

    /// Une extension plus ancienne que celle du dossier du morph : on lui
    /// demande de se recharger, ce qui relit ce dossier. Une extension non
    /// empaquetée ne le fait jamais d'elle-même — la personne gardait l'ancien
    /// code après chaque mise à jour du morph.
    async fn recharger_si_perimee(&self, hello: &Value) {
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
        if !crate::liaison::plus_recente(&livree, version) {
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
        if let Err(e) = self
            .pousser(json!({ "type": "reload", "version": livree }))
            .await
        {
            eprintln!("pont navigateur : rechargement non demandé : {e}");
        }
    }

    /// Onglets sur lesquels l'utilisateur a pressé Stop.
    pub async fn blocked_tabs(&self) -> Vec<u64> {
        self.blocked.lock().await.clone()
    }

    /// Envoie une commande à l'extension et attend sa réponse.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let sender = self
            .tx
            .lock()
            .await
            .as_ref()
            .map(|(_, s)| s.clone())
            .ok_or_else(|| {
                "L'extension Locaryn n'est pas connectée au navigateur. Appelez browser_status pour les étapes d'installation.".to_string()
            })?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (rep_tx, rep_rx) = oneshot::channel();
        self.pending.lock().await.insert(id, rep_tx);
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

    async fn accept_loop(self: Arc<Self>, listener: TcpListener) {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let b = self.clone();
                    tokio::spawn(async move {
                        match b.handle(stream).await {
                            Err(e) if e != SANS_CODE => eprintln!("pont navigateur : {e}"),
                            _ => {}
                        }
                    });
                }
                Err(e) => eprintln!("pont navigateur : accept : {e}"),
            }
        }
    }

    async fn handle(self: Arc<Self>, stream: TcpStream) -> Result<(), String> {
        let verifier = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
            let origine = req
                .headers()
                .get("origin")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if origine_autorisee(origine) {
                Ok(resp)
            } else {
                let mut refus = ErrorResponse::new(Some("origine refusée".into()));
                *refus.status_mut() = StatusCode::FORBIDDEN;
                Err(refus)
            }
        };
        let ws = tokio_tungstenite::accept_hdr_async(stream, verifier)
            .await
            .map_err(|e| format!("poignée de main refusée : {e}"))?;
        let (mut sink, mut flux) = ws.split();

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
        if !jeton_ok {
            if est_hello && self.prendre_appairage().await {
                // Relié depuis Locaryn : l'extension garde ce code et
                // l'utilisera seule ensuite.
                sink.send(Message::text(
                    json!({"type":"paired","token": self.token}).to_string(),
                ))
                .await
                .map_err(|e| format!("appairage : {e}"))?;
            } else {
                let raison = if jeton.is_empty() {
                    SANS_CODE
                } else {
                    "code d'appairage invalide"
                };
                let refus = json!({"type":"refused","reason": raison}).to_string();
                if let Err(e) = sink.send(Message::text(refus)).await {
                    eprintln!("pont navigateur : refus non transmis : {e}");
                }
                return Err(raison.into());
            }
        }
        sink.send(Message::text(json!({"type":"welcome"}).to_string()))
            .await
            .map_err(|e| format!("accueil : {e}"))?;

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let conn = self.next_conn.fetch_add(1, Ordering::Relaxed);
        *self.tx.lock().await = Some((conn, tx));
        self.recharger_si_perimee(&hello).await;
        *self.hello.lock().await = Some(hello);

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
                self.on_message(&texte).await;
            }
        }
        ecrivain.abort();
        let mut tx = self.tx.lock().await;
        if tx.as_ref().map(|(c, _)| *c) == Some(conn) {
            *tx = None;
            *self.hello.lock().await = None;
        }
        drop(tx);
        self.fail_pending("La connexion avec l'extension s'est fermée.")
            .await;
        Ok(())
    }

    async fn on_message(&self, texte: &str) {
        let Ok(v) = serde_json::from_str::<Value>(texte) else {
            eprintln!("pont navigateur : message illisible");
            return;
        };
        if v.get("type") == Some(&json!("event")) {
            if v.get("event") == Some(&json!("user_stop")) {
                if let Some(id) = v.get("tab_id").and_then(Value::as_u64) {
                    self.blocked.lock().await.push(id);
                }
            }
            if v.get("event") == Some(&json!("panel_message")) {
                let mut file = self.panneau.lock().await;
                file.push(v.get("message").cloned().unwrap_or(Value::Null));
                let trop = file.len().saturating_sub(MAX_PANNEAU);
                file.drain(..trop);
            }
            return;
        }
        let Some(id) = v.get("id").and_then(Value::as_u64) else {
            return;
        };
        let Some(attente) = self.pending.lock().await.remove(&id) else {
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

    async fn fail_pending(&self, raison: &str) {
        let mut p = self.pending.lock().await;
        for (_, attente) in p.drain() {
            if attente.send(Err(raison.to_string())).is_err() {
                eprintln!("pont navigateur : attente abandonnée");
            }
        }
    }
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

    #[test]
    fn seules_les_origines_d_extension_passent() {
        assert!(origine_autorisee("chrome-extension://abcdef"));
        assert!(!origine_autorisee("https://evil.example"));
        assert!(!origine_autorisee(""));
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
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let code = "c".repeat(40);
        let b = Bridge::start(port, code.clone()).await;
        let essai = || async move {
            let mut req = format!("ws://127.0.0.1:{port}")
                .into_client_request()
                .unwrap();
            req.headers_mut()
                .insert("origin", "chrome-extension://essai".parse().unwrap());
            let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
            ws.send(Message::text(
                json!({"type":"hello","token":""}).to_string(),
            ))
            .await
            .unwrap();
            let rep = ws.next().await.unwrap().unwrap();
            serde_json::from_str::<Value>(rep.to_text().unwrap()).unwrap()
        };
        let refus = essai().await;
        assert_eq!(refus["type"], "refused");
        assert_eq!(refus["reason"], SANS_CODE);

        b.ouvrir_appairage(Duration::from_secs(30)).await;
        let relie = essai().await;
        assert_eq!(relie["type"], "paired");
        assert_eq!(relie["token"], code.as_str());

        // Une seule extension par ouverture.
        assert_eq!(essai().await["type"], "refused");
    }
}
