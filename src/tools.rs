//! Outils MCP du navigateur.
//!
//! Le modèle de conversation planifie ; Laya tranche vite les questions
//! fermées : quel élément, quel onglet, cette action est-elle risquée,
//! l'objectif est-il atteint. Sans Laya, chaque outil retombe sur un repli
//! lexical et le dit dans sa réponse : rien n'est simulé en silence.

use crate::bridge::{load_or_create_token, Bridge};
use crate::config::BrowserConfig;
use crate::liaison::{
    choisir_auto, copier, dossier_extension, navigateur_par_defaut, Navigateur, DUREE_APPAIRAGE,
};
use base64::Engine;
use locaryn_morph_kit::laya::{read_config, Candidate, Laya};
use locaryn_morph_kit::mcp::{str_prop, tool};
use locaryn_morph_kit::risk::assess;
use locaryn_morph_kit::text::{short, shortlist, text_arg};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Nombre d'éléments soumis à Laya après présélection lexicale.
const SHORTLIST: usize = 40;
const SEUIL_CONFIANCE: f32 = 0.6;

/// Ce qu'on retient d'une page vue, pour juger une action sans la relire.
#[derive(Default, Clone)]
struct PageMemo {
    title: String,
    url: String,
    names: HashMap<u64, String>,
}

pub struct Browser {
    bridge: Arc<Bridge>,
    laya: Arc<Laya>,
    warmed: AtomicBool,
    cfg: BrowserConfig,
    memo: Mutex<HashMap<u64, PageMemo>>,
    last_tab: Mutex<Option<u64>>,
    /// Le dernier lancement du navigateur par `browser_link` : un second appel
    /// rapproché (le bouton, puis le modèle) n'ouvre pas un onglet de plus.
    derniere_liaison: Mutex<Option<Instant>>,
    /// Le dernier navigateur nommé (par `browser` ou `browser_launch`) : les
    /// commandes suivantes sans navigateur y restent. « Ouvre Opera », puis
    /// « cherche ceci » : la recherche se fait dans Opera.
    dernier_navigateur: Mutex<Option<String>>,
}

/// Le temps laissé à une extension déjà installée pour se relier après le
/// lancement de son navigateur (elle se présente toutes les trois secondes).
const ATTENTE_APRES_LANCEMENT: Duration = Duration::from_secs(10);

/// Le délai pendant lequel `browser_link` ne relance pas le navigateur.
const LIAISON_RECENTE: Duration = Duration::from_secs(20);

/// Le temps laissé à une extension déjà chargée pour se relier seule avant
/// d'ouvrir le guide dans le navigateur.
const ATTENTE_AVANT_GUIDE: Duration = Duration::from_secs(8);

impl Browser {
    pub async fn new() -> Result<Self, String> {
        let cfg: BrowserConfig = read_config();
        let token = load_or_create_token()?;
        let bridge = Bridge::start(cfg.port, token).await;
        bridge.preferer(&cible_par_defaut(&cfg.browser)).await;
        Ok(Self {
            bridge,
            laya: Arc::new(Laya::new()),
            warmed: AtomicBool::new(false),
            cfg,
            memo: Mutex::new(HashMap::new()),
            last_tab: Mutex::new(None),
            derniere_liaison: Mutex::new(None),
            dernier_navigateur: Mutex::new(None),
        })
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        self.warm_up(name);
        let args = self.completer_navigateur(name, args).await;
        match name {
            "browser_status" => self.status().await,
            "browser_link" => self.link(&args).await,
            "browser_launch" => self.launch(&args).await,
            "ui_panel_inbox" => self.panel_inbox().await,
            "ui_panel_reply" => self.panel_reply(&args).await,
            "ui_panel_models" => self.panel_models(&args).await,
            "ui_browsers" => self.browsers().await,
            "browser_find_tab" => self.find_tab(&args).await,
            "browser_snapshot" => self.snapshot(args).await,
            "browser_find_element" => self.find_element(&args).await,
            // Les anciens noms restent compris : le nom court était pris par
            // le navigateur intégré de Locaryn, qui le servait à la place.
            "browser_click_element" | "browser_click" => self.click(args).await,
            "browser_type_text" | "browser_type" => self.type_text(args).await,
            "browser_screenshot" => self.screenshot(args).await,
            "browser_goal_reached" => self.goal_reached(&args).await,
            "browser_assess_action" => self.assess_action(&args).await,
            other => match passthrough(other) {
                Some(method) => self.bridge.call(method, args).await,
                None => Err(format!("Outil navigateur inconnu : {other}")),
            },
        }
    }

    /// Retient le navigateur nommé, ou ajoute le dernier nommé à une commande
    /// qui n'en nomme pas, s'il est toujours relié. La liste des onglets sans
    /// navigateur reste celle de tous.
    async fn completer_navigateur(&self, outil: &str, mut args: Value) -> Value {
        if !vise_un_navigateur(outil) || outil == "browser_launch" {
            return args;
        }
        let nomme = args
            .get("browser")
            .and_then(Value::as_str)
            .map(|b| b.trim().to_ascii_lowercase())
            .filter(|b| !b.is_empty());
        if let Some(nom) = nomme {
            *self.dernier_navigateur.lock().await = Some(nom);
            return args;
        }
        if outil == "browser_list_tabs" {
            return args;
        }
        let dernier = self.dernier_navigateur.lock().await.clone();
        if let Some(nom) = dernier {
            if self.bridge.est_relie(&nom).await {
                if let Value::Object(o) = &mut args {
                    o.insert("browser".into(), json!(nom));
                }
            }
        }
        args
    }

    /// Charge Laya en arrière-plan dès le premier vrai usage : le chargement
    /// dure des dizaines de secondes, autant qu'il se fasse pendant que le
    /// modèle de conversation réfléchit.
    fn warm_up(&self, tool: &str) {
        if tool == "browser_status"
            || tool == "browser_link"
            || tool.starts_with("ui_")
            || self.warmed.swap(true, Ordering::Relaxed)
        {
            return;
        }
        let laya = self.laya.clone();
        tokio::spawn(async move {
            if let Err(e) = laya.warm().await {
                eprintln!("préchauffage de Laya : {e}");
            }
        });
    }

    // ── État ────────────────────────────────────────────────────────────────

    async fn status(&self) -> Result<Value, String> {
        let navigateurs = self.bridge.navigateurs().await;
        let connected = !navigateurs.is_empty();
        let mut out = json!({
            "extension_connected": connected,
            // Plusieurs navigateurs peuvent être reliés : un outil vise l'un
            // d'eux par `browser`, sinon le navigateur préféré des réglages.
            "browsers": navigateurs,
            "extension_version_shipped": version_livree(),
            "port": self.bridge.port,
            // Le modèle de décision local reste un détail interne : le
            // modèle de conversation n'a pas à le nommer à l'utilisateur.
            "decisions": {
                "ready": self.laya.is_running().await,
                "last_error": self.laya.last_error().await,
            },
            "tabs_stopped_by_user": self.bridge.blocked_tabs().await,
        });
        if let Some(e) = self.bridge.bind_error().await {
            out["bridge_error"] = json!(e);
        }
        if !connected {
            // Le code d'appairage ne passe plus par le modèle : browser_link
            // relie l'extension sans que personne ne le recopie.
            out["setup"] = json!(
                "Extension non reliée : appelez browser_link (avec le navigateur de la personne), \
                 puis donnez-lui les étapes qu'il rend."
            );
        }
        Ok(out)
    }

    /// Relier le navigateur : ouvre la fenêtre d'appairage, le dossier de
    /// l'extension et la page des extensions du navigateur choisi.
    async fn link(&self, args: &Value) -> Result<Value, String> {
        // Relu à chaque appel : le choix vient d'être enregistré dans les
        // réglages, alors que le serveur a lu les siens au démarrage.
        let depuis_reglages = read_config::<BrowserConfig>().browser;
        self.bridge
            .preferer(&cible_par_defaut(&depuis_reglages))
            .await;
        let choisi = args
            .get("browser")
            .and_then(Value::as_str)
            .filter(|b| !b.trim().is_empty())
            .unwrap_or(depuis_reglages.as_str())
            .trim()
            .to_ascii_lowercase();
        let nomme = Navigateur::depuis(&choisi);
        // La fenêtre d'appairage accepte l'extension de N'IMPORTE QUEL
        // navigateur qui attend : réglé sur Opera, « Relier » refusait
        // l'extension que la personne venait d'installer dans Chrome.
        let avant: Vec<String> = noms(&self.bridge.navigateurs().await);
        self.bridge.ouvrir_appairage(DUREE_APPAIRAGE, None).await;
        // Une extension déjà chargée et en attente se présente toutes les
        // trois secondes : elle reçoit le code seule, sans rien ouvrir.
        let fin = Instant::now() + ATTENTE_AVANT_GUIDE;
        let mut nouveaux: Vec<String> = Vec::new();
        while Instant::now() < fin && nouveaux.is_empty() {
            tokio::time::sleep(Duration::from_millis(500)).await;
            nouveaux = noms(&self.bridge.navigateurs().await)
                .into_iter()
                .filter(|n| !avant.contains(n))
                .collect();
        }
        let navigateurs = self.bridge.navigateurs().await;
        if !nouveaux.is_empty() {
            let relies: Vec<String> = nouveaux.iter().map(|n| nom_affiche(n)).collect();
            return Ok(json!({
                "extension_connected": true,
                "browsers": navigateurs,
                "message": format!("{} relié à Locaryn, sans code à recopier.", relies.join(", ")),
            }));
        }
        // Rien de nouveau : le navigateur choisi est-il déjà relié ? En
        // « auto », un navigateur relié suffit.
        let deja = match nomme {
            Some(n) => navigateurs
                .iter()
                .any(|x| x["browser"].as_str() == Some(n.nom)),
            None => !navigateurs.is_empty(),
        };
        if deja {
            return Ok(json!({
                "extension_connected": true,
                "browsers": navigateurs,
                "message": format!(
                    "Déjà relié : {}. Pour un autre navigateur, chargez-y l'extension : elle se relie seule dans les {} secondes.",
                    liste_navigateurs(&navigateurs),
                    DUREE_APPAIRAGE.as_secs()
                ),
            }));
        }
        // « auto » ou rien : un navigateur ouvert, sinon celui du système.
        let navigateur = nomme.unwrap_or_else(choisir_auto);
        let dossier = dossier_extension(navigateur.dossier);
        let chemin = dossier.as_ref().map(|d| d.display().to_string());
        let chemin_copie = chemin.as_deref().is_some_and(copier);
        let guide = crate::guide::ecrire(&navigateur, chemin.as_deref());
        // Un seul lancement du navigateur, et pas deux en vingt secondes.
        let deja_ouvert = {
            let mut derniere = self.derniere_liaison.lock().await;
            let recente = derniere.is_some_and(|t| t.elapsed() < LIAISON_RECENTE);
            if !recente {
                *derniere = Some(Instant::now());
            }
            recente
        };
        let guide_ouvert =
            deja_ouvert || navigateur.ouvrir(&navigateur.cibles_liaison(guide.as_deref()));
        Ok(json!({
            "guide_page": guide.as_ref().map(|g| g.display().to_string()),
            "guide_page_opened": guide_ouvert,
            "guide_page_already_open": deja_ouvert,
            "browser": navigateur.nom,
            "pairing_window_seconds": DUREE_APPAIRAGE.as_secs(),
            "extension_folder": chemin,
            "folder_path_copied": chemin_copie,
            "extensions_page": navigateur.page,
            "message": consigne(&navigateur, chemin.as_deref(), chemin_copie),
        }))
    }

    async fn attendre_la_liaison(&self, vise: Option<&str>, delai: Duration) -> bool {
        let fin = Instant::now() + delai;
        loop {
            let relie = match vise {
                Some(nom) => self.bridge.est_relie(nom).await,
                None => self.bridge.is_connected().await,
            };
            if relie || Instant::now() >= fin {
                return relie;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Lance un navigateur s'il n'est pas ouvert, attend que son extension se
    /// relie, puis en fait le navigateur des commandes suivantes.
    async fn launch(&self, args: &Value) -> Result<Value, String> {
        let demande = args
            .get("browser")
            .and_then(Value::as_str)
            .map(|b| b.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let navigateur = if demande.is_empty() || demande == "auto" {
            // « Mon navigateur » : le dernier utilisé, sinon celui des
            // réglages ou du système, sinon un navigateur ouvert.
            let dernier = self.dernier_navigateur.lock().await.clone();
            let reglage = cible_par_defaut(&read_config::<BrowserConfig>().browser);
            dernier
                .as_deref()
                .and_then(Navigateur::depuis)
                .or_else(|| Navigateur::depuis(&reglage))
                .unwrap_or_else(choisir_auto)
        } else {
            Navigateur::depuis(&demande)
                .ok_or_else(|| format!("Navigateur inconnu : « {demande} »."))?
        };
        let nom = navigateur.nom;
        let url = args
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string);
        let deja_ouvert = tokio::task::spawn_blocking(move || navigateur.est_ouvert())
            .await
            .map_err(|e| format!("état du navigateur : {e}"))?;
        let lance = if deja_ouvert || self.bridge.est_relie(nom).await {
            false
        } else {
            tokio::task::spawn_blocking(move || match url_de_lancement(url.as_deref()) {
                Some(u) => navigateur.ouvrir(&[u]),
                None => navigateur.demarrer(),
            })
            .await
            .map_err(|e| format!("lancement : {e}"))?
        };
        let relie = self
            .attendre_la_liaison(Some(nom), ATTENTE_APRES_LANCEMENT)
            .await;
        if !relie {
            return Ok(json!({
                "browser": nom,
                "launched": lance,
                "already_open": deja_ouvert,
                "extension_connected": false,
                "message": format!(
                    "{} est {}, mais son extension Locaryn n'est pas reliée. Appelez browser_link avec browser=\"{nom}\" pour l'installer.",
                    nom_affiche(nom),
                    if lance { "lancé" } else { "ouvert" }
                ),
            }));
        }
        *self.dernier_navigateur.lock().await = Some(nom.to_string());
        // Ouvert avant l'appel : l'adresse demandée s'ouvre par l'extension.
        let onglet = match (&args["url"], lance) {
            (Value::String(u), false) if !u.trim().is_empty() => Some(
                self.bridge
                    .call("tab_open", json!({ "url": u, "browser": nom }))
                    .await?,
            ),
            _ => None,
        };
        Ok(json!({
            "browser": nom,
            "launched": lance,
            "already_open": deja_ouvert,
            "extension_connected": true,
            "tab": onglet,
            "message": format!(
                "{} est prêt : les commandes suivantes sans `browser` vont à lui.",
                nom_affiche(nom)
            ),
        }))
    }

    /// Les navigateurs reliés, pour le panneau du morph dans Locaryn.
    async fn browsers(&self) -> Result<Value, String> {
        let navigateurs = self.bridge.navigateurs().await;
        let livree = version_livree();
        let mut lignes: Vec<String> = navigateurs
            .iter()
            .map(|n| {
                let nom = nom_affiche(n["browser"].as_str().unwrap_or("?"));
                let version = n["extension_version"].as_str().unwrap_or("?");
                let etat = match livree.as_deref() {
                    Some(l) if l != version => format!(" (mise à jour vers {l} en cours)"),
                    _ => String::new(),
                };
                format!("{nom} — extension {version}{etat}")
            })
            .collect();
        if lignes.is_empty() {
            lignes.push("Aucun navigateur relié.".into());
        }
        if let Some(l) = &livree {
            lignes.push(format!("Version livrée avec le morph : {l}."));
        }
        Ok(json!({
            "browsers": navigateurs,
            "extension_version_shipped": livree,
            "message": lignes.join(" · "),
        }))
    }

    // ── Barre latérale de l'extension ───────────────────────────────────────
    //
    // Réservés au panneau du morph dans Locaryn (préfixe `ui_` : le modèle ne
    // les voit pas). Le panneau relève les consignes tapées dans la barre
    // latérale, les poste dans la conversation, et renvoie la réponse.

    async fn panel_inbox(&self) -> Result<Value, String> {
        let messages = self.bridge.prendre_messages_panneau().await;
        Ok(json!({
            "extension_connected": self.bridge.is_connected().await,
            "messages": messages,
        }))
    }

    async fn panel_reply(&self, args: &Value) -> Result<Value, String> {
        let text = text_arg(args, "text")?;
        let id = args.get("id").cloned().unwrap_or(Value::Null);
        let erreur = args.get("error").and_then(Value::as_bool).unwrap_or(false);
        // La réponse revient à la barre latérale qui a posé la question.
        let cible = args.get("browser").and_then(Value::as_str);
        self.bridge
            .pousser(
                json!({ "type": "panel_reply", "id": id, "text": text, "error": erreur }),
                cible,
            )
            .await?;
        Ok(json!({ "delivered": true }))
    }

    /// La liste des modèles de la conversation, poussée aux barres latérales
    /// pour leur sélecteur.
    async fn panel_models(&self, args: &Value) -> Result<Value, String> {
        if !self.bridge.is_connected().await {
            return Ok(json!({ "delivered": false }));
        }
        let message = json!({
            "type": "panel_models",
            "active": args.get("active").cloned().unwrap_or(Value::Null),
            "installed": args.get("installed").cloned().unwrap_or_else(|| json!([])),
        });
        self.bridge.pousser(message, None).await?;
        Ok(json!({ "delivered": true }))
    }

    // ── Onglets ─────────────────────────────────────────────────────────────

    async fn find_tab(&self, args: &Value) -> Result<Value, String> {
        let wanted = text_arg(args, "description")?;
        let params = json!({ "browser": args.get("browser").cloned().unwrap_or(Value::Null) });
        let tabs = self.bridge.call("tabs_list", params).await?;
        let list = tabs["tabs"].as_array().cloned().unwrap_or_default();
        if list.is_empty() {
            return Err("Aucun onglet ouvert.".into());
        }
        let candidates: Vec<Candidate> = list
            .iter()
            .filter_map(|t| {
                t["tab_id"].as_u64()?;
                Some(Candidate {
                    id: id_onglet(t),
                    description: format!(
                        "{} — {}",
                        t["title"].as_str()?,
                        short(t["url"].as_str()?, 90)
                    ),
                })
            })
            .collect();
        let question = format!("Quel onglet correspond à : {wanted} ?");
        let r = self
            .laya
            .rank(&json!({ "demande": wanted }), &question, &candidates)
            .await;
        let (ranking, engine, warning) = (r.ranking, r.engine, r.warning);
        let best = ranking.first().cloned();
        let tab = best
            .as_ref()
            .and_then(|(id, _)| list.iter().find(|t| id_onglet(t) == *id));
        Ok(json!({
            "best": tab,
            "confidence": best.map(|b| b.1),
            "alternatives": ranking.iter().skip(1).take(3).filter_map(|(id, p)| {
                list.iter().find(|t| id_onglet(t) == *id).map(|t| json!({"tab": t, "score": p}))
            }).collect::<Vec<_>>(),
            "engine": engine,
            "warning": warning,
        }))
    }

    // ── Lecture et recherche d'éléments ─────────────────────────────────────

    async fn snapshot(&self, args: Value) -> Result<Value, String> {
        let res = self.bridge.call("snapshot", args).await?;
        self.remember(&res).await;
        Ok(res)
    }

    async fn remember(&self, snap: &Value) {
        let Some(tab) = snap["tab_id"].as_u64() else {
            return;
        };
        let names = snap["elements"]
            .as_array()
            .map(|els| {
                els.iter()
                    .filter_map(|e| {
                        let name = format!(
                            "{} {}",
                            e["role"].as_str()?,
                            e["name"].as_str().unwrap_or("")
                        );
                        Some((e["ref"].as_u64()?, name))
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.memo.lock().await.insert(
            tab,
            PageMemo {
                title: snap["title"].as_str().unwrap_or("").into(),
                url: snap["url"].as_str().unwrap_or("").into(),
                names,
            },
        );
        *self.last_tab.lock().await = Some(tab);
    }

    async fn find_element(&self, args: &Value) -> Result<Value, String> {
        let goal = text_arg(args, "goal")?;
        let mut params = json!({ "max_elements": 300, "text_chars": 400 });
        copier_cible(args, &mut params);
        let snap = self.snapshot(params).await?;
        let elements = snap["elements"].as_array().cloned().unwrap_or_default();
        let short_list = shortlist_elements(&goal, &elements, SHORTLIST);
        if short_list.is_empty() {
            return Err("Aucun élément interactif visible sur cette page.".into());
        }
        let candidates: Vec<Candidate> = short_list.iter().map(candidate_of).collect();
        let state = json!({ "objectif": goal });
        let instruction = "Quel élément permet d'atteindre l'objectif ?";
        let r = self.laya.rank(&state, instruction, &candidates).await;
        let (ranking, engine, warning) = (r.ranking, r.engine, r.warning);
        let find = |id: &str| {
            short_list
                .iter()
                .find(|e| format!("e{}", e["ref"]) == id)
                .cloned()
        };
        let best = ranking
            .first()
            .and_then(|(id, p)| find(id).map(|e| (e, *p)));
        Ok(json!({
            "tab_id": snap["tab_id"],
            "best": best.as_ref().map(|(e, p)| json!({"element": e, "confidence": p, "confident": *p >= SEUIL_CONFIANCE})),
            "alternatives": ranking.iter().skip(1).take(3).filter_map(|(id, p)| find(id).map(|e| json!({"element": e, "score": p}))).collect::<Vec<_>>(),
            "engine": engine,
            "warning": warning,
        }))
    }

    // ── Actions ─────────────────────────────────────────────────────────────

    async fn click(&self, args: Value) -> Result<Value, String> {
        let target = self.describe_target(&args).await;
        if let Some(stop) = self.gate(&args, "cliquer", &target).await {
            return Ok(stop);
        }
        self.bridge.call("click", args).await
    }

    async fn type_text(&self, args: Value) -> Result<Value, String> {
        if args.get("submit") == Some(&json!(true)) {
            let target = self.describe_target(&args).await;
            if let Some(stop) = self.gate(&args, "saisir puis valider", &target).await {
                return Ok(stop);
            }
        }
        self.bridge.call("type", args).await
    }

    async fn describe_target(&self, args: &Value) -> Value {
        let tab = match args["tab_id"].as_u64() {
            Some(t) => Some(t),
            None => *self.last_tab.lock().await,
        };
        let memo = match tab {
            Some(t) => self.memo.lock().await.get(&t).cloned().unwrap_or_default(),
            None => PageMemo::default(),
        };
        let element = args["ref"]
            .as_u64()
            .and_then(|r| memo.names.get(&r).cloned());
        json!({ "page": memo.title, "adresse": memo.url, "element": element.unwrap_or_default() })
    }

    /// Faut-il arrêter l'action pour demander confirmation ? Rend la réponse à
    /// donner au modèle si oui.
    async fn gate(&self, args: &Value, verb: &str, target: &Value) -> Option<Value> {
        if !self.cfg.confirm_risky || args.get("confirmed") == Some(&json!(true)) {
            return None;
        }
        let element = target["element"].as_str().unwrap_or("");
        let action = format!("{verb} sur « {element} »");
        let use_laya = self.cfg.laya_risk_check && !element.is_empty();
        let risk = assess(&self.laya, &action, &[], use_laya).await;
        risk.risky().then(|| {
            json!({
                "needs_confirmation": true,
                "action": format!("{verb} : {element}"),
                "page": target["page"],
                "risk": { "lexical": risk.lexical, "estimate": risk.laya },
                "next": "Action non exécutée. Décrivez-la à l'utilisateur et demandez son accord ; \
                         s'il accepte, rappelez l'outil avec confirmed=true."
            })
        })
    }

    async fn screenshot(&self, args: Value) -> Result<Value, String> {
        let res = self.bridge.call("screenshot", args).await?;
        let url = res["data_url"].as_str().ok_or("capture vide")?;
        let b64 = url
            .split_once(',')
            .map(|(_, b)| b)
            .ok_or("capture illisible")?;
        let png = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("capture illisible : {e}"))?;
        let path = media_dir().join(format!("browser-{}.png", now_ms()));
        std::fs::create_dir_all(path.parent().unwrap_or(&PathBuf::from(".")))
            .map_err(|e| format!("dossier de capture : {e}"))?;
        std::fs::write(&path, &png).map_err(|e| format!("écriture de la capture : {e}"))?;
        Ok(json!({ "tab_id": res["tab_id"], "path": path, "bytes": png.len() }))
    }

    // ── Jugements ───────────────────────────────────────────────────────────

    async fn goal_reached(&self, args: &Value) -> Result<Value, String> {
        let goal = text_arg(args, "goal")?;
        let mut params = json!({ "max_elements": 20, "text_chars": 1500 });
        copier_cible(args, &mut params);
        let snap = self.snapshot(params).await?;
        let state =
            json!({ "titre": snap["title"], "adresse": snap["url"], "texte": snap["text"] });
        let p = self
            .laya
            .judge(
                &state,
                &format!("L'objectif « {goal} » est-il atteint sur cette page ?"),
            )
            .await
            .map_err(|e| format!("Estimation locale indisponible : {e}"))?;
        Ok(json!({
            "probability": p,
            "reached": p >= 0.5,
            "note": "Estimation rapide sur le texte de la page ; vérifiez par le snapshot si l'enjeu est important."
        }))
    }

    async fn assess_action(&self, args: &Value) -> Result<Value, String> {
        let action = text_arg(args, "action")?;
        let risk = assess(&self.laya, &action, &[], true).await;
        Ok(json!({
            "risky": risk.risky(),
            "lexical": risk.lexical,
            "probability": risk.laya,
            "estimate_error": if risk.laya.is_none() { self.laya.last_error().await } else { None },
        }))
    }
}

// ── Utilitaires ─────────────────────────────────────────────────────────────

/// Le navigateur visé par défaut : celui des réglages, ou en « auto » celui
/// du système (vide s'il est inconnu : le dernier relié).
fn cible_par_defaut(reglage: &str) -> String {
    match Navigateur::depuis(reglage) {
        Some(n) => n.nom.to_string(),
        None => navigateur_par_defaut()
            .map(|n| n.nom.to_string())
            .unwrap_or_default(),
    }
}

/// L'adresse à donner au navigateur qu'on lance : seulement une adresse web
/// (un navigateur refuse ses pages internes passées en argument).
fn url_de_lancement(url: Option<&str>) -> Option<String> {
    url.filter(|u| u.starts_with("http://") || u.starts_with("https://"))
        .map(str::to_string)
}

/// L'onglet et le navigateur visés, recopiés d'un appel vers une commande.
fn copier_cible(args: &Value, params: &mut Value) {
    for cle in ["tab_id", "browser"] {
        if let Some(v) = args.get(cle) {
            params[cle] = v.clone();
        }
    }
}

/// L'identifiant d'un onglet pour Laya : deux navigateurs peuvent avoir le
/// même numéro d'onglet.
fn id_onglet(t: &Value) -> String {
    format!("t{}{}", t["browser"].as_str().unwrap_or(""), t["tab_id"])
}

/// La version de l'extension livrée avec ce morph, telle que la personne la
/// lit (`version_name` du manifeste : « 0.1.0-beta.17 »).
fn version_livree() -> Option<String> {
    dossier_extension("extension").and_then(|d| crate::liaison::nom_de_version_du_dossier(&d))
}

fn nom_affiche(nom: &str) -> String {
    let mut c = nom.chars();
    c.next()
        .map(|p| p.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn noms(navigateurs: &[Value]) -> Vec<String> {
    navigateurs
        .iter()
        .filter_map(|n| n["browser"].as_str().map(str::to_string))
        .collect()
}

fn liste_navigateurs(navigateurs: &[Value]) -> String {
    navigateurs
        .iter()
        .filter_map(|n| n["browser"].as_str().map(nom_affiche))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Les gestes qui restent à la personne, pour le navigateur ouvert.
fn consigne(navigateur: &Navigateur, chemin: Option<&str>, copie: bool) -> String {
    let dossier = chemin.map_or_else(
        || format!("`{}` du morph", navigateur.dossier),
        |c| format!("« {c} »"),
    );
    let colle = if copie {
        " (son chemin est copié : collez-le dans la fenêtre)"
    } else {
        ""
    };
    let gestes = if navigateur.nom == "firefox" {
        format!(
            "Une page de guide est ouverte dans Firefox. 1) Ouvrez {} et cliquez « Charger un module complémentaire temporaire ». \
             2) Choisissez le fichier manifest.json du dossier {dossier}{colle}. \
             3) Dans about:addons → l'extension → Autorisations, accordez l'accès à tous les sites.",
            navigateur.page
        )
    } else {
        format!(
            "Une page de guide est ouverte dans le navigateur. 1) Collez l'adresse {} dans un nouvel onglet et activez le « Mode développeur ». \
             2) Cliquez « Charger l'extension non empaquetée » et choisissez le dossier {dossier}{colle}.",
            navigateur.page
        )
    };
    format!(
        "{gestes} C'est tout : dans les {} secondes, l'extension se relie seule à Locaryn, sans code. \
         Après ce délai, rappelez browser_link.",
        DUREE_APPAIRAGE.as_secs()
    )
}

fn passthrough(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "browser_list_tabs" => "tabs_list",
        "browser_focus_tab" => "tab_focus",
        "browser_open_tab" => "tab_open",
        "browser_close_tab" => "tab_close",
        "browser_navigate" => "navigate",
        "browser_press_key" => "press_key",
        "browser_scroll_page" | "browser_scroll" => "scroll",
        "browser_select_option" => "select_option",
        "browser_hover" => "hover",
        "browser_wait" => "wait",
        "browser_evaluate" => "evaluate",
        "browser_history_search" => "history_search",
        "browser_release" => "release",
        "browser_group_tabs" => "tabs_group",
        "browser_ungroup_tabs" => "tabs_ungroup",
        "browser_list_groups" => "groups_list",
        _ => return None,
    })
}

/// Le nom seul départage mieux que « rôle + nom » (mesuré : 3 bonnes réponses
/// sur 4 contre 1) ; sans nom, le rôle et la destination prennent le relais.
fn candidate_of(e: &Value) -> Candidate {
    let name = e["name"].as_str().unwrap_or("").trim();
    let description = if name.is_empty() {
        let href = e["href"]
            .as_str()
            .map(|h| format!(" → {}", short(h, 60)))
            .unwrap_or_default();
        format!("{}{href}", e["role"].as_str().unwrap_or("élément"))
    } else {
        name.to_string()
    };
    Candidate {
        id: format!("e{}", e["ref"]),
        description,
    }
}

fn shortlist_elements(goal: &str, elements: &[Value], n: usize) -> Vec<Value> {
    shortlist(
        goal,
        elements,
        n,
        |e| {
            format!(
                "{} {} {}",
                e["role"].as_str().unwrap_or(""),
                e["name"].as_str().unwrap_or(""),
                e["href"].as_str().unwrap_or("")
            )
        },
        |e| usize::from(e["in_viewport"] == json!(true)),
    )
}

fn media_dir() -> PathBuf {
    std::env::var("LOCARYN_EXTENSION_MEDIA_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("locaryn-browser"))
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

// ── Déclaration des outils ──────────────────────────────────────────────────

fn tab_prop() -> Value {
    json!({ "type": "integer", "description": "Identifiant d'onglet (browser_list_tabs). Omis : le dernier onglet piloté, sinon l'onglet actif." })
}

fn ref_props() -> Value {
    json!({
        "tab_id": tab_prop(),
        "ref": { "type": "integer", "description": "Numéro d'élément issu de browser_snapshot ou browser_find_element." }
    })
}

fn tab_tools() -> Vec<Value> {
    vec![
        tool("browser_status", "État du pont : navigateurs reliés (plusieurs possibles, chacun nommé : comet, opera, chrome…), version de leur extension, onglets arrêtés par l'utilisateur.", json!({}), &[]),
        tool("browser_link", "Relie le navigateur de la personne à Locaryn : ouvre une page de guide dans le navigateur choisi (chemin du dossier de l'extension copié), puis l'extension se relie seule pendant deux minutes, sans code. À appeler quand browser_status dit que l'extension n'est pas reliée.", json!({ "browser": { "type": "string", "enum": ["auto", "chrome", "edge", "opera", "brave", "vivaldi", "comet", "firefox"], "description": "Le navigateur de la personne ; omis : celui des réglages du morph, « auto » : celui du système." } }), &[]),
        tool("browser_launch", "Lance un navigateur précis (opera, comet, chrome…) s'il n'est pas ouvert, avec une adresse si donnée, attend que son extension se relie, et en fait le navigateur des commandes suivantes. « Ouvre Opera » : appelez cet outil, puis les autres outils agissent dans Opera.", json!({ "browser": { "type": "string", "enum": ["auto", "chrome", "edge", "opera", "brave", "vivaldi", "comet", "firefox"], "description": "Le navigateur nommé par la personne ; « auto » ou omis : son navigateur habituel (le dernier utilisé, sinon celui du système)." }, "url": str_prop("Adresse à ouvrir (facultatif)") }), &[]),
        tool("browser_list_tabs", "Liste tous les onglets ouverts, dans toutes les fenêtres (titre, adresse, actif, épinglé, son en cours). `query` filtre par titre ou adresse.", json!({ "query": str_prop("Filtre sur le titre ou l'adresse"), "window_id": { "type": "integer" } }), &[]),
        tool("browser_find_tab", "Retrouve, parmi les onglets déjà ouverts, celui qui correspond à une description (« le devis chez Leroy Merlin », « ma boîte mail »).", json!({ "description": str_prop("Ce que l'onglet contient ou montre") }), &["description"]),
        tool("browser_focus_tab", "Met un onglet au premier plan (et sa fenêtre).", json!({ "tab_id": tab_prop() }), &["tab_id"]),
        tool("browser_open_tab", "Ouvre une adresse dans un nouvel onglet.", json!({ "url": str_prop("Adresse complète"), "background": { "type": "boolean", "description": "Ne pas prendre le focus" } }), &["url"]),
        tool("browser_close_tab", "Ferme un onglet.", json!({ "tab_id": tab_prop() }), &["tab_id"]),
        tool("browser_navigate", "Charge une adresse dans un onglet, ou revient en arrière, avance, recharge.", json!({ "tab_id": tab_prop(), "url": str_prop("Adresse à charger"), "action": { "type": "string", "enum": ["back", "forward", "reload"] } }), &[]),
        tool("browser_history_search", "Cherche dans l'historique du navigateur (pages visitées, même onglet fermé).", json!({ "query": str_prop("Mots recherchés"), "max": { "type": "integer" } }), &["query"]),
        tool("browser_group_tabs", "Range n'importe quels onglets ouverts (de toutes les fenêtres, ouverts par la personne ou non) dans un groupe nommé et coloré : trier par sujet, garder des résultats de côté. Chromium ; Firefox n'a pas de groupes. Sans `group_id`, crée un groupe ; avec, ajoute les onglets à ce groupe et peut le renommer, le recolorer ou le replier.", json!({ "tab_ids": { "type": "array", "items": { "type": "integer" }, "description": "Les onglets à grouper" }, "group_id": { "type": "integer", "description": "Groupe existant (voir browser_list_groups)" }, "title": str_prop("Nom du groupe"), "color": { "type": "string", "enum": ["grey", "blue", "red", "yellow", "green", "pink", "purple", "cyan", "orange"] }, "collapsed": { "type": "boolean", "description": "Replier le groupe" } }), &["tab_ids"]),
        tool("browser_ungroup_tabs", "Sort des onglets de leur groupe.", json!({ "tab_ids": { "type": "array", "items": { "type": "integer" } } }), &["tab_ids"]),
        tool("browser_list_groups", "Liste les groupes d'onglets : nom, couleur, replié, onglets qu'ils contiennent.", json!({}), &[]),
        tool("browser_release", "Termine le contrôle d'un onglet : le cadre et la pastille disparaissent. À appeler quand la tâche est finie.", json!({ "tab_id": tab_prop() }), &[]),
    ]
}

fn page_tools() -> Vec<Value> {
    vec![
        tool("browser_snapshot", "Lit un onglet : titre, adresse, éléments interactifs numérotés (ref), texte visible, défilement. Base de toute action.", json!({ "tab_id": tab_prop(), "max_elements": { "type": "integer" }, "text_chars": { "type": "integer" } }), &[]),
        tool("browser_find_element", "Trouve l'élément à actionner pour un objectif (« ajouter au panier », « champ de recherche »). Rend le meilleur, sa confiance et des alternatives.", json!({ "tab_id": tab_prop(), "goal": str_prop("Ce qu'on veut faire") }), &["goal"]),
        tool("browser_click_element", "Clique un élément (clic gauche par défaut). Une action jugée irréversible (achat, suppression, envoi) n'est pas exécutée : la réponse demande de confirmer avec l'utilisateur, puis de rappeler avec confirmed=true.", json!({ "tab_id": tab_prop(), "ref": { "type": "integer" }, "x": { "type": "number" }, "y": { "type": "number" }, "button": { "type": "string", "enum": ["left", "right"] }, "double": { "type": "boolean" }, "confirmed": { "type": "boolean", "description": "true seulement après accord explicite de l'utilisateur" } }), &[]),
        tool("browser_type_text", "Saisit du texte dans un champ (remplace le contenu sauf clear=false). submit=true valide ensuite ; cette validation est soumise au même garde-fou que browser_click_element.", json!({ "tab_id": tab_prop(), "ref": { "type": "integer" }, "text": str_prop("Texte à saisir"), "clear": { "type": "boolean" }, "submit": { "type": "boolean" }, "confirmed": { "type": "boolean" } }), &["ref", "text"]),
        tool("browser_press_key", "Envoie une touche à l'élément actif ou à `ref` : Enter, Escape, Tab, ArrowDown, Control+a…", json!({ "tab_id": tab_prop(), "ref": { "type": "integer" }, "key": str_prop("Touche, avec modificateurs séparés par +") }), &["key"]),
        tool("browser_scroll_page", "Fait défiler la page ou un élément.", json!({ "tab_id": tab_prop(), "ref": { "type": "integer" }, "direction": { "type": "string", "enum": ["down", "up", "top", "bottom"] }, "amount": { "type": "integer", "description": "Pixels ; défaut : 80 % de la hauteur visible" } }), &["direction"]),
        tool("browser_select_option", "Choisit une option dans une liste déroulante, par valeur ou texte affiché.", json!({ "tab_id": tab_prop(), "ref": { "type": "integer" }, "value": str_prop("Valeur ou texte de l'option") }), &["ref", "value"]),
        tool("browser_hover", "Survole un élément (menus qui s'ouvrent au survol).", ref_props(), &["ref"]),
        tool("browser_wait", "Attend qu'un texte ou un sélecteur CSS apparaisse, ou une durée en ms.", json!({ "tab_id": tab_prop(), "text": { "type": "string" }, "selector": { "type": "string" }, "ms": { "type": "integer" }, "timeout_ms": { "type": "integer" } }), &[]),
        tool("browser_screenshot", "Capture la partie visible d'un onglet (l'active un instant si besoin, puis rend le focus). Rend le chemin du fichier PNG.", json!({ "tab_id": tab_prop() }), &[]),
        tool("browser_evaluate", "Exécute du JavaScript dans la page et rend le résultat sérialisable. Plusieurs sites l'interdisent (CSP). Dernier recours après snapshot.", json!({ "tab_id": tab_prop(), "code": str_prop("Expression JavaScript") }), &["code"]),
    ]
}

fn judgement_tools() -> Vec<Value> {
    vec![
        tool("browser_goal_reached", "Estime si l'objectif est atteint d'après le texte de la page. Une probabilité, pas une preuve.", json!({ "tab_id": tab_prop(), "goal": str_prop("L'objectif à vérifier") }), &["goal"]),
        tool("browser_assess_action", "Juge, avant d'agir, si une action décrite est irréversible ou lourde de conséquences.", json!({ "action": str_prop("L'action envisagée") }), &["action"]),
    ]
}

/// Outils du panneau du morph : l'hôte les cache au modèle (préfixe `ui_`).
fn panel_tools() -> Vec<Value> {
    vec![
        tool("ui_panel_inbox", "Relève les consignes tapées dans la barre latérale de l'extension (réservé au panneau du morph).", json!({}), &[]),
        tool("ui_panel_reply", "Renvoie une réponse à la barre latérale de l'extension (réservé au panneau du morph).", json!({ "text": str_prop("Réponse à afficher"), "id": str_prop("Identifiant de la consigne"), "browser": str_prop("Le navigateur qui a posé la question"), "error": { "type": "boolean" } }), &["text"]),
        tool("ui_panel_models", "Donne aux barres latérales la liste des modèles de la conversation (réservé au panneau du morph).", json!({ "active": str_prop("Le modèle actif"), "installed": { "type": "array", "items": { "type": "string" } } }), &[]),
        tool("ui_browsers", "Liste les navigateurs reliés et la version de leur extension (réservé au panneau du morph).", json!({}), &[]),
    ]
}

/// Les outils qui agissent dans un navigateur : tous, sauf ceux qui en
/// relient un, en donnent l'état ou jugent une phrase.
fn vise_un_navigateur(nom: &str) -> bool {
    nom.starts_with("browser_")
        && !matches!(
            nom,
            "browser_status" | "browser_link" | "browser_launch" | "browser_assess_action"
        )
}

fn browser_prop() -> Value {
    json!({
        "type": "string",
        "description": "Le navigateur visé quand plusieurs sont reliés (comet, opera, chrome… : voir browser_status). Omis : celui des réglages, sinon le dernier relié ; browser_list_tabs sans navigateur liste les onglets de tous."
    })
}

pub fn tools_list() -> Value {
    let mut all = tab_tools();
    all.extend(page_tools());
    all.extend(judgement_tools());
    for t in &mut all {
        let nom = t["name"].as_str().unwrap_or("").to_string();
        if vise_un_navigateur(&nom) {
            if let Some(props) = t["inputSchema"]["properties"].as_object_mut() {
                props.insert("browser".into(), browser_prop());
            }
        }
    }
    all.extend(panel_tools());
    json!({ "tools": all })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chaque_outil_declare_a_une_route() {
        let tools = tools_list();
        let speciaux = [
            "browser_status",
            "browser_link",
            "browser_launch",
            "browser_find_tab",
            "browser_snapshot",
            "browser_find_element",
            "browser_click_element",
            "browser_type_text",
            "browser_screenshot",
            "browser_goal_reached",
            "browser_assess_action",
            "ui_panel_inbox",
            "ui_panel_reply",
            "ui_panel_models",
            "ui_browsers",
        ];
        for t in tools["tools"].as_array().unwrap() {
            let n = t["name"].as_str().unwrap();
            assert!(
                speciaux.contains(&n) || passthrough(n).is_some(),
                "{n} sans route"
            );
        }
    }

    #[test]
    fn la_preselection_remonte_l_element_qui_partage_les_mots() {
        let els = vec![
            json!({"ref":1,"role":"link","name":"Accueil","in_viewport":true}),
            json!({"ref":2,"role":"button","name":"Ajouter au panier","in_viewport":false}),
        ];
        assert_eq!(shortlist_elements("ajouter panier", &els, 1)[0]["ref"], 2);
    }
}
