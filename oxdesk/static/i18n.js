// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Two languages for what a person reads: English, which is what the code
// writes, and Brazilian Portuguese (specs/022-two-languages.md).
//
// The pages and the daemon keep writing English. This script translates
// what reaches the screen, by looking each text up in a catalog keyed by
// the English text itself:
//
//   - an exact entry: "Measure again" -> "Medir de novo";
//   - a template, for text with values in it: "{} binaries in {}" ->
//     "{} binários em {}". `{}` captures in order; the Portuguese side may
//     reorder with `{0}`, `{1}`.
//
// Anything with no entry stays in English, so a missing translation shows
// up as English, never as a blank. Text a person wrote or a machine
// reported (file and process names, logs, hostnames) is not copy: its
// container carries `data-no-i18n`, and nothing inside is touched.
//
// It must load in <head>, after the catalog (/i18n/catalogs.js) and before
// the page's own scripts: the observer then sees every node as the parser
// inserts it, and the first paint is already translated.
(function () {
  'use strict';
  const KEY = 'oxdesk.lang';
  const LANGS = { en: 'English', 'pt-BR': 'Português' };

  function pick() {
    let saved = null;
    try { saved = localStorage.getItem(KEY); } catch (e) { /* private mode */ }
    if (saved && LANGS[saved]) return saved;
    for (const l of navigator.languages || [navigator.language || 'en']) {
      if (/^pt\b/i.test(l)) return 'pt-BR';
      if (/^en\b/i.test(l)) return 'en';
    }
    return 'en';
  }

  const lang = pick();
  const missing = new Set();
  const api = {
    lang,
    languages: LANGS,
    /** Saves the choice and reloads: translating back to English in place
     *  would need the original of every node, and a reload is exact. */
    set(l) {
      if (!LANGS[l] || l === lang) return;
      try { localStorage.setItem(KEY, l); } catch (e) { /* stays per session */ }
      location.reload();
    },
    t: (s) => s,
    /** The English strings shown with no translation, for the check. */
    missing: () => [...missing].sort(),
  };
  window.oxdeskI18n = api;
  document.documentElement.lang = lang;

  // The switch: any element with data-lang, in either language. It looks
  // like the footer it sits in, and the language in use is marked.
  document.addEventListener('click', (ev) => {
    const a = ev.target.closest && ev.target.closest('[data-lang]');
    if (!a) return;
    ev.preventDefault();
    api.set(a.dataset.lang);
  });
  document.addEventListener('DOMContentLoaded', () => {
    for (const a of document.querySelectorAll('[data-lang]')) {
      if (a.dataset.lang === lang) a.setAttribute('aria-current', 'true');
    }
    const css = document.createElement('style');
    css.textContent =
      '.lang a{color:inherit;text-decoration:none;cursor:pointer;opacity:.75}' +
      '.lang a:hover{opacity:1;text-decoration:underline}' +
      '.lang a[aria-current]{opacity:1;font-weight:600;cursor:default;text-decoration:none}';
    document.head.appendChild(css);
  });
  if (lang === 'en') return;

  const catalog = (window.OXDESK_CATALOG || {})[lang] || {};
  const exact = new Map();
  const templates = [];
  const outputs = new Set(Object.values(catalog));
  const HOLE = /\{\d*\}/g;
  const escape = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  for (const [en, out] of Object.entries(catalog)) {
    if (/\{\d*\}/.test(en)) {
      // A template that is mostly holes ("{}: {}") would match almost any
      // text; scripts/i18n-check.py refuses those, and so does this.
      if ((en.replace(HOLE, '').match(/[A-Za-z]/g) || []).length < 3) continue;
      const parts = en.split(HOLE).map(escape);
      templates.push({ re: new RegExp('^' + parts.join('(.+?)') + '$', 's'), out });
    } else {
      exact.set(en, out);
    }
  }
  // Longer templates first: "{} of {} binaries" must win over "{} binaries".
  templates.sort((a, b) => b.re.source.length - a.re.source.length);

  function fill(out, caps) {
    let i = 0;
    return out.replace(HOLE, (h) => {
      const n = h.length > 2 ? Number(h.slice(1, -1)) : i++;
      return caps[n] !== undefined ? caps[n] : '';
    });
  }

  const DATA = /^[\d\s.,:/%+()~-]*(?:[KMGT]i?B|[kMG]?bps|fps|ms|min|s|h|d)?$/i;
  const cache = new Map();
  function lookup(norm) {
    let hit = cache.get(norm);
    if (hit !== undefined) return hit;
    hit = exact.get(norm);
    if (hit === undefined) {
      for (const t of templates) {
        const m = norm.match(t.re);
        if (m) { hit = fill(t.out, m.slice(1)); break; }
      }
    }
    if (hit === undefined) {
      hit = null;
      // Numbers with a unit ("1.3 MB", "24.0/31.9 GB") are data, not copy,
      // and a page that writes back a label it read gets our own output.
      if (/[A-Za-z]{2,}/.test(norm) && !DATA.test(norm) && !outputs.has(norm)) missing.add(norm);
    }
    // Values change on every poll (uptime, sizes); the cache must not grow
    // with them forever.
    if (cache.size > 4000) cache.clear();
    cache.set(norm, hit);
    return hit;
  }

  function tr(s) {
    if (!s) return s;
    const norm = s.replace(/\s+/g, ' ').trim();
    if (!norm) return s;
    const hit = lookup(norm);
    if (hit === null) return s;
    return s.match(/^\s*/)[0] + hit + s.match(/\s*$/)[0];
  }
  api.t = tr;

  // What this script wrote, so its own writes are not read back as new
  // English (and logged as missing) by the observer.
  const written = new WeakMap();
  const ATTRS = ['title', 'placeholder', 'aria-label'];
  const SKIP = new Set(['SCRIPT', 'STYLE', 'CODE', 'PRE', 'TEXTAREA']);

  function excluded(el) {
    for (let e = el; e && e.nodeType === 1; e = e.parentElement) {
      if (SKIP.has(e.nodeName) || e.hasAttribute('data-no-i18n')) return true;
    }
    return false;
  }

  function text(node) {
    const v = node.nodeValue;
    if (written.get(node) === v) return;
    if (excluded(node.parentElement)) return;
    const out = tr(v);
    if (out !== v) { node.nodeValue = out; written.set(node, out); }
  }

  const writtenAttr = new WeakMap();
  function attrs(el) {
    let mine = writtenAttr.get(el);
    for (const a of ATTRS) {
      const v = el.getAttribute(a);
      if (v === null || (mine && mine[a] === v)) continue;
      const out = tr(v);
      if (out !== v) {
        el.setAttribute(a, out);
        if (!mine) { mine = {}; writtenAttr.set(el, mine); }
        mine[a] = out;
      }
    }
    if (el.nodeName === 'INPUT' && (el.type === 'button' || el.type === 'submit') && el.value) {
      const out = tr(el.value);
      if (out !== el.value) el.value = out;
    }
  }

  function walk(node) {
    if (node.nodeType === 3) return text(node);
    if (node.nodeType !== 1 || excluded(node)) return;
    attrs(node);
    const w = document.createTreeWalker(node, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
      acceptNode: (n) =>
        n.nodeType === 1 && (SKIP.has(n.nodeName) || n.hasAttribute('data-no-i18n'))
          ? NodeFilter.FILTER_REJECT
          : NodeFilter.FILTER_ACCEPT,
    });
    for (let n = w.nextNode(); n; n = w.nextNode()) {
      if (n.nodeType === 3) text(n); else attrs(n);
    }
  }

  new MutationObserver((records) => {
    for (const r of records) {
      if (r.type === 'characterData') text(r.target);
      else if (r.type === 'attributes') { if (!excluded(r.target)) attrs(r.target); }
      else r.addedNodes.forEach(walk);
    }
  }).observe(document.documentElement, {
    subtree: true, childList: true, characterData: true,
    attributes: true, attributeFilter: ATTRS,
  });

  // document.title is not a node the observer sees change.
  const title = document.querySelector('title');
  if (title) new MutationObserver(() => {
    const out = tr(document.title);
    if (out !== document.title) document.title = out;
  }).observe(title, { childList: true, characterData: true, subtree: true });
})();
