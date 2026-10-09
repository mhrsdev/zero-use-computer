#!/usr/bin/env node
// Checks the settings panel's translations against English
// (crates/computer-use/src/panel/lang/en.json), with no panel running:
//
//   node scripts/panel-i18n-check.mjs [fa|zh|ru ...]
//
// For each language: every text and setting is there, a `{name}` in the
// English is in the translation (and no other), the plural forms are the
// ones the language uses, and each guide page (help/<page>.<code>.html) has
// the same tags and links as the English one. Exit code 1 on any problem.
import { readFileSync, existsSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const panel = join(dirname(fileURLToPath(import.meta.url)), "../crates/computer-use/src/panel");
const en = JSON.parse(readFileSync(join(panel, "lang/en.json"), "utf8"));
const codes = process.argv.slice(2).length ? process.argv.slice(2) : ["fa", "zh", "ru"];
const FORMS = { fa: ["one", "other"], zh: ["other"], ru: ["one", "few", "many", "other"] };
const names = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(",");
const ticks = (s) => (s.match(/`/g) || []).length;
const tags = (h) => [...h.matchAll(/<\/?[a-zA-Z0-9]+(?:\s[^>]*)?>/g)].map((m) => {
  const href = /href="([^"]*)"/.exec(m[0]);
  const cls = /class="([^"]*)"/.exec(m[0]);
  return m[0].replace(/\s.*>/, ">") + (href ? "@" + href[1] : "") + (cls ? "." + cls[1] : "");
});
let bad = 0;
const say = (code, msg) => { bad++; console.log(`${code}: ${msg}`); };

for (const code of codes) {
  const file = join(panel, `lang/${code}.json`);
  let t;
  try { t = JSON.parse(readFileSync(file, "utf8")); } catch (e) { say(code, `${file}: ${e.message}`); continue; }
  for (const [k, v] of Object.entries(en.ui)) {
    const x = t.ui && t.ui[k];
    if (typeof x !== "string" || !x.trim()) { say(code, `ui: missing ${JSON.stringify(k)}`); continue; }
    if (names(k) !== names(x)) say(code, `ui: {placeholders} differ in ${JSON.stringify(k)} -> ${JSON.stringify(x)}`);
    if (ticks(k) !== ticks(x)) say(code, `ui: backticks differ in ${JSON.stringify(k)}`);
    if (/^\s/.test(k) !== /^\s/.test(x) && false) say(code, `ui: leading space`);
  }
  for (const k of Object.keys(t.ui || {})) if (!(k in en.ui)) say(code, `ui: ${JSON.stringify(k)} is not an English text (a key must be copied exactly)`);
  for (const [k, [label, help]] of Object.entries(en.settings)) {
    const x = t.settings && t.settings[k];
    if (!Array.isArray(x) || x.length !== 2 || !x[0] || !x[1]) { say(code, `settings: missing or not [label, help]: ${k}`); continue; }
    if (names(help) !== names(x[1])) say(code, `settings: {placeholders} differ in ${k}`);
    if (ticks(help) !== ticks(x[1])) say(code, `settings: backticks differ in ${k}`);
  }
  for (const k of Object.keys(t.settings || {})) if (!(k in en.settings)) say(code, `settings: unknown key ${k}`);
  for (const noun of Object.keys(en.plural)) {
    const f = t.plural && t.plural[noun];
    if (!f) { say(code, `plural: missing ${noun}`); continue; }
    for (const form of FORMS[code]) if (!f[form]) say(code, `plural: ${noun} lacks "${form}"`);
    for (const form of Object.keys(f)) if (!FORMS[code].includes(form)) say(code, `plural: ${noun} has "${form}", which ${code} doesn't use`);
  }
  for (const f of readdirSync(join(panel, "help")).filter((f) => /^[a-z]+\.html$/.test(f))) {
    const slug = f.replace(".html", "");
    const tr = join(panel, `help/${slug}.${code}.html`);
    const src = readFileSync(join(panel, "help", f), "utf8");
    if (!existsSync(tr)) { say(code, `help: ${slug}.${code}.html is missing`); continue; }
    const out = readFileSync(tr, "utf8");
    if (out === src) say(code, `help: ${slug}.${code}.html is still the English text`);
    const a = tags(src).join(" "), b = tags(out).join(" ");
    if (a !== b) say(code, `help: ${slug}.${code}.html has other tags or links than the English page`);
    if (/<script|onclick|javascript:/i.test(out)) say(code, `help: ${slug}.${code}.html has a script`);
  }
}
console.log(bad ? `${bad} problem(s)` : "all translations are whole");
process.exit(bad ? 1 : 0);
