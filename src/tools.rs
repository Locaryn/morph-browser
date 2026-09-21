//! Outils MCP du navigateur.
//!
//! Le modèle de conversation planifie ; Laya tranche vite les questions
//! fermées : quel élément, quel onglet, cette action est-elle risquée,
//! l'objectif est-il atteint. Sans Laya, chaque outil retombe sur un repli
//! lexical et le dit dans sa réponse : rien n'est simulé en silence.

use crate::bridge::{load_or_create_token, Bridge};
use crate::config::BrowserConfig;
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
}

impl Browser {
    pub async fn new() -> Result<Self, String> {
        let cfg: BrowserConfig = read_config();
        let token = load_or_create_token()?;
        Ok(Self {
            bridge: Bridge::start(cfg.port, token).await,
            laya: Arc::new(Laya::new()),
            warmed: AtomicBool::new(false),
            cfg,
            memo: Mutex::new(HashMap::new()),
            last_tab: Mutex::new(None),
        })
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        self.warm_up(name);
        match name {
            "browser_status" => self.status().await,
            "browser_find_tab" => self.find_tab(&args).await,
            "browser_snapshot" => self.snapshot(args).await,
            "browser_find_element" => self.find_element(&args).await,
            "browser_click" => self.click(args).await,
            "browser_type" => self.type_text(args).await,
            "browser_screenshot" => self.screenshot(args).await,
            "browser_goal_reached" => self.goal_reached(&args).await,
            "browser_assess_action" => self.assess_action(&args).await,
            other => match passthrough(other) {
                Some(method) => self.bridge.call(method, args).await,
                None => Err(format!("Outil navigateur inconnu : {other}")),
            },
        }
    }

    /// Charge Laya en arrière-plan dès le premier vrai usage : le chargement
    /// dure des dizaines de secondes, autant qu'il se fasse pendant que le
    /// modèle de conversation réfléchit.
    fn warm_up(&self, tool: &str) {
        if tool == "browser_status" || self.warmed.swap(true, Ordering::Relaxed) {
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
        let connected = self.bridge.is_connected().await;
        let mut out = json!({
            "extension_connected": connected,
            "browser": self.bridge.hello().await.and_then(|h| h.get("browser").cloned()),
            "port": self.bridge.port,
            "laya": {
                "loaded": self.laya.is_running().await,
                "last_error": self.laya.last_error().await,
                "note": "Laya se charge au premier usage (quelques secondes à quelques dizaines)."
            },
            "tabs_stopped_by_user": self.bridge.blocked_tabs().await,
        });
        if let Some(e) = self.bridge.bind_error().await {
            out["bridge_error"] = json!(e);
        }
        if !connected {
            out["pairing_code"] = json!(self.bridge.token());
            out["setup"] = json!(
                "1) Dans le navigateur, ouvrez chrome://extensions, activez le mode développeur, \
                 « Charger l'extension non empaquetée » et choisissez le dossier `extension` du morph. \
                 2) Cliquez l'icône Locaryn, collez le code d'appairage, Enregistrer."
            );
        }
        Ok(out)
    }

    // ── Onglets ─────────────────────────────────────────────────────────────

    async fn find_tab(&self, args: &Value) -> Result<Value, String> {
        let wanted = text_arg(args, "description")?;
        let tabs = self.bridge.call("tabs_list", json!({})).await?;
        let list = tabs["tabs"].as_array().cloned().unwrap_or_default();
        if list.is_empty() {
            return Err("Aucun onglet ouvert.".into());
        }
        let candidates: Vec<Candidate> = list
            .iter()
            .filter_map(|t| {
                let id = t["tab_id"].as_u64()?;
                Some(Candidate {
                    id: format!("t{id}"),
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
            .and_then(|(id, _)| list.iter().find(|t| format!("t{}", t["tab_id"]) == *id));
        Ok(json!({
            "best": tab,
            "confidence": best.map(|b| b.1),
            "alternatives": ranking.iter().skip(1).take(3).filter_map(|(id, p)| {
                list.iter().find(|t| format!("t{}", t["tab_id"]) == *id).map(|t| json!({"tab": t, "score": p}))
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
        if let Some(t) = args.get("tab_id") {
            params["tab_id"] = t.clone();
        }
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
                "risk": { "lexical": risk.lexical, "laya": risk.laya },
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
        if let Some(t) = args.get("tab_id") {
            params["tab_id"] = t.clone();
        }
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
            .map_err(|e| format!("Laya indisponible, impossible d'estimer : {e}"))?;
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
            "laya_probability": risk.laya,
            "laya_error": if risk.laya.is_none() { self.laya.last_error().await } else { None },
        }))
    }
}

// ── Utilitaires ─────────────────────────────────────────────────────────────

fn passthrough(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "browser_list_tabs" => "tabs_list",
        "browser_focus_tab" => "tab_focus",
        "browser_open_tab" => "tab_open",
        "browser_close_tab" => "tab_close",
        "browser_navigate" => "navigate",
        "browser_press_key" => "press_key",
        "browser_scroll" => "scroll",
        "browser_select_option" => "select_option",
        "browser_hover" => "hover",
        "browser_wait" => "wait",
        "browser_evaluate" => "evaluate",
        "browser_history_search" => "history_search",
        "browser_release" => "release",
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
        tool("browser_status", "État du pont avec le navigateur : extension connectée, Laya chargé, onglets arrêtés par l'utilisateur. Si l'extension n'est pas connectée, donne le code d'appairage et les étapes d'installation.", json!({}), &[]),
        tool("browser_list_tabs", "Liste tous les onglets ouverts, dans toutes les fenêtres (titre, adresse, actif, épinglé, son en cours). `query` filtre par titre ou adresse.", json!({ "query": str_prop("Filtre sur le titre ou l'adresse"), "window_id": { "type": "integer" } }), &[]),
        tool("browser_find_tab", "Retrouve, parmi les onglets déjà ouverts, celui qui correspond à une description (« le devis chez Leroy Merlin », « ma boîte mail »). Décidé par Laya.", json!({ "description": str_prop("Ce que l'onglet contient ou montre") }), &["description"]),
        tool("browser_focus_tab", "Met un onglet au premier plan (et sa fenêtre).", json!({ "tab_id": tab_prop() }), &["tab_id"]),
        tool("browser_open_tab", "Ouvre une adresse dans un nouvel onglet.", json!({ "url": str_prop("Adresse complète"), "background": { "type": "boolean", "description": "Ne pas prendre le focus" } }), &["url"]),
        tool("browser_close_tab", "Ferme un onglet.", json!({ "tab_id": tab_prop() }), &["tab_id"]),
        tool("browser_navigate", "Charge une adresse dans un onglet, ou revient en arrière, avance, recharge.", json!({ "tab_id": tab_prop(), "url": str_prop("Adresse à charger"), "action": { "type": "string", "enum": ["back", "forward", "reload"] } }), &[]),
        tool("browser_history_search", "Cherche dans l'historique du navigateur (pages visitées, même onglet fermé).", json!({ "query": str_prop("Mots recherchés"), "max": { "type": "integer" } }), &["query"]),
        tool("browser_release", "Termine le contrôle d'un onglet : le cadre et la pastille disparaissent. À appeler quand la tâche est finie.", json!({ "tab_id": tab_prop() }), &[]),
    ]
}

fn page_tools() -> Vec<Value> {
    vec![
        tool("browser_snapshot", "Lit un onglet : titre, adresse, éléments interactifs numérotés (ref), texte visible, défilement. Base de toute action.", json!({ "tab_id": tab_prop(), "max_elements": { "type": "integer" }, "text_chars": { "type": "integer" } }), &[]),
        tool("browser_find_element", "Trouve l'élément à actionner pour un objectif (« ajouter au panier », « champ de recherche »). Laya départage les candidats de la page ; rend le meilleur, sa confiance et des alternatives.", json!({ "tab_id": tab_prop(), "goal": str_prop("Ce qu'on veut faire") }), &["goal"]),
        tool("browser_click", "Clique un élément (clic gauche par défaut). Une action jugée irréversible (achat, suppression, envoi) n'est pas exécutée : la réponse demande de confirmer avec l'utilisateur, puis de rappeler avec confirmed=true.", json!({ "tab_id": tab_prop(), "ref": { "type": "integer" }, "x": { "type": "number" }, "y": { "type": "number" }, "button": { "type": "string", "enum": ["left", "right"] }, "double": { "type": "boolean" }, "confirmed": { "type": "boolean", "description": "true seulement après accord explicite de l'utilisateur" } }), &[]),
        tool("browser_type", "Saisit du texte dans un champ (remplace le contenu sauf clear=false). submit=true valide ensuite ; cette validation est soumise au même garde-fou que browser_click.", json!({ "tab_id": tab_prop(), "ref": { "type": "integer" }, "text": str_prop("Texte à saisir"), "clear": { "type": "boolean" }, "submit": { "type": "boolean" }, "confirmed": { "type": "boolean" } }), &["ref", "text"]),
        tool("browser_press_key", "Envoie une touche à l'élément actif ou à `ref` : Enter, Escape, Tab, ArrowDown, Control+a…", json!({ "tab_id": tab_prop(), "ref": { "type": "integer" }, "key": str_prop("Touche, avec modificateurs séparés par +") }), &["key"]),
        tool("browser_scroll", "Fait défiler la page ou un élément.", json!({ "tab_id": tab_prop(), "ref": { "type": "integer" }, "direction": { "type": "string", "enum": ["down", "up", "top", "bottom"] }, "amount": { "type": "integer", "description": "Pixels ; défaut : 80 % de la hauteur visible" } }), &["direction"]),
        tool("browser_select_option", "Choisit une option dans une liste déroulante, par valeur ou texte affiché.", json!({ "tab_id": tab_prop(), "ref": { "type": "integer" }, "value": str_prop("Valeur ou texte de l'option") }), &["ref", "value"]),
        tool("browser_hover", "Survole un élément (menus qui s'ouvrent au survol).", ref_props(), &["ref"]),
        tool("browser_wait", "Attend qu'un texte ou un sélecteur CSS apparaisse, ou une durée en ms.", json!({ "tab_id": tab_prop(), "text": { "type": "string" }, "selector": { "type": "string" }, "ms": { "type": "integer" }, "timeout_ms": { "type": "integer" } }), &[]),
        tool("browser_screenshot", "Capture la partie visible d'un onglet (l'active un instant si besoin, puis rend le focus). Rend le chemin du fichier PNG.", json!({ "tab_id": tab_prop() }), &[]),
        tool("browser_evaluate", "Exécute du JavaScript dans la page et rend le résultat sérialisable. Plusieurs sites l'interdisent (CSP). Dernier recours après snapshot.", json!({ "tab_id": tab_prop(), "code": str_prop("Expression JavaScript") }), &["code"]),
    ]
}

fn judgement_tools() -> Vec<Value> {
    vec![
        tool("browser_goal_reached", "Estime, par Laya, si l'objectif est atteint d'après le texte de la page. Une probabilité, pas une preuve.", json!({ "tab_id": tab_prop(), "goal": str_prop("L'objectif à vérifier") }), &["goal"]),
        tool("browser_assess_action", "Juge, avant d'agir, si une action décrite est irréversible ou lourde de conséquences.", json!({ "action": str_prop("L'action envisagée") }), &["action"]),
    ]
}

pub fn tools_list() -> Value {
    let mut all = tab_tools();
    all.extend(page_tools());
    all.extend(judgement_tools());
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
            "browser_find_tab",
            "browser_snapshot",
            "browser_find_element",
            "browser_click",
            "browser_type",
            "browser_screenshot",
            "browser_goal_reached",
            "browser_assess_action",
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
