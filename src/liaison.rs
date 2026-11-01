//! Relier le navigateur de la personne : trouver le dossier de l'extension,
//! l'ouvrir dans l'explorateur, et ouvrir la page des extensions du
//! navigateur choisi. Le chargement de l'extension reste un geste de la
//! personne (mode développeur) : aucun navigateur ne permet de l'imposer.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Le temps laissé pour charger l'extension après `browser_link`.
pub const DUREE_APPAIRAGE: Duration = Duration::from_secs(120);

/// Un navigateur Chromium que l'on sait ouvrir sur sa page des extensions.
#[derive(Clone, Copy)]
pub struct Navigateur {
    /// L'adresse de la page des extensions.
    pub page: &'static str,
    /// Nom de l'exécutable connu de Windows (App Paths) ou du PATH.
    exe: &'static str,
    /// Nom de l'application sous macOS.
    app_mac: &'static str,
    /// Chemins d'installation habituels sous Windows, relatifs aux dossiers
    /// de programmes (LOCALAPPDATA, ProgramFiles, ProgramFiles(x86)).
    chemins_windows: &'static [&'static str],
}

const CONNUS: &[(&str, Navigateur)] = &[
    (
        "chrome",
        Navigateur {
            page: "chrome://extensions",
            exe: "chrome",
            app_mac: "Google Chrome",
            chemins_windows: &["Google\\Chrome\\Application\\chrome.exe"],
        },
    ),
    (
        "edge",
        Navigateur {
            page: "edge://extensions",
            exe: "msedge",
            app_mac: "Microsoft Edge",
            chemins_windows: &["Microsoft\\Edge\\Application\\msedge.exe"],
        },
    ),
    (
        "opera",
        Navigateur {
            page: "opera://extensions",
            exe: "opera",
            app_mac: "Opera",
            chemins_windows: &[
                "Programs\\Opera\\opera.exe",
                "Programs\\Opera GX\\opera.exe",
                "Opera\\opera.exe",
            ],
        },
    ),
    (
        "brave",
        Navigateur {
            page: "brave://extensions",
            exe: "brave",
            app_mac: "Brave Browser",
            chemins_windows: &["BraveSoftware\\Brave-Browser\\Application\\brave.exe"],
        },
    ),
    (
        "vivaldi",
        Navigateur {
            page: "vivaldi://extensions",
            exe: "vivaldi",
            app_mac: "Vivaldi",
            chemins_windows: &["Vivaldi\\Application\\vivaldi.exe"],
        },
    ),
];

impl Navigateur {
    pub fn depuis(nom: &str) -> Option<Self> {
        let nom = nom.trim().to_ascii_lowercase();
        CONNUS
            .iter()
            .find(|(cle, _)| nom.contains(cle))
            .map(|(_, n)| *n)
    }

    /// Ouvre la page des extensions. `false` : le navigateur est introuvable
    /// ou a refusé ; la personne tape l'adresse elle-même.
    pub fn ouvrir_page_extensions(self) -> bool {
        if cfg!(windows) {
            if let Some(exe) = self.chemin_windows() {
                return lancer(Command::new(exe).arg(self.page));
            }
            // Repli : l'exécutable déclaré à Windows (App Paths).
            let mut c = Command::new("cmd");
            c.args(["/C", "start", "", self.exe, self.page]);
            return lancer(&mut c);
        }
        if cfg!(target_os = "macos") {
            let mut c = Command::new("open");
            c.args(["-a", self.app_mac, self.page]);
            return lancer(&mut c);
        }
        lancer(Command::new(self.exe).arg(self.page))
    }

    fn chemin_windows(self) -> Option<PathBuf> {
        let racines: Vec<PathBuf> = ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .filter_map(|v| std::env::var_os(v).map(PathBuf::from))
            .collect();
        racines
            .iter()
            .flat_map(|r| self.chemins_windows.iter().map(move |c| r.join(c)))
            .find(|p| p.is_file())
    }
}

/// Le dossier `extension` livré avec le morph : à côté du dossier `bin` du
/// paquet installé, ou à la racine du dépôt en développement.
pub fn dossier_extension() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .skip(1)
        .map(|d| d.join("extension"))
        .find(|d| d.join("manifest.json").is_file())
}

/// Ouvre un dossier dans l'explorateur de fichiers du système.
pub fn ouvrir_dossier(dossier: &Path) -> bool {
    let programme = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    lancer(Command::new(programme).arg(dossier))
}

/// Lance sans attendre ni ouvrir de console. `explorer` rend un code non nul
/// même quand il réussit : seul l'échec du lancement compte.
fn lancer(c: &mut Command) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    match c.spawn() {
        Ok(_) => true,
        Err(e) => {
            eprintln!("liaison du navigateur : {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_navigateurs_se_reconnaissent_par_leur_nom() {
        assert_eq!(
            Navigateur::depuis("Opera GX").map(|n| n.page),
            Some("opera://extensions")
        );
        assert_eq!(
            Navigateur::depuis("chrome").map(|n| n.page),
            Some("chrome://extensions")
        );
        assert_eq!(
            Navigateur::depuis("Microsoft Edge").map(|n| n.page),
            Some("edge://extensions")
        );
        assert!(Navigateur::depuis("firefox").is_none());
        assert!(Navigateur::depuis("").is_none());
    }
}
