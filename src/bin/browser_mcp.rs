//! Serveur MCP stdio du morph navigateur.

use locaryn_morph_kit::mcp::serve;
use locaryn_plugin_browser::tools::{tools_list, Browser};
use std::sync::Arc;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() {
    let browser = match Browser::new().await {
        Ok(b) => Arc::new(b),
        Err(e) => {
            eprintln!("morph-browser : démarrage impossible : {e}");
            std::process::exit(1);
        }
    };
    serve("plugin-browser", VERSION, tools_list(), |name, args| {
        let browser = browser.clone();
        async move { browser.call(&name, args).await }
    })
    .await;
}
