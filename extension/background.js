// Service worker : relie le serveur MCP local (WebSocket) aux onglets du navigateur.
"use strict";
const api = typeof browser !== "undefined" ? browser : chrome; // Firefox expose `browser`, les navigateurs Chromium `chrome`

const DEFAULT_PORT = 17421;
const KEEPALIVE_MS = 20000;
const NAV_TIMEOUT_MS = 20000;

const state = {
  ws: null,
  connected: false,
  token: "",
  port: DEFAULT_PORT,
  lastTabId: null,
  live: new Set(), // onglets dont le cadre est actuellement affiché
  blocked: new Set(),
  reconnectTimer: null,
};

// ── Connexion au serveur ────────────────────────────────────────────────────

async function loadConfig() {
  const c = await api.storage.local.get(["token", "port"]);
  state.token = (c.token || "").trim();
  state.port = Number(c.port) || DEFAULT_PORT;
}

const VERT = "#3e8c66";
const ROUGE = "#c2453d";

function setBadge(text, color = VERT) {
  api.action.setBadgeText({ text });
  api.action.setBadgeBackgroundColor({ color });
}

/** Point rouge : l'extension n'est pas reliée à Locaryn. */
function badgeDeconnecte() {
  setBadge("●", ROUGE);
}

function connect() {
  clearTimeout(state.reconnectTimer);
  // Sans code, on se présente quand même : quand la personne clique « Relier »
  // dans Locaryn, le pont envoie le code et l'extension le garde.
  if (!state.connected) badgeDeconnecte();
  if (state.ws && state.ws.readyState <= WebSocket.OPEN) return;
  let ws;
  try {
    ws = new WebSocket(`ws://127.0.0.1:${state.port}`);
  } catch (e) {
    console.warn("Locaryn : connexion impossible", e);
    return scheduleReconnect();
  }
  state.ws = ws;
  ws.onopen = () => {
    ws.send(JSON.stringify({
      type: "hello",
      token: state.token,
      browser: navigator.userAgent,
      version: api.runtime.getManifest().version,
    }));
  };
  ws.onmessage = (ev) => onServerMessage(ws, ev.data);
  ws.onclose = () => {
    state.connected = false;
    badgeDeconnecte();
    scheduleReconnect();
  };
  ws.onerror = () => ws.close();
}

function scheduleReconnect() {
  clearTimeout(state.reconnectTimer);
  state.reconnectTimer = setTimeout(connect, 3000);
}

async function onServerMessage(ws, raw) {
  let msg;
  try {
    msg = JSON.parse(raw);
  } catch (e) {
    console.warn("Locaryn : message illisible", e);
    return;
  }
  if (msg.type === "paired" && typeof msg.token === "string" && msg.token) {
    // Relié depuis Locaryn : le code sert aux connexions suivantes.
    state.token = msg.token;
    await api.storage.local.set({ token: msg.token });
    state.connected = true;
    setBadge("●");
    return;
  }
  if (msg.type === "welcome") {
    state.connected = true;
    setBadge("●");
    return;
  }
  if (msg.type === "refused") {
    // Sans code, attendre que la personne relie l'extension n'est pas une
    // erreur : on réessaie en silence.
    if (state.token) {
      console.warn("Locaryn : appairage refusé —", msg.reason);
      setBadge("!", ROUGE);
    }
    return;
  }
  if (msg.type === "reload") {
    // Le morph apporte une version plus récente : relire son dossier.
    console.info("Locaryn : rechargement vers", msg.version);
    api.runtime.reload();
    return;
  }
  if (msg.type === "panel_ack") {
    await panelMarkDelivered(Array.isArray(msg.ids) ? msg.ids : []);
    return;
  }
  if (msg.type === "panel_reply") {
    await panelAddReply(msg);
    return;
  }
  if (msg.id === undefined || !msg.method) return;
  try {
    const result = await dispatch(msg.method, msg.params || {});
    ws.send(JSON.stringify({ id: msg.id, result }));
  } catch (e) {
    ws.send(JSON.stringify({ id: msg.id, error: String(e && e.message ? e.message : e) }));
  }
}

function notifyServer(payload) {
  if (state.ws && state.ws.readyState === WebSocket.OPEN) {
    state.ws.send(JSON.stringify({ type: "event", ...payload }));
  }
}

api.alarms.create("keepalive", { periodInMinutes: 0.5 });
api.alarms.onAlarm.addListener(() => connect());
setInterval(() => {
  if (state.ws && state.ws.readyState === WebSocket.OPEN) state.ws.send(JSON.stringify({ type: "ping" }));
}, KEEPALIVE_MS);

api.storage.onChanged.addListener(async (changes, area) => {
  // Seuls le code et le port touchent la connexion ; la conversation de la
  // barre latérale vit dans `session` et ne doit pas la couper.
  if (area !== "local" || !("token" in changes || "port" in changes)) return;
  await loadConfig();
  if (state.ws) state.ws.close();
  connect();
});

loadConfig().then(connect);

// ── Onglets ─────────────────────────────────────────────────────────────────

function describeTab(t) {
  return {
    tab_id: t.id,
    window_id: t.windowId,
    index: t.index,
    title: t.title || "",
    url: t.url || t.pendingUrl || "",
    active: t.active,
    pinned: t.pinned,
    audible: Boolean(t.audible),
    status: t.status,
    group_id: t.groupId,
    controlled: state.lastTabId === t.id && !state.blocked.has(t.id),
    blocked: state.blocked.has(t.id),
  };
}

async function resolveTab(p) {
  if (p.tab_id !== undefined && p.tab_id !== null) return api.tabs.get(Number(p.tab_id));
  if (state.lastTabId !== null) {
    try {
      return await api.tabs.get(state.lastTabId);
    } catch (e) {
      state.lastTabId = null; // l'onglet a été fermé
    }
  }
  const [t] = await api.tabs.query({ active: true, lastFocusedWindow: true });
  if (!t) throw new Error("Aucun onglet actif.");
  return t;
}

function refuseIfBlocked(tabId) {
  if (state.blocked.has(tabId)) {
    throw new Error("L'utilisateur a arrêté le contrôle de cet onglet (bouton Stop). Demandez-lui de le réautoriser depuis l'icône de l'extension.");
  }
}

async function ensureContent(tabId) {
  try {
    await api.scripting.executeScript({ target: { tabId }, files: ["content.js"] });
  } catch (e) {
    throw new Error(`Cet onglet ne peut pas être piloté (page protégée du navigateur ?) : ${e.message}`);
  }
}

async function sendToTab(tabId, message) {
  const r = await api.tabs.sendMessage(tabId, message);
  if (r && r.error) throw new Error(r.error);
  return r;
}

async function markControlled(tab, label) {
  state.lastTabId = tab.id;
  state.live.add(tab.id);
  api.action.setBadgeText({ tabId: tab.id, text: "◉" });
  await sendToTab(tab.id, { type: "overlay", on: true, label });
}

/** Exécute une commande dans la page, avec cadre animé et pastille. */
async function pageCommand(p, cmd, label) {
  const tab = await resolveTab(p);
  refuseIfBlocked(tab.id);
  await ensureContent(tab.id);
  await markControlled(tab, label);
  try {
    const r = await sendToTab(tab.id, { type: "cmd", cmd, args: p });
    return { tab_id: tab.id, ...r };
  } catch (e) {
    // Un clic qui change de page détruit le script avant qu'il ne réponde.
    if (/message port closed|Receiving end does not exist/i.test(e.message)) {
      return { tab_id: tab.id, navigated: true };
    }
    throw e;
  }
}

function waitComplete(tabId) {
  return new Promise((resolve) => {
    const done = () => {
      api.tabs.onUpdated.removeListener(on);
      clearTimeout(timer);
      resolve();
    };
    const on = (id, info) => {
      if (id === tabId && info.status === "complete") done();
    };
    const timer = setTimeout(done, NAV_TIMEOUT_MS);
    api.tabs.onUpdated.addListener(on);
  });
}

api.tabs.onUpdated.addListener(async (tabId, info) => {
  // Le cadre survit à une navigation tant que l'onglet reste contrôlé.
  if (info.status !== "complete" || !state.live.has(tabId) || state.blocked.has(tabId)) return;
  try {
    await ensureContent(tabId);
    await sendToTab(tabId, { type: "overlay", on: true, label: "Navigation" });
  } catch (e) {
    console.debug("Locaryn : cadre non rétabli", e.message);
  }
});

api.tabs.onRemoved.addListener((tabId) => {
  state.blocked.delete(tabId);
  state.live.delete(tabId);
  if (state.lastTabId === tabId) state.lastTabId = null;
});

async function screenshot(p) {
  const tab = await resolveTab(p);
  refuseIfBlocked(tab.id);
  const [previous] = await api.tabs.query({ active: true, windowId: tab.windowId });
  const changed = previous && previous.id !== tab.id;
  if (changed) {
    await api.tabs.update(tab.id, { active: true });
    await new Promise((r) => setTimeout(r, 300));
  }
  try {
    await ensureContent(tab.id);
    await sendToTab(tab.id, { type: "overlay_peek", hidden: true });
    const data_url = await api.tabs.captureVisibleTab(tab.windowId, { format: "png" });
    await sendToTab(tab.id, { type: "overlay_peek", hidden: false });
    return { tab_id: tab.id, data_url };
  } finally {
    if (changed) await api.tabs.update(previous.id, { active: true });
  }
}

async function evaluate(p) {
  const tab = await resolveTab(p);
  refuseIfBlocked(tab.id);
  await ensureContent(tab.id);
  await markControlled(tab, "Script");
  const [res] = await api.scripting.executeScript({
    target: { tabId: tab.id },
    world: "MAIN",
    args: [String(p.code || "")],
    func: async (code) => {
      try {
        const v = await (0, eval)(code);
        return { ok: true, value: JSON.parse(JSON.stringify(v === undefined ? null : v)) };
      } catch (e) {
        return { ok: false, error: String(e) };
      }
    },
  });
  const r = res && res.result;
  if (!r || !r.ok) throw new Error(`Script refusé ou en erreur : ${r ? r.error : "aucun résultat"}`);
  return { tab_id: tab.id, value: r.value };
}

// ── Méthodes exposées au serveur ────────────────────────────────────────────

// ── Groupes d'onglets (Chromium) ────────────────────────────────────────────

function groupesDisponibles() {
  if (!api.tabGroups || !api.tabs.group) {
    throw new Error("Ce navigateur n'a pas de groupes d'onglets (Firefox).");
  }
}

function idsOnglets(p) {
  const ids = (Array.isArray(p.tab_ids) ? p.tab_ids : []).map(Number).filter(Number.isInteger);
  if (ids.length === 0) throw new Error("Donnez au moins un onglet dans `tab_ids`.");
  return ids;
}

async function decrireGroupe(g) {
  const onglets = await api.tabs.query({ groupId: g.id });
  return {
    group_id: g.id,
    window_id: g.windowId,
    title: g.title || "",
    color: g.color,
    collapsed: g.collapsed,
    tab_ids: onglets.map((t) => t.id),
  };
}

async function tabs_group(p) {
  groupesDisponibles();
  const tabIds = idsOnglets(p);
  const options = { tabIds };
  if (p.group_id !== undefined && p.group_id !== null) options.groupId = Number(p.group_id);
  const gid = await api.tabs.group(options);
  const maj = {};
  if (typeof p.title === "string") maj.title = p.title;
  if (typeof p.color === "string") maj.color = p.color;
  if (typeof p.collapsed === "boolean") maj.collapsed = p.collapsed;
  const g = Object.keys(maj).length ? await api.tabGroups.update(gid, maj) : await api.tabGroups.get(gid);
  return { group: await decrireGroupe(g) };
}

async function tabs_ungroup(p) {
  groupesDisponibles();
  const tabIds = idsOnglets(p);
  await api.tabs.ungroup(tabIds);
  return { ungrouped: tabIds };
}

async function groups_list() {
  groupesDisponibles();
  const groupes = await api.tabGroups.query({});
  return { groups: await Promise.all(groupes.map(decrireGroupe)) };
}

const METHODS = {
  async tabs_list(p) {
    const tabs = await api.tabs.query(p.window_id ? { windowId: Number(p.window_id) } : {});
    const q = (p.query || "").toLowerCase();
    return {
      tabs: tabs
        .map(describeTab)
        .filter((t) => !q || t.title.toLowerCase().includes(q) || t.url.toLowerCase().includes(q)),
    };
  },
  async tab_focus(p) {
    const t = await api.tabs.update(Number(p.tab_id), { active: true });
    await api.windows.update(t.windowId, { focused: true });
    return { tab: describeTab(t) };
  },
  async tab_open(p) {
    const t = await api.tabs.create({ url: p.url, active: !p.background });
    await waitComplete(t.id);
    const fresh = await api.tabs.get(t.id);
    if (!p.no_control) {
      try {
        await ensureContent(fresh.id);
        await markControlled(fresh, "Ouverture");
      } catch (e) {
        console.debug("Locaryn : onglet non pilotable", e.message);
      }
    }
    return { tab: describeTab(fresh) };
  },
  async tab_close(p) {
    await api.tabs.remove(Number(p.tab_id));
    return { closed: Number(p.tab_id) };
  },
  async navigate(p) {
    const tab = await resolveTab(p);
    refuseIfBlocked(tab.id);
    if (p.action === "back") await api.tabs.goBack(tab.id);
    else if (p.action === "forward") await api.tabs.goForward(tab.id);
    else if (p.action === "reload") await api.tabs.reload(tab.id);
    else if (p.url) await api.tabs.update(tab.id, { url: p.url });
    else throw new Error("navigate : donnez `url` ou `action` (back, forward, reload).");
    state.lastTabId = tab.id;
    await waitComplete(tab.id);
    return { tab: describeTab(await api.tabs.get(tab.id)) };
  },
  async history_search(p) {
    const items = await api.history.search({ text: p.query || "", maxResults: Number(p.max) || 20, startTime: 0 });
    return { items: items.map((i) => ({ title: i.title, url: i.url, visits: i.visitCount, last_visit: i.lastVisitTime })) };
  },
  async release(p) {
    const id = p.tab_id !== undefined ? Number(p.tab_id) : state.lastTabId;
    if (id === null || id === undefined) return { released: null };
    try {
      await sendToTab(id, { type: "overlay", on: false });
    } catch (e) {
      console.debug("Locaryn : rien à libérer", e.message);
    }
    api.action.setBadgeText({ tabId: id, text: "" });
    state.live.delete(id);
    return { released: id };
  },
  screenshot,
  evaluate,
  tabs_group,
  tabs_ungroup,
  groups_list,
  snapshot: (p) => pageCommand(p, "snapshot", "Lecture de la page"),
  click: (p) => pageCommand(p, "click", p.button === "right" ? "Clic droit" : "Clic"),
  type: (p) => pageCommand(p, "type", "Saisie"),
  press_key: (p) => pageCommand(p, "press_key", "Touche"),
  scroll: (p) => pageCommand(p, "scroll", "Défilement"),
  select_option: (p) => pageCommand(p, "select_option", "Sélection"),
  hover: (p) => pageCommand(p, "hover", "Survol"),
  wait: (p) => pageCommand(p, "wait", "Attente"),
};

async function dispatch(method, params) {
  const fn = METHODS[method];
  if (!fn) throw new Error(`Méthode inconnue côté extension : ${method}`);
  return fn(params);
}

// ── Barre latérale : la conversation avec Locaryn ───────────────────────────
//
// La conversation vit dans `storage.session` : la barre latérale la relit en
// s'ouvrant, et le service worker peut s'endormir sans la perdre. Une consigne
// part au serveur MCP du morph par le pont ; Locaryn la relève, la poste dans
// la conversation ouverte, et renvoie la réponse par le même chemin.

const PANEL_KEY = "panel";
const DRAFT_KEY = "panelDraft";
const PANEL_MAX = 60;

function sessionStore() {
  return api.storage.session || api.storage.local;
}

async function panelLoad() {
  const r = await sessionStore().get(PANEL_KEY);
  const p = r[PANEL_KEY];
  return p && Array.isArray(p.messages) ? p : { messages: [] };
}

async function panelSave(p) {
  p.messages = p.messages.slice(-PANEL_MAX);
  await sessionStore().set({ [PANEL_KEY]: p });
}

function newId() {
  return typeof crypto !== "undefined" && crypto.randomUUID
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

/** L'onglet que la personne regarde, pour donner le contexte de la consigne. */
async function activePage() {
  try {
    const [t] = await api.tabs.query({ active: true, lastFocusedWindow: true });
    return t ? { tab_id: t.id, url: t.url || "", title: t.title || "" } : null;
  } catch (e) {
    console.warn("Locaryn : onglet actif illisible", e);
    return null;
  }
}

async function panelSend(text, attachment) {
  const clean = String(text || "").trim();
  if (!clean && !attachment) throw new Error("Message vide.");
  if (!state.ws || state.ws.readyState !== WebSocket.OPEN || !state.connected) {
    throw new Error("Locaryn n'est pas relié : ouvrez Locaryn, puis le morph navigateur › Relier au navigateur.");
  }
  const message = {
    id: newId(),
    text: clean,
    attachment: attachment || null,
    page: (attachment && attachment.page) || (await activePage()),
    at: Date.now(),
  };
  const p = await panelLoad();
  p.messages.push({ ...message, role: "user", status: "sent" });
  await panelSave(p);
  await sessionStore().remove(DRAFT_KEY);
  notifyServer({ event: "panel_message", message });
  return { id: message.id };
}

async function panelMarkDelivered(ids) {
  const p = await panelLoad();
  let changed = false;
  for (const m of p.messages) {
    if (m.role === "user" && ids.includes(m.id) && m.status === "sent") {
      m.status = "delivered";
      changed = true;
    }
  }
  if (changed) await panelSave(p);
}

async function panelAddReply(msg) {
  const p = await panelLoad();
  for (const m of p.messages) {
    if (m.role === "user" && m.id === msg.id) m.status = "answered";
  }
  p.messages.push({
    id: newId(),
    role: "assistant",
    text: String(msg.text || ""),
    error: Boolean(msg.error),
    at: Date.now(),
  });
  await panelSave(p);
}

/** Ouvre la barre latérale. À appeler dans le geste de la personne, avant
 *  toute attente : Chrome refuse sinon (« may only be called in response to a
 *  user gesture »). */
function openPanel(tab) {
  if (api.sidePanel && api.sidePanel.open && tab) {
    return api.sidePanel.open({ windowId: tab.windowId });
  }
  if (api.sidebarAction && api.sidebarAction.open) return api.sidebarAction.open();
  return Promise.reject(new Error("Pas de barre latérale dans ce navigateur."));
}

// L'icône ouvre la barre latérale (Chromium). Firefox : clic sur l'icône.
if (api.sidePanel && api.sidePanel.setPanelBehavior) {
  api.sidePanel
    .setPanelBehavior({ openPanelOnActionClick: true })
    .catch((e) => console.warn("Locaryn : comportement de l'icône", e));
} else if (api.sidebarAction) {
  api.action.onClicked.addListener(() => {
    api.sidebarAction.toggle().catch((e) => console.warn("Locaryn : barre latérale", e));
  });
}

// ── Clic droit : « Demander à Locaryn » ─────────────────────────────────────

const MENU_ID = "locaryn-ask";

function createMenu() {
  if (!api.contextMenus) return;
  api.contextMenus.removeAll(() => {
    api.contextMenus.create(
      {
        id: MENU_ID,
        title: "Demander à Locaryn",
        contexts: ["selection", "page", "link", "image"],
      },
      () => {
        if (api.runtime.lastError) console.warn("Locaryn : menu", api.runtime.lastError.message);
      },
    );
  });
}

api.runtime.onInstalled.addListener(createMenu);
if (api.runtime.onStartup) api.runtime.onStartup.addListener(createMenu);

/** La sélection avec ses retours à la ligne : `selectionText` les écrase. */
async function fullSelection(tab, fallback) {
  try {
    const [r] = await api.scripting.executeScript({
      target: { tabId: tab.id, allFrames: false },
      func: () => String(window.getSelection ? window.getSelection() : ""),
    });
    const text = r && typeof r.result === "string" ? r.result.trim() : "";
    return text || fallback;
  } catch (e) {
    console.debug("Locaryn : sélection lue par le menu seulement", e.message);
    return fallback;
  }
}

function onMenuClick(info, tab) {
  if (info.menuItemId !== MENU_ID) return;
  // D'abord ouvrir : le geste de la personne ne survit pas à une attente.
  openPanel(tab).catch((e) => console.warn("Locaryn : barre latérale", e));
  const page = tab ? { tab_id: tab.id, url: tab.url || "", title: tab.title || "" } : null;
  (async () => {
    let attachment;
    if (info.selectionText) {
      const text = tab ? await fullSelection(tab, info.selectionText) : info.selectionText;
      attachment = { kind: "selection", text: text.slice(0, 20000), page };
    } else if (info.linkUrl) {
      attachment = { kind: "link", url: info.linkUrl, text: info.linkText || "", page };
    } else if (info.srcUrl && info.mediaType === "image") {
      attachment = { kind: "image", url: info.srcUrl, page };
    } else {
      attachment = { kind: "page", page };
    }
    await sessionStore().set({ [DRAFT_KEY]: attachment });
  })().catch((e) => console.warn("Locaryn : pièce jointe du clic droit", e));
}

// Sans la permission (manifeste encore ancien, pas rechargé), pas de menu :
// le reste du service worker doit tourner quand même.
if (api.contextMenus) api.contextMenus.onClicked.addListener(onMenuClick);

// ── Messages venant des pages et de la fenêtre de l'extension ───────────────

/** Une réponse asynchrone à `runtime.onMessage`, erreurs comprises. */
function answerAsync(promise, reply) {
  promise
    .then((r) => reply(r))
    .catch((e) => reply({ error: String(e && e.message ? e.message : e) }));
  return true;
}

api.runtime.onMessage.addListener((msg, sender, reply) => {
  if (msg.type === "panel_send") return answerAsync(panelSend(msg.text, msg.attachment), reply);
  if (msg.type === "panel_clear") {
    return answerAsync(panelSave({ messages: [] }).then(() => ({ ok: true })), reply);
  }
  if (msg.type === "user_stop" && sender.tab) {
    const id = sender.tab.id;
    state.blocked.add(id);
    state.live.delete(id);
    if (state.lastTabId === id) state.lastTabId = null;
    api.action.setBadgeText({ tabId: id, text: "" });
    notifyServer({ event: "user_stop", tab_id: id });
    reply({ ok: true });
  } else if (msg.type === "released" && sender.tab) {
    state.live.delete(sender.tab.id);
    api.action.setBadgeText({ tabId: sender.tab.id, text: "" });
    reply({ ok: true });
  } else if (msg.type === "popup_status") {
    reply({ connected: state.connected, blocked: [...state.blocked], hasToken: Boolean(state.token) });
  } else if (msg.type === "unblock_all") {
    state.blocked.clear();
    reply({ ok: true });
  }
  return false;
});
