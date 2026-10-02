// Scene data for the launch film. Every string the system "says" follows the
// engine's real formats (see VERIFIED_FEATURES.md):
//   tree line   cu/tree.rs:154-219   `<indent><index> <role> "<name>" value="…" (flags)`
//   diff line   cu/tree.rs:704-727   `~ <i> <line>  (was: <old>)` / `- <i> <old line>`
//   reports     cu/engine.rs:4807-4816, 1108
//   card mask   cu/privacy.rs (test finds_card_numbers_only)
window.SCENE = {
  // screen #1 — `get_app_state {"app":"Mail"}` (indent: 1 space per level)
  tree: [
    { ix: 0, d: 0, t: 'window "Inbox — Mail"', el: "#win" },
    { ix: 1, d: 1, t: "toolbar", el: ".win-toolbar" },
    { ix: 2, d: 2, t: 'button "Reply"', el: "#tb-reply" },
    { ix: 3, d: 2, t: 'button "Archive"', el: "#tb-archive" },
    { ix: 4, d: 2, t: 'button "Delete"', el: "#tb-delete", target: true },
    { ix: 5, d: 2, t: 'search field "Search" (editable)', el: ".tb-search" },
    { ix: 6, d: 1, t: 'outline "Mailboxes"', el: ".side" },
    { ix: 7, d: 2, t: 'tree item "Inbox" value="12" (selected)', el: ".side-item:nth-child(1)" },
    { ix: 8, d: 2, t: 'tree item "Drafts"', el: ".side-item:nth-child(2)" },
    { ix: 9, d: 2, t: 'tree item "Sent"', el: ".side-item:nth-child(3)" },
    { ix: 10, d: 2, t: 'tree item "Archive"', el: ".side-item:nth-child(4)" },
    { ix: 11, d: 2, t: 'tree item "Trash"', el: ".side-item:nth-child(5)" },
    { ix: 12, d: 1, t: 'list "Messages"', el: "#list" },
    { ix: 13, d: 2, t: 'list item "Lena Ortiz, Meeting notes"', el: "#row-0" },
    { ix: 14, d: 2, t: 'list item "Weekly digest, Issue 41" (selected)', el: "#row-1" },
    { ix: 15, d: 2, t: 'list item "Weekly digest, Issue 42" (selected)', el: "#row-2" },
    { ix: 16, d: 2, t: 'list item "Weekly digest, Issue 43" (selected)', el: "#row-3" },
    { ix: 17, d: 2, t: 'list item "Billing, Card •••• •••• •••• 1111 charged"', el: "#row-4", masked: true },
    { ix: 18, d: 2, t: 'list item "Kai Moreno, Lunch on Friday?"', el: "#row-5" },
    { ix: 19, d: 2, t: 'list item "Ana Ruiz, Re: Roadmap"', el: "#row-6" },
    { ix: 20, d: 1, t: 'text "3 messages selected"', el: "#read-a" },
  ],

  // screen #2 — the change report after `click … element_index: 4`.
  // New window, new numbers: they never collide with screen #1's
  // (test dialog_is_followed_and_main_window_recognised_after).
  rep2: [
    '21 dialog "Confirm"',
    ' 22 text "Delete 3 messages?"',
    ' 23 text "They will be moved to Trash."',
    ' 24 button "Cancel"',
    ' 25 button "OK"',
  ],

  // back on screen #1 — only what changed since the model last saw it.
  rep3: [
    { mk: "~", ix: 7, t: 'tree item "Inbox" value="9" (selected)', was: 'tree item "Inbox" value="12" (selected)' },
    { mk: "-", ix: 14, t: 'list item "Weekly digest, Issue 41" (selected)' },
    { mk: "-", ix: 15, t: 'list item "Weekly digest, Issue 42" (selected)' },
    { mk: "-", ix: 16, t: 'list item "Weekly digest, Issue 43" (selected)' },
    { mk: "~", ix: 20, t: 'text "No message selected"', was: 'text "3 messages selected"' },
  ],

  // the call path (screen space). Backend actions verified in
  // cu/macos/mod.rs:486 (AXPress), cu/windows/mod.rs:282 (Invoke),
  // cu/linux/atspi.rs:228 (DoAction).
  pipe: {
    top: 300,
    bottom: 640,
    xs: [110, 470, 900, 1290, 1700],
    callNodes: [
      { t: "agent", s: ["any MCP client"] },
      { t: "computer-use-mcp", s: ["MCP · JSON-RPC 2.0 · stdio"] },
      { t: "engine", s: ["element 4 → its handle"] },
      { t: "backend", s: ["macOS · <b>AXPress</b>", "Windows · <b>UIA Invoke</b>", "Linux · <b>AT-SPI DoAction</b>"] },
      { t: "Mail", s: ['button "Delete"'] },
    ],
    callEdges: [
      { t: 'click {"element_index":4}', call: true },
      { t: "Engine::call_tool" },
      { t: "perform_action(h, native)" },
      { t: "accessibility press" },
    ],
    retNodes: [
      null,
      { t: "diff", s: ["against what the model saw"] },
      { t: "verify", s: ["did the press take effect?"] },
      { t: "settle", s: ["re-read until two reads agree"] },
      null,
    ],
    result: 'State after the action: now on screen #2 (new), window "Confirm":',
  },

  // documented MCP clients (docs/CONNECT.md, examples/)
  clients: ["Claude Code", "Codex", "Cursor", "VS Code", "Claude Desktop"],

  // the 25 built-in tools (cu/tools.rs:1132-1158)
  tools: [
    "list_apps", "launch_app", "get_app_state", "click", "perform_secondary_action",
    "set_value", "select_text", "scroll", "drag", "draw", "trace_image", "design",
    "scene", "locate", "press_key", "type_text", "find_element", "wait_for",
    "screenshot", "batch", "get_clipboard", "set_clipboard", "window",
    "get_notifications", "script",
  ],
};
