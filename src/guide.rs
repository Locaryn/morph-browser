//! La page qui guide le chargement de l'extension. Les navigateurs Chromium
//! refusent d'ouvrir leurs pages internes (`opera://extensions`…) quand un
//! autre programme les leur passe : on ouvre à la place une page locale qui
//! donne l'adresse et le chemin du dossier, chacun avec un bouton Copier.

use crate::liaison::Navigateur;
use std::path::PathBuf;

const MODELE: &str = include_str!("guide.html");

/// Écrit la page dans le dossier temporaire et rend son chemin.
pub fn ecrire(navigateur: &Navigateur, dossier: Option<&str>) -> Option<PathBuf> {
    let chemin = std::env::temp_dir().join("locaryn-relier-navigateur.html");
    match std::fs::write(&chemin, contenu(navigateur, dossier)) {
        Ok(()) => Some(chemin),
        Err(e) => {
            eprintln!("page de liaison {} : {e}", chemin.display());
            None
        }
    }
}

/// La page remplie pour ce navigateur.
pub fn contenu(navigateur: &Navigateur, dossier: Option<&str>) -> String {
    MODELE
        .replace("{{NOM}}", &echapper(nom_affiche(navigateur.nom)))
        .replace("{{ETAPES}}", &etapes(navigateur, dossier))
}

fn etapes(navigateur: &Navigateur, dossier: Option<&str>) -> String {
    let adresse = champ(
        "adresse",
        navigateur.page,
        "Copiez cette adresse et collez-la dans la barre d'adresse d'un nouvel onglet :",
    );
    let dossier_html = match dossier {
        Some(d) => champ(
            "dossier",
            d,
            "Choisissez ce dossier (copiez son chemin et collez-le dans la fenêtre) :",
        ),
        None => format!(
            "<p>Choisissez le dossier <kbd>{}</kbd> du morph.</p>",
            echapper(navigateur.dossier)
        ),
    };
    if navigateur.nom == "firefox" {
        format!(
            "    <li>{adresse}</li>\n    <li><p>Cliquez « Charger un module complémentaire temporaire », puis le fichier <kbd>manifest.json</kbd>.</p>{dossier_html}</li>\n    <li><p>Dans <kbd>about:addons</kbd> → l'extension Locaryn → Autorisations, accordez l'accès à tous les sites.</p></li>"
        )
    } else {
        format!(
            "    <li>{adresse}</li>\n    <li><p>En haut à droite de la page, activez le « Mode développeur ».</p></li>\n    <li><p>Cliquez « Charger l'extension non empaquetée ».</p>{dossier_html}</li>"
        )
    }
}

fn champ(id: &str, valeur: &str, consigne: &str) -> String {
    let v = echapper(valeur);
    format!(
        "<p>{consigne}</p><div class=\"champ\"><input id=\"{id}\" value=\"{v}\" readonly aria-label=\"{v}\"><button type=\"button\" data-copier=\"{id}\">Copier</button></div>"
    )
}

fn nom_affiche(nom: &str) -> &str {
    match nom {
        "chrome" => "Chrome",
        "edge" => "Edge",
        "opera" => "Opera",
        "brave" => "Brave",
        "vivaldi" => "Vivaldi",
        "firefox" => "Firefox",
        autre => autre,
    }
}

fn echapper(texte: &str) -> String {
    texte
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_page_donne_l_adresse_et_le_dossier_du_navigateur() {
        let opera = Navigateur::depuis("opera").expect("opera");
        let page = contenu(&opera, Some(r"C:\Users\a&b\extension"));
        assert!(page.contains("Relier Opera à Locaryn"));
        assert!(page.contains("value=\"opera://extensions\""));
        assert!(page.contains(r"C:\Users\a&amp;b\extension"));
        assert!(page.contains("Mode développeur"));
        assert!(!page.contains("{{"));
    }

    #[test]
    fn firefox_a_ses_propres_gestes() {
        let firefox = Navigateur::depuis("firefox").expect("firefox");
        let page = contenu(&firefox, None);
        assert!(page.contains("about:debugging#/runtime/this-firefox"));
        assert!(page.contains("module complémentaire temporaire"));
        assert!(page.contains("extension-firefox"));
    }
}
