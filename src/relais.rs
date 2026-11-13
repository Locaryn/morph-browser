//! Deux serveurs du morph pour un seul port.
//!
//! Locaryn peut lancer le serveur MCP du morph deux fois : une par
//! l'application de bureau, une par son service d'arrière-plan. Un seul
//! obtient le port du pont ; l'autre ne voyait jamais l'extension. Son
//! « Relier » ouvrait une fenêtre d'appairage qu'aucune extension ne pouvait
//! atteindre (l'extension restait sans code), et ses commandes répondaient
//! « extension non connectée ».
//!
//! L'instance sans port passe donc par celle qui l'a : une connexion au même
//! port, sans origine de navigateur (une page web en envoie toujours une), et
//! avec le code d'appairage en en-tête — un secret que seule la session de
//! l'utilisateur peut lire sur le disque.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

/// L'en-tête qui porte le code d'appairage d'une instance sœur.
pub const EN_TETE: &str = "x-locaryn-relais";

const DELAI_CONNEXION: Duration = Duration::from_secs(5);

/// Envoie une action à l'instance qui tient le port et rend son résultat.
pub async fn demander(
    port: u16,
    jeton: &str,
    action: &str,
    params: Value,
    delai: Duration,
) -> Result<Value, String> {
    let mut requete = format!("ws://127.0.0.1:{port}")
        .into_client_request()
        .map_err(|e| format!("relais : adresse : {e}"))?;
    let valeur = HeaderValue::from_str(jeton).map_err(|e| format!("relais : code : {e}"))?;
    requete.headers_mut().insert(EN_TETE, valeur);
    let (mut ws, _) = timeout(DELAI_CONNEXION, tokio_tungstenite::connect_async(requete))
        .await
        .map_err(|_| "relais : l'autre instance du morph ne répond pas".to_string())?
        .map_err(|e| format!("relais : connexion à l'autre instance du morph : {e}"))?;
    ws.send(Message::text(
        json!({ "action": action, "params": params }).to_string(),
    ))
    .await
    .map_err(|e| format!("relais : envoi : {e}"))?;
    let reponse = timeout(delai, ws.next())
        .await
        .map_err(|_| format!("relais : pas de réponse à « {action} »"))?
        .ok_or("relais : connexion fermée sans réponse")?
        .map_err(|e| format!("relais : lecture : {e}"))?;
    if let Err(e) = ws.close(None).await {
        eprintln!("relais : fermeture : {e}");
    }
    let v: Value = serde_json::from_str(reponse.to_text().unwrap_or(""))
        .map_err(|e| format!("relais : réponse illisible : {e}"))?;
    match v.get("error").and_then(Value::as_str) {
        Some(e) => Err(e.to_string()),
        None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
    }
}
