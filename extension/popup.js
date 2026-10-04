"use strict";
const api = typeof browser !== "undefined" ? browser : chrome; // Firefox expose `browser`, les navigateurs Chromium `chrome`

const $ = (id) => document.getElementById(id);

async function refresh() {
  const s = await api.runtime.sendMessage({ type: "popup_status" });
  $("dot").classList.toggle("on", s.connected);
  $("state").textContent = s.connected
    ? "Connecté à Locaryn"
    : s.hasToken
      ? "Locaryn ne répond pas (lancé ? mêmes code et port ?)"
      : "Entrez le code d'appairage";
  $("unblock").hidden = s.blocked.length === 0;
}

async function init() {
  const c = await api.storage.local.get(["token", "port", "groupTabs"]);
  $("token").value = c.token || "";
  $("port").value = c.port || 17421;
  $("group").checked = Boolean(c.groupTabs);
  await refresh();
}

$("save").addEventListener("click", async () => {
  await api.storage.local.set({
    token: $("token").value.trim(),
    port: Number($("port").value) || 17421,
    groupTabs: $("group").checked,
  });
  setTimeout(refresh, 800);
});

$("unblock").addEventListener("click", async () => {
  await api.runtime.sendMessage({ type: "unblock_all" });
  refresh();
});

init();
