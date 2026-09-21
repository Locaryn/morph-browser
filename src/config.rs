//! Réglages du morph, lus dans le fichier que l'hôte désigne.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    /// Port local du pont avec l'extension.
    #[serde(default = "port_defaut")]
    pub port: u16,
    /// Demander confirmation avant une action jugée irréversible.
    #[serde(default = "vrai")]
    pub confirm_risky: bool,
    /// Faire juger le risque par Laya en plus du repli lexical.
    #[serde(default = "vrai")]
    pub laya_risk_check: bool,
}

fn port_defaut() -> u16 {
    17421
}

fn vrai() -> bool {
    true
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            port: port_defaut(),
            confirm_risky: true,
            laya_risk_check: true,
        }
    }
}
