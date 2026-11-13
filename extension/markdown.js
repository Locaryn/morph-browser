// Mise en forme des réponses de la barre latérale : la réflexion du modèle à
// part (repliée), le Markdown rendu en nœuds DOM. Aucun innerHTML : le texte
// vient d'un modèle, il ne doit jamais devenir du HTML actif.
"use strict";

/** Balises de réflexion connues, en minuscules (même liste que l'app). */
const REASONING_TAGS = ["think", "thinking", "reasoning", "reflection", "scratchpad"];
const OPEN_RE = new RegExp(`<(${REASONING_TAGS.join("|")})\\s*>`, "i");
const ORPHAN_CLOSE_RE = new RegExp(`</(${REASONING_TAGS.join("|")})\\s*>`, "i");

/**
 * Sépare la réflexion de la réponse. Gère le bloc non refermé (réflexion en
 * cours) et la balise fermante orpheline des modèles dont le gabarit ouvre
 * lui-même `<think>` : tout ce qui la précède est de la réflexion.
 * @param {string} text
 * @returns {{ reasoning: string, answer: string }}
 */
function splitReasoning(text) {
  let rest = String(text || "");
  const blocks = [];
  const orphan = rest.match(ORPHAN_CLOSE_RE);
  const open = rest.match(OPEN_RE);
  if (orphan && orphan.index !== undefined && (!open || open.index === undefined || orphan.index < open.index)) {
    blocks.push(rest.slice(0, orphan.index));
    rest = rest.slice(orphan.index + orphan[0].length);
  }
  let answer = "";
  for (;;) {
    const o = rest.match(OPEN_RE);
    if (!o || o.index === undefined) { answer += rest; break; }
    answer += rest.slice(0, o.index);
    const after = rest.slice(o.index + o[0].length);
    const close = after.match(new RegExp(`</${o[1]}\\s*>`, "i"));
    if (!close || close.index === undefined) { blocks.push(after); break; }
    blocks.push(after.slice(0, close.index));
    rest = after.slice(close.index + close[0].length);
  }
  return {
    reasoning: blocks.map((b) => b.trim()).filter(Boolean).join("\n\n"),
    answer: answer.trim(),
  };
}

// ── En ligne ────────────────────────────────────────────────────────────────

/** Ordre de priorité : code, liens, gras, italique, barré. */
const INLINE_RE = /`([^`\n]+)`|\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)|\*\*([^*\n]+?)\*\*|__([^_\n]+?)__|\*([^*\s][^*\n]*?)\*|_([^_\s][^_\n]*?)_|~~([^~\n]+?)~~|(https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"])/g;

/**
 * @param {string} tag
 * @param {string} text
 * @returns {HTMLElement}
 */
function inlineNode(tag, text) {
  const n = document.createElement(tag);
  appendInline(n, text);
  return n;
}

/**
 * @param {string} href
 * @param {string} label
 * @returns {HTMLAnchorElement}
 */
function linkNode(href, label) {
  const a = document.createElement("a");
  a.href = href;
  a.target = "_blank";
  a.rel = "noopener noreferrer";
  a.textContent = label;
  return a;
}

/**
 * @param {RegExpExecArray} m
 * @returns {Node}
 */
function inlineMatch(m) {
  if (m[1] !== undefined) {
    const c = document.createElement("code");
    c.textContent = m[1];
    return c;
  }
  if (m[2] !== undefined) return linkNode(m[3], m[2]);
  if (m[4] !== undefined || m[5] !== undefined) return inlineNode("strong", m[4] ?? m[5]);
  if (m[6] !== undefined || m[7] !== undefined) return inlineNode("em", m[6] ?? m[7]);
  if (m[8] !== undefined) return inlineNode("del", m[8]);
  return linkNode(m[9], m[9]);
}

/**
 * Ajoute à `parent` le texte d'une ligne avec sa mise en forme en ligne.
 * @param {Node} parent
 * @param {string} text
 */
function appendInline(parent, text) {
  const re = new RegExp(INLINE_RE.source, "g");
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > last) parent.appendChild(document.createTextNode(text.slice(last, m.index)));
    parent.appendChild(inlineMatch(m));
    last = m.index + m[0].length;
  }
  if (last < text.length) parent.appendChild(document.createTextNode(text.slice(last)));
}

// ── Blocs ───────────────────────────────────────────────────────────────────

const FENCE_RE = /^\s*(```|~~~)/;
const HEADING_RE = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
const RULE_RE = /^\s{0,3}([-*_])(\s*\1){2,}\s*$/;
const ITEM_RE = /^(\s*)([-*+]|\d{1,3}[.)])\s+(.*)$/;
const QUOTE_RE = /^\s{0,3}>\s?(.*)$/;

/**
 * Bloc de code clôturé ; renvoie l'indice de la ligne suivante.
 * @param {string[]} lines
 * @param {number} i
 * @param {Node} out
 * @returns {number}
 */
function readFence(lines, i, out) {
  const fence = lines[i].trim().slice(0, 3);
  const body = [];
  let j = i + 1;
  while (j < lines.length && !lines[j].trim().startsWith(fence)) body.push(lines[j++]);
  const pre = document.createElement("pre");
  const code = document.createElement("code");
  code.textContent = body.join("\n");
  pre.appendChild(code);
  out.appendChild(pre);
  return j + 1;
}

/**
 * Liste (à puces ou numérotée) ; renvoie l'indice de la ligne suivante.
 * @param {string[]} lines
 * @param {number} i
 * @param {Node} out
 * @returns {number}
 */
function readList(lines, i, out) {
  const ordered = /\d/.test(ITEM_RE.exec(lines[i])[2]);
  const list = document.createElement(ordered ? "ol" : "ul");
  let j = i;
  let li = null;
  while (j < lines.length) {
    const m = ITEM_RE.exec(lines[j]);
    if (m && /\d/.test(m[2]) === ordered) {
      li = inlineNode("li", m[3]);
      list.appendChild(li);
    } else if (li && lines[j].trim() && /^\s{2,}/.test(lines[j])) {
      li.appendChild(document.createTextNode(" "));
      appendInline(li, lines[j].trim());
    } else break;
    j++;
  }
  out.appendChild(list);
  return j;
}

/**
 * Citation ; renvoie l'indice de la ligne suivante.
 * @param {string[]} lines
 * @param {number} i
 * @param {Node} out
 * @returns {number}
 */
function readQuote(lines, i, out) {
  const body = [];
  let j = i;
  for (let m = QUOTE_RE.exec(lines[j] || ""); m && j < lines.length; m = QUOTE_RE.exec(lines[++j] || "")) {
    body.push(m[1]);
  }
  const q = document.createElement("blockquote");
  q.appendChild(renderMarkdown(body.join("\n")));
  out.appendChild(q);
  return j;
}

/**
 * Paragraphe : lignes consécutives jusqu'à une ligne vide ou un autre bloc.
 * @param {string[]} lines
 * @param {number} i
 * @param {Node} out
 * @returns {number}
 */
function readParagraph(lines, i, out) {
  const p = document.createElement("p");
  let j = i;
  while (j < lines.length && lines[j].trim() && !startsBlock(lines[j])) {
    if (j > i) p.appendChild(document.createElement("br"));
    appendInline(p, lines[j].trim());
    j++;
  }
  out.appendChild(p);
  return j === i ? i + 1 : j;
}

/**
 * @param {string} line
 * @returns {boolean}
 */
function startsBlock(line) {
  return FENCE_RE.test(line) || HEADING_RE.test(line) || RULE_RE.test(line) || ITEM_RE.test(line) || QUOTE_RE.test(line);
}

/**
 * Rend un texte Markdown en fragment DOM.
 * @param {string} text
 * @returns {DocumentFragment}
 */
function renderMarkdown(text) {
  const out = document.createDocumentFragment();
  const lines = String(text || "").replace(/\r\n?/g, "\n").split("\n");
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) { i++; continue; }
    if (FENCE_RE.test(line)) { i = readFence(lines, i, out); continue; }
    const h = HEADING_RE.exec(line);
    if (h) {
      out.appendChild(inlineNode(`h${Math.min(h[1].length + 2, 6)}`, h[2]));
      i++;
      continue;
    }
    if (RULE_RE.test(line)) { out.appendChild(document.createElement("hr")); i++; continue; }
    if (ITEM_RE.test(line)) { i = readList(lines, i, out); continue; }
    if (QUOTE_RE.test(line)) { i = readQuote(lines, i, out); continue; }
    i = readParagraph(lines, i, out);
  }
  return out;
}

globalThis.LocarynMarkdown = { splitReasoning, renderMarkdown };
