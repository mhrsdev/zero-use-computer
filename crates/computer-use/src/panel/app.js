"use strict";
const $ = (id) => document.getElementById(id);
const slug = (s) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
function h(tag, props, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k === "class") el.className = v;
    else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (k in el && k !== "list") el[k] = v;
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids.flat()) if (kid !== null && kid !== undefined && kid !== false) el.append(kid);
  return el;
}
async function post(route, body) {
  const r = await fetch(route, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body || {}) });
  return r.json();
}
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/* ---------- language ----------
   Every text the page shows goes through t(). English is the text itself;
   another language is a table of the English texts and their translations
   (lang/<code>.json). A text with {name} in it takes its values from the
   second argument; a message the server made in English (it has no way to
   know the language) is matched against those same texts, with its numbers
   and names in the places of the {name}s. A text with no translation shows
   in English. */
const LANGUAGES = [["auto", "Auto"], ["en", "English"], ["fa", "فارسی"], ["zh", "中文"], ["ru", "Русский"]];
let L = { code: "en", ui: Object.create(null), settings: Object.create(null), plural: Object.create(null), rx: [] };
const fill = (s, vars) => (vars ? s.replace(/\{(\w+)\}/g, (m, k) => (k in vars ? vars[k] : m)) : s);
// A text made by the server: whole, else sentence by sentence, each by its
// own words or by a template with its numbers and names in place.
function viaTemplate(s) {
  if (L.ui[s] !== undefined) return L.ui[s];
  for (const [re, tpl] of L.rx) {
    const m = re.exec(s);
    if (m) return tpl.replace(/\{(\d+)\}/g, (x, i) => (L.ui[m[+i]] !== undefined ? L.ui[m[+i]] : m[+i]));
  }
  return undefined;
}
function t(s, vars) {
  let r = L.ui[s];
  if (r === undefined && !vars && L.code !== "en") {
    r = viaTemplate(s);
    if (r === undefined && /[.?!]\s/.test(s)) {
      let hit = false;
      const out = s.split(/(?<=[.?!])\s+/).map((p) => { const x = viaTemplate(p); if (x !== undefined) hit = true; return x !== undefined ? x : p; });
      if (hit) r = out.join(" ");
    }
  }
  return fill(r === undefined ? s : r, vars);
}
// "5 settings": the noun in the form the language uses for the number.
function tn(n, noun) {
  const forms = L.plural[noun];
  const form = forms ? forms[new Intl.PluralRules(L.code).select(n)] || forms.other || Object.values(forms)[0] : n === 1 ? noun : noun + "s";
  return n + " " + form;
}
function compile(ui) {
  const rx = [];
  for (const [k, v] of Object.entries(ui)) {
    if (!/\{\w+\}/.test(k)) continue;
    const names = [];
    const source = k.split(/(\{\w+\})/).map((part) => {
      const m = /^\{(\w+)\}$/.exec(part);
      if (!m) return part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      names.push(m[1]); return "(.+?)";
    }).join("");
    let tpl = v; names.forEach((n, i) => { tpl = tpl.split("{" + n + "}").join("{" + (i + 1) + "}"); });
    rx.push([new RegExp("^" + source + "$", "s"), tpl, names.length]);
  }
  return rx.sort((a, b) => b[0].source.length - a[0].source.length);
}
const detect = () => { const l = String(navigator.language || "en").slice(0, 2).toLowerCase(); return ["fa", "zh", "ru"].includes(l) ? l : "en"; };
async function useLanguage(choice) {
  const code = choice === "auto" ? detect() : choice;
  let data = null;
  if (code !== "en") { try { data = await (await fetch("lang/" + code + ".json")).json(); } catch (e) { data = null; } }
  const table = (o) => Object.assign(Object.create(null), o || {});
  L = { code: data ? code : "en", ui: table(data && data.ui), settings: table(data && data.settings), plural: table(data && data.plural), rx: [] };
  L.rx = compile(L.ui);
  const root = document.documentElement;
  root.lang = L.code; root.dir = L.code === "fa" ? "rtl" : "ltr";
  applyStatic();
}
// The texts that are in the page's HTML, not made by the script.
const STATIC_TEXT = [["done", "Done"], ["dlg_no", "Cancel"], ["dlg_yes", "Change"], ["dlg_t", "Change protected settings?"], ["panel_word", "panel"]];
const STATIC_ATTR = [["q", "placeholder", "Search all settings"], ["nav", "aria-label", "Settings groups"], ["themes", "aria-label", "Theme"], ["lang", "aria-label", "Language"], ["lang", "title", "Language"]];
function applyStatic() {
  for (const [id, en] of STATIC_TEXT) { const el = $(id); if (el) el.textContent = (id === "panel_word" ? " " : "") + t(en); }
  for (const [id, attr, en] of STATIC_ATTR) { const el = $(id); if (el) el.setAttribute(attr, t(en)); }
  for (const b of document.querySelectorAll("#themes button")) b.textContent = t({ light: "Light", system: "Auto", dark: "Dark" }[b.dataset.t]);
  const sel = $("lang"); sel.textContent = "";
  for (const [code, name] of LANGUAGES) sel.append(h("option", { value: code, selected: (S ? S.language : "auto") === code }, code === "auto" ? t("Auto") : name));
}

/* ---------- palette ---------- */
function hexToHsl(hex) {
  const r = parseInt(hex.slice(1, 3), 16) / 255, g = parseInt(hex.slice(3, 5), 16) / 255, b = parseInt(hex.slice(5, 7), 16) / 255;
  const mx = Math.max(r, g, b), mn = Math.min(r, g, b); let hh = 0, s = 0; const l = (mx + mn) / 2;
  if (mx !== mn) {
    const d = mx - mn; s = l > .5 ? d / (2 - mx - mn) : d / (mx + mn);
    hh = mx === r ? (g - b) / d + (g < b ? 6 : 0) : mx === g ? (b - r) / d + 2 : (r - g) / d + 4; hh *= 60;
  }
  return [hh, s * 100, l * 100];
}
function applyPalette(accent, dark) {
  const m = /^#[0-9a-f]{6}/i.test(accent || "") ? accent : "#1A73E8";
  const [hh, s0] = hexToHsl(m); const s = Math.max(35, Math.min(90, s0));
  const c = (sat, l) => `hsl(${hh.toFixed(0)} ${sat.toFixed(0)}% ${l}%)`;
  const t = dark ? {
    "--primary": c(s, 78), "--on-primary": c(s, 14), "--primary-container": c(s * .8, 28), "--on-primary-container": c(s, 90),
    "--surface": c(12, 9), "--surface-1": c(10, 13), "--surface-2": c(9, 17), "--surface-3": c(9, 22),
    "--on-surface": c(8, 90), "--on-surface-variant": c(6, 72), "--outline": c(5, 56), "--outline-variant": c(6, 28),
    "--error": "#f2b8b5", "--on-error": "#601410", "--ok": "#7dd99a",
  } : {
    "--primary": c(s, 40), "--on-primary": "#ffffff", "--primary-container": c(Math.min(s, 85), 90), "--on-primary-container": c(s, 14),
    "--surface": c(25, 98), "--surface-1": c(20, 95), "--surface-2": c(18, 92), "--surface-3": c(16, 88),
    "--on-surface": c(10, 10), "--on-surface-variant": c(8, 32), "--outline": c(8, 48), "--outline-variant": c(10, 80),
    "--error": "#b3261e", "--on-error": "#ffffff", "--ok": "#146c2e",
  };
  for (const [k, v] of Object.entries(t)) document.documentElement.style.setProperty(k, v);
}
const mq = window.matchMedia("(prefers-color-scheme: dark)");
const isDark = () => { const t = document.documentElement.dataset.theme; return t === "dark" || (t === "system" && mq.matches); };
function theme(t) {
  document.documentElement.dataset.theme = t;
  applyPalette(document.documentElement.dataset.accent, isDark());
  for (const b of document.querySelectorAll("#themes button")) b.setAttribute("aria-pressed", String(b.dataset.t === t));
}
// A change of colours repaints; it never rebuilds the page (typed text stays).
mq.addEventListener("change", () => theme(document.documentElement.dataset.theme));

/* ---------- state ---------- */
let schema = null, S = null, ready = false, page = "overview", snackTimer = 0, overview = null;
const E = {}; // key -> entry
const view = { query: "", changedOnly: false, advanced: false };

function snack(text, bad) {
  const el = $("snack"); el.textContent = t(text); el.className = "show" + (bad ? " bad" : "");
  clearTimeout(snackTimer); snackTimer = setTimeout(() => { el.className = ""; }, bad ? 6000 : 2400);
}
const val = (key) => { const e = E[key]; return e ? (key in S.values ? S.values[key] : e.default) : undefined; };
const changed = (e) => e.key in S.values && !same(S.values[e.key], e.default);
function labelOf(e) {
  const tr = L.settings[e.key];
  if (tr && tr[0]) return tr[0];
  const k = e.key.split(".").pop().replace(/_/g, " "); return k.charAt(0).toUpperCase() + k.slice(1);
}
const helpOf = (e) => { const tr = L.settings[e.key]; return tr && tr[1] ? tr[1] : e.help; };
function shortKey(k) { return (E[k] ? labelOf(E[k]) : k) + " (" + k + ")"; }

function confirmDialog(keys, text) {
  return new Promise((resolve) => {
    const body = $("dlg_b"); body.textContent = "";
    if (text) body.append(h("div", {}, t(text)));
    else body.append(h("div", {}, t("These settings limit what the agent can do or see, or how the program updates. Check that you meant to change them:")),
      h("ul", {}, keys.map((k) => h("li", {}, shortKey(k)))));
    const dlg = $("dlg"); const done = (v) => { dlg.close(); resolve(v); };
    $("dlg_yes").onclick = () => done(true); $("dlg_no").onclick = () => done(false);
    dlg.oncancel = () => resolve(false);
    dlg.showModal(); $("dlg_no").focus();
  });
}

// Calls a route; when the server says the change needs a yes, asks and repeats.
async function ask(route, body) {
  let r = await post(route, { ...body, confirmed: false });
  if (!r.ok && (r.confirm || r.confirm_text)) {
    if (!(await confirmDialog(r.confirm, r.confirm_text))) return { ok: false, cancelled: true };
    r = await post(route, { ...body, confirmed: true });
  }
  return r;
}
function take(r) {
  for (const [k, v] of Object.entries(r.values || {})) {
    const e = E[k]; if (!e) continue;
    if (same(v, e.default)) delete S.values[k]; else S.values[k] = v;
  }
  if (r.theme) { document.documentElement.dataset.accent = r.accent; S.theme = r.theme; theme(r.theme); }
}
async function change(pairs, okText) {
  const r = await ask("set", { changes: pairs.map(([key, value]) => ({ key, value })) });
  if (r.cancelled) { render(); return false; }
  if (!r.ok) { snack(r.error || "Not saved.", true); render(); return false; }
  take(r); snack(okText || "Saved. Running servers use it now.");
  if (r.language && r.language !== S.language) { S.language = r.language; await useLanguage(r.language); }
  render(); return true;
}
async function reset(keys) {
  const r = await ask("reset", { keys });
  if (r.cancelled) return;
  if (!r.ok) return snack(r.error || "Not reset.", true);
  take(r);
  if (r.language && r.language !== S.language) { S.language = r.language; await useLanguage(r.language); }
  snack("Back to the default."); render();
}

/* ---------- controls ---------- */
function control(e) {
  const id = "c_" + e.key.replace(/\./g, "_"), v = val(e.key);
  const commit = (x) => change([[e.key, x]]);
  switch (e.type) {
    case "bool": {
      const i = h("input", { type: "checkbox", id, checked: !!v, role: "switch", "aria-label": labelOf(e), onchange: () => commit(i.checked) });
      return h("label", { class: "switch" }, i, h("span"));
    }
    case "choice": {
      const name = (c) => (e.key === "panel.language" && c !== "auto" ? (LANGUAGES.find((l) => l[0] === c) || [0, c])[1] : c);
      if (e.choices.length <= 3) return h("div", { class: "seg", role: "group", "aria-label": labelOf(e) },
        e.choices.map((c) => h("button", { "aria-pressed": String(v === c), onclick: () => v !== c && commit(c) }, name(c))));
      return h("select", { id, "aria-label": labelOf(e), onchange: (ev) => commit(ev.target.value) },
        e.choices.map((c) => h("option", { value: c, selected: v === c }, name(c))));
    }
    case "range": {
      const num = h("input", { type: "number", id, min: e.min, max: e.max, step: e.step, value: v });
      const rng = h("input", { type: "range", min: e.min, max: e.max, step: e.step, value: v, "aria-label": t("{name} slider", { name: labelOf(e) }) });
      rng.oninput = () => { num.value = rng.value; };
      rng.onchange = () => commit(Number(rng.value));
      num.onchange = () => { if (num.value === "" || isNaN(Number(num.value))) return render(); rng.value = num.value; commit(Number(num.value)); };
      return [rng, num, e.unit && h("span", { class: "unit" }, t(e.unit))];
    }
    case "int": case "float": {
      const num = h("input", { type: "number", id, step: e.type === "float" ? "any" : "1", value: v });
      num.onchange = () => { if (num.value === "" || isNaN(Number(num.value))) return render(); commit(Number(num.value)); };
      return [num, e.unit && h("span", { class: "unit" }, t(e.unit))];
    }
    case "color": {
      const text = h("input", { type: "text", id, value: v, spellcheck: false, size: 10, style: "width:120px", dir: "ltr" });
      const pick = h("input", { type: "color", value: /^#[0-9a-f]{6}/i.test(v) ? v.slice(0, 7) : "#000000", "aria-label": t("{name} picker", { name: labelOf(e) }) });
      pick.onchange = () => commit(pick.value.toUpperCase());
      text.onchange = () => commit(text.value.trim());
      return [pick, text];
    }
    case "list": {
      const box = h("div", { class: "chips" });
      for (const item of v) box.append(h("span", { class: "tag" }, item,
        h("button", { "aria-label": t("Remove {item}", { item }), title: t("Remove"), onclick: () => commit(v.filter((x) => x !== item)) }, "×")));
      const add = h("input", { type: "text", placeholder: t("Add and press Enter"), "aria-label": t("Add to {name}", { name: labelOf(e) }), spellcheck: false });
      add.onkeydown = (ev) => { if (ev.key === "Enter" && add.value.trim()) { ev.preventDefault(); commit([...v.filter((x) => x !== add.value.trim()), add.value.trim()]); } };
      box.append(add); return box;
    }
    case "secret": {
      const i = h("input", { type: "password", id, autocomplete: "off", spellcheck: false, placeholder: v ? t("Saved {hint}. Type to replace.", { hint: v }) : t("Not set") });
      i.onchange = async () => { if (!i.value) return; const x = i.value; i.value = ""; await commit(x); };
      return i;
    }
    default: {
      const i = h("input", { type: "text", id, value: v, spellcheck: false, autocomplete: "off", dir: "ltr", placeholder: e.type === "hotkey" ? t("for example ctrl+alt+escape") : "" });
      i.onchange = () => commit(i.value);
      return i;
    }
  }
}

function row(e) {
  const isChanged = changed(e);
  const def = e.type === "secret" ? t("none") : Array.isArray(e.default) ? (e.default.join(", ") || t("empty")) : (String(e.default) === "" ? t("empty") : String(e.default));
  return h("div", { class: "row" },
    h("div", { class: "text" },
      h("label", { class: "label", for: "c_" + e.key.replace(/\./g, "_") }, labelOf(e)),
      h("div", { class: "help" }, helpOf(e)),
      h("div", { class: "meta" }, h("code", {}, e.key),
        isChanged && h("span", { class: "badge changed" }, t("changed")),
        e.restart && h("span", { class: "badge" }, t("applies after a restart")),
        e.confirm && h("span", { class: "badge warn" }, t("asks to confirm")),
        e.advanced && h("span", { class: "badge" }, t("advanced")),
        isChanged && h("span", {}, t("default:") + " ", h("code", {}, def)))),
    h("div", { class: "ctl" }, control(e), isChanged && e.type !== "secret" && h("button", { class: "btn small", onclick: () => reset([e.key]), title: t("Back to the default") }, t("Reset"))));
}

/* ---------- previews ---------- */
const POINTER_NAMES = ["crystal", "paper", "jelly", "ice", "metal", "orbit"];

function pointerPicker() {
  const e = E["overlay.cursor_style"], v = val(e.key);
  const cell = (name, glyph, text) => h("button", { class: "pick", "aria-pressed": String(v === name), title: text ? t(text) : name, onclick: () => v !== name && change([[e.key, name]]) },
    glyph ? h("span", { class: "glyph", "aria-hidden": "true" }, glyph) : h("img", { src: "cursor/" + name + ".png", alt: "" }), h("span", {}, name));
  return h("div", { class: "card" }, h("h3", {}, t("Pointer style")),
    h("div", { class: "picker", role: "group", "aria-label": t("Pointer style") },
      cell("random", "?", "A different one each session, and for each agent at once"), cell("classic", "➤", "The plain arrow"), POINTER_NAMES.map((n) => cell(n))));
}

const STYLES = ["mixed", "hand", "sine", "arc", "spring", "spiral"];
function pathPreview(key, who) {
  const e = E[key], v = val(key);
  const canvas = h("canvas", { width: 960, height: 480, role: "img", "aria-label": t("Three sample paths of the {style} style", { style: v }) });
  const info = h("div", { class: "help", role: "status" });
  const draw = async () => {
    const r = await post("path", { style: v, real: key === "mouse_path" });
    if (!r.ok) return;
    const ctx = canvas.getContext("2d"), cs = getComputedStyle(document.documentElement);
    const k = canvas.width / 480; ctx.clearRect(0, 0, canvas.width, canvas.height);
    const cols = [cs.getPropertyValue("--primary"), cs.getPropertyValue("--on-surface-variant"), cs.getPropertyValue("--error")];
    r.paths.forEach((p, i) => {
      ctx.strokeStyle = cols[i % 3]; ctx.lineWidth = 2.4 * k / 2; ctx.lineJoin = "round"; ctx.globalAlpha = .9; ctx.beginPath();
      p.points.forEach(([x, y], j) => (j ? ctx.lineTo(x * k, y * k) : ctx.moveTo(x * k, y * k))); ctx.stroke();
      if (r.jumps && p.points.length) {
        // The jump: no path, the pointer is simply there.
        ctx.setLineDash([4 * k / 2, 6 * k / 2]); ctx.globalAlpha = .35; ctx.beginPath();
        ctx.moveTo(r.from[0] * k, r.from[1] * k); ctx.lineTo(p.points[0][0] * k, p.points[0][1] * k); ctx.stroke(); ctx.setLineDash([]);
      }
    });
    ctx.globalAlpha = 1; ctx.fillStyle = cs.getPropertyValue("--outline");
    ctx.beginPath(); ctx.arc(r.from[0] * k, r.from[1] * k, 5 * k / 2, 0, 7); ctx.fill();
    ctx.fillStyle = cs.getPropertyValue("--primary"); ctx.beginPath(); ctx.arc(r.to[0] * k, r.to[1] * k, 7 * k / 2, 0, 7); ctx.fill();
    info.textContent = r.paths.map((p) => p.style + " " + Math.round(p.ms) + " ms").join(" · ") + ". " + t("Each press makes new ones: no two hands are the same.");
  };
  draw();
  return h("div", { class: "card" }, h("h3", {}, t(who)),
    h("div", { class: "preview" },
      h("div", { class: "chips2", role: "group", "aria-label": t("Path style") }, STYLES.map((s) => h("button", { class: "chip", "aria-pressed": String(v === s), onclick: () => v !== s && change([[key, s]]) }, s)),
        h("button", { class: "chip", onclick: draw }, t("Another set"))),
      canvas, info,
      key === "mouse_path" && h("div", { class: "note pad0" }, t("The dashed line is a jump: the real mouse goes at once to 70 to 140 px from the target, travels only that last stretch, then goes back to where you left it. The pointer in the overlay glides the whole way."))));
}

const STATES = [["thinking", "Thinking"], ["working", "Working"], ["done", "Done"], ["error", "Error"], ["paused", "Paused"], ["stopped", "Stopped"]];
let overlayState = "working";
function overlayPreview() {
  const colour = val("overlay.color_" + overlayState), width = val("overlay.border_width"), glow = val("overlay.glow_size");
  const label = String(val("overlay.label_" + overlayState)).replace("{hotkey}", (overview && overview.stop_key) || "the stop key");
  const style = val("overlay.cursor_style"), shown = POINTER_NAMES.includes(style) ? style : "crystal";
  const whole = val("overlay.border_target") === "screen";
  const scale = 0.4, g = Math.round(glow * scale), w = Math.max(1, Math.round(width * scale));
  const ring = h("div", { class: "ring", style: `${whole ? "inset:0" : "left:14%;top:18%;width:62%;height:62%"};border:${w}px solid ${colour};box-shadow:0 0 ${g}px ${Math.round(g / 3)}px ${colour},inset 0 0 ${g}px ${colour};border-radius:${whole ? 10 : 6}px;opacity:${val("overlay.show_border") ? 1 : 0}` });
  return h("div", { class: "card" }, h("h3", {}, t("Preview")),
    h("div", { class: "preview" },
      h("div", { class: "chips2", role: "group", "aria-label": t("State") }, STATES.map(([s, text]) => h("button", { class: "chip", "aria-pressed": String(overlayState === s), onclick: () => { overlayState = s; render(); } }, t(text)))),
      h("div", { class: "mock" }, h("div", { class: "win" }, h("i"), h("i"), h("i")), ring,
        val("overlay.show_label") && h("div", { class: "pill" }, h("b", { style: "background:" + colour }), label),
        val("overlay.show_cursor") && h("div", { class: "ptr" }, h("img", { src: "cursor/" + shown + ".png", alt: "" }), val("overlay.cursor_tag") && h("span", { style: "background:" + val("overlay.cursor_color") }, val("overlay.cursor_tag")))),
      h("div", { class: "note pad0" }, t("Drawn from your settings, to show how they fit together. The real overlay is click-through and left out of the agent's screenshots."))));
}

function tokenCard() {
  const cost = (px) => Math.ceil(px * Math.round(px * 9 / 16) / 750);
  const at = (px) => t("{px} px → about {tokens} tokens", { px: px.toLocaleString("en"), tokens: cost(px).toLocaleString("en") });
  const mx = val("screenshot.max_dimension"), ov = val("screenshot.overview_max_dimension"), base = cost(E["screenshot.max_dimension"].default);
  const diff = (px) => { const d = Math.round((cost(px) / base - 1) * 100); return d === 0 ? "" : " " + t("({d}% against the default)", { d: (d > 0 ? "+" : "") + d }); };
  return h("div", { class: "card" }, h("h3", {}, t("What a picture costs")),
    h("ul", { class: "facts" },
      h("li", {}, h("span", { class: "k" }, t("A whole screen")), h("span", { class: "v" }, at(mx) + diff(mx))),
      ov > 0 && h("li", {}, h("span", { class: "k" }, t("An overview")), h("span", { class: "v" }, at(ov) + diff(ov)))),
    h("div", { class: "note" }, t("A picture counts as width × height ÷ 750 tokens, here for a 16:9 screen; text counts as four characters a token. An estimate: your model counts its own way, and the prompt cache makes repeats cheaper.")));
}

/* ---------- decision card ---------- */
function decisionCard() {
  const D = S.decision; const st = h("div", { class: "msg", role: "status" });
  const say = (text, ok) => { st.textContent = t(text); st.className = "msg " + (ok === undefined ? "" : ok ? "ok" : "bad"); };
  const base = { jev: "https://api.typesafe.ai", openai: "https://api.openai.com/v1" };
  const models = { jev: "jev-latest", openai: t("e.g. gpt-4.1-nano, llama-3.1-8b-instant") };
  const radios = ["jev", "openai"].map((v) => h("input", { type: "radio", name: "provider", value: v, checked: D.provider === v }));
  const kind = (title, text, r) => h("label", { class: "kind" }, r, h("div", {}, h("b", {}, t(title)), h("span", {}, t(text))));
  const url = h("input", { type: "url", id: "d_url", value: D.base_url, autocomplete: "off", spellcheck: false });
  const model = h("input", { type: "text", id: "d_model", value: D.model, autocomplete: "off", spellcheck: false, dir: "ltr" });
  const key = h("input", { type: "password", id: "d_key", autocomplete: "off", spellcheck: false });
  const keyHint = h("div", { class: "hint" }), baseHint = h("div", { class: "hint" });
  const prov = () => (radios.find((r) => r.checked) || {}).value || "";
  const savedHint = () => (D.key_hint ? t("Saved: {hint}. Leave empty to keep it.", { hint: D.key_hint }) : "");
  const refresh = () => { const p = prov() || "jev"; url.placeholder = base[p]; model.placeholder = models[p]; baseHint.textContent = t("Leave empty for {url}.", { url: base[p] }); };
  keyHint.textContent = D.key_hint ? savedHint() : D.key_env ? t("Read from the environment variable {name}.", { name: D.key_env }) : t("Not needed for a server on this computer.");
  for (const r of radios) r.onchange = () => { url.value = ""; model.value = ""; refresh(); };
  refresh();
  const form = () => ({ provider: prov(), base_url: url.value, model: model.value, api_key: key.value });
  const buttons = [];
  const busy = (on) => buttons.forEach((b) => { b.disabled = on; });
  const run = (fn) => async () => { busy(true); try { await fn(); } catch (err) { say(t("The server didn't answer: {error}", { error: err }), false); } busy(false); };
  buttons.push(
    h("button", { class: "btn filled", onclick: run(async () => {
      if (!prov()) return say("Choose a kind of model first.", false);
      say("Saving…"); const r = await post("save", form());
      if (r.ok) { key.value = ""; S.decision = (await post("state")).decision; Object.assign(D, S.decision); keyHint.textContent = savedHint(); }
      say(r.ok ? r.message : r.error, r.ok);
    }) }, t("Save")),
    h("button", { class: "btn", onclick: run(async () => {
      if (!prov()) return say("Choose a kind of model first.", false);
      say("Asking the model…"); const r = await post("test", form()); say(r.ok ? r.message : r.error, r.ok);
    }) }, t("Test")),
    h("button", { class: "btn danger", onclick: run(async () => {
      if (!confirm(t("Remove the decision model and its key?"))) return;
      const r = await post("remove");
      if (r.ok) { radios.forEach((x) => { x.checked = false; }); url.value = model.value = key.value = ""; keyHint.textContent = ""; S.decision = (await post("state")).decision; Object.assign(D, S.decision); refresh(); }
      say(r.ok ? r.message : r.error, r.ok);
    }) }, t("Remove")));
  return h("div", { class: "card pad" }, h("h3", {}, t("Model and API key")),
    h("p", { class: "help" }, t("A fast model the agent asks yes/no, choice and score questions about what is on screen, so it reads less and decides sooner. Your key is saved on this computer and never goes through the chat.")),
    h("div", { class: "kinds" },
      kind("Jev (TypeSafe System One)", "Made for decisions: answers in well under a second, with probabilities. Also any server that speaks the same API.", radios[0]),
      kind("OpenAI-compatible chat model", "OpenAI, Groq, Cerebras, OpenRouter, Ollama, LM Studio… A small, fast model works best.", radios[1])),
    h("div", { class: "field" }, h("label", { for: "d_url" }, t("Address (base URL)")), url, baseHint),
    h("div", { class: "field" }, h("label", { for: "d_model" }, t("Model")), model),
    h("div", { class: "field" }, h("label", { for: "d_key" }, t("API key")), key, keyHint),
    h("div", { class: "actions" }, buttons), st);
}

/* ---------- pages ---------- */
const PAGES = []; // {id, title, section, render(main), count}
const page_ = (id, title, section, render) => PAGES.push({ id, title, section, render });

function ago(secs) {
  if (secs == null) return t("never");
  if (secs < 90) return t("just now");
  if (secs < 5400) return t("{x} ago", { x: tn(Math.round(secs / 60), "minute") });
  if (secs < 129600) return t("{x} ago", { x: tn(Math.round(secs / 3600), "hour") });
  return t("{x} ago", { x: tn(Math.round(secs / 86400), "day") });
}

async function overviewPage(main) {
  const o = overview = await post("overview");
  if (!o.ok) return main.append(h("div", { class: "empty" }, t(o.error)));
  const on = (b, yes, no) => h("span", {}, h("span", { class: "dot " + (b ? "on" : "off") }), b ? yes : no);
  const style = val("overlay.cursor_style"), mx = val("screenshot.max_dimension");
  const u = o.update;
  const waits = { restart: "the computer to restart", start: "the next start", manual: "you to run `computer-use-mcp update --install`" };
  const update = !u.enabled ? t("Off. It never updates by itself.") : u.pending
    ? t("Version {v} is downloaded and waits for {when}.", { v: u.pending, when: t(waits[u.install] || "a restart") })
    : (u.last_check_secs_ago == null ? t("On; it has not looked yet.") : t("On, last looked {when}.", { when: ago(u.last_check_secs_ago) }));
  const nChanged = Object.keys(S.values).length;
  main.append(
    h("div", { class: "grid2" },
      h("div", { class: "tile" }, h("div", { class: "small" }, t("Version")), h("div", { class: "big ltr" }, o.version), h("div", { class: "small ltr" }, o.platform)),
      h("div", { class: "tile" }, h("div", { class: "small" }, t("Emergency stop")), h("div", { class: "big ltr" }, o.stop_key || t("none")), h("div", { class: "small" }, o.settings_key ? [t("This panel:") + " ", h("span", { class: "ltr" }, o.settings_key)] : t("No key for this panel"))),
      h("div", { class: "tile" }, h("div", { class: "small" }, t("Settings changed")), h("div", { class: "big" }, String(nChanged)), h("div", { class: "small" }, t("of {n}", { n: schema.entries.length })))),
    h("div", { class: "card" }, h("h3", {}, t("Right now")),
      h("ul", { class: "facts" },
        h("li", {}, h("span", { class: "k" }, t("Pointer")), h("span", { class: "v" }, style === "random" ? t("A random one each session, and a different one for each agent at once.") : style === "classic" ? t("The plain arrow.") : t("Always {style}.", { style }))),
        h("li", {}, h("span", { class: "k" }, t("Pictures")), h("span", { class: "v" }, o.screenshots ? t("At most {px} px on the long side (about {tokens} tokens for a whole 16:9 screen), {attach} attached.", { px: mx, tokens: Math.ceil(mx * Math.round(mx * 9 / 16) / 750).toLocaleString("en"), attach: val("screenshot.attach") }) : t("Off: the agent works from the tree alone."))),
        h("li", {}, h("span", { class: "k" }, t("Several agents")), h("span", { class: "v" }, o.hub.enabled ? on(o.hub.running, t("A hub is running on port {port}.", { port: o.hub.port }), t("On; no hub is running right now.")) : on(false, "", t("Off.")))),
        h("li", {}, h("span", { class: "k" }, t("Decision model")), h("span", { class: "v" }, o.decision || t("None set up."))),
        h("li", {}, h("span", { class: "k" }, t("Tools")), h("span", { class: "v" }, t("{x} allowed by the settings.", { x: tn(o.tools, "tool") }))),
        h("li", {}, h("span", { class: "k" }, t("Updates")), h("span", { class: "v" }, update)),
        h("li", {}, h("span", { class: "k" }, t("Audit log")), h("span", { class: "v" }, o.audit ? t("On.") : t("Off."))),
        h("li", {}, h("span", { class: "k" }, t("Program")), h("span", { class: "v ltr" }, o.exe)),
        h("li", {}, h("span", { class: "k" }, t("Settings file")), h("span", { class: "v" }, o.settings_file ? [h("span", { class: "ltr" }, o.settings_file), o.settings_file_exists ? "" : " " + t("(not made yet: the first change makes it)")] : t("None: this server keeps its settings in memory."))))),
    h("div", { class: "card pad" }, h("h3", {}, t("Where to start")),
      h("p", { class: "help" }, t("If the agent spends too much, try the Low tokens profile. If it can't read an app, try Best quality. To record a demo, Showcase makes the pointer and border bigger.")),
      h("div", { class: "actions" }, h("button", { class: "btn filled", onclick: () => go("profiles") }, t("Profiles")), h("button", { class: "btn", onclick: () => go("pointer") }, t("Choose a pointer")), h("button", { class: "btn", onclick: () => go("tools-and-tokens") }, t("Tools and tokens")))));
}

async function profilesPage(main) {
  const r = await post("profiles");
  if (!r.ok) return main.append(h("div", { class: "empty" }, t(r.error)));
  const apply = async (p) => {
    const x = await ask("profile_apply", { id: p.id });
    if (x.cancelled) return;
    if (!x.ok) return snack(x.error, true);
    take(x); S = { ...S, values: (await post("state")).values }; snack(t("{name} applied.", { name: t(p.label) })); render();
  };
  const list = (items) => items.map((p) => h("div", { class: "profile" },
    h("div", { class: "text" }, h("div", { class: "label" }, p.builtin ? t(p.label) : p.label), h("div", { class: "help" }, p.builtin ? t(p.blurb) : p.blurb),
      h("div", { class: "meta" }, p.differs ? t("{x} would change", { x: tn(p.differs, "setting") }) : t("Already applied"), " · ", t("{x} in it", { x: tn(p.settings, "setting") }))),
    h("button", { class: "btn", disabled: !p.differs, onclick: () => apply(p) }, t("Apply")),
    !p.builtin && h("button", { class: "btn danger", onclick: async () => { if (!confirm(t("Delete the profile “{name}”?", { name: p.label }))) return; const d = await post("profile_delete", { id: p.id }); if (d.ok) render(); else snack(d.error, true); } }, t("Delete"))));
  const builtin = r.profiles.filter((p) => p.builtin), mine = r.profiles.filter((p) => !p.builtin);
  const name = h("input", { type: "text", placeholder: t("Name, for example Work laptop"), "aria-label": t("Profile name"), maxlength: 60 });
  main.append(h("div", { class: "card" }, h("h3", {}, t("Comes with the program")), list(builtin),
      h("div", { class: "note" }, t("A profile only sets the settings it names and puts back the defaults of the ones another profile sets. It never touches protected settings or secrets."))),
    h("div", { class: "card" }, h("h3", {}, t("Yours")), mine.length ? list(mine) : h("div", { class: "note" }, t("None yet. Change some settings, then keep them here under a name."))),
    h("div", { class: "card pad" }, h("h3", {}, t("Keep the current settings as a profile")),
      h("div", { class: "actions" }, name, h("button", { class: "btn filled", onclick: async () => {
        const x = await post("profile_save", { label: name.value });
        if (!x.ok) return snack(x.error, true);
        snack(x.left_out ? t("Kept {x}. Left out {n} that a profile may not hold (secrets, protected settings, ports).", { x: tn(x.settings, "setting"), n: x.left_out }) : t("Kept {x}.", { x: tn(x.settings, "setting") }));
        render();
      } }, t("Keep")))));
}

async function toolsPage(main) {
  const r = await post("tools");
  if (!r.ok) return main.append(h("div", { class: "empty" }, t(r.error)));
  const dis = val("tools.disabled"), only = val("tools.enabled");
  const total = r.tools.filter((x) => x.offered).reduce((a, x) => a + x.tokens, 0);
  const set = (name, on) => change([["tools.disabled", on ? dis.filter((x) => x.toLowerCase() !== name) : [...dis, name]]]);
  main.append(h("div", { class: "card" },
    only.length > 0 && h("div", { class: "note" }, t("Only {n} tools are listed in tools.enabled, so the others stay hidden whatever the switches say.", { n: only.length })),
    r.tools.map((x) => h("div", { class: "toolrow" },
      h("label", { class: "switch" }, h("input", { type: "checkbox", role: "switch", checked: !x.disabled, "aria-label": x.name, onchange: (ev) => set(x.name, ev.target.checked) }), h("span")),
      h("span", { class: "name" }, x.name), h("span", { class: "cost" }, "~" + x.tokens + " tok"))),
    h("div", { class: "note" }, t("The tool list is sent with every request, so what is on costs about {tokens} tokens each time{manager}. Costs are the descriptions in use ({descriptions}), counted as four characters a token.", {
      tokens: total.toLocaleString("en"),
      manager: r.manager === "dispatch" ? t(", before the tool manager hides most of them until the model asks (tools.manager is dispatch)") : "",
      descriptions: val("tools.descriptions") }))));
}

async function appsPage(main) {
  const r = await post("apps");
  if (!r.ok) return main.append(h("div", { class: "empty" }, t(r.error)));
  if (!r.apps.length) return main.append(h("div", { class: "card pad" }, h("p", { class: "help" }, t("Nothing counted yet. Each time the agent looks at an app, the program notes how much its accessibility tree told it and whether it had to use pixels; the apps that needed pixels most show here. It is kept in {file} (screenshot.record_apps).", { file: r.file }))));
  main.append(h("div", { class: "card scroll" }, h("table", { class: "table" },
    h("thead", {}, h("tr", {}, ["App", "Looks", "Little in the tree", "Text read off screen", "Pictures sent", "Left out"].map((x, i) => h("th", { class: i ? "num" : "" }, t(x))))),
    h("tbody", {}, r.apps.map((a) => h("tr", {}, h("td", {}, a.app), h("td", { class: "num" }, a.looks), h("td", { class: "num" }, a.little_tree + " (" + a.share_little + "%)"), h("td", { class: "num" }, a.text_read), h("td", { class: "num" }, a.pictures), h("td", { class: "num" }, a.pictures_left_out)))))),
    h("div", { class: "note" }, t("Apps with a high share in the third column are the ones the tree serves badly: they get text read off the screen and more pictures. Kept in {file}.", { file: r.file })));
}

async function auditPage(main) {
  const r = await post("audit", { lines: 200 });
  if (!r.ok) return main.append(h("div", { class: "empty" }, t(r.error)));
  const fmt = (l) => { try { const j = JSON.parse(l); const ts = j.ts ? new Date(j.ts * 1000).toLocaleTimeString() + "  " : ""; return ts + Object.entries(j).filter(([k]) => k !== "ts").map(([k, v]) => k + "=" + (typeof v === "string" ? v : JSON.stringify(v))).join("  "); } catch (e) { return l; } };
  main.append(h("div", { class: "card" },
    h("div", { class: "row" }, h("div", { class: "text" }, h("div", { class: "label" }, t("Write the audit log")), h("div", { class: "help" }, t("A line for each call: the tool, the app and how it ended. Never the arguments, the tree or a picture.")), h("div", { class: "meta" }, h("code", {}, r.file))),
      h("div", { class: "ctl" }, control(E["audit.enabled"]))),
    r.lines.length ? h("pre", { class: "log", tabindex: 0, "aria-label": t("Audit log") }, r.lines.map(fmt).join("\n")) : h("div", { class: "note" }, r.exists ? t("The file is empty.") : r.enabled ? t("Nothing written yet.") : t("No log yet: turn it on above.")),
    r.lines.length > 0 && h("div", { class: "note" }, t("The last {n} lines", { n: r.lines.length }) + (r.size ? " " + t("of {kb} KB", { kb: (r.size / 1024).toFixed(0) }) : "") + ". ", h("button", { class: "btn small", onclick: () => render() }, t("Refresh")))));
}

async function filePage(main) {
  const r = await post("raw_get");
  if (!r.ok) return main.append(h("div", { class: "empty" }, t(r.error)));
  const area = h("textarea", { spellcheck: false, "aria-label": t("Settings file"), value: r.text });
  const st = h("div", { class: "msg", role: "status" });
  const say = (text, ok) => { st.textContent = t(text); st.className = "msg " + (ok === undefined ? "" : ok ? "ok" : "bad"); };
  let timer = 0;
  const check = async () => {
    const x = await post("raw_check", { text: area.value });
    if (!x.ok) return say(x.error, false);
    say(x.changed.length ? (x.protected.length ? t("Valid. {x} would change, {n} of them protected.", { x: tn(x.changed.length, "setting"), n: x.protected.length }) : t("Valid. {x} would change.", { x: tn(x.changed.length, "setting") })) : t("Valid. Nothing would change."), true);
  };
  area.oninput = () => { clearTimeout(timer); timer = setTimeout(check, 500); };
  main.append(h("div", { class: "card pad" },
    r.broken && h("p", { class: "msg bad" }, t("The file as saved has a mistake in it; mend it here.")),
    h("p", { class: "help" }, t("The settings file as text, comments and all. Secrets are covered; leave them as they are to keep them. It is checked as a whole before anything is written, so a typo can't break the file.")),
    area, h("div", { class: "actions" },
      h("button", { class: "btn filled", onclick: async () => {
        clearTimeout(timer); // a check still to come mustn't hide what the save says
        const x = await ask("raw_save", { text: area.value, hash: r.hash });
        if (x.cancelled) return;
        if (!x.ok) return say(x.error, false);
        snack("Saved. Running servers use it now."); const s = await post("state"); S.values = s.values; render();
      } }, t("Save")),
      h("button", { class: "btn", onclick: check }, t("Check")), h("button", { class: "btn", onclick: () => render() }, t("Revert"))), st,
    h("div", { class: "note pad0" }, t("File:") + " ", h("span", { class: "ltr" }, r.path))));
}

/* The import page: what an import would do is shown first, setting by
   setting (now, and after it), with the ones this version can't take marked
   as such; the settings from before an import are kept, to go back to. */
async function importPage(main) {
  const x = await post("export");
  const out = h("textarea", { class: "short", readOnly: true, "aria-label": t("Your settings"), value: x.ok ? x.text : "" });
  const inp = h("textarea", { class: "short", spellcheck: false, placeholder: "[screenshot]\nmax_dimension = 1024", "aria-label": t("Settings to import") });
  const shownValue = (v) => (v === "" || v === null || v === undefined ? "—" : Array.isArray(v) ? (v.join(", ") || "—") : typeof v === "boolean" ? (v ? t("on") : t("off")) : String(v));
  const preview = h("div", { class: "preview-box" });
  const go_ = h("button", { class: "btn filled", disabled: true }, t("Import"));
  const hideSame = { on: true };
  let plan = null, timer = 0, token = 0;
  const showPlan = (p, err) => {
    preview.textContent = ""; plan = p && p.ok ? p : null;
    go_.disabled = true; go_.textContent = t("Import");
    if (err) return preview.append(h("div", { class: "msg bad" }, t(err)));
    if (!p) return;
    const c = p.counts, left = c.unsupported + c.invalid, apply = c.change + c.protected;
    if (p.exported_by && p.exported_by !== p.this_version) preview.append(h("div", { class: "help" }, t("Made by version {v}; this is version {w}.", { v: p.exported_by, w: p.this_version })));
    preview.append(h("div", { class: "chips2", style: "margin:8px 0" },
      h("span", { class: "badge changed" }, t("{n} to change", { n: apply })),
      c.protected > 0 && h("span", { class: "badge warn" }, t("{n} ask to confirm", { n: c.protected })),
      left > 0 && h("span", { class: "badge warn" }, t("{n} this version can't take", { n: left })),
      c.same > 0 && h("button", { class: "chip", "aria-pressed": String(!hideSame.on), onclick: () => { hideSame.on = !hideSame.on; showPlan(p); } }, t("{n} already the same", { n: c.same }))));
    const rows = p.rows.filter((r) => !(hideSame.on && r.status === "same"));
    const badge = { change: ["changed", "changed"], protected: ["asks to confirm", "warn"], same: ["same", ""], unsupported: ["not in this version", "warn"], invalid: ["can't take it", "warn"] };
    if (rows.length) preview.append(h("div", { class: "scroll" }, h("table", { class: "table" },
      h("thead", {}, h("tr", {}, [t("Setting"), t("Now"), t("After the import"), ""].map((x) => h("th", {}, x)))),
      h("tbody", {}, rows.map((r) => {
        const e = E[r.key], [text, cls] = badge[r.status], off = r.status === "unsupported" || r.status === "invalid";
        return h("tr", { class: off ? "off" : "" },
          h("td", {}, e && h("div", { class: "label" }, labelOf(e)), h("code", {}, r.key)),
          h("td", { class: "ltr" }, off ? "" : shownValue(r.before)),
          h("td", { class: "ltr" }, shownValue(r.after)),
          h("td", {}, h("span", { class: "badge " + cls }, t(text)), r.note && h("div", { class: "hint" }, t(r.note))));
      })))));
    if (apply > 0) { go_.disabled = false; go_.textContent = left > 0 ? t("Import the {n} it can take", { n: apply }) : t("Import {x}", { x: tn(apply, "setting") }); }
    else if (left === 0) preview.append(h("div", { class: "hint" }, t("Nothing would change.")));
    if (left > 0) preview.append(h("div", { class: "hint" }, t("The {n} this version can't take are left out; the rest go in.", { n: left })));
  };
  const look = async () => {
    const mine = ++token;
    if (!inp.value.trim()) return showPlan(null);
    const p = await post("import_preview", { text: inp.value });
    if (mine === token) showPlan(p.ok ? p : null, p.ok ? "" : p.error);
  };
  inp.oninput = () => { clearTimeout(timer); timer = setTimeout(look, 400); };
  go_.onclick = async () => {
    if (!plan) return;
    const r = await ask("import", { text: inp.value, skip_unsupported: plan.counts.unsupported + plan.counts.invalid > 0 });
    if (r.cancelled) return;
    if (!r.ok) return snack(r.error, true);
    const s = await post("state"); S.values = s.values;
    const n = Object.keys(r.values).length, left = (r.skipped || []).length;
    snack(left ? t("Imported {x}; left out {n} this version can't take.", { x: tn(n, "setting"), n: left }) : t("Imported {x}.", { x: tn(n, "setting") })); render();
  };
  const undo = x.undo_secs_ago == null ? null : h("div", { class: "card pad" }, h("h3", {}, t("Go back")),
    h("p", { class: "help" }, t("The settings as they were before the last import are kept ({when}).", { when: ago(x.undo_secs_ago) })),
    h("div", { class: "actions" }, h("button", { class: "btn", onclick: async () => {
      const r = await ask("import_undo", {});
      if (r.cancelled) return;
      if (!r.ok) return snack(r.error, true);
      S.values = r.values; snack(r.message); render();
    } }, t("Undo the last import"))));
  main.append(h("div", { class: "card pad" }, h("h3", {}, t("Export")),
      h("p", { class: "help" }, x.ok && x.text.trim() ? (x.skipped_secrets ? t("The settings that differ from the defaults. No secrets: {x} left out.", { x: tn(x.skipped_secrets, "secret") }) : t("The settings that differ from the defaults. No secrets: none were set.")) : t("Nothing differs from the defaults yet.")),
      out, h("div", { class: "actions" },
        h("button", { class: "btn", onclick: async () => { try { await navigator.clipboard.writeText(out.value); snack("Copied."); } catch (e) { out.select(); snack("Press Ctrl+C to copy.", true); } } }, t("Copy")),
        h("button", { class: "btn", onclick: () => { const a = h("a", { href: URL.createObjectURL(new Blob([out.value], { type: "text/plain" })), download: "zero-settings.toml" }); a.click(); } }, t("Download")))),
    h("div", { class: "card pad" }, h("h3", {}, t("Import")),
      h("p", { class: "help" }, t("Paste settings as TOML (what Export makes). You see each one as it is now and as it would be before anything is changed; protected ones ask first, and settings this version doesn't have are shown and left out. The settings you have now are kept, so you can go back.")),
      inp, preview, h("div", { class: "actions" }, go_)),
    undo || []);
}

/* ---------- updates ---------- */
function span(secs) {
  if (secs < 90) return t("under 2 minutes");
  if (secs < 5400) return tn(Math.round(secs / 60), "minute");
  if (secs < 129600) return tn(Math.round(secs / 3600), "hour");
  return tn(Math.round(secs / 86400), "day");
}
const PRESETS = [["Never", 0], ["5 min", 5], ["10 min", 10], ["30 min", 30], ["1 hour", 60], ["12 hours", 720], ["Daily", 1440]];
async function updatesCard() {
  const u = await post("update_status");
  if (!u.ok) return h("div", { class: "empty" }, t(u.error));
  const mins = u.enabled ? Math.round(u.every_secs / 60) : 0;
  const act = (route, text, extra) => async () => {
    const r = await ask(route, extra ? extra() : {});
    if (r.cancelled) return;
    if (!r.ok) return snack(r.error, true);
    snack(r.message || text); const st = await post("state"); S.values = st.values; render();
  };
  const preset = (label, m) => h("button", { class: "chip", "aria-pressed": String(mins === m), onclick: () => mins !== m && change(m === 0 ? [["update.enabled", false]] : [["update.enabled", true], ["update.check_every_mins", m]], m === 0 ? t("It will not look for updates.") : t("It looks every {x}.", { x: t(label) })) }, t(label));
  const fact = (k, v) => h("li", {}, h("span", { class: "k" }, t(k)), h("span", { class: "v" }, v));
  const keep = u.previous && u.previous.settings_kept && h("label", { class: "check" }, h("input", { type: "checkbox", id: "back_settings", checked: true }),
    h("span", {}, t("Also put back the settings from before that update (the ones you have now are kept beside the file)")));
  const looking = !u.enabled ? t("Off. Nothing is looked for or downloaded.")
    : t("Every {x}", { x: span(u.every_secs) }) + (u.last_check_secs_ago == null ? t(", not yet") : "; " + t("last {when}", { when: ago(u.last_check_secs_ago) })) + (u.next_in_secs != null ? "; " + t("next in about {x}", { x: span(u.next_in_secs) }) : "") + ".";
  const which = (u.pin ? t("Staying on {v}.", { v: u.pin }) : u.channel === "prerelease" ? t("The newest, pre-releases included.") : t("The newest stable release.")) + (u.skip_version ? " " + t("Never {v}.", { v: u.skip_version }) : "");
  const waiting = !u.pending ? t("Nothing.") : u.pending.wanted ? t("Version {v}, downloaded {when}. It goes in {install}.", { v: u.pending.version, when: ago(u.pending.since_secs), install: t(u.when) })
    : t("Version {v}, but the settings no longer take it (skipped, pinned or a pre-release): it will not go in.", { v: u.pending.version });
  const back = u.previous ? (u.previous.settings_kept ? t("Version {v} (kept when the last update went in), with the settings you had then.", { v: u.previous.version }) : t("Version {v} (kept when the last update went in).", { v: u.previous.version })) : t("Nothing kept yet.");
  return h("div", { class: "card" }, h("h3", {}, t("Status and controls")),
    h("div", { class: "preview" }, h("div", { class: "help" }, t("How often it looks for a new release")),
      h("div", { class: "chips2", role: "group", "aria-label": t("How often it looks") }, PRESETS.map(([l, m]) => preset(l, m)))),
    h("ul", { class: "facts" },
      fact("This version", u.current),
      fact("Looking", looking),
      u.rate_limited_for_secs && fact("GitHub", t("Asked us to slow down; looking again in about {x}.", { x: span(u.rate_limited_for_secs) })),
      fact("Which", which),
      fact("Waiting", waiting),
      fact("Can go back to", back)),
    u.notes && h("div", { class: "preview" }, h("div", { class: "help" }, t("What {v} says about itself", { v: u.notes.version })),
      h("pre", { class: "log", style: "padding:12px 0" }, u.notes.text || t("(no notes)")), u.notes.page && h("a", { href: u.notes.page, target: "_blank", rel: "noopener noreferrer" }, u.notes.page)),
    h("div", { class: "actions", style: "padding:0 24px 12px" },
      h("button", { class: "btn filled", disabled: !u.can_update, onclick: async (ev) => { ev.target.disabled = true; snack("Looking…"); await act("update_check")(); } }, t("Check now")),
      h("button", { class: "btn", disabled: !u.pending || !u.pending.wanted, onclick: act("update_install") }, t("Put it in place now")),
      h("button", { class: "btn danger", disabled: !u.previous, onclick: act("update_rollback", undefined, () => ({ settings: keep ? $("back_settings").checked : false })) }, u.previous ? t("Go back to {v}", { v: u.previous.version }) : t("Go back"))),
    keep && h("div", { class: "actions", style: "padding:0 24px 20px" }, keep),
    h("div", { class: "note" }, t("An update is downloaded and checked against GitHub's SHA-256, then goes in only when a server starts, never while an agent may be working. Looking every few minutes is allowed (GitHub lets 60 anonymous requests an hour, and an unchanged answer doesn't count).")));
}

async function connectPage(main) {
  const r = await post("connect_list");
  if (!r.ok) return main.append(h("div", { class: "empty" }, t(r.error)));
  const badge = (st) => ({ not_found: ["Not found here", ""], not_installed: ["Not added", ""], installed: ["Added", "changed"], different: ["Added, other program", "warn"], unreadable: ["Can't change safely", "warn"] }[st.kind]);
  const act = (c, action) => async () => {
    const x = await ask("connect_do", { client: c.id, action });
    if (x.cancelled) return;
    if (!x.ok) return snack(x.error, true);
    snack(t(x.message) + (x.note ? " " + t(x.note) : "")); render();
  };
  main.append(...[h("p", { class: "lead" }, t("Add Zero to the agents you use with a button. Only its own entry is written; the rest of each agent's settings stays as it was, and a copy of the file is kept next to it as .bak.")),
    r.unstable && h("div", { class: "card pad" }, h("p", { class: "msg bad" }, t("This program runs from {from}, a place that gets cleaned out or moved. A copy is kept at {to} and that is what the agents are given.", { from: r.running_from, to: r.program }))),
    r.clients.map((c) => {
      const [text, cls] = badge(c.state), st = c.state.kind, can = st !== "not_found" && st !== "unreadable" || c.id === "codex";
      return h("div", { class: "card" },
        h("div", { class: "row" },
          h("div", { class: "text" }, h("div", { class: "label" }, c.label), h("div", { class: "help ltr" }, c.where),
            h("div", { class: "meta" }, h("span", { class: "badge " + cls }, t(text)),
              st === "different" && h("span", {}, t("points at") + " ", h("code", {}, c.state.points_at)),
              st === "unreadable" && h("span", {}, t(c.state.why)))),
          h("div", { class: "ctl" },
            h("button", { class: "btn filled", disabled: !can || st === "installed", onclick: act(c, "install") }, st === "different" ? t("Replace") : t("Add")),
            h("button", { class: "btn", disabled: st !== "installed" && st !== "different", onclick: act(c, "remove") }, t("Remove")),
            h("button", { class: "btn", onclick: async () => { try { await navigator.clipboard.writeText(c.entry); snack("Copied."); } catch (e) { snack("Press Ctrl+C on the text below.", true); } } }, t("Copy")))),
        h("pre", { class: "log", tabindex: 0, "aria-label": t("What would be written for {name}", { name: c.label }) }, c.entry),
        h("div", { class: "note" }, t("After adding:") + " " + c.after));
    }),
    h("div", { class: "note" }, t("From a terminal the same thing is `computer-use-mcp install` (every agent found here), `--client NAME` for one, `--list` to look and `--remove` to take it out."))].flat().filter(Boolean));
}

/* ---------- the guide ---------- */
function helpPage(slug) {
  return async (main) => {
    const r = await fetch("help/" + (L.code === "en" ? "" : L.code + "/") + slug);
    if (!r.ok) return main.append(h("div", { class: "empty" }, t("That page isn't here.")));
    main.append(h("div", { class: "card pad prose", innerHTML: await r.text() }));
  };
}

async function shortcutPage(main) {
  const r = await post("shortcut_list");
  if (!r.ok) return main.append(h("div", { class: "empty" }, t(r.error)));
  const act = (s, action) => async () => {
    const x = await ask("shortcut_do", { place: s.id, action });
    if (x.cancelled) return;
    if (!x.ok) return snack(x.error, true);
    snack(t(x.message) + (x.note ? " " + t(x.note) : "")); render();
  };
  main.append(...[
    h("p", { class: "lead" }, t("An icon that opens this panel, so you don't need the key or a terminal. It starts the program with settings, which serves the panel on its own port and opens it in your browser (or shows the one already open).")),
    r.shortcuts.map((s) => h("div", { class: "card" }, h("div", { class: "row" },
      h("div", { class: "text" }, h("div", { class: "label" }, s.label),
        h("div", { class: "help ltr" }, s.path || t("This system has no such place.")),
        h("div", { class: "meta" },
          h("span", { class: "badge " + (s.exists ? (s.ours ? "changed" : "warn") : "") }, s.exists ? (s.ours ? t("There") : t("Another file of that name")) : t("Not there")),
          s.exists && s.starts && !s.current && h("span", {}, t("starts") + " ", h("code", {}, s.starts), "; " + t("make it again to start") + " ", h("code", {}, r.program)))),
      h("div", { class: "ctl" },
        h("button", { class: "btn filled", disabled: !s.path || (s.exists && !s.ours), onclick: act(s, "create") }, s.exists ? t("Make again") : t("Make")),
        h("button", { class: "btn", disabled: !s.exists || !s.ours, onclick: act(s, "remove") }, t("Remove")))))),
    h("div", { class: "note" }, t("From a terminal: computer-use-mcp shortcut (the desktop), --menu, --both, --remove, --list. Only shortcuts this program made are ever removed.")),
  ].flat().filter(Boolean));
}

/* ---------- pages made from the settings groups ---------- */
function groupPage(g) {
  return async (main) => {
    const rows = schema.entries.filter((e) => e.group === g && e.type !== "custom" && visible(e));
    const lead = main.querySelector(".lead");
    if (g === "Decision model") main.append(decisionCard());
    if (g === "Pointer") { main.append(pointerPicker()); main.append(pathPreview("overlay.cursor_path", "How the pointer glides")); }
    if (g === "Real mouse") main.append(pathPreview("mouse_path", "How the real mouse goes"));
    if (g === "Overlay") main.append(overlayPreview());
    if (g === "Updates") main.append(await updatesCard());
    if (g === "Screenshots") main.append(tokenCard());
    if (rows.length) main.append(h("div", { class: "card" }, rows.map(row)));
    else main.append(h("div", { class: "empty" }, view.changedOnly ? t("Nothing in this group differs from its default.") : t("Nothing to show.")));
    return lead;
  };
}

function visible(e) {
  if (view.changedOnly && !changed(e)) return false;
  if (!view.advanced && e.advanced && !changed(e)) return false;
  return true;
}

/* ---------- shell ---------- */
function registerPages() {
  page_("overview", "Overview", "Home", overviewPage);
  page_("connect", "Connect an agent", "Home", connectPage);
  page_("shortcut", "Desktop shortcut", "Home", shortcutPage);
  for (const g of schema.groups) page_(slug(g), g, "Settings", groupPage(g));
  page_("profiles", "Profiles", "Tools", profilesPage);
  page_("tool-list", "Tool list", "Tools", toolsPage);
  page_("apps-report", "Apps report", "Tools", appsPage);
  page_("audit-log", "Audit log", "Tools", auditPage);
  page_("settings-file", "Settings file", "Tools", filePage);
  page_("import-export", "Import and export", "Tools", importPage);
  for (const p of schema.help) page_("help-" + p.slug, p.title, "Guide", helpPage(p.slug));
}

function renderNav() {
  const nav = $("nav"); nav.textContent = ""; let last = "";
  for (const p of PAGES) {
    if (p.section !== last) { nav.append(h("div", { class: "section" }, t(p.section))); last = p.section; }
    const count = p.section === "Settings" ? schema.entries.filter((e) => e.group === p.title && changed(e)).length : 0;
    nav.append(h("button", { "aria-current": String(!view.query && p.id === page), onclick: () => { view.query = ""; $("q").value = ""; go(p.id); } },
      h("span", {}, t(p.title)), count > 0 && h("span", { class: "count" }, t("{n} changed", { n: count }))));
  }
}

let renderId = 0;
async function render() {
  if (!ready) return;
  renderNav();
  // The page is built apart and put in place only if no newer one was asked
  // for meanwhile (a page that waits on the server must not land on another).
  const mine = ++renderId, main = $("main"), y = window.scrollY, frag = document.createDocumentFragment();
  const show = () => { main.textContent = ""; main.append(frag); };
  if (view.query) {
    const q = view.query.toLowerCase();
    const rows = schema.entries.filter((e) => e.type !== "custom" && (e.key + " " + labelOf(e) + " " + helpOf(e) + " " + e.help + " " + t(e.group)).toLowerCase().includes(q) && (!view.changedOnly || changed(e)));
    frag.append(h("h2", {}, t("Search results")), h("p", { class: "lead" }, t("{x} match “{query}”.", { x: tn(rows.length, "setting"), query: view.query })));
    if (rows.length) { const card = h("div", { class: "card" }); let last = ""; for (const e of rows) { if (e.group !== last) { card.append(h("h3", {}, t(e.group))); last = e.group; } card.append(row(e)); } frag.append(card); }
    else frag.append(h("div", { class: "empty" }, t("Nothing matches.")));
    return show();
  }
  const p = PAGES.find((x) => x.id === page) || PAGES[0];
  frag.append(h("h2", {}, t(p.title)));
  if (p.section === "Settings") {
    frag.append(h("p", { class: "lead" }, t(schema.blurbs[p.title] || "")),
      h("div", { class: "tools" },
        h("button", { class: "chip", "aria-pressed": String(view.changedOnly), onclick: () => { view.changedOnly = !view.changedOnly; render(); } }, t("Changed from default")),
        h("button", { class: "chip", "aria-pressed": String(view.advanced), onclick: () => { view.advanced = !view.advanced; render(); } }, t("Show advanced"))));
  }
  try { await p.render(frag); } catch (err) { frag.append(h("div", { class: "empty" }, t("The server didn't answer: {error}", { error: err }))); }
  frag.append(h("footer", { class: "where" }, S.path ? [t("Settings file:") + " ", h("span", { class: "ltr" }, S.path)] : t("This server keeps its settings in memory, so nothing can be saved here.")));
  if (mine !== renderId) return;
  show();
  window.scrollTo(0, y);
}

function go(id) { page = id; history.replaceState(null, "", "#" + id); render(); window.scrollTo(0, 0); }

async function load() {
  const [sc, st] = await Promise.all([fetch("schema.json").then((r) => r.json()), post("state")]);
  if (!st.ok) { $("main").textContent = t(st.error || "Couldn't read the settings."); return; }
  schema = sc; S = st;
  for (const e of schema.entries) E[e.key] = e;
  $("ver").textContent = "v" + schema.version;
  document.documentElement.dataset.accent = S.accent; theme(S.theme);
  await useLanguage(S.language || "auto");
  registerPages();
  const want = PAGES.find((p) => p.id === location.hash.slice(1));
  page = want ? want.id : "overview"; ready = true;
  post("overview").then((o) => { overview = o; });
  render();
}

$("q").addEventListener("input", (ev) => { view.query = ev.target.value.trim(); render(); });
window.addEventListener("keydown", (ev) => { if (ev.key === "/" && !/^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement.tagName)) { ev.preventDefault(); $("q").focus(); } });
for (const b of document.querySelectorAll("#themes button")) b.onclick = async () => {
  theme(b.dataset.t); if (!ready) return;
  const r = await post("set", { confirmed: false, changes: [{ key: "panel.theme", value: b.dataset.t }] });
  if (r.ok) { take(r); if (page === "panel") render(); else renderNav(); } else snack(r.error, true);
};
$("lang").onchange = async (ev) => {
  if (!ready) return;
  const r = await post("set", { confirmed: false, changes: [{ key: "panel.language", value: ev.target.value }] });
  if (!r.ok) return snack(r.error, true);
  take(r); S.language = r.language || ev.target.value;
  await useLanguage(S.language); render();
};
$("done").onclick = async () => { try { await post("close"); } catch (e) {} document.body.textContent = t("You can close this tab."); window.close(); };
window.addEventListener("hashchange", () => { if (!ready) return; const p = PAGES.find((x) => x.id === location.hash.slice(1)); if (p && p.id !== page) { page = p.id; render(); } });
theme(document.documentElement.dataset.theme);
load().catch((err) => { $("main").textContent = t("The server didn't answer: {error}", { error: err }); });
