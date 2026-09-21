//! Pont vers Laya, le moteur de décision non autorégressif.
//!
//! Laya ne planifie pas et ne génère rien : il répond à des questions typées
//! (`choice`, `noul`) sur un état, en un seul passage. On l'emploie donc comme
//! couche rapide **sous** le modèle de conversation : choisir l'élément visé
//! parmi des candidats, juger le risque d'une action, estimer si l'objectif est
//! atteint. Le modèle de conversation garde la planification.
//!
//! Le modèle tourne dans un processus Python qui reste vivant : le chargement
//! coûte des secondes, chaque réponse ensuite quelques dizaines de millisecondes.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::text::lexical_score;

const SCRIPT: &str = include_str!("../python/laya_server.py");
/// Au-delà d'une vingtaine d'options Laya perd en précision : on procède par
/// tournoi de groupes de cette taille.
const GROUPE_MAX: usize = 12;
const DELAI_CHARGEMENT: Duration = Duration::from_secs(300);
const DELAI_REPONSE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayaConfig {
    /// `multilingual` (défaut), `english`, `typed-decisions` ou `router`.
    #[serde(default = "checkpoint_defaut")]
    pub laya_checkpoint: String,
    /// `cuda` ou `cpu`. Vide : Laya choisit.
    #[serde(default)]
    pub laya_device: String,
}

fn checkpoint_defaut() -> String {
    "multilingual".into()
}

impl Default for LayaConfig {
    fn default() -> Self {
        Self {
            laya_checkpoint: checkpoint_defaut(),
            laya_device: String::new(),
        }
    }
}

/// Lit le fichier de réglages désigné par l'hôte ; les défauts si absent ou invalide.
pub fn read_config<T: Default + for<'de> Deserialize<'de>>() -> T {
    let Some(p) = std::env::var("LOCARYN_EXTENSION_CONFIG_FILE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
    else {
        return T::default();
    };
    let brut = match std::fs::read_to_string(&p) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("réglages illisibles ({}) : {e}", p.display());
            return T::default();
        }
    };
    serde_json::from_str(&brut).unwrap_or_else(|e| {
        eprintln!("réglages invalides ({}) : {e}", p.display());
        T::default()
    })
}

/// Un candidat à départager.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub id: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub id: String,
    pub confidence: f32,
    /// Les candidats du dernier tour, du plus au moins probable.
    pub ranking: Vec<(String, f32)>,
}

/// Un classement de candidats et la façon dont il a été obtenu.
#[derive(Debug, Clone)]
pub struct Ranked {
    pub ranking: Vec<(String, f32)>,
    /// `{"name":"laya","ms":…}` ou `{"name":"lexical"}`.
    pub engine: Value,
    /// Pourquoi Laya n'a pas servi, si c'est le cas.
    pub warning: Option<String>,
}

struct Process {
    child: Child,
    stdin: ChildStdin,
    stdout: tokio::io::Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

pub struct Laya {
    config: LayaConfig,
    process: Mutex<Option<Process>>,
    last_error: Mutex<Option<String>>,
}

impl Default for Laya {
    fn default() -> Self {
        Self::new()
    }
}

impl Laya {
    pub fn new() -> Self {
        Self {
            config: read_config(),
            process: Mutex::new(None),
            last_error: Mutex::new(None),
        }
    }

    /// Dernière raison pour laquelle Laya n'a pas répondu, si elle existe.
    pub async fn last_error(&self) -> Option<String> {
        self.last_error.lock().await.clone()
    }

    /// Classe des candidats : Laya si possible, repli lexical sinon, dit tel quel.
    pub async fn rank(&self, state: &Value, instruction: &str, candidates: &[Candidate]) -> Ranked {
        let debut = std::time::Instant::now();
        match self.choose(state, instruction, candidates).await {
            Ok(c) => Ranked {
                ranking: c.ranking,
                engine: json!({ "name": "laya", "ms": debut.elapsed().as_millis() as u64 }),
                warning: None,
            },
            Err(e) => {
                let goal = state.to_string();
                let mut ranking: Vec<(String, f32)> = candidates
                    .iter()
                    .map(|c| (c.id.clone(), lexical_score(&goal, &c.description) as f32))
                    .collect();
                ranking.sort_by(|a, b| b.1.total_cmp(&a.1));
                Ranked {
                    ranking,
                    engine: json!({ "name": "lexical" }),
                    warning: Some(format!("Laya indisponible, classement lexical : {e}")),
                }
            }
        }
    }

    /// Charge le modèle sans lui poser de question.
    pub async fn warm(&self) -> Result<(), String> {
        self.request(json!({ "op": "ping" })).await.map(|_| ())
    }

    /// Le modèle est-il chargé et prêt ?
    pub async fn is_running(&self) -> bool {
        self.process.lock().await.is_some()
    }

    /// Choisir le meilleur candidat pour `instruction`.
    pub async fn choose(
        &self,
        state: &Value,
        instruction: &str,
        options: &[Candidate],
    ) -> Result<Choice, String> {
        if options.is_empty() {
            return Err("aucun candidat à départager".into());
        }
        if options.len() == 1 {
            return Ok(Choice {
                id: options[0].id.clone(),
                confidence: 1.0,
                ranking: vec![(options[0].id.clone(), 1.0)],
            });
        }
        let mut manche: Vec<Candidate> = options.to_vec();
        loop {
            let mut gagnants = Vec::new();
            let mut dernier: Option<Choice> = None;
            for groupe in manche.chunks(GROUPE_MAX) {
                let c = self.choose_group(state, instruction, groupe).await?;
                if let Some(g) = groupe.iter().find(|o| o.id == c.id) {
                    gagnants.push(g.clone());
                }
                dernier = Some(c);
            }
            if manche.len() <= GROUPE_MAX {
                return dernier.ok_or_else(|| "aucune réponse de Laya".to_string());
            }
            if gagnants.len() >= manche.len() {
                return Err("le tournoi ne converge pas".into());
            }
            manche = gagnants;
        }
    }

    async fn choose_group(
        &self,
        state: &Value,
        instruction: &str,
        options: &[Candidate],
    ) -> Result<Choice, String> {
        let criteria: Map<String, Value> = options
            .iter()
            .map(|o| (o.id.clone(), Value::String(o.description.clone())))
            .collect();
        let q = json!({ "pick": { "type": "choice", "instructions": instruction, "criteria": criteria } });
        let answers = self.predict(state, q).await?;
        let a = answers
            .get("pick")
            .ok_or_else(|| "réponse de Laya sans champ « pick »".to_string())?;
        parse_choice(a)
    }

    /// Probabilité (0 à 1) que l'affirmation posée soit vraie.
    pub async fn judge(&self, state: &Value, instruction: &str) -> Result<f32, String> {
        let q = json!({ "verdict": { "type": "noul", "instructions": instruction } });
        let answers = self.predict(state, q).await?;
        answers
            .get("verdict")
            .and_then(|a| a.get("noul"))
            .and_then(Value::as_f64)
            .map(|p| p as f32)
            .ok_or_else(|| "réponse de Laya sans probabilité".to_string())
    }

    async fn predict(&self, state: &Value, questions: Value) -> Result<Value, String> {
        let res = self
            .request(json!({ "op": "predict", "state": state, "questions": questions }))
            .await;
        if let Err(e) = &res {
            *self.last_error.lock().await = Some(e.clone());
        }
        res?.get("answers")
            .cloned()
            .ok_or_else(|| "réponse de Laya sans réponses".to_string())
    }

    async fn request(&self, mut req: Value) -> Result<Value, String> {
        let mut garde = self.process.lock().await;
        if garde.is_none() {
            *garde = Some(self.spawn().await?);
        }
        let Some(p) = garde.as_mut() else {
            return Err("processus Laya absent".into());
        };
        p.next_id += 1;
        req["id"] = json!(p.next_id);
        let ligne = format!("{req}\n");
        let resultat = echange(p, &ligne).await;
        match resultat {
            Ok(v) if v.get("ok") == Some(&Value::Bool(true)) => Ok(v),
            Ok(v) => Err(v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Laya a refusé la demande")
                .to_string()),
            Err(e) => {
                // Un processus dans un état douteux ne sert plus : le suivant repart propre.
                if let Some(mut p) = garde.take() {
                    if let Err(k) = p.child.kill().await {
                        eprintln!("arrêt de Laya : {k}");
                    }
                }
                Err(e)
            }
        }
    }

    async fn spawn(&self) -> Result<Process, String> {
        let python = find_python().ok_or_else(|| {
            "Python introuvable : Laya (pip install laya) demande Python 3.10 ou plus".to_string()
        })?;
        let script = write_script()?;
        let mut child = Command::new(&python)
            .envs(python_env(&self.config))
            .arg("-u")
            .arg(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("impossible de lancer Python : {e}"))?;
        let stdin = child.stdin.take().ok_or("stdin de Laya indisponible")?;
        let stdout = child.stdout.take().ok_or("stdout de Laya indisponible")?;
        let mut lignes = BufReader::new(stdout).lines();
        let accueil = timeout(DELAI_CHARGEMENT, lignes.next_line())
            .await
            .map_err(|_| {
                "Laya met trop de temps à se charger (premier lancement : téléchargement du modèle ?)"
                    .to_string()
            })?
            .map_err(|e| format!("lecture de Laya impossible : {e}"))?;
        let Some(accueil) = accueil else {
            return Err(format!(
                "Laya s'est arrêté au démarrage : {}",
                stderr_tail(&mut child).await
            ));
        };
        let v: Value = serde_json::from_str(&accueil)
            .map_err(|e| format!("accueil de Laya illisible : {e}"))?;
        if v.get("ready") != Some(&Value::Bool(true)) {
            let raison = v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("raison inconnue");
            return Err(format!(
                "Laya indisponible ({raison}). Installez-le avec : {python} -m pip install laya"
            ));
        }
        Ok(Process {
            child,
            stdin,
            stdout: lignes,
            next_id: 0,
        })
    }
}

async fn echange(p: &mut Process, ligne: &str) -> Result<Value, String> {
    p.stdin
        .write_all(ligne.as_bytes())
        .await
        .map_err(|e| format!("écriture vers Laya impossible : {e}"))?;
    p.stdin
        .flush()
        .await
        .map_err(|e| format!("écriture vers Laya impossible : {e}"))?;
    match timeout(DELAI_REPONSE, p.stdout.next_line()).await {
        Err(_) => Err("Laya n'a pas répondu à temps".into()),
        Ok(Err(e)) => Err(format!("lecture de Laya impossible : {e}")),
        Ok(Ok(None)) => Err("Laya s'est arrêté".into()),
        Ok(Ok(Some(l))) => {
            serde_json::from_str(&l).map_err(|e| format!("réponse de Laya illisible : {e}"))
        }
    }
}

async fn stderr_tail(child: &mut Child) -> String {
    use tokio::io::AsyncReadExt;
    let Some(mut e) = child.stderr.take() else {
        return String::new();
    };
    let mut buf = String::new();
    if timeout(Duration::from_secs(2), e.read_to_string(&mut buf))
        .await
        .is_err()
    {
        eprintln!("lecture de l'erreur de Laya : délai dépassé");
    }
    let lignes: Vec<&str> = buf.lines().filter(|l| !l.trim().is_empty()).collect();
    lignes[lignes.len().saturating_sub(3)..].join(" / ")
}

/// Extrait le choix d'une réponse `choice`. Les probabilités sont lues sous
/// l'un des noms que Laya emploie.
fn parse_choice(a: &Value) -> Result<Choice, String> {
    let id = a
        .get("choice")
        .and_then(Value::as_str)
        .ok_or("réponse « choice » sans étiquette")?
        .to_string();
    let mut ranking: Vec<(String, f32)> = ["probabilities", "probs", "distribution"]
        .iter()
        .find_map(|k| a.get(*k).and_then(Value::as_object))
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_f64().map(|p| (k.clone(), p as f32)))
                .collect()
        })
        .unwrap_or_default();
    ranking.sort_by(|x, y| y.1.total_cmp(&x.1));
    // Le champ `confidence` de Laya mesure autre chose que la part du choix
    // retenu (0,44 pour un choix à 0,80) : ce qui compte ici est cette part.
    let confidence = match ranking.first() {
        Some((_, p)) => *p,
        None => a.get("confidence").and_then(Value::as_f64).unwrap_or(0.0) as f32,
    };
    if ranking.is_empty() {
        ranking.push((id.clone(), confidence));
    }
    Ok(Choice {
        id,
        confidence,
        ranking,
    })
}

fn write_script() -> Result<PathBuf, String> {
    let dossier = std::env::var("LOCARYN_TEMP_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("locaryn-laya");
    std::fs::create_dir_all(&dossier).map_err(|e| format!("dossier temporaire : {e}"))?;
    let chemin = dossier.join("laya_server.py");
    std::fs::write(&chemin, SCRIPT).map_err(|e| format!("écriture du serveur Laya : {e}"))?;
    Ok(chemin)
}

/// Le Python à employer : l'environnement géré par l'hôte d'abord, puis le PATH.
pub fn find_python() -> Option<String> {
    for venv in venvs() {
        let exe = if cfg!(windows) {
            venv.join("Scripts").join("python.exe")
        } else {
            venv.join("bin").join("python")
        };
        if exe.exists() {
            return Some(exe.to_string_lossy().to_string());
        }
    }
    let sonde = std::process::Command::new("python")
        .arg("--version")
        .output();
    if matches!(sonde, Ok(o) if o.status.success()) {
        return Some("python".to_string());
    }
    None
}

fn venvs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(v) = std::env::var("LOCARYN_PYTHON_VENV") {
        if !v.trim().is_empty() {
            out.push(PathBuf::from(v));
        }
    }
    for key in ["LOCARYN_MODELS_DIR", "LOCARYN_EXTENSION_MODELS_DIR"] {
        if let Ok(dir) = std::env::var(key) {
            if let Some(parent) = Path::new(&dir).parent() {
                out.push(parent.join("python-env"));
                out.push(parent.join(".venv"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        out.push(cwd.join(".venv"));
    }
    out
}

/// Cache HuggingFace et temporaires : ils suivent le volume des poids, jamais
/// le disque système.
fn python_env(cfg: &LayaConfig) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("TRANSFORMERS_NO_TF", "1".to_string()),
        ("USE_TF", "0".to_string()),
        ("TF_CPP_MIN_LOG_LEVEL", "3".to_string()),
        ("PYTHONIOENCODING", "utf-8".to_string()),
        ("LAYA_CHECKPOINT", cfg.laya_checkpoint.clone()),
        ("LAYA_DEVICE", cfg.laya_device.clone()),
    ];
    if let Ok(hf) = std::env::var("LOCARYN_HF_CACHE_DIR") {
        if !hf.trim().is_empty() {
            if let Err(e) = std::fs::create_dir_all(&hf) {
                eprintln!("cache HuggingFace : {e}");
            }
            env.push(("HF_HOME", hf));
        }
    }
    if let Ok(tmp) = std::env::var("LOCARYN_TEMP_DIR") {
        if !tmp.trim().is_empty() {
            env.push(("TMPDIR", tmp.clone()));
            env.push(("TEMP", tmp.clone()));
            env.push(("TMP", tmp));
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_choix_se_lit_avec_ses_probabilites_triees() {
        let a = json!({"choice":"b","confidence":0.7,"probabilities":{"a":0.1,"b":0.7,"c":0.2}});
        let c = parse_choice(&a).unwrap();
        assert_eq!(c.id, "b");
        assert!((c.confidence - 0.7).abs() < 1e-6);
        assert_eq!(c.ranking[0].0, "b");
        assert_eq!(c.ranking[1].0, "c");
    }

    #[test]
    fn un_choix_sans_etiquette_est_refuse() {
        assert!(parse_choice(&json!({"confidence":1.0})).is_err());
    }

    #[tokio::test]
    async fn un_seul_candidat_ne_demande_pas_de_modele() {
        let laya = Laya::new();
        let c = laya
            .choose(
                &json!({}),
                "x",
                &[Candidate {
                    id: "e1".into(),
                    description: "d".into(),
                }],
            )
            .await
            .unwrap();
        assert_eq!(c.id, "e1");
        assert!(!laya.is_running().await);
    }
}
