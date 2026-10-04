// Script injecté dans la page : lecture, actions, et signalisation visible du contrôle.
(() => {
  "use strict";
  const api = typeof browser !== "undefined" ? browser : chrome; // Firefox expose `browser`, les navigateurs Chromium `chrome`
  if (window.__locarynLoaded) return;
  window.__locarynLoaded = true;

  // Filet de sécurité si `browser_release` n'est jamais appelé : large,
  // volontairement, pour qu'un temps de réflexion entre deux outils ne
  // fasse jamais disparaître puis réapparaître le cadre (l'appel explicite
  // reste la façon normale de terminer).
  const IDLE_MS = 180000;
  const TITLE_MARK = "◉ ";
  const INTERACTIVE =
    'a[href],button,input,select,textarea,summary,[role=button],[role=link],[role=tab],[role=menuitem],' +
    '[role=checkbox],[role=radio],[role=switch],[role=option],[role=combobox],[role=textbox],' +
    '[contenteditable=""],[contenteditable=true],[onclick],[tabindex]:not([tabindex="-1"])';

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  // ── Références stables aux éléments ───────────────────────────────────────

  const ids = new WeakMap();
  const byId = new Map();
  let nextRef = 1;

  function refOf(el) {
    let id = ids.get(el);
    if (!id) {
      id = nextRef++;
      ids.set(el, id);
      byId.set(id, new WeakRef(el));
    }
    return id;
  }

  function elementOf(ref) {
    const el = byId.get(Number(ref))?.deref();
    if (!el || !el.isConnected) {
      throw new Error(`Élément ${ref} introuvable ou disparu : faites un nouveau snapshot.`);
    }
    return el;
  }

  // ── Lecture de la page ────────────────────────────────────────────────────

  function collect(root, out) {
    root.querySelectorAll(INTERACTIVE).forEach((el) => out.push(el));
    root.querySelectorAll("*").forEach((el) => {
      if (el.shadowRoot) collect(el.shadowRoot, out);
    });
    return out;
  }

  function isVisible(el) {
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return false;
    const s = getComputedStyle(el);
    return s.visibility !== "hidden" && s.display !== "none" && Number(s.opacity) > 0;
  }

  function nameOf(el) {
    const direct =
      el.getAttribute("aria-label") ||
      el.getAttribute("alt") ||
      el.getAttribute("title") ||
      el.getAttribute("placeholder");
    if (direct) return direct.trim().slice(0, 100);
    if (el.labels && el.labels.length) return el.labels[0].innerText.trim().slice(0, 100);
    const text = (el.innerText || el.textContent || "").replace(/\s+/g, " ").trim();
    return (text || el.value || el.getAttribute("name") || "").toString().slice(0, 100);
  }

  function roleOf(el) {
    const explicit = el.getAttribute("role");
    if (explicit) return explicit;
    const tag = el.tagName.toLowerCase();
    if (tag === "a") return "link";
    if (tag === "input") return el.type === "checkbox" || el.type === "radio" ? el.type : el.type === "submit" || el.type === "button" ? "button" : "textbox";
    if (tag === "select") return "combobox";
    if (tag === "textarea") return "textbox";
    return tag;
  }

  function describe(el) {
    const r = el.getBoundingClientRect();
    const d = {
      ref: refOf(el),
      role: roleOf(el),
      name: nameOf(el),
      rect: [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)],
      in_viewport: r.bottom > 0 && r.top < innerHeight && r.right > 0 && r.left < innerWidth,
    };
    if (el.tagName === "A" && el.href) d.href = el.href.slice(0, 160);
    if ("value" in el && el.type !== "password" && el.value) d.value = String(el.value).slice(0, 80);
    if (el.type === "checkbox" || el.type === "radio") d.checked = el.checked;
    if (el.disabled) d.disabled = true;
    return d;
  }

  function snapshot(args) {
    const max = Number(args.max_elements) || 80;
    const all = collect(document, []).filter(isVisible).map(describe);
    const shown = [...all.filter((e) => e.in_viewport), ...all.filter((e) => !e.in_viewport)].slice(0, max);
    const text = (document.body?.innerText || "").replace(/\n{3,}/g, "\n\n").trim();
    return {
      title: document.title.replace(TITLE_MARK, ""),
      url: location.href,
      scroll: { x: Math.round(scrollX), y: Math.round(scrollY), max_y: Math.round(document.documentElement.scrollHeight - innerHeight) },
      viewport: [innerWidth, innerHeight],
      element_count: all.length,
      elements: shown,
      text: text.slice(0, Number(args.text_chars) || 3000),
      frames: document.querySelectorAll("iframe").length,
    };
  }

  // ── Actions ───────────────────────────────────────────────────────────────

  function target(args) {
    if (args.ref !== undefined && args.ref !== null) return elementOf(args.ref);
    if (args.x !== undefined && args.y !== undefined) {
      const el = document.elementFromPoint(Number(args.x), Number(args.y));
      if (!el) throw new Error("Aucun élément à ces coordonnées.");
      return el;
    }
    throw new Error("Indiquez `ref` (issu d'un snapshot) ou `x` et `y`.");
  }

  function center(el) {
    const r = el.getBoundingClientRect();
    return [r.x + r.width / 2, r.y + r.height / 2];
  }

  async function reach(el) {
    el.scrollIntoView({ block: "center", inline: "center" });
    await sleep(120);
    const [x, y] = center(el);
    overlay.moveCursor(x, y);
    await sleep(320);
    return [x, y];
  }

  function fire(el, type, x, y, extra) {
    const init = { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, view: window, ...extra };
    const Ctor = type.startsWith("pointer") ? PointerEvent : MouseEvent;
    el.dispatchEvent(new Ctor(type, init));
  }

  async function click(args) {
    const el = target(args);
    const [x, y] = await reach(el);
    const right = args.button === "right";
    overlay.ripple(x, y, right ? "right" : "left");
    const b = right ? { button: 2, buttons: 2 } : { button: 0, buttons: 1 };
    fire(el, "pointerover", x, y);
    fire(el, "pointerdown", x, y, b);
    fire(el, "mousedown", x, y, b);
    if (typeof el.focus === "function") el.focus({ preventScroll: true });
    fire(el, "pointerup", x, y, { ...b, buttons: 0 });
    fire(el, "mouseup", x, y, { ...b, buttons: 0 });
    if (right) fire(el, "contextmenu", x, y, b);
    else {
      el.click();
      if (args.double) {
        el.click();
        fire(el, "dblclick", x, y, b);
      }
    }
    await sleep(150);
    return { clicked: describe(el).name, url: location.href };
  }

  function setNativeValue(el, value) {
    const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value").set.call(el, value);
  }

  async function type(args) {
    const el = target(args);
    await reach(el);
    el.focus({ preventScroll: true });
    const text = String(args.text ?? "");
    if (el.isContentEditable) {
      if (args.clear !== false) document.execCommand("selectAll");
      document.execCommand("insertText", false, text);
    } else if ("value" in el) {
      setNativeValue(el, args.clear === false ? el.value + text : text);
      el.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText", data: text }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
    } else {
      throw new Error("Cet élément n'accepte pas de saisie.");
    }
    if (args.submit) await pressKey({ key: "Enter", ref: refOf(el) });
    return { typed: text.length, target: describe(el).name };
  }

  async function pressKey(args) {
    const el = args.ref !== undefined && args.ref !== null ? elementOf(args.ref) : document.activeElement || document.body;
    const parts = String(args.key || "").split("+");
    const key = parts.pop();
    const mods = parts.map((m) => m.toLowerCase());
    const init = {
      key,
      code: key.length === 1 ? `Key${key.toUpperCase()}` : key,
      bubbles: true,
      cancelable: true,
      ctrlKey: mods.includes("control") || mods.includes("ctrl"),
      shiftKey: mods.includes("shift"),
      altKey: mods.includes("alt"),
      metaKey: mods.includes("meta") || mods.includes("cmd"),
    };
    el.dispatchEvent(new KeyboardEvent("keydown", init));
    el.dispatchEvent(new KeyboardEvent("keypress", init));
    if (key === "Enter" && el.form) el.form.requestSubmit();
    el.dispatchEvent(new KeyboardEvent("keyup", init));
    await sleep(100);
    return { pressed: args.key };
  }

  async function scroll(args) {
    const el = args.ref !== undefined && args.ref !== null ? elementOf(args.ref) : null;
    const step = Number(args.amount) || Math.round(innerHeight * 0.8);
    const box = el || window;
    if (args.direction === "top") box.scrollTo({ top: 0 });
    else if (args.direction === "bottom") box.scrollTo({ top: (el || document.documentElement).scrollHeight });
    else box.scrollBy({ top: args.direction === "up" ? -step : step });
    await sleep(250);
    return { scroll_y: Math.round(scrollY) };
  }

  async function selectOption(args) {
    const el = target(args);
    if (el.tagName !== "SELECT") throw new Error("Cet élément n'est pas une liste déroulante.");
    const wanted = String(args.value);
    const opt = [...el.options].find((o) => o.value === wanted || o.text.trim() === wanted);
    if (!opt) throw new Error(`Option introuvable : ${wanted}. Disponibles : ${[...el.options].map((o) => o.text.trim()).join(", ")}`);
    await reach(el);
    el.value = opt.value;
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return { selected: opt.text.trim() };
  }

  async function hover(args) {
    const el = target(args);
    const [x, y] = await reach(el);
    fire(el, "pointerover", x, y);
    fire(el, "mouseover", x, y);
    fire(el, "mousemove", x, y);
    return { hovered: describe(el).name };
  }

  async function wait(args) {
    const timeout = Math.min(Number(args.timeout_ms) || 8000, 30000);
    if (args.ms) {
      await sleep(Math.min(Number(args.ms), 30000));
      return { waited_ms: Number(args.ms) };
    }
    const start = Date.now();
    while (Date.now() - start < timeout) {
      if (args.text && document.body.innerText.includes(args.text)) return { found: "text", after_ms: Date.now() - start };
      if (args.selector && document.querySelector(args.selector)) return { found: "selector", after_ms: Date.now() - start };
      await sleep(200);
    }
    throw new Error("Délai dépassé : la condition attendue n'est pas apparue.");
  }

  const COMMANDS = { snapshot, click, type, press_key: pressKey, scroll, select_option: selectOption, hover, wait };

  // ── Signalisation : cadre, pastille, curseur, onglet ─────────────────────

  const overlay = (() => {
    let host = null;
    let root = null;
    let idleTimer = null;
    let peeking = false;
    let originalIcons = [];
    let ourIcon = null;
    let titleObserver = null;

    const CSS = `
      :host { all: initial; }
      .frame { position: fixed; inset: 0; pointer-events: none; z-index: 2147483646; }
      .frame::before {
        content: ""; position: absolute; inset: 0; padding: 4px; border-radius: 0;
        background: conic-gradient(from var(--a, 0deg), #3e8c66, #a7d8bd, #3e8c66 30%, #1f4d38 55%, #3e8c66);
        -webkit-mask: linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0);
        -webkit-mask-composite: xor; mask-composite: exclude;
        animation: spin 4s linear infinite;
      }
      .frame::after {
        content: ""; position: absolute; inset: 0; box-shadow: inset 0 0 38px 6px rgba(62,140,102,.35);
        animation: breathe 2.6s ease-in-out infinite;
      }
      @property --a { syntax: "<angle>"; initial-value: 0deg; inherits: false; }
      @keyframes spin { to { --a: 360deg; } }
      @keyframes breathe { 50% { opacity: .45; } }
      .pill {
        position: fixed; top: 14px; left: 50%; transform: translateX(-50%); z-index: 2147483647;
        display: flex; align-items: center; gap: 12px; min-height: 44px; padding: 0 6px 0 16px;
        background: rgba(20,28,24,.92); color: #eef4f0; border: 1px solid rgba(167,216,189,.35);
        border-radius: 999px; font: 500 13px/1.2 system-ui, sans-serif; pointer-events: auto;
        box-shadow: 0 6px 24px rgba(0,0,0,.28);
      }
      .dot { width: 9px; height: 9px; border-radius: 50%; background: #6fcf9d; animation: pulse 1.4s ease-in-out infinite; }
      @keyframes pulse { 50% { transform: scale(1.5); opacity: .5; } }
      .label { opacity: .7; max-width: 22ch; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      button {
        all: unset; box-sizing: border-box; min-height: 36px; min-width: 44px; padding: 0 14px; border-radius: 999px;
        background: #b3413b; color: #fff; font: 600 12px system-ui, sans-serif; text-align: center;
        display: grid; place-items: center; cursor: pointer; transition: background .15s, transform .1s;
      }
      button:hover { background: #cf4f48; }
      button:active { transform: scale(.94); }
      button:focus-visible { outline: 2px solid #fff; outline-offset: 2px; }
      .cursor { position: fixed; left: 0; top: 0; width: 22px; height: 22px; pointer-events: none; z-index: 2147483647;
        transition: transform .3s cubic-bezier(.3,.8,.3,1); filter: drop-shadow(0 1px 2px rgba(0,0,0,.4)); }
      .ring { position: fixed; width: 14px; height: 14px; margin: -7px 0 0 -7px; border-radius: 50%;
        border: 2px solid #6fcf9d; pointer-events: none; z-index: 2147483647; animation: ripple .7s ease-out forwards; }
      .ring.right { border-color: #e2a54a; }
      .ring.b { animation-delay: .12s; }
      @keyframes ripple { from { transform: scale(.4); opacity: 1; } to { transform: scale(4.2); opacity: 0; } }
      .toast { position: fixed; top: 14px; left: 50%; transform: translateX(-50%); z-index: 2147483647; padding: 12px 18px;
        background: rgba(60,20,18,.94); color: #fff; border-radius: 999px; font: 500 13px system-ui, sans-serif; }
      @media (prefers-reduced-motion: reduce) { .frame::before, .frame::after, .dot { animation: none; } }
    `;

    function build(label) {
      host = document.createElement("locaryn-overlay");
      root = host.attachShadow({ mode: "closed" });
      root.innerHTML = `
        <style>${CSS}</style>
        <div class="frame"></div>
        <div class="pill" role="status">
          <span class="dot"></span><span>Locaryn contrôle cet onglet</span><span class="label"></span>
          <button type="button" aria-label="Arrêter le contrôle de cet onglet">Stop</button>
        </div>
        <svg class="cursor" viewBox="0 0 24 24" style="transform:translate(-40px,-40px)">
          <path d="M3 2l7 19 3-8 8-3z" fill="#3e8c66" stroke="#eef4f0" stroke-width="1.5" stroke-linejoin="round"/>
        </svg>`;
      root.querySelector("button").addEventListener("click", stop);
      document.documentElement.appendChild(host);
      setLabel(label);
    }

    function setLabel(label) {
      const el = root && root.querySelector(".label");
      if (el) el.textContent = label ? `· ${label}` : "";
    }

    function markTab() {
      if (!document.title.startsWith(TITLE_MARK)) document.title = TITLE_MARK + document.title;
      if (!titleObserver && document.querySelector("title")) {
        titleObserver = new MutationObserver(() => {
          if (host && !document.title.startsWith(TITLE_MARK)) document.title = TITLE_MARK + document.title;
        });
        titleObserver.observe(document.querySelector("title"), { childList: true });
      }
      markFavicon();
    }

    function markFavicon() {
      if (ourIcon) return;
      originalIcons = [...document.querySelectorAll('link[rel~="icon"]')];
      const src = originalIcons[0]?.href || `${location.origin}/favicon.ico`;
      const draw = (img) => {
        const c = document.createElement("canvas");
        c.width = c.height = 32;
        const g = c.getContext("2d");
        if (img) g.drawImage(img, 0, 0, 32, 32);
        g.fillStyle = "#3e8c66";
        g.beginPath();
        g.arc(23, 23, 9, 0, Math.PI * 2);
        g.fill();
        g.strokeStyle = "#fff";
        g.lineWidth = 2.5;
        g.stroke();
        try {
          return c.toDataURL("image/png");
        } catch (e) {
          return null; // image d'un autre domaine : le canevas est marqué
        }
      };
      const apply = (url) => {
        if (!url || !host) return;
        originalIcons.forEach((l) => l.remove());
        ourIcon = document.createElement("link");
        ourIcon.rel = "icon";
        ourIcon.href = url;
        document.head.appendChild(ourIcon);
      };
      const img = new Image();
      img.crossOrigin = "anonymous";
      img.onload = () => apply(draw(img) || draw(null));
      img.onerror = () => apply(draw(null));
      img.src = src;
    }

    function unmarkTab() {
      if (titleObserver) titleObserver.disconnect();
      titleObserver = null;
      if (document.title.startsWith(TITLE_MARK)) document.title = document.title.slice(TITLE_MARK.length);
      if (ourIcon) {
        ourIcon.remove();
        originalIcons.forEach((l) => document.head.appendChild(l));
        ourIcon = null;
      }
    }

    function on(label) {
      if (!host) {
        build(label);
        markTab();
      } else {
        setLabel(label);
      }
      clearTimeout(idleTimer);
      idleTimer = setTimeout(off, IDLE_MS);
    }

    function off() {
      clearTimeout(idleTimer);
      if (host) host.remove();
      host = root = null;
      unmarkTab();
      api.runtime.sendMessage({ type: "released" }).catch(() => {});
    }

    function stop() {
      const shell = document.createElement("locaryn-overlay");
      const r = shell.attachShadow({ mode: "closed" });
      r.innerHTML = `<style>${CSS}</style><div class="toast">Contrôle arrêté — vous avez repris la main</div>`;
      api.runtime.sendMessage({ type: "user_stop" }).catch(() => {});
      off();
      document.documentElement.appendChild(shell);
      setTimeout(() => shell.remove(), 3500);
    }

    function moveCursor(x, y) {
      const c = root && root.querySelector(".cursor");
      if (c) c.style.transform = `translate(${x - 3}px, ${y - 2}px)`;
    }

    function ripple(x, y, kind) {
      if (!root) return;
      ["a", "b"].forEach((n) => {
        const ring = document.createElement("div");
        ring.className = `ring ${kind} ${n}`;
        ring.style.left = `${x}px`;
        ring.style.top = `${y}px`;
        root.appendChild(ring);
        setTimeout(() => ring.remove(), 1000);
      });
    }

    function peek(hidden) {
      peeking = hidden;
      if (host) host.style.display = hidden ? "none" : "";
    }

    return { on, off, moveCursor, ripple, peek, isPeeking: () => peeking };
  })();

  // ── Messages de l'extension ───────────────────────────────────────────────

  api.runtime.onMessage.addListener((msg, _sender, reply) => {
    if (msg.type === "overlay") {
      if (msg.on) overlay.on(msg.label);
      else overlay.off();
      reply({ ok: true });
      return false;
    }
    if (msg.type === "overlay_peek") {
      overlay.peek(msg.hidden);
      reply({ ok: true });
      return false;
    }
    if (msg.type === "cmd") {
      const fn = COMMANDS[msg.cmd];
      if (!fn) {
        reply({ error: `Commande inconnue : ${msg.cmd}` });
        return false;
      }
      Promise.resolve()
        .then(() => fn(msg.args || {}))
        .then((r) => reply(r))
        .catch((e) => reply({ error: e.message || String(e) }));
      return true;
    }
    return false;
  });
})();
