//! Garde-fou de risque : la question posée à Laya, et un repli lexical qui
//! reste actif même quand Laya répond (mesuré : Laya seul rate des cas).

use crate::laya::Laya;
use serde_json::json;

/// La question posée avant toute action qui pourrait ne pas se défaire.
pub const RISK_QUESTION: &str = "Cette action est-elle irréversible ou lourde de conséquences : suppression de données, paiement, envoi, engagement ?";

/// Probabilité de Laya à partir de laquelle on demande confirmation. Mesuré :
/// suppression 0,97 ; paiement 0,54 ; actions anodines 0,02 au plus.
pub const SEUIL_RISQUE: f32 = 0.3;

const MOTS_A_RISQUE: &[&str] = &[
    "supprimer",
    "effacer",
    "delete",
    "remove",
    "erase",
    "formater",
    "uninstall",
    "désinstaller",
    "desinstaller",
    "acheter",
    "buy",
    "purchase",
    "payer",
    "pay",
    "commander",
    "order",
    "checkout",
    "envoyer",
    "send",
    "publier",
    "publish",
    "confirmer",
    "confirm",
    "valider",
    "submit",
    "transférer",
    "transfer",
    "virement",
    "unsubscribe",
    "résilier",
    "resilier",
    "cancel subscription",
    "vider",
    "empty",
    "reset",
    "réinitialiser",
    "reinitialiser",
    "sign out",
    "déconnecter",
    "deconnecter",
    "logout",
];

/// Le mot apparaît-il en début de mot ? « order » sonne dans « orders », pas
/// dans « border » : un faux positif de plus, c'est une confirmation de trop.
pub fn has_word_start(bas: &str, mot: &str) -> bool {
    bas.match_indices(mot).any(|(i, _)| {
        bas[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
    })
}

/// Repli sans modèle : le texte contient-il un mot à risque ?
pub fn lexical_risk(texte: &str) -> bool {
    let bas = texte.to_lowercase();
    MOTS_A_RISQUE.iter().any(|m| has_word_start(&bas, m))
}

/// Verdict sur une action : les deux avis, gardés séparés pour être montrés.
#[derive(Debug, Clone, Copy)]
pub struct Risk {
    pub lexical: bool,
    pub laya: Option<f32>,
}

impl Risk {
    pub fn risky(&self) -> bool {
        self.lexical || self.laya.is_some_and(|p| p >= SEUIL_RISQUE)
    }
}

/// Juge une action décrite en clair. `extra` ajoute des mots propres au
/// morph (commandes shell, raccourcis clavier…).
pub async fn assess(laya: &Laya, action: &str, extra: &[&str], use_laya: bool) -> Risk {
    let bas = action.to_lowercase();
    let lexical = lexical_risk(action) || extra.iter().any(|m| has_word_start(&bas, m));
    let laya = if use_laya && !action.trim().is_empty() {
        laya.judge(&json!({ "action": action }), RISK_QUESTION)
            .await
            .ok()
    } else {
        None
    };
    Risk { lexical, laya }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_actions_destructrices_sont_reperees() {
        assert!(lexical_risk("Bouton « Supprimer le compte »"));
        assert!(lexical_risk("Proceed to CHECKOUT"));
    }

    #[test]
    fn un_mot_a_l_interieur_d_un_autre_ne_compte_pas() {
        assert!(!lexical_risk("border-radius: 4px"));
        assert!(!lexical_risk("Get-Date -Format yyyy"));
        assert!(!lexical_risk("Code postal"));
        assert!(lexical_risk("Payer maintenant"));
        assert!(lexical_risk("3 orders en attente : supprimer"));
    }

    #[test]
    fn une_action_anodine_passe() {
        assert!(!lexical_risk("Ouvrir le menu des paramètres d'affichage"));
    }

    #[test]
    fn le_seuil_de_laya_declenche_seul() {
        assert!(Risk {
            lexical: false,
            laya: Some(0.54)
        }
        .risky());
        assert!(!Risk {
            lexical: false,
            laya: Some(0.02)
        }
        .risky());
        assert!(Risk {
            lexical: true,
            laya: None
        }
        .risky());
    }
}
