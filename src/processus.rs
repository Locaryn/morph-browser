//! Quel navigateur est au bout d'une connexion locale.
//!
//! Le User-Agent ne suffit pas : Comet se présente comme « Google Chrome ».
//! Le programme qui tient la connexion, lui, ne ment pas : `comet.exe`,
//! `opera.exe`, `chrome.exe`… On le lit dans la table des connexions TCP du
//! système, à partir du port local de l'extension.

use std::process::Command;

/// Le nom du programme qui tient la connexion TCP locale `port_distant` →
/// `port_pont` (en minuscules, sans `.exe`), s'il se trouve.
pub fn programme_de_la_connexion(port_distant: u16, port_pont: u16) -> Option<String> {
    if cfg!(windows) {
        let pid = pid_windows(port_distant, port_pont)?;
        return nom_du_pid_windows(pid);
    }
    if cfg!(target_os = "linux") {
        return nom_linux(port_distant);
    }
    None
}

fn sortie(programme: &str, args: &[&str]) -> Option<String> {
    let mut c = Command::new(programme);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    match c.output() {
        Ok(o) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => {
            eprintln!("{programme} : sortie {}", o.status);
            None
        }
        Err(e) => {
            eprintln!("{programme} : {e}");
            None
        }
    }
}

/// `netstat -ano -p TCP` : la ligne dont l'adresse locale porte le port de
/// l'extension et l'adresse distante celui du pont.
fn pid_windows(port_distant: u16, port_pont: u16) -> Option<u32> {
    let texte = sortie("netstat", &["-ano", "-p", "TCP"])?;
    pid_dans_netstat(&texte, port_distant, port_pont)
}

fn pid_dans_netstat(texte: &str, port_distant: u16, port_pont: u16) -> Option<u32> {
    let local = format!(":{port_distant}");
    let distant = format!(":{port_pont}");
    texte.lines().find_map(|l| {
        let champs: Vec<&str> = l.split_whitespace().collect();
        match champs.as_slice() {
            [_, loc, dist, _, pid] if loc.ends_with(&local) && dist.ends_with(&distant) => {
                pid.parse().ok()
            }
            _ => None,
        }
    })
}

fn nom_du_pid_windows(pid: u32) -> Option<String> {
    let filtre = format!("PID eq {pid}");
    let texte = sortie("tasklist", &["/FI", &filtre, "/FO", "CSV", "/NH"])?;
    nom_dans_tasklist(&texte)
}

fn nom_dans_tasklist(texte: &str) -> Option<String> {
    let ligne = texte.lines().find(|l| l.starts_with('"'))?;
    let nom = ligne.trim_start_matches('"').split('"').next()?;
    Some(nom.trim_end_matches(".exe").to_ascii_lowercase())
}

/// `ss -tnpH` : `users:(("chrome",pid=…))` sur la ligne du port local.
fn nom_linux(port_distant: u16) -> Option<String> {
    let texte = sortie("ss", &["-tnpH"])?;
    let local = format!(":{port_distant} ");
    let ligne = texte.lines().find(|l| l.contains(&local))?;
    let debut = ligne.find("((\"")? + 3;
    let fin = ligne[debut..].find('"')? + debut;
    Some(ligne[debut..fin].to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netstat_et_tasklist_se_lisent() {
        let netstat = "\n  Proto  Adresse locale         Adresse distante       État\n  \
            TCP    127.0.0.1:17421        127.0.0.1:52311        ESTABLISHED     4242\n  \
            TCP    127.0.0.1:52311        127.0.0.1:17421        ESTABLISHED     9876\n";
        assert_eq!(pid_dans_netstat(netstat, 52311, 17421), Some(9876));
        assert_eq!(pid_dans_netstat(netstat, 50000, 17421), None);
        let tasklist = "\"comet.exe\",\"9876\",\"Console\",\"1\",\"120 000 Ko\"\r\n";
        assert_eq!(nom_dans_tasklist(tasklist).as_deref(), Some("comet"));
    }

    /// Sur une vraie connexion locale, le programme trouvé est ce test.
    #[cfg(windows)]
    #[test]
    fn le_programme_d_une_vraie_connexion_se_trouve() {
        let ecoute = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port_pont = ecoute.local_addr().unwrap().port();
        let client = std::net::TcpStream::connect(("127.0.0.1", port_pont)).unwrap();
        let port_client = client.local_addr().unwrap().port();
        let nom = programme_de_la_connexion(port_client, port_pont).unwrap();
        assert!(nom.starts_with("locaryn_plugin_browser"), "{nom}");
    }
}
