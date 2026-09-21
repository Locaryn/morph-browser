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
}

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

    pub fn token(&self) -> &str {
        &self.token
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
                        if let Err(e) = b.handle(stream).await {
                            eprintln!("pont navigateur : {e}");
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
        let jeton_ok = hello.get("type") == Some(&json!("hello"))
            && hello.get("token").and_then(Value::as_str) == Some(self.token.as_str());
        if !jeton_ok {
            let refus = json!({"type":"refused","reason":"code d'appairage invalide"}).to_string();
            if let Err(e) = sink.send(Message::text(refus)).await {
                eprintln!("pont navigateur : refus non transmis : {e}");
            }
            return Err("code d'appairage invalide".into());
        }
        sink.send(Message::text(json!({"type":"welcome"}).to_string()))
            .await
            .map_err(|e| format!("accueil : {e}"))?;

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let conn = self.next_conn.fetch_add(1, Ordering::Relaxed);
        *self.tx.lock().await = Some((conn, tx));
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
    Ok(token)
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
}
