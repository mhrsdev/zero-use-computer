# Languages of the settings panel

English is what the panel is written in. Each other language is one file
here (`fa.json`, `zh.json`, `ru.json`), the page asks for it when it needs
it (`lang/<code>.json`), and the guide's pages are `../help/<page>.<code>.html`.

`en.json` is the list every language follows: **every text the page shows**,
made by `scripts/panel-i18n.mjs` from the page's code (every `t("…")`), the
settings (labels and help) and the messages the server makes in English.
Don't edit it by hand; make it again after changing a text:

    computer-use-mcp settings --no-browser         # prints the address
    node scripts/panel-i18n.mjs <that address>

A language file has the same shape:

    {
     "ui":       { "<English text>": "<translation>", … },
     "plural":   { "setting": { "one": "…", "other": "…" }, … },
     "settings": { "<key>": ["<label>", "<help>"], … }
    }

* A `{name}` in a text is where the page puts a number, a name or a path:
  keep it, in the same words, wherever the sentence needs it.
* A message the server makes in English (they are in the list too) is
  matched sentence by sentence, with a `{name}` for what varies.
* `plural` has the forms the language uses (`Intl.PluralRules`: Persian
  `one`/`other`, Chinese `other`, Russian `one`/`few`/`many`/`other`).
* A text missing from a language shows in English; a test fails for it.

Check a language (no panel needed):

    node scripts/panel-i18n-check.mjs fa

To add a language: add it to `LANGUAGES` in `../lang.rs` (and `app.js`, and the
`panel.language` choices in `../schema.rs` and `config.rs`), make its
`.json` and the nine guide pages, and for a right-to-left script list it in
`is_rtl` and check the CSS at the end of `../index.html`. The page is laid out
with logical properties (`margin-inline-start`, `text-align: start`), so
mirroring needs only `dir="rtl"`; keys, code, paths and numbers stay left to
right (`.ltr`, `code`).

Fonts: Persian ships Vazirmatn (`fonts/`, SIL Open Font License, see
`fonts/OFL.txt`). Chinese and Russian use what the system has.
