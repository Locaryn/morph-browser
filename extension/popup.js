"use strict";

const $ = (id) => document.getElementById(id);

async function refresh() {
  const s = await chrome.runtime.sendMessage({ type: "popup_status" });
  $("dot").classList.toggle("on", s.connected);
  $("state").textContent = s.connected
    ? "Connecté à Locaryn"
    : s.hasToken
      ? "Locaryn ne répond pas (lancé ? mêmes code et port ?)"
      : "Entrez le code d'appairage";
  $("unblock").hidden = s.blocked.length === 0;
}

async function init() {
  const c = await chrome.storage.local.get(["token", "port", "groupTabs"]);
  $("token").value = c.token || "";
  $("port").value = c.port || 17421;
  $("group").checked = Boolean(c.groupTabs);
  await refresh();
}

$("save").addEventListener("click", async () => {
  await chrome.storage.local.set({
    token: $("token").value.trim(),
    port: Number($("port").value) || 17421,
    groupTabs: $("group").checked,
  });
  setTimeout(refresh, 800);
});

$("unblock").addEventListener("click", async () => {
  await chrome.runtime.sendMessage({ type: "unblock_all" });
  refresh();
});

init();
