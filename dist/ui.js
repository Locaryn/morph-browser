// Relais de la barre latérale du navigateur (slot `topbar.actions`, invisible).
//
// L'extension du navigateur a sa propre barre latérale : on y tape une
// consigne, éventuellement avec la sélection jointe par le clic droit. La
// consigne arrive au serveur MCP du morph par le pont local ; ce relais la
// relève (`ui_panel_inbox`), la poste dans la conversation ouverte comme un
// message tapé, puis renvoie la réponse finie (`chat.reply`) à la barre
// latérale (`ui_panel_reply`). Les outils `ui_*` sont cachés au modèle.
//
// Il donne aussi aux barres latérales la liste des modèles de la conversation
// (`models.list` de l'hôte, `ui_panel_models`), et applique le modèle qu'on y
// choisit (`models.select`). Plusieurs navigateurs peuvent être reliés : la
// réponse revient à celui qui a posé la question.
//
// Aucun rendu : l'élément reste masqué dans la barre du haut.
(function () {
  "use strict";

  var TAG = "locaryn-browser-relay";
  /** Rythme de relève tant que tout va bien. */
  var RELEVE_MS = 2000;
  /** Après une erreur (morph arrêté, outil absent) : moins souvent. */
  var RELEVE_ERREUR_MS = 15000;
  /** Le setter du composeur n'est pris en compte qu'au rendu suivant. */
  var AVANT_ENVOI_MS = 150;
  /** Une réponse qui ne vient pas libère le relais. */
  var REPONSE_MAX_MS = 600000;
  var MAX_SELECTION = 8000;
  /** Même sans changement, la liste des modèles est redonnée à ce rythme :
   *  un navigateur relié entre-temps la reçoit aussi. */
  var MODELES_MS = 20000;

  function nomAffiche(nom) {
    return nom ? nom.charAt(0).toUpperCase() + nom.slice(1) : "";
  }

  function api() {
    return window.locaryn || window.LocarynPluginAPI || null;
  }

  function invoquer(a, outil, args) {
    var appel = a.tools && a.tools.invoke ? a.tools.invoke(outil, args) : a.invokeExtensionTool(outil, args);
    return Promise.resolve(appel).then(function (r) {
      if (typeof r !== "string") return r;
      try {
        return JSON.parse(r);
      } catch (e) {
        console.warn("[navigateur] réponse non JSON de", outil, e);
        return null;
      }
    });
  }

  function citer(texte) {
    return texte
      .split("\n")
      .map(function (l) {
        return "> " + l;
      })
      .join("\n");
  }

  /** Le message posté dans la conversation : la consigne, puis d'où elle vient. */
  function composer(m) {
    var morceaux = [];
    var texte = (m.text || "").trim();
    var pj = m.attachment || null;
    var page = (pj && pj.page) || m.page || null;
    if (texte) morceaux.push(texte);
    else if (pj && pj.kind === "selection") morceaux.push("Que peux-tu me dire de ce passage ?");
    else if (pj) morceaux.push("Que peux-tu me dire de ceci ?");
    var origine = m.browser
      ? "(Consigne envoyée depuis la barre latérale de " + nomAffiche(m.browser) + " — browser « " + m.browser + " »"
      : "(Consigne envoyée depuis la barre latérale du navigateur";
    if (page && (page.title || page.url)) {
      origine += " — onglet « " + (page.title || page.url) + " »";
      if (page.url) origine += " " + page.url;
      if (page.tab_id !== undefined && page.tab_id !== null) origine += ", tab_id " + page.tab_id;
    }
    morceaux.push(origine + ".)");
    if (pj && pj.kind === "selection" && pj.text) {
      morceaux.push("Sélection :\n" + citer(String(pj.text).slice(0, MAX_SELECTION)));
    } else if (pj && pj.kind === "link") {
      morceaux.push("Lien : " + (pj.text ? pj.text + " — " : "") + pj.url);
    } else if (pj && pj.kind === "image") {
      morceaux.push("Image : " + pj.url);
    }
    return morceaux.join("\n\n");
  }

  function Relais() {
    this.actif = false;
    this.minuteur = null;
    this.file = [];
    this.enCours = null;
    this.desabonner = null;
    this.modeles = { signature: "", depuis: 0 };
  }

  Relais.prototype.demarrer = function () {
    if (this.actif) return;
    var a = api();
    if (!a) return;
    var self = this;
    this.actif = true;
    this.desabonner = a.events.on("chat.reply", function (d) {
      self.surReponse(d);
    });
    this.planifier(RELEVE_MS);
  };

  Relais.prototype.arreter = function () {
    this.actif = false;
    clearTimeout(this.minuteur);
    if (this.desabonner) this.desabonner();
    this.desabonner = null;
  };

  Relais.prototype.planifier = function (ms) {
    var self = this;
    clearTimeout(this.minuteur);
    if (!this.actif) return;
    this.minuteur = setTimeout(function () {
      self.tour();
    }, ms);
  };

  Relais.prototype.tour = function () {
    var self = this;
    var a = api();
    if (!a || !this.actif) return;
    // Une réponse attendue trop longtemps n'arrivera plus.
    if (this.enCours && Date.now() - this.enCours.depuis > REPONSE_MAX_MS) {
      this.repondre(this.enCours, "Locaryn n'a pas répondu à temps.", true);
      this.enCours = null;
    }
    invoquer(a, "ui_panel_inbox", {})
      .then(function (r) {
        var recus = r && Array.isArray(r.messages) ? r.messages : [];
        for (var i = 0; i < recus.length; i++) {
          var m = recus[i];
          if (!m) continue;
          if (m.kind === "model") self.choisirModele(m);
          else self.file.push(m);
        }
        if (r && r.extension_connected) self.donnerModeles(false);
        self.poster();
        self.planifier(RELEVE_MS);
      })
      .catch(function (e) {
        console.debug("[navigateur] relève impossible :", e);
        self.planifier(RELEVE_ERREUR_MS);
      });
  };

  /** Poste la prochaine consigne, une à la fois, quand le composeur est libre. */
  Relais.prototype.poster = function () {
    var a = api();
    if (!a || this.enCours || this.file.length === 0) return;
    // La personne écrit dans Locaryn : ne pas écraser son texte.
    if ((a.chat.getText() || "").trim()) return;
    var m = this.file.shift();
    var self = this;
    var envoi = { id: m.id, browser: m.browser || null, depuis: Date.now(), session: a.chat.getSessionId() };
    this.enCours = envoi;
    a.chat.setText(composer(m));
    setTimeout(function () {
      // Aucun composeur affiché (réglages, marketplace…) : le texte n'a pas pris.
      if (!(a.chat.getText() || "").trim()) {
        self.repondre(envoi, "Ouvrez une conversation dans Locaryn, puis renvoyez la consigne.", true);
        self.enCours = null;
        return;
      }
      a.chat.submit();
      // Rien n'est parti (aucune conversation affichée) : le texte est resté.
      setTimeout(function () {
        if (self.enCours && self.enCours.id === m.id && (a.chat.getText() || "").trim()) {
          a.chat.setText("");
          self.repondre(envoi, "Ouvrez une conversation dans Locaryn, puis renvoyez la consigne.", true);
          self.enCours = null;
        }
      }, 1500);
    }, AVANT_ENVOI_MS);
  };

  Relais.prototype.surReponse = function (d) {
    if (!this.enCours) return;
    var session = d && d.sessionId ? String(d.sessionId) : null;
    // Une autre conversation a fini : ce n'est pas notre réponse.
    if (this.enCours.session && session && session !== this.enCours.session) return;
    var envoi = this.enCours;
    this.enCours = null;
    this.repondre(envoi, d && d.text ? String(d.text) : "", false);
    this.poster();
  };

  Relais.prototype.repondre = function (envoi, texte, erreur) {
    var a = api();
    if (!a) return;
    var args = { id: envoi.id, text: texte || "(réponse vide)", error: Boolean(erreur) };
    if (envoi.browser) args.browser = envoi.browser;
    invoquer(a, "ui_panel_reply", args).catch(
      function (e) {
        console.warn("[navigateur] réponse non transmise à la barre latérale :", e);
      },
    );
  };

  /** Les modèles de la conversation affichée, si l'hôte sait les donner. */
  function lireModeles(a) {
    if (!a.models || typeof a.models.list !== "function") return Promise.resolve(null);
    return Promise.resolve(a.models.list()).then(
      function (m) {
        return m && Array.isArray(m.installed) ? m : null;
      },
      function (e) {
        // Aucune conversation affichée : pas de liste, ce n'est pas une panne.
        console.debug("[navigateur] modèles indisponibles :", e);
        return null;
      },
    );
  }

  /** Donne la liste aux barres latérales quand elle change, et de temps en temps. */
  Relais.prototype.donnerModeles = function (forcer) {
    var a = api();
    if (!a) return;
    var self = this;
    lireModeles(a).then(function (m) {
      if (!m) return;
      var signature = (m.active || "") + "|" + m.installed.join(",");
      var vieille = Date.now() - self.modeles.depuis > MODELES_MS;
      if (!forcer && !vieille && signature === self.modeles.signature) return;
      self.modeles = { signature: signature, depuis: Date.now() };
      invoquer(a, "ui_panel_models", { active: m.active || "", installed: m.installed }).catch(function (e) {
        console.warn("[navigateur] modèles non transmis à la barre latérale :", e);
      });
    });
  };

  /** Le modèle choisi dans une barre latérale devient celui de la conversation. */
  Relais.prototype.choisirModele = function (m) {
    var a = api();
    var self = this;
    if (!a || !a.models || typeof a.models.select !== "function" || !m.model) {
      console.warn("[navigateur] changement de modèle impossible : Locaryn trop ancien ou modèle vide");
      return;
    }
    Promise.resolve(a.models.select(String(m.model)))
      .catch(function (e) {
        console.warn("[navigateur] modèle non changé :", e);
      })
      .then(function () {
        self.donnerModeles(true);
      });
  };

  // Un seul relais, même si le slot est rendu deux fois.
  var relais = window.__locarynBrowserRelay || (window.__locarynBrowserRelay = new Relais());
  var montes = 0;

  if (!customElements.get(TAG)) {
    customElements.define(
      TAG,
      class extends HTMLElement {
        connectedCallback() {
          this.style.display = "none";
          montes += 1;
          relais.demarrer();
        }
        disconnectedCallback() {
          montes = Math.max(0, montes - 1);
          if (montes === 0) relais.arreter();
        }
      },
    );
  }
})();
