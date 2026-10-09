#!/usr/bin/env node
// Makes crates/computer-use/src/panel/lang/en.json: every text of the
// settings panel in English, the list each language follows (see
// crates/computer-use/src/panel/lang/README.md).
//
//   node scripts/panel-i18n.mjs <panel address>      write en.json
//   node scripts/panel-i18n.mjs <panel address> --check
//                                                    say what a language lacks
//
// The panel address is what `computer-use-mcp settings --no-browser` prints.
// The settings, their help, the groups and the profiles come from the
// running panel; the texts of the page's own code are read out of app.js
// (every `t("…")`, `snack("…")`, `say("…")`) plus the ones it passes in
// variables, listed here.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const panel = join(here, "../crates/computer-use/src/panel");
const base = (process.argv[2] || "").replace(/\/?$/, "/");
const check = process.argv.includes("--check");
if (!process.argv[2]) { console.error("usage: panel-i18n.mjs <panel address> [--check]"); process.exit(2); }

const get = async (p) => (await fetch(base + p)).json();
const post = async (p, b = {}) => (await fetch(base + p, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(b) })).json();

// Texts the code hands to t() in a variable, a table or a call the scan
// below doesn't read.
const IN_VARIABLES = [
  // the page's own HTML
  "Done", "Cancel", "Change", "Change protected settings?", "panel", "Search all settings", "Settings groups", "Theme", "Language", "Light", "Auto", "Dark",
  // sections of the menu and pages
  "Home", "Settings", "Tools", "Guide", "Overview", "Connect an agent", "Desktop shortcut", "Profiles", "Tool list", "Apps report", "Audit log", "Settings file", "Import and export",
  // the update page
  "the computer to restart", "the next start", "you to run `computer-use-mcp update --install`", "a restart",
  "This version", "Looking", "GitHub", "Which", "Waiting", "Can go back to",
  "Never", "5 min", "10 min", "30 min", "1 hour", "12 hours", "Daily",
  "when the server starts after the computer restarts", "the next time the server starts", "with `computer-use-mcp update --install`",
  // previews and cards
  "A different one each session, and for each agent at once", "The plain arrow", "How the pointer glides", "How the real mouse goes",
  "Thinking", "Working", "Done", "Error", "Paused", "Stopped",
  "Jev (TypeSafe System One)", "Made for decisions: answers in well under a second, with probabilities. Also any server that speaks the same API.",
  "OpenAI-compatible chat model", "OpenAI, Groq, Cerebras, OpenRouter, Ollama, LM Studio… A small, fast model works best.",
  "App", "Looks", "Little in the tree", "Text read off screen", "Pictures sent", "Left out",
  // badges
  "Not found here", "Not added", "Added", "Added, other program", "Can't change safely",
  "changed", "asks to confirm", "same", "not in this version", "can't take it",
  // seen in the HTML of the page and the sentences of the program
  "Saving…", "Asking the model…", "Looking…", "Copied.", "Press Ctrl+C to copy.", "Press Ctrl+C on the text below.",
  "Saved. Running servers use it now.", "Not saved.", "Not reset.", "Back to the default.", "Choose a kind of model first.",
  "Couldn't read the settings.", "You can close this tab.",
];

// Messages the server makes in English: the page matches them against
// these, a sentence at a time, with `{…}` standing for a number, a name or
// a path (and a name that is itself one of these texts is translated).
const FROM_THE_SERVER = [
  "reload the page: it didn't say which version of the file it showed",
  "this server keeps its settings in memory, so there is no file",
  "say which settings to change", "a change has no key", "a change needs a value", "say which settings to reset", "that is not a setting", "there is no such profile",
  "this server keeps its settings in memory, so they can't be saved here", "this server keeps its settings in memory",
  "Saved to {path}.", "The agent can use it now.", "Removed.", "The agent has no decision model now.",
  "choose a kind of model", "the address must start with https:// (or http:// for a server on this computer)", "each field must be one line",
  "these settings need the user's confirmation first", "this needs the user's confirmation first",
  "`{key}` is not a setting", "`{key}` is changed on its own page", "{key}: must be one of {choices}", "{key}: must be between {min} and {max}",
  "{key}: must be on or off", "{key}: must be a number", "{key}: must be text", "{key}: must be a list", "{key}: must be one line of text",
  "{key}: must be a whole number, not negative",
  "the settings file changed since it was shown here (another tab, a command, a profile): press Revert to see it as it is now, then make your change again",
  "The settings file has a mistake in it and may hold a secret, so it isn't shown here.", "Fix it in a text editor: {path}",
  "not a setting: {keys}", "the file is too long to be a settings file",
  "there is no release of this program for this system",
  "{v} is the latest under these settings.", "{v} is downloaded and checked.", "It goes in {when}.",
  "no update is waiting", "{v} is in place.", "Start your MCP client again to use it.",
  "{v} is no longer wanted by the settings (skipped, another version pinned, or a pre-release on the stable channel), so it was forgotten",
  "Put version {v} in place of this program now?", "Your MCP clients use it the next time they start.",
  "there is no earlier version kept to go back to",
  "Go back to version {v}?", "The version you have now is not taken again unless you clear update.skip_version.",
  "The settings you had before that update come back with it; the ones you have now are kept beside the file.",
  "{v} is back.", "{v} will not be taken again.",
  "The settings from before the update are back.", "The ones you had are kept in {path}.",
  "give the profile a name", "that name belongs to a profile that comes with the program", "nothing differs from the defaults, so there is nothing to keep",
  "that name can't be kept", "can't make the profiles folder", "that is not one of your profiles",
  "Your own: a snapshot of settings you changed.",
  "say desktop or menu", "say create or remove", "Removed {path}.", "There was none.", "Made {path}.",
  "Take the panel's shortcut away from {path}?",
  "Put a shortcut to this panel at {path}?", "It starts {program} settings, which opens the panel in your browser.",
  "choose an agent", "say install or remove",
  "no import has been made yet, so there is nothing to go back to",
  "Put the settings back as they were before the last import?", "The ones you have now are kept, so this can be undone too.",
  "couldn't keep the settings you have now, so nothing was changed",
  "The settings from before the import are back.",
  "that is too long to be a list of settings", "there are no settings in it",
  "This version ({v}) has no such setting", "This version ({v}) has no such setting: the export is from {w}, a newer one",
  "It is changed on its own page, not by an import",
  "must be one line of text", "must be text", "must be on or off", "must be a number", "must be a list", "a list holds text",
  "must be a whole number, not negative", "a saved secret is replaced, not reset, here",
  "must be one of {choices}", "must be between {min} and {max}",
];

// Short names of things, in the forms a number needs.
const PLURAL = {
  setting: { one: "setting", other: "settings" },
  tool: { one: "tool", other: "tools" },
  secret: { one: "secret", other: "secrets" },
  minute: { one: "minute", other: "minutes" },
  hour: { one: "hour", other: "hours" },
  day: { one: "day", other: "days" },
};

const unquote = (lit) => JSON.parse(lit);
const scan = (src) => {
  const out = new Set();
  const re = /\b(?:t|snack|say|fact|confirm)\(\s*("(?:[^"\\\n]|\\.)*")/g;
  for (let m; (m = re.exec(src)); ) out.add(unquote(m[1]));
  // `fact("…")`-like calls and the second texts of kind(title, text, r) and cell().
  return out;
};

const src = readFileSync(join(panel, "app.js"), "utf8");
const schema = await get("schema.json");
const profiles = await post("profiles");

const ui = new Set([...scan(src), ...IN_VARIABLES, ...FROM_THE_SERVER]);
for (const g of schema.groups) ui.add(g);
for (const b of Object.values(schema.blurbs)) ui.add(b);
for (const p of schema.help) ui.add(p.title);
for (const p of profiles.profiles || []) if (p.builtin) { ui.add(p.label); ui.add(p.blurb); }
for (const e of schema.entries) if (e.unit) ui.add(e.unit);
for (const x of [...ui]) if (!/\p{L}/u.test(x)) ui.delete(x);

const label = (key) => { const k = key.split(".").pop().replace(/_/g, " "); return k.charAt(0).toUpperCase() + k.slice(1); };
const settings = {};
for (const e of schema.entries) if (e.type !== "custom") settings[e.key] = [label(e.key), e.help];

const en = {
  ui: Object.fromEntries([...ui].sort((a, b) => a.localeCompare(b)).map((s) => [s, s])),
  plural: PLURAL,
  settings,
};

if (!check) {
  writeFileSync(join(panel, "lang/en.json"), JSON.stringify(en, null, 1) + "\n");
  console.log(`en.json: ${Object.keys(en.ui).length} texts, ${Object.keys(settings).length} settings`);
} else {
  for (const code of ["fa", "zh", "ru"]) {
    const t = JSON.parse(readFileSync(join(panel, `lang/${code}.json`), "utf8"));
    const lacks = Object.keys(en.ui).filter((s) => !(t.ui && t.ui[s]));
    const lackSettings = Object.keys(settings).filter((k) => !(t.settings && t.settings[k]));
    console.log(`${code}: lacks ${lacks.length} texts and ${lackSettings.length} settings`);
    for (const s of lacks.slice(0, 20)) console.log("   " + s);
  }
}
