//! Serveur MCP minimal sur stdio (JSON-RPC, une requête par ligne).

use serde_json::{json, Value};
use std::future::Future;
use std::io::Write;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Contenu texte d'une réponse d'outil.
pub fn text_content(value: &Value) -> Value {
    let texte = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    json!({ "content": [{ "type": "text", "text": texte }] })
}

/// Déclaration d'un outil MCP.
pub fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": props, "required": required }
    })
}

/// Un paramètre de type chaîne, avec sa description.
pub fn str_prop(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}

fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Répond aux requêtes de `stdin` jusqu'à sa fermeture.
///
/// `tools` est la réponse complète de `tools/list` ; `call` exécute un outil.
pub async fn serve<F, Fut>(server_name: &str, version: &str, tools: Value, call: F)
where
    F: Fn(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle(&request, server_name, version, &tools, &call).await,
            Err(error) => error_response(Value::Null, -32700, format!("JSON invalide : {error}")),
        };
        if response.is_null() {
            continue;
        }
        if let Ok(serialized) = serde_json::to_string(&response) {
            println!("{serialized}");
            if let Err(e) = std::io::stdout().flush() {
                eprintln!("sortie standard fermée : {e}");
                return;
            }
        }
    }
}

async fn handle<F, Fut>(
    request: &Value,
    server_name: &str,
    version: &str,
    tools: &Value,
    call: &F,
) -> Value
where
    F: Fn(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match method {
        "initialize" => success(
            id,
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": server_name, "version": version }
            }),
        ),
        "tools/list" => success(id, tools.clone()),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call(name, args).await {
                Ok(value) => success(id, text_content(&value)),
                Err(error) => error_response(id, -32000, error),
            }
        }
        m if m.starts_with("notifications/") => Value::Null,
        _ => error_response(id, -32601, format!("méthode MCP inconnue : {method}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn un_outil_inconnu_remonte_son_erreur() {
        let requete = json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"x"}});
        let rep = handle(&requete, "t", "0", &json!({}), &|n: String, _| async move {
            Err(format!("inconnu : {n}"))
        })
        .await;
        assert_eq!(rep["error"]["message"], "inconnu : x");
        assert_eq!(rep["id"], 7);
    }

    #[tokio::test]
    async fn une_notification_ne_recoit_pas_de_reponse() {
        let requete = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        let rep = handle(&requete, "t", "0", &json!({}), &|_: String, _| async {
            Ok(json!({}))
        })
        .await;
        assert!(rep.is_null());
    }
}
