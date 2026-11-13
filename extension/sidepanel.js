// Barre latérale : la conversation avec Locaryn. L'état vit dans le service
// worker (storage.session) ; cette page l'affiche et lui envoie les consignes.
"use strict";
const api = typeof browser !== "undefined" ? browser : chrome; // Firefox expose `browser`, les navigateurs Chromium `chrome`

const PANEL_KEY = "panel";
const DRAFT_KEY = "panelDraft";
const MODELS_KEY = "panelModels";
const LABEL_KEY = "browserLabel";
/** Au-delà, une consigne non relevée laisse penser que Locaryn est fermé. */
const UNDELIVERED_MS = 8000;

const $ = (id) => document.getElementById(id);
const store = api.storage.session || api.storage.local;

const view = {
  messages: [],
  draft: null,
  connected: false,
  sending: false,
  seen: new Set(),
  models: null,
  label: "",
  switching: false,
};

/** La version lisible (« 0.1.0-beta.17 »), la même que celle du morph. */
function versionName() {
  const m = api.runtime.getManifest();
  return m.version_name || m.version;
}

// ── Rendu ───────────────────────────────────────────────────────────────────

const KIND_LABEL = {
  selection: "Sélection",
  link: "Lien",
  image: "Image",
  page: "Page",
};

function attachmentSummary(a) {
  if (!a) return "";
  if (a.kind === "selection") return a.text || "";
  if (a.kind === "link") return a.text ? `${a.text} — ${a.url}` : a.url;
  if (a.kind === "image") return a.url || "";
  return (a.page && (a.page.title || a.page.url)) || "";
}

function el(tag, cls, text) {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
}

function statusLine(m) {
  if (m.status === "answered") return null;
  if (m.status === "delivered") return { text: "Reçu par Locaryn", warn: false };
  if (Date.now() - (m.at || 0) > UNDELIVERED_MS) {
    return { text: "Pas encore reçu : Locaryn est-il ouvert ?", warn: true };
  }
  return { text: "Envoyé", warn: false };
}

function renderUser(m) {
  const box = el("div", "msg user");
  const bubble = el("div", "bubble");
  if (m.attachment) {
    const q = el("div", "quote");
    q.append(el("b", "", KIND_LABEL[m.attachment.kind] || "Pièce jointe"));
    q.append(document.createTextNode(attachmentSummary(m.attachment)));
    bubble.append(q);
  }
  if (m.text) bubble.append(document.createTextNode(m.text));
  box.append(bubble);
  const s = statusLine(m);
  if (s) box.append(el("div", s.warn ? "meta warn" : "meta", s.text));
  return box;
}

function renderAssistant(m) {
  const box = el("div", m.error ? "msg assistant error" : "msg assistant");
  box.append(el("div", "who", "Locaryn"));
  box.append(el("div", "text", m.text));
  return box;
}

function renderWorking() {
  const box = el("div", "msg assistant");
  box.append(el("div", "who", "Locaryn"));
  const dots = el("div", "working");
  dots.setAttribute("aria-label", "Locaryn travaille");
  dots.append(el("span"), el("span"), el("span"));
  box.append(dots);
  return box;
}

function renderThread() {
  const thread = $("thread");
  const atBottom = thread.scrollHeight - thread.scrollTop - thread.clientHeight < 40;
  thread.querySelectorAll(".msg").forEach((n) => n.remove());
  $("empty").hidden = view.messages.length > 0;
  for (const m of view.messages) {
    const n = m.role === "assistant" ? renderAssistant(m) : renderUser(m);
    // Seul un message nouveau s'anime : le fil est redessiné toutes les 3 s.
    if (!view.seen.has(m.id)) n.classList.add("fresh");
    view.seen.add(m.id);
    thread.append(n);
  }
  const last = view.messages[view.messages.length - 1];
  if (last && last.role === "user" && last.status === "delivered") thread.append(renderWorking());
  if (atBottom || view.sending) thread.scrollTop = thread.scrollHeight;
}

function renderDraft() {
  const a = view.draft;
  $("attachment").hidden = !a;
  if (!a) return;
  const page = a.page && a.page.title ? ` · ${a.page.title}` : "";
  $("att-kind").textContent = `${KIND_LABEL[a.kind] || "Pièce jointe"}${page}`;
  $("att-text").textContent = attachmentSummary(a);
}

function renderState() {
  $("dot").classList.toggle("on", view.connected);
  $("state-text").textContent = view.connected ? "Relié" : "Non relié";
  const empty = !$("input").value.trim() && !view.draft;
  $("send").disabled = view.sending || empty;
  $("send").classList.toggle("loading", view.sending);
}

/** Le sélecteur de modèle : caché tant que Locaryn n'a pas donné sa liste. */
function renderModels() {
  const m = view.models;
  const row = $("model-row");
  const select = $("model");
  const tags = m && Array.isArray(m.installed) ? [...m.installed] : [];
  if (m && m.active && !tags.includes(m.active)) tags.unshift(m.active);
  row.hidden = !view.connected || tags.length === 0;
  if (row.hidden) return;
  const actuels = [...select.options].map((o) => o.value).join("\n");
  if (actuels !== tags.join("\n")) {
    select.replaceChildren(...tags.map((t) => {
      const o = document.createElement("option");
      o.value = t;
      o.textContent = t;
      return o;
    }));
  }
  select.value = m.active || tags[0];
  select.disabled = view.switching;
}

function renderVersion() {
  const navigateur = view.label ? ` · ${view.label}` : "";
  const texte = `Extension ${versionName()}${navigateur}`;
  $("version").textContent = texte;
  $("empty-version").textContent = texte;
  $("state").title = texte;
}

function showError(text) {
  $("error").hidden = !text;
  $("error").textContent = text || "";
}

// ── État ────────────────────────────────────────────────────────────────────

async function loadAll() {
  const r = await store.get([PANEL_KEY, DRAFT_KEY, MODELS_KEY, LABEL_KEY]);
  const p = r[PANEL_KEY];
  view.messages = p && Array.isArray(p.messages) ? p.messages : [];
  view.draft = r[DRAFT_KEY] || null;
  view.models = r[MODELS_KEY] || null;
  view.label = typeof r[LABEL_KEY] === "string" ? r[LABEL_KEY] : "";
  renderThread();
  renderDraft();
  renderState();
  renderModels();
  renderVersion();
}

async function refreshStatus() {
  try {
    const s = await api.runtime.sendMessage({ type: "popup_status" });
    view.connected = Boolean(s && s.connected);
    $("unblock").hidden = !(s && s.blocked && s.blocked.length);
  } catch (e) {
    console.warn("Locaryn : état de l'extension illisible", e);
    view.connected = false;
  }
  renderState();
  renderModels();
}

api.storage.onChanged.addListener((changes) => {
  if (PANEL_KEY in changes || DRAFT_KEY in changes || MODELS_KEY in changes || LABEL_KEY in changes) {
    loadAll().catch((e) => console.warn("Locaryn : conversation illisible", e));
  }
});

// ── Envoi ───────────────────────────────────────────────────────────────────

async function send(text) {
  if (view.sending) return;
  const clean = text.trim();
  if (!clean && !view.draft) return;
  view.sending = true;
  showError("");
  renderState();
  try {
    const r = await api.runtime.sendMessage({ type: "panel_send", text: clean, attachment: view.draft });
    if (r && r.error) throw new Error(r.error);
    $("input").value = "";
    autosize();
  } catch (e) {
    showError(String(e && e.message ? e.message : e));
  } finally {
    view.sending = false;
    renderState();
  }
}

function autosize() {
  const t = $("input");
  t.style.height = "auto";
  t.style.height = `${Math.min(t.scrollHeight, 160)}px`;
}

$("form").addEventListener("submit", (e) => {
  e.preventDefault();
  send($("input").value);
});

$("input").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    send($("input").value);
  }
});

$("input").addEventListener("input", () => {
  autosize();
  renderState();
});

document.querySelectorAll(".idea").forEach((b) => {
  b.addEventListener("click", () => send(b.textContent || ""));
});

$("att-remove").addEventListener("click", async () => {
  await store.remove(DRAFT_KEY);
  $("input").focus();
});

$("clear").addEventListener("click", async () => {
  const r = await api.runtime.sendMessage({ type: "panel_clear" });
  if (r && r.error) showError(r.error);
});

// ── Réglages ────────────────────────────────────────────────────────────────

$("settings-toggle").addEventListener("click", async () => {
  const s = $("settings");
  s.hidden = !s.hidden;
  $("settings-toggle").setAttribute("aria-expanded", String(!s.hidden));
  if (!s.hidden) {
    const c = await api.storage.local.get(["token", "port"]);
    $("token").value = c.token || "";
    $("port").value = c.port || 17421;
  }
});

$("save").addEventListener("click", async () => {
  await api.storage.local.set({
    token: $("token").value.trim(),
    port: Number($("port").value) || 17421,
  });
  setTimeout(refreshStatus, 800);
});

$("unblock").addEventListener("click", async () => {
  await api.runtime.sendMessage({ type: "unblock_all" });
  refreshStatus();
});

$("model").addEventListener("change", async (e) => {
  const tag = e.target.value;
  view.switching = true;
  renderModels();
  showError("");
  try {
    const r = await api.runtime.sendMessage({ type: "panel_model", model: tag });
    if (r && r.error) showError(r.error);
  } catch (err) {
    showError(String(err && err.message ? err.message : err));
  } finally {
    view.switching = false;
    renderModels();
  }
});

renderVersion();

loadAll().catch((e) => console.warn("Locaryn : conversation illisible", e));
refreshStatus();
// L'état de la liaison et les consignes restées sans accusé changent seuls.
setInterval(() => {
  refreshStatus();
  renderThread();
}, 3000);
$("input").focus();
