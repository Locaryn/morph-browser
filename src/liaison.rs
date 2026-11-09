//! Relier le navigateur de la personne : trouver le dossier de l'extension,
//! copier son chemin, et ouvrir la page du
//! navigateur où l'on charge une extension. Le chargement reste un geste de la
//! personne : hors de leur boutique, aucun navigateur ne permet de l'imposer.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Le temps laissé pour charger l'extension après `browser_link`.
pub const DUREE_APPAIRAGE: Duration = Duration::from_secs(120);

/// Un navigateur que l'on sait ouvrir sur sa page de chargement d'extension.
#[derive(Clone, Copy)]
pub struct Navigateur {
    /// Le nom court, celui des réglages.
    pub nom: &'static str,
    /// L'adresse de la page où l'on charge une extension non empaquetée.
    pub page: &'static str,
    /// Le dossier de l'extension, dans le paquet du morph.
    pub dossier: &'static str,
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
            nom: "chrome",
            dossier: "extension",
            page: "chrome://extensions",
            exe: "chrome",
            app_mac: "Google Chrome",
            chemins_windows: &["Google\\Chrome\\Application\\chrome.exe"],
        },
    ),
    (
        "edge",
        Navigateur {
            nom: "edge",
            dossier: "extension",
            page: "edge://extensions",
            exe: "msedge",
            app_mac: "Microsoft Edge",
            chemins_windows: &["Microsoft\\Edge\\Application\\msedge.exe"],
        },
    ),
    (
        "opera",
        Navigateur {
            nom: "opera",
            dossier: "extension",
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
            nom: "brave",
            dossier: "extension",
            page: "brave://extensions",
            exe: "brave",
            app_mac: "Brave Browser",
            chemins_windows: &["BraveSoftware\\Brave-Browser\\Application\\brave.exe"],
        },
    ),
    (
        "vivaldi",
        Navigateur {
            nom: "vivaldi",
            dossier: "extension",
            page: "vivaldi://extensions",
            exe: "vivaldi",
            app_mac: "Vivaldi",
            chemins_windows: &["Vivaldi\\Application\\vivaldi.exe"],
        },
    ),
    (
        "comet",
        Navigateur {
            nom: "comet",
            dossier: "extension",
            // Comet (Perplexity) est un Chromium : il sert l'adresse de Chrome.
            page: "chrome://extensions",
            exe: "comet",
            app_mac: "Comet",
            chemins_windows: &["Perplexity\\Comet\\Application\\comet.exe"],
        },
    ),
    (
        "firefox",
        Navigateur {
            nom: "firefox",
            // La page qui porte « Charger un module complémentaire temporaire ».
            page: "about:debugging#/runtime/this-firefox",
            dossier: "extension-firefox",
            exe: "firefox",
            app_mac: "Firefox",
            chemins_windows: &["Mozilla Firefox\\firefox.exe"],
        },
    ),
];

/// Le navigateur par défaut du système, s'il est connu ; sinon Chrome.
pub fn par_defaut() -> Navigateur {
    programme_par_defaut()
        .and_then(|p| Navigateur::depuis(&p))
        .unwrap_or(CONNUS[0].1)
}

/// L'identifiant du navigateur par défaut, tel que le système le nomme
/// (`ChromeHTML`, `MSEdgeHTM`, `FirefoxURL-…`, `brave-browser.desktop`…).
fn programme_par_defaut() -> Option<String> {
    let mut c = if cfg!(windows) {
        let mut c = Command::new("reg");
        c.args([
            "query",
            r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice",
            "/v",
            "ProgId",
        ]);
        c
    } else if cfg!(target_os = "linux") {
        let mut c = Command::new("xdg-settings");
        c.args(["get", "default-web-browser"]);
        c
    } else {
        return None;
    };
    sans_console(&mut c);
    let sortie = c.output().ok().filter(|o| o.status.success())?;
    let texte = String::from_utf8_lossy(&sortie.stdout);
    // `reg` : « ProgId    REG_SZ    ChromeHTML » ; xdg-settings : le nom seul.
    texte
        .lines()
        .rev()
        .find_map(|l| l.split_whitespace().last())
        .map(str::to_string)
}

impl Navigateur {
    pub fn depuis(nom: &str) -> Option<Self> {
        let nom = nom.trim().to_ascii_lowercase();
        if nom.is_empty() || nom == "auto" {
            return None;
        }
        CONNUS
            .iter()
            .find(|(cle, _)| nom.contains(cle))
            .map(|(_, n)| *n)
    }

    /// Ouvre la page des extensions. `false` : le navigateur est introuvable
    /// ou a refusé ; la personne tape l'adresse elle-même.
    pub fn ouvrir_page_extensions(self) -> bool {
        self.ouvrir(self.page)
    }

    /// Ouvre une adresse ou un fichier local dans ce navigateur.
    pub fn ouvrir(self, cible: &str) -> bool {
        if cfg!(windows) {
            if let Some(exe) = self.chemin_windows() {
                return lancer(Command::new(exe).arg(cible));
            }
            // Repli : l'exécutable déclaré à Windows (App Paths).
            let mut c = Command::new("cmd");
            c.args(["/C", "start", "", self.exe, cible]);
            return lancer(&mut c);
        }
        if cfg!(target_os = "macos") {
            let mut c = Command::new("open");
            c.args(["-a", self.app_mac, cible]);
            return lancer(&mut c);
        }
        lancer(Command::new(self.exe).arg(cible))
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

/// Le dossier de l'extension livré avec le morph (`extension`, ou
/// `extension-firefox`) : à côté du dossier `bin` du paquet installé, ou à la
/// racine du dépôt en développement.
pub fn dossier_extension(nom: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .skip(1)
        .map(|d| d.join(nom))
        .find(|d| d.join("manifest.json").is_file())
}

/// Copier un texte dans le presse-papiers : le chemin du dossier se colle
/// ensuite dans la fenêtre « Charger l'extension non empaquetée ».
pub fn copier(texte: &str) -> bool {
    let (programme, args): (&str, &[&str]) = if cfg!(windows) {
        // Lu sur l'entrée : aucun guillemet à échapper dans le chemin.
        (
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "Set-Clipboard -Value ([Console]::In.ReadToEnd().TrimEnd())",
            ],
        )
    } else if cfg!(target_os = "macos") {
        ("pbcopy", &[])
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        ("wl-copy", &[])
    } else {
        ("xclip", &["-selection", "clipboard"])
    };
    let mut c = Command::new(programme);
    c.args(args).stdin(std::process::Stdio::piped());
    sans_console(&mut c);
    let mut enfant = match c.spawn() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("presse-papiers : {programme} : {e}");
            return false;
        }
    };
    if let Some(mut entree) = enfant.stdin.take() {
        use std::io::Write as _;
        if let Err(e) = entree.write_all(texte.as_bytes()) {
            eprintln!("presse-papiers : {e}");
            return false;
        }
    }
    enfant.wait().map(|s| s.success()).unwrap_or(false)
}

fn sans_console(c: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = c;
}

/// Lance sans attendre ni ouvrir de console : seul l'échec du lancement compte.
fn lancer(c: &mut Command) -> bool {
    sans_console(c);
    // Un navigateur lancé d'ici garderait sinon la sortie du serveur MCP — le
    // canal qui le relie à l'application — ouverte aussi longtemps que lui.
    c.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
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
        assert_eq!(
            Navigateur::depuis("firefox").map(|n| n.dossier),
            Some("extension-firefox")
        );
        assert!(Navigateur::depuis("").is_none());
        assert!(Navigateur::depuis("auto").is_none());
    }

    #[test]
    fn le_navigateur_par_defaut_se_lit_dans_le_nom_du_systeme() {
        for (programme, nom) in [
            ("ChromeHTML", "chrome"),
            ("MSEdgeHTM", "edge"),
            ("BraveHTML", "brave"),
            ("OperaStable", "opera"),
            ("VivaldiHTM.ZIUNDNSV2NNAAP6BYLXRAS2J2E", "vivaldi"),
            ("FirefoxURL-308046B0AF4A39CB", "firefox"),
            ("CometHTM.WBIOHI4CNSRN6N4ARSPBJJYHDA", "comet"),
            ("brave-browser.desktop", "brave"),
            ("google-chrome.desktop", "chrome"),
        ] {
            assert_eq!(
                Navigateur::depuis(programme).map(|n| n.nom),
                Some(nom),
                "{programme}"
            );
        }
    }
}
