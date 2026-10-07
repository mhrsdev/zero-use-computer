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
const plural = (n, one, many) => n + " " + (n === 1 ? one : many || one + "s");

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
mq.addEventListener("change", () => { theme(document.documentElement.dataset.theme); if (ready) render(); });

/* ---------- state ---------- */
let schema = null, S = null, ready = false, page = "overview", snackTimer = 0, overview = null;
const E = {}; // key -> entry
const view = { query: "", changedOnly: false, advanced: false };

function snack(text, bad) {
  const el = $("snack"); el.textContent = text; el.className = "show" + (bad ? " bad" : "");
  clearTimeout(snackTimer); snackTimer = setTimeout(() => { el.className = ""; }, bad ? 6000 : 2400);
}
const val = (key) => { const e = E[key]; return e ? (key in S.values ? S.values[key] : e.default) : undefined; };
const changed = (e) => e.key in S.values && !same(S.values[e.key], e.default);
function labelOf(e) { const k = e.key.split(".").pop().replace(/_/g, " "); return k.charAt(0).toUpperCase() + k.slice(1); }
function shortKey(k) { return (E[k] ? labelOf(E[k]) : k) + " (" + k + ")"; }

function confirmDialog(keys, text) {
  return new Promise((resolve) => {
    const body = $("dlg_b"); body.textContent = "";
    if (text) body.append(h("div", {}, text));
    else body.append(h("div", {}, "These settings limit what the agent can do or see, or how the program updates. Check that you meant to change them:"),
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
  take(r); snack(okText || "Saved. Running servers use it now."); render(); return true;
}
async function reset(keys) {
  const r = await ask("reset", { keys });
  if (r.cancelled) return;
  if (!r.ok) return snack(r.error || "Not reset.", true);
  take(r); snack("Back to the default."); render();
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
      if (e.choices.length <= 3) return h("div", { class: "seg", role: "group", "aria-label": labelOf(e) },
        e.choices.map((c) => h("button", { "aria-pressed": String(v === c), onclick: () => v !== c && commit(c) }, c)));
      return h("select", { id, "aria-label": labelOf(e), onchange: (ev) => commit(ev.target.value) },
        e.choices.map((c) => h("option", { value: c, selected: v === c }, c)));
    }
    case "range": {
      const num = h("input", { type: "number", id, min: e.min, max: e.max, step: e.step, value: v });
      const rng = h("input", { type: "range", min: e.min, max: e.max, step: e.step, value: v, "aria-label": labelOf(e) + " slider" });
      rng.oninput = () => { num.value = rng.value; };
      rng.onchange = () => commit(Number(rng.value));
      num.onchange = () => { if (num.value === "" || isNaN(Number(num.value))) return render(); rng.value = num.value; commit(Number(num.value)); };
      return [rng, num, e.unit && h("span", { class: "unit" }, e.unit)];
    }
    case "int": case "float": {
      const num = h("input", { type: "number", id, step: e.type === "float" ? "any" : "1", value: v });
      num.onchange = () => { if (num.value === "" || isNaN(Number(num.value))) return render(); commit(Number(num.value)); };
      return [num, e.unit && h("span", { class: "unit" }, e.unit)];
    }
    case "color": {
      const text = h("input", { type: "text", id, value: v, spellcheck: false, size: 10, style: "width:120px" });
      const pick = h("input", { type: "color", value: /^#[0-9a-f]{6}/i.test(v) ? v.slice(0, 7) : "#000000", "aria-label": labelOf(e) + " picker" });
      pick.onchange = () => commit(pick.value.toUpperCase());
      text.onchange = () => commit(text.value.trim());
      return [pick, text];
    }
    case "list": {
      const box = h("div", { class: "chips" });
      for (const item of v) box.append(h("span", { class: "tag" }, item,
        h("button", { "aria-label": "Remove " + item, title: "Remove", onclick: () => commit(v.filter((x) => x !== item)) }, "×")));
      const add = h("input", { type: "text", placeholder: "Add and press Enter", "aria-label": "Add to " + labelOf(e), spellcheck: false });
      add.onkeydown = (ev) => { if (ev.key === "Enter" && add.value.trim()) { ev.preventDefault(); commit([...v.filter((x) => x !== add.value.trim()), add.value.trim()]); } };
      box.append(add); return box;
    }
    case "secret": {
      const i = h("input", { type: "password", id, autocomplete: "off", spellcheck: false, placeholder: v ? "Saved " + v + ". Type to replace." : "Not set" });
      i.onchange = async () => { if (!i.value) return; const x = i.value; i.value = ""; await commit(x); };
      return i;
    }
    default: {
      const i = h("input", { type: "text", id, value: v, spellcheck: false, autocomplete: "off", placeholder: e.type === "hotkey" ? "for example ctrl+alt+escape" : "" });
      i.onchange = () => commit(i.value);
      return i;
    }
  }
}

function row(e) {
  const isChanged = changed(e);
  const def = e.type === "secret" ? "none" : Array.isArray(e.default) ? (e.default.join(", ") || "empty") : (String(e.default) === "" ? "empty" : String(e.default));
  return h("div", { class: "row" },
    h("div", { class: "text" },
      h("label", { class: "label", for: "c_" + e.key.replace(/\./g, "_") }, labelOf(e)),
      h("div", { class: "help" }, e.help),
      h("div", { class: "meta" }, h("code", {}, e.key),
        isChanged && h("span", { class: "badge changed" }, "changed"),
        e.restart && h("span", { class: "badge" }, "applies after a restart"),
        e.confirm && h("span", { class: "badge warn" }, "asks to confirm"),
        e.advanced && h("span", { class: "badge" }, "advanced"),
        isChanged && h("span", {}, "default: ", h("code", {}, def)))),
    h("div", { class: "ctl" }, control(e), isChanged && e.type !== "secret" && h("button", { class: "btn small", onclick: () => reset([e.key]), title: "Back to the default" }, "Reset")));
}

/* ---------- previews ---------- */
const POINTER_NAMES = ["crystal", "paper", "jelly", "ice", "metal", "orbit"];

function pointerPicker() {
  const e = E["overlay.cursor_style"], v = val(e.key);
  const cell = (name, glyph, text) => h("button", { class: "pick", "aria-pressed": String(v === name), title: text || name, onclick: () => v !== name && change([[e.key, name]]) },
    glyph ? h("span", { class: "glyph", "aria-hidden": "true" }, glyph) : h("img", { src: "cursor/" + name + ".png", alt: "" }), h("span", {}, name));
  return h("div", { class: "card" }, h("h3", {}, "Pointer style"),
    h("div", { class: "picker", role: "group", "aria-label": "Pointer style" },
      cell("random", "?", "A different one each session, and for each agent at once"), cell("classic", "➤", "The plain arrow"), POINTER_NAMES.map((n) => cell(n))));
}

const STYLES = ["mixed", "hand", "sine", "arc", "spring", "spiral"];
function pathPreview(key, who) {
  const e = E[key], v = val(key);
  const canvas = h("canvas", { width: 960, height: 480, role: "img", "aria-label": "Three sample paths of the " + v + " style" });
  const info = h("div", { class: "help", role: "status" });
  const draw = async () => {
    const r = await post("path", { style: v });
    if (!r.ok) return;
    const ctx = canvas.getContext("2d"), cs = getComputedStyle(document.documentElement);
    const k = canvas.width / 480; ctx.clearRect(0, 0, canvas.width, canvas.height);
    const cols = [cs.getPropertyValue("--primary"), cs.getPropertyValue("--on-surface-variant"), cs.getPropertyValue("--error")];
    r.paths.forEach((p, i) => {
      ctx.strokeStyle = cols[i % 3]; ctx.lineWidth = 2.4 * k / 2; ctx.lineJoin = "round"; ctx.globalAlpha = .9; ctx.beginPath();
      p.points.forEach(([x, y], j) => (j ? ctx.lineTo(x * k, y * k) : ctx.moveTo(x * k, y * k))); ctx.stroke();
    });
    ctx.globalAlpha = 1; ctx.fillStyle = cs.getPropertyValue("--outline");
    ctx.beginPath(); ctx.arc(r.from[0] * k, r.from[1] * k, 5 * k / 2, 0, 7); ctx.fill();
    ctx.fillStyle = cs.getPropertyValue("--primary"); ctx.beginPath(); ctx.arc(r.to[0] * k, r.to[1] * k, 7 * k / 2, 0, 7); ctx.fill();
    info.textContent = r.paths.map((p) => p.style + " " + Math.round(p.ms) + " ms").join(" · ") + ". Each press makes new ones: no two hands are the same.";
  };
  draw();
  return h("div", { class: "card" }, h("h3", {}, who),
    h("div", { class: "preview" },
      h("div", { class: "chips2", role: "group", "aria-label": "Path style" }, STYLES.map((s) => h("button", { class: "chip", "aria-pressed": String(v === s), onclick: () => v !== s && change([[key, s]]) }, s)),
        h("button", { class: "chip", onclick: draw }, "Another set")),
      canvas, info,
      key === "mouse_path" && h("div", { class: "note pad0" }, "The real mouse only travels the last 70 to 140 px of a reach, then goes back to where you left it. The pointer in the overlay glides the whole way.")));
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
  return h("div", { class: "card" }, h("h3", {}, "Preview"),
    h("div", { class: "preview" },
      h("div", { class: "chips2", role: "group", "aria-label": "State" }, STATES.map(([s, t]) => h("button", { class: "chip", "aria-pressed": String(overlayState === s), onclick: () => { overlayState = s; render(); } }, t))),
      h("div", { class: "mock" }, h("div", { class: "win" }, h("i"), h("i"), h("i")), ring,
        val("overlay.show_label") && h("div", { class: "pill" }, h("b", { style: "background:" + colour }), label),
        val("overlay.show_cursor") && h("div", { class: "ptr" }, h("img", { src: "cursor/" + shown + ".png", alt: "" }), val("overlay.cursor_tag") && h("span", { style: "background:" + val("overlay.cursor_color") }, val("overlay.cursor_tag")))),
      h("div", { class: "note pad0" }, "Drawn from your settings, to show how they fit together. The real overlay is click-through and left out of the agent's screenshots.")));
}

function tokenCard() {
  const cost = (px) => Math.ceil(px * Math.round(px * 9 / 16) / 750);
  const at = (px) => px.toLocaleString("en") + " px → about " + cost(px).toLocaleString("en") + " tokens";
  const mx = val("screenshot.max_dimension"), ov = val("screenshot.overview_max_dimension"), base = cost(E["screenshot.max_dimension"].default);
  const diff = (px) => { const d = Math.round((cost(px) / base - 1) * 100); return d === 0 ? "" : " (" + (d > 0 ? "+" : "") + d + "% against the default)"; };
  return h("div", { class: "card" }, h("h3", {}, "What a picture costs"),
    h("ul", { class: "facts" },
      h("li", {}, h("span", { class: "k" }, "A whole screen"), h("span", { class: "v" }, at(mx) + diff(mx))),
      ov > 0 && h("li", {}, h("span", { class: "k" }, "An overview"), h("span", { class: "v" }, at(ov) + diff(ov)))),
    h("div", { class: "note" }, "A picture counts as width × height ÷ 750 tokens, here for a 16:9 screen; text counts as four characters a token. An estimate: your model counts its own way, and the prompt cache makes repeats cheaper."));
}

/* ---------- decision card ---------- */
function decisionCard() {
  const D = S.decision; const st = h("div", { class: "msg", role: "status" });
  const say = (t, ok) => { st.textContent = t; st.className = "msg " + (ok === undefined ? "" : ok ? "ok" : "bad"); };
  const base = { jev: "https://api.typesafe.ai", openai: "https://api.openai.com/v1" };
  const models = { jev: "jev-latest", openai: "e.g. gpt-4.1-nano, llama-3.1-8b-instant" };
  const radios = ["jev", "openai"].map((v) => h("input", { type: "radio", name: "provider", value: v, checked: D.provider === v }));
  const kind = (title, text, r) => h("label", { class: "kind" }, r, h("div", {}, h("b", {}, title), h("span", {}, text)));
  const url = h("input", { type: "url", id: "d_url", value: D.base_url, autocomplete: "off", spellcheck: false });
  const model = h("input", { type: "text", id: "d_model", value: D.model, autocomplete: "off", spellcheck: false });
  const key = h("input", { type: "password", id: "d_key", autocomplete: "off", spellcheck: false });
  const keyHint = h("div", { class: "hint" }), baseHint = h("div", { class: "hint" });
  const prov = () => (radios.find((r) => r.checked) || {}).value || "";
  const refresh = () => { const p = prov() || "jev"; url.placeholder = base[p]; model.placeholder = models[p]; baseHint.textContent = "Leave empty for " + base[p] + "."; };
  keyHint.textContent = D.key_hint ? "Saved: " + D.key_hint + ". Leave empty to keep it." : D.key_env ? "Read from the environment variable " + D.key_env + "." : "Not needed for a server on this computer.";
  for (const r of radios) r.onchange = () => { url.value = ""; model.value = ""; refresh(); };
  refresh();
  const form = () => ({ provider: prov(), base_url: url.value, model: model.value, api_key: key.value });
  const buttons = [];
  const busy = (on) => buttons.forEach((b) => { b.disabled = on; });
  const run = (fn) => async () => { busy(true); try { await fn(); } catch (err) { say("The server didn't answer: " + err, false); } busy(false); };
  buttons.push(
    h("button", { class: "btn filled", onclick: run(async () => {
      if (!prov()) return say("Choose a kind of model first.", false);
      say("Saving…"); const r = await post("save", form());
      if (r.ok) { key.value = ""; if (r.key_hint) keyHint.textContent = "Saved: " + r.key_hint + ". Leave empty to keep it."; D.provider = prov(); }
      say(r.ok ? r.message : r.error, r.ok);
    }) }, "Save"),
    h("button", { class: "btn", onclick: run(async () => {
      if (!prov()) return say("Choose a kind of model first.", false);
      say("Asking the model…"); const r = await post("test", form()); say(r.ok ? r.message : r.error, r.ok);
    }) }, "Test"),
    h("button", { class: "btn danger", onclick: run(async () => {
      if (!confirm("Remove the decision model and its key?")) return;
      const r = await post("remove");
      if (r.ok) { radios.forEach((x) => { x.checked = false; }); url.value = model.value = key.value = ""; keyHint.textContent = ""; refresh(); }
      say(r.ok ? r.message : r.error, r.ok);
    }) }, "Remove"));
  return h("div", { class: "card pad" }, h("h3", {}, "Model and API key"),
    h("p", { class: "help" }, "A fast model the agent asks yes/no, choice and score questions about what is on screen, so it reads less and decides sooner. Your key is saved on this computer and never goes through the chat."),
    h("div", { class: "kinds" },
      kind("Jev (TypeSafe System One)", "Made for decisions: answers in well under a second, with probabilities. Also any server that speaks the same API.", radios[0]),
      kind("OpenAI-compatible chat model", "OpenAI, Groq, Cerebras, OpenRouter, Ollama, LM Studio… A small, fast model works best.", radios[1])),
    h("div", { class: "field" }, h("label", { for: "d_url" }, "Address (base URL)"), url, baseHint),
    h("div", { class: "field" }, h("label", { for: "d_model" }, "Model"), model),
    h("div", { class: "field" }, h("label", { for: "d_key" }, "API key"), key, keyHint),
    h("div", { class: "actions" }, buttons), st);
}

/* ---------- pages ---------- */
const PAGES = []; // {id, title, section, render(main), count}
const page_ = (id, title, section, render) => PAGES.push({ id, title, section, render });

function ago(secs) {
  if (secs == null) return "never";
  if (secs < 90) return "just now";
  if (secs < 5400) return Math.round(secs / 60) + " minutes ago";
  if (secs < 129600) return Math.round(secs / 3600) + " hours ago";
  return Math.round(secs / 86400) + " days ago";
}

async function overviewPage(main) {
  const o = overview = await post("overview");
  if (!o.ok) return main.append(h("div", { class: "empty" }, o.error));
  const on = (b, yes, no) => h("span", {}, h("span", { class: "dot " + (b ? "on" : "off") }), b ? yes : no);
  const style = val("overlay.cursor_style"), mx = val("screenshot.max_dimension");
  const u = o.update;
  const update = !u.enabled ? "Off. It never updates by itself." : u.pending
    ? "Version " + u.pending + " is downloaded and waits for " + ({ restart: "the computer to restart", start: "the next start", manual: "you to run `computer-use-mcp update --install`" }[u.install] || "a restart") + "."
    : (u.last_check_secs_ago == null ? "On; it has not looked yet." : "On, last looked " + ago(u.last_check_secs_ago) + ".");
  const nChanged = Object.keys(S.values).length;
  main.append(
    h("div", { class: "grid2" },
      h("div", { class: "tile" }, h("div", { class: "small" }, "Version"), h("div", { class: "big" }, o.version), h("div", { class: "small" }, o.platform)),
      h("div", { class: "tile" }, h("div", { class: "small" }, "Emergency stop"), h("div", { class: "big" }, o.stop_key || "none"), h("div", { class: "small" }, o.settings_key ? "This panel: " + o.settings_key : "No key for this panel")),
      h("div", { class: "tile" }, h("div", { class: "small" }, "Settings changed"), h("div", { class: "big" }, String(nChanged)), h("div", { class: "small" }, "of " + schema.entries.length))),
    h("div", { class: "card" }, h("h3", {}, "Right now"),
      h("ul", { class: "facts" },
        h("li", {}, h("span", { class: "k" }, "Pointer"), h("span", { class: "v" }, style === "random" ? "A random one each session, and a different one for each agent at once." : style === "classic" ? "The plain arrow." : "Always " + style + ".")),
        h("li", {}, h("span", { class: "k" }, "Pictures"), h("span", { class: "v" }, o.screenshots ? "At most " + mx + " px on the long side (about " + Math.ceil(mx * Math.round(mx * 9 / 16) / 750).toLocaleString("en") + " tokens for a whole 16:9 screen), " + val("screenshot.attach") + " attached." : "Off: the agent works from the tree alone.")),
        h("li", {}, h("span", { class: "k" }, "Several agents"), h("span", { class: "v" }, o.hub.enabled ? on(o.hub.running, "A hub is running on port " + o.hub.port + ".", "On; no hub is running right now.") : on(false, "", "Off."))),
        h("li", {}, h("span", { class: "k" }, "Decision model"), h("span", { class: "v" }, o.decision || "None set up.")),
        h("li", {}, h("span", { class: "k" }, "Tools"), h("span", { class: "v" }, plural(o.tools, "tool") + " allowed by the settings.")),
        h("li", {}, h("span", { class: "k" }, "Updates"), h("span", { class: "v" }, update)),
        h("li", {}, h("span", { class: "k" }, "Audit log"), h("span", { class: "v" }, o.audit ? "On." : "Off.")),
        h("li", {}, h("span", { class: "k" }, "Program"), h("span", { class: "v" }, o.exe)),
        h("li", {}, h("span", { class: "k" }, "Settings file"), h("span", { class: "v" }, o.settings_file ? o.settings_file + (o.settings_file_exists ? "" : " (not made yet: the first change makes it)") : "None: this server keeps its settings in memory.")))),
    h("div", { class: "card pad" }, h("h3", {}, "Where to start"),
      h("p", { class: "help" }, "If the agent spends too much, try the Low tokens profile. If it can't read an app, try Best quality. To record a demo, Showcase makes the pointer and border bigger."),
      h("div", { class: "actions" }, h("button", { class: "btn filled", onclick: () => go("profiles") }, "Profiles"), h("button", { class: "btn", onclick: () => go("pointer") }, "Choose a pointer"), h("button", { class: "btn", onclick: () => go("tools-and-tokens") }, "Tools and tokens"))));
}

async function profilesPage(main) {
  const r = await post("profiles");
  if (!r.ok) return main.append(h("div", { class: "empty" }, r.error));
  const apply = async (p) => {
    const x = await ask("profile_apply", { id: p.id });
    if (x.cancelled) return;
    if (!x.ok) return snack(x.error, true);
    take(x); S = { ...S, values: (await post("state")).values }; snack(p.label + " applied."); render();
  };
  const list = (items) => items.map((p) => h("div", { class: "profile" },
    h("div", { class: "text" }, h("div", { class: "label" }, p.label), h("div", { class: "help" }, p.blurb),
      h("div", { class: "meta" }, p.differs ? plural(p.differs, "setting") + " would change" : "Already applied", " · ", plural(p.settings, "setting") + " in it")),
    h("button", { class: "btn", disabled: !p.differs, onclick: () => apply(p) }, "Apply"),
    !p.builtin && h("button", { class: "btn danger", onclick: async () => { if (!confirm("Delete the profile “" + p.label + "”?")) return; const d = await post("profile_delete", { id: p.id }); if (d.ok) render(); else snack(d.error, true); } }, "Delete")));
  const builtin = r.profiles.filter((p) => p.builtin), mine = r.profiles.filter((p) => !p.builtin);
  const name = h("input", { type: "text", placeholder: "Name, for example Work laptop", "aria-label": "Profile name", maxlength: 60 });
  main.append(h("div", { class: "card" }, h("h3", {}, "Comes with the program"), list(builtin),
      h("div", { class: "note" }, "A profile only sets the settings it names and puts back the defaults of the ones another profile sets. It never touches protected settings or secrets.")),
    h("div", { class: "card" }, h("h3", {}, "Yours"), mine.length ? list(mine) : h("div", { class: "note" }, "None yet. Change some settings, then keep them here under a name.")),
    h("div", { class: "card pad" }, h("h3", {}, "Keep the current settings as a profile"),
      h("div", { class: "actions" }, name, h("button", { class: "btn filled", onclick: async () => {
        const x = await post("profile_save", { label: name.value });
        if (!x.ok) return snack(x.error, true);
        snack("Kept " + plural(x.settings, "setting") + (x.left_out ? ". Left out " + x.left_out + " that a profile may not hold (secrets, protected settings, ports)." : "."));
        render();
      } }, "Keep"))));
}

async function toolsPage(main) {
  const r = await post("tools");
  if (!r.ok) return main.append(h("div", { class: "empty" }, r.error));
  const dis = val("tools.disabled"), only = val("tools.enabled");
  const total = r.tools.filter((t) => t.offered).reduce((a, t) => a + t.tokens, 0);
  const set = (name, on) => change([["tools.disabled", on ? dis.filter((x) => x.toLowerCase() !== name) : [...dis, name]]]);
  main.append(h("div", { class: "card" },
    only.length > 0 && h("div", { class: "note" }, "Only " + only.length + " tools are listed in tools.enabled, so the others stay hidden whatever the switches say."),
    r.tools.map((t) => h("div", { class: "toolrow" },
      h("label", { class: "switch" }, h("input", { type: "checkbox", role: "switch", checked: !t.disabled, "aria-label": t.name, onchange: (ev) => set(t.name, ev.target.checked) }), h("span")),
      h("span", { class: "name" }, t.name), h("span", { class: "cost" }, "~" + t.tokens + " tok"))),
    h("div", { class: "note" }, "The tool list is sent with every request, so what is on costs about " + total.toLocaleString("en") + " tokens each time" + (r.manager === "dispatch" ? ", before the tool manager hides most of them until the model asks (tools.manager is dispatch)" : "") + ". Costs are the descriptions in use (" + val("tools.descriptions") + "), counted as four characters a token.")));
}

async function appsPage(main) {
  const r = await post("apps");
  if (!r.ok) return main.append(h("div", { class: "empty" }, r.error));
  if (!r.apps.length) return main.append(h("div", { class: "card pad" }, h("p", { class: "help" }, "Nothing counted yet. Each time the agent looks at an app, the program notes how much its accessibility tree told it and whether it had to use pixels; the apps that needed pixels most show here. It is kept in " + r.file + " (screenshot.record_apps).")));
  main.append(h("div", { class: "card scroll" }, h("table", { class: "table" },
    h("thead", {}, h("tr", {}, ["App", "Looks", "Little in the tree", "Text read off screen", "Pictures sent", "Left out"].map((t, i) => h("th", { class: i ? "num" : "" }, t)))),
    h("tbody", {}, r.apps.map((a) => h("tr", {}, h("td", {}, a.app), h("td", { class: "num" }, a.looks), h("td", { class: "num" }, a.little_tree + " (" + a.share_little + "%)"), h("td", { class: "num" }, a.text_read), h("td", { class: "num" }, a.pictures), h("td", { class: "num" }, a.pictures_left_out)))))),
    h("div", { class: "note" }, "Apps with a high share in the third column are the ones the tree serves badly: they get text read off the screen and more pictures. Kept in " + r.file + "."));
}

async function auditPage(main) {
  const r = await post("audit", { lines: 200 });
  if (!r.ok) return main.append(h("div", { class: "empty" }, r.error));
  const fmt = (l) => { try { const j = JSON.parse(l); const t = j.ts ? new Date(j.ts * 1000).toLocaleTimeString() + "  " : ""; return t + Object.entries(j).filter(([k]) => k !== "ts").map(([k, v]) => k + "=" + (typeof v === "string" ? v : JSON.stringify(v))).join("  "); } catch (e) { return l; } };
  main.append(h("div", { class: "card" },
    h("div", { class: "row" }, h("div", { class: "text" }, h("div", { class: "label" }, "Write the audit log"), h("div", { class: "help" }, "A line for each call: the tool, the app and how it ended. Never the arguments, the tree or a picture."), h("div", { class: "meta" }, h("code", {}, r.file))),
      h("div", { class: "ctl" }, control(E["audit.enabled"]))),
    r.lines.length ? h("pre", { class: "log", tabindex: 0, "aria-label": "Audit log" }, r.lines.map(fmt).join("\n")) : h("div", { class: "note" }, r.exists ? "The file is empty." : r.enabled ? "Nothing written yet." : "No log yet: turn it on above."),
    r.lines.length > 0 && h("div", { class: "note" }, "The last " + r.lines.length + " lines" + (r.size ? " of " + (r.size / 1024).toFixed(0) + " KB" : "") + ". ", h("button", { class: "btn small", onclick: () => render() }, "Refresh"))));
}

async function filePage(main) {
  const r = await post("raw_get");
  if (!r.ok) return main.append(h("div", { class: "empty" }, r.error));
  const area = h("textarea", { spellcheck: false, "aria-label": "Settings file", value: r.text });
  const st = h("div", { class: "msg", role: "status" });
  const say = (t, ok) => { st.textContent = t; st.className = "msg " + (ok === undefined ? "" : ok ? "ok" : "bad"); };
  let timer = 0;
  const check = async () => {
    const x = await post("raw_check", { text: area.value });
    if (!x.ok) return say(x.error, false);
    say(x.changed.length ? "Valid. " + plural(x.changed.length, "setting") + " would change" + (x.protected.length ? ", " + x.protected.length + " of them protected." : ".") : "Valid. Nothing would change.", true);
  };
  area.oninput = () => { clearTimeout(timer); timer = setTimeout(check, 500); };
  main.append(h("div", { class: "card pad" },
    r.broken && h("p", { class: "msg bad" }, "The file as saved has a mistake in it; mend it here."),
    h("p", { class: "help" }, "The settings file as text, comments and all. Secrets are covered; leave them as they are to keep them. It is checked as a whole before anything is written, so a typo can't break the file."),
    area, h("div", { class: "actions" },
      h("button", { class: "btn filled", onclick: async () => {
        const x = await ask("raw_save", { text: area.value });
        if (x.cancelled) return;
        if (!x.ok) return say(x.error, false);
        snack("Saved. Running servers use it now."); const s = await post("state"); S.values = s.values; filePage_reset();
      } }, "Save"),
      h("button", { class: "btn", onclick: check }, "Check"), h("button", { class: "btn", onclick: () => render() }, "Revert")), st,
    h("div", { class: "note pad0" }, "File: " + r.path)));
}
function filePage_reset() { render(); }

async function importPage(main) {
  const x = await post("export");
  const out = h("textarea", { class: "short", readOnly: true, "aria-label": "Your settings", value: x.ok ? x.text : "" });
  const inp = h("textarea", { class: "short", spellcheck: false, placeholder: "[screenshot]\nmax_dimension = 1024", "aria-label": "Settings to import" });
  main.append(h("div", { class: "card pad" }, h("h3", {}, "Export"),
      h("p", { class: "help" }, x.ok && x.text.trim() ? "The settings that differ from the defaults. No secrets: " + (x.skipped_secrets ? plural(x.skipped_secrets, "secret") + " left out." : "none were set.") : "Nothing differs from the defaults yet."),
      out, h("div", { class: "actions" },
        h("button", { class: "btn", onclick: async () => { try { await navigator.clipboard.writeText(out.value); snack("Copied."); } catch (e) { out.select(); snack("Press Ctrl+C to copy.", true); } } }, "Copy"),
        h("button", { class: "btn", onclick: () => { const a = h("a", { href: URL.createObjectURL(new Blob([out.value], { type: "text/plain" })), download: "zero-settings.toml" }); a.click(); } }, "Download"))),
    h("div", { class: "card pad" }, h("h3", {}, "Import"),
      h("p", { class: "help" }, "Paste settings as TOML (what Export makes). Each one is changed as if you had set it on its page; protected ones ask first, and anything that isn't a setting stops the whole import."),
      inp, h("div", { class: "actions" }, h("button", { class: "btn filled", onclick: async () => {
        const r = await ask("import", { text: inp.value });
        if (r.cancelled) return;
        if (!r.ok) return snack(r.error, true);
        const s = await post("state"); S.values = s.values; snack("Imported " + plural(Object.keys(r.values).length, "setting") + "."); render();
      } }, "Import"))));
}

/* ---------- updates ---------- */
function span(secs) {
  if (secs < 90) return "under 2 minutes";
  if (secs < 5400) return Math.round(secs / 60) + " minutes";
  if (secs < 129600) return Math.round(secs / 3600) + " hours";
  return Math.round(secs / 86400) + " days";
}
const PRESETS = [["Never", 0], ["5 min", 5], ["10 min", 10], ["30 min", 30], ["1 hour", 60], ["12 hours", 720], ["Daily", 1440]];
async function updatesCard() {
  const u = await post("update_status");
  if (!u.ok) return h("div", { class: "empty" }, u.error);
  const mins = u.enabled ? Math.round(u.every_secs / 60) : 0;
  const act = (route, text) => async () => {
    const r = await ask(route, {});
    if (r.cancelled) return;
    if (!r.ok) return snack(r.error, true);
    snack(r.message || text); const st = await post("state"); S.values = st.values; render();
  };
  const preset = (label, m) => h("button", { class: "chip", "aria-pressed": String(mins === m), onclick: () => mins !== m && change(m === 0 ? [["update.enabled", false]] : [["update.enabled", true], ["update.check_every_mins", m]], m === 0 ? "It will not look for updates." : "It looks every " + label + ".") }, label);
  const fact = (k, v) => h("li", {}, h("span", { class: "k" }, k), h("span", { class: "v" }, v));
  return h("div", { class: "card" }, h("h3", {}, "Status and controls"),
    h("div", { class: "preview" }, h("div", { class: "help" }, "How often it looks for a new release"),
      h("div", { class: "chips2", role: "group", "aria-label": "How often it looks" }, PRESETS.map(([l, m]) => preset(l, m)))),
    h("ul", { class: "facts" },
      fact("This version", u.current),
      fact("Looking", !u.enabled ? "Off. Nothing is looked for or downloaded." : "Every " + span(u.every_secs) + (u.last_check_secs_ago == null ? ", not yet" : "; last " + ago(u.last_check_secs_ago)) + (u.next_in_secs != null ? "; next in about " + span(u.next_in_secs) : "") + "."),
      u.rate_limited_for_secs && fact("GitHub", "Asked us to slow down; looking again in about " + span(u.rate_limited_for_secs) + "."),
      fact("Which", (u.pin ? "Staying on " + u.pin + "." : u.channel === "prerelease" ? "The newest, pre-releases included." : "The newest stable release.") + (u.skip_version ? " Never " + u.skip_version + "." : "")),
      fact("Waiting", u.pending ? "Version " + u.pending.version + ", downloaded " + ago(u.pending.since_secs) + ". It goes in " + u.when + "." : "Nothing."),
      fact("Can go back to", u.previous ? "Version " + u.previous.version + " (kept when the last update went in)." : "Nothing kept yet.")),
    u.notes && h("div", { class: "preview" }, h("div", { class: "help" }, "What " + u.notes.version + " says about itself"),
      h("pre", { class: "log", style: "padding:12px 0" }, u.notes.text || "(no notes)"), u.notes.page && h("a", { href: u.notes.page, target: "_blank", rel: "noopener noreferrer" }, u.notes.page)),
    h("div", { class: "actions", style: "padding:0 24px 20px" },
      h("button", { class: "btn filled", disabled: !u.can_update, onclick: async (ev) => { ev.target.disabled = true; snack("Looking…"); await act("update_check")(); } }, "Check now"),
      h("button", { class: "btn", disabled: !u.pending, onclick: act("update_install") }, "Put it in place now"),
      h("button", { class: "btn danger", disabled: !u.previous, onclick: act("update_rollback") }, "Go back" + (u.previous ? " to " + u.previous.version : ""))),
    h("div", { class: "note" }, "An update is downloaded and checked against GitHub's SHA-256, then goes in only when a server starts, never while an agent may be working. Looking every few minutes is allowed (GitHub lets 60 anonymous requests an hour, and an unchanged answer doesn't count)."));
}

async function connectPage(main) {
  const r = await post("connect_list");
  if (!r.ok) return main.append(h("div", { class: "empty" }, r.error));
  const badge = (st) => ({ not_found: ["Not found here", ""], not_installed: ["Not added", ""], installed: ["Added", "changed"], different: ["Added, other program", "warn"], unreadable: ["Can't change safely", "warn"] }[st.kind]);
  const act = (c, action) => async () => {
    const x = await ask("connect_do", { client: c.id, action });
    if (x.cancelled) return;
    if (!x.ok) return snack(x.error, true);
    snack(x.message + (x.note ? " " + x.note : "")); render();
  };
  main.append(...[h("p", { class: "lead" }, "Add Zero to the agents you use with a button. Only its own entry is written; the rest of each agent's settings stays as it was, and a copy of the file is kept next to it as .bak."),
    r.unstable && h("div", { class: "card pad" }, h("p", { class: "msg bad" }, "This program runs from " + r.running_from + ", a place that gets cleaned out or moved. A copy is kept at " + r.program + " and that is what the agents are given.")),
    r.clients.map((c) => {
      const [text, cls] = badge(c.state), st = c.state.kind, can = st !== "not_found" && st !== "unreadable" || c.id === "codex";
      return h("div", { class: "card" },
        h("div", { class: "row" },
          h("div", { class: "text" }, h("div", { class: "label" }, c.label), h("div", { class: "help" }, c.where),
            h("div", { class: "meta" }, h("span", { class: "badge " + cls }, text),
              st === "different" && h("span", {}, "points at ", h("code", {}, c.state.points_at)),
              st === "unreadable" && h("span", {}, c.state.why))),
          h("div", { class: "ctl" },
            h("button", { class: "btn filled", disabled: !can || st === "installed", onclick: act(c, "install") }, st === "different" ? "Replace" : "Add"),
            h("button", { class: "btn", disabled: st !== "installed" && st !== "different", onclick: act(c, "remove") }, "Remove"),
            h("button", { class: "btn", onclick: async () => { try { await navigator.clipboard.writeText(c.entry); snack("Copied."); } catch (e) { snack("Press Ctrl+C on the text below.", true); } } }, "Copy"))),
        h("pre", { class: "log", tabindex: 0, "aria-label": "What would be written for " + c.label }, c.entry),
        h("div", { class: "note" }, "After adding: " + c.after));
    }),
    h("div", { class: "note" }, "From a terminal the same thing is `computer-use-mcp install` (every agent found here), `--client NAME` for one, `--list` to look and `--remove` to take it out.")].flat().filter(Boolean));
}

/* ---------- the guide ---------- */
function helpPage(slug) {
  return async (main) => {
    const r = await fetch("help/" + slug);
    if (!r.ok) return main.append(h("div", { class: "empty" }, "That page isn't here."));
    main.append(h("div", { class: "card pad prose", innerHTML: await r.text() }));
  };
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
    else main.append(h("div", { class: "empty" }, view.changedOnly ? "Nothing in this group differs from its default." : "Nothing to show."));
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
    if (p.section !== last) { nav.append(h("div", { class: "section" }, p.section)); last = p.section; }
    const count = p.section === "Settings" ? schema.entries.filter((e) => e.group === p.title && changed(e)).length : 0;
    nav.append(h("button", { "aria-current": String(!view.query && p.id === page), onclick: () => { view.query = ""; $("q").value = ""; go(p.id); } },
      h("span", {}, p.title), count > 0 && h("span", { class: "count" }, count + " changed")));
  }
}

async function render() {
  if (!ready) return;
  renderNav();
  const main = $("main"), y = window.scrollY; main.textContent = "";
  if (view.query) {
    const q = view.query.toLowerCase();
    const rows = schema.entries.filter((e) => e.type !== "custom" && (e.key + " " + labelOf(e) + " " + e.help + " " + e.group).toLowerCase().includes(q) && (!view.changedOnly || changed(e)));
    main.append(h("h2", {}, "Search results"), h("p", { class: "lead" }, plural(rows.length, "setting") + " match “" + view.query + "”."));
    if (rows.length) { const card = h("div", { class: "card" }); let last = ""; for (const e of rows) { if (e.group !== last) { card.append(h("h3", {}, e.group)); last = e.group; } card.append(row(e)); } main.append(card); }
    else main.append(h("div", { class: "empty" }, "Nothing matches."));
    return;
  }
  const p = PAGES.find((x) => x.id === page) || PAGES[0];
  main.append(h("h2", {}, p.title));
  if (p.section === "Settings") {
    main.append(h("p", { class: "lead" }, schema.blurbs[p.title] || ""),
      h("div", { class: "tools" },
        h("button", { class: "chip", "aria-pressed": String(view.changedOnly), onclick: () => { view.changedOnly = !view.changedOnly; render(); } }, "Changed from default"),
        h("button", { class: "chip", "aria-pressed": String(view.advanced), onclick: () => { view.advanced = !view.advanced; render(); } }, "Show advanced")));
  }
  try { await p.render(main); } catch (err) { main.append(h("div", { class: "empty" }, "The server didn't answer: " + err)); }
  main.append(h("footer", { class: "where" }, S.path ? "Settings file: " + S.path : "This server keeps its settings in memory, so nothing can be saved here."));
  window.scrollTo(0, y);
}

function go(id) { page = id; history.replaceState(null, "", "#" + id); render(); window.scrollTo(0, 0); }

async function load() {
  const [sc, st] = await Promise.all([fetch("schema.json").then((r) => r.json()), post("state")]);
  if (!st.ok) { $("main").textContent = st.error || "Couldn't read the settings."; return; }
  schema = sc; S = st;
  for (const e of schema.entries) E[e.key] = e;
  $("ver").textContent = "v" + schema.version;
  document.documentElement.dataset.accent = S.accent; theme(S.theme);
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
  if (r.ok) { take(r); render(); } else snack(r.error, true);
};
$("done").onclick = async () => { try { await post("close"); } catch (e) {} document.body.textContent = "You can close this tab."; window.close(); };
window.addEventListener("hashchange", () => { if (!ready) return; const p = PAGES.find((x) => x.id === location.hash.slice(1)); if (p && p.id !== page) { page = p.id; render(); } });
theme(document.documentElement.dataset.theme);
load().catch((err) => { $("main").textContent = "The server didn't answer: " + err; });
