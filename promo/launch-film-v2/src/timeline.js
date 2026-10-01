// Zero Use Computer — film 2. One paused, seekable GSAP timeline.
// Every event is placed in BEATS (cues.json) on the grid of the music, a real
// track at 130 BPM, so picture and music share one grid. Layout is measured once after fonts load.
(function () {
  const C = window.CUES;
  const S = window.SCENE;
  const BEAT = 60 / C.bpm;
  const B = (b) => b * BEAT; // beats -> seconds
  const NS = "http://www.w3.org/2000/svg";
  const $ = (s, r) => (r || document).querySelector(s);
  const $$ = (s, r) => Array.from((r || document).querySelectorAll(s));
  const COL = { work: "#1e88e5", think: "#d4a017", pause: "#78909c", stop: "#ff6d00", done: "#2e7d32" };

  const F_FULL = { x: 0, y: 0, scale: 1 };
  const F_MID = { x: 153.6, y: 26, scale: 0.84 };
  const F_PUSH = { x: 48, y: 24, scale: 0.95 };
  const F_FAR = { x: 672, y: 378, scale: 0.3 };
  const toScreen = (f, p) => ({ x: f.x + f.scale * p.x, y: f.y + f.scale * p.y });

  function el(tag, attrs, parent) {
    const n = document.createElementNS(NS, tag);
    for (const k in attrs) n.setAttribute(k, attrs[k]);
    if (parent) parent.appendChild(n);
    return n;
  }
  function div(cls, html, parent, style) {
    const n = document.createElement("div");
    if (cls) n.className = cls;
    if (html != null) n.innerHTML = html;
    if (style) Object.assign(n.style, style);
    if (parent) parent.appendChild(n);
    return n;
  }
  const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;");

  function build() {
    const tl = gsap.timeline({ paused: true });
    const scr = $("#screen").getBoundingClientRect();
    const k = scr.width / 1920;
    const box = (sel) => {
      const r = $(sel).getBoundingClientRect();
      return { x: (r.left - scr.left) / k, y: (r.top - scr.top) / k, w: r.width / k, h: r.height / k };
    };
    const center = (b) => ({ x: b.x + b.w / 2, y: b.y + b.h / 2 });

    const set = (t, v, at) => tl.set(t, v, B(at));
    const to = (t, v, at) => tl.to(t, v, B(at));
    const ft = (t, a, b, at) => tl.fromTo(t, a, Object.assign({ immediateRender: false }, b), B(at));
    const cam = (f, at, dur, ease) => to("#camera", { x: f.x, y: f.y, scale: f.scale, duration: dur, ease: ease || "expo.inOut" }, at);
    const cut = (id, at) => set(id, { opacity: 0 }, at); // hard cut out, on the beat

    let labelNow = null;
    const label = (state, at) => {
      const id = { work: "#lbl-work", think: "#lbl-think", pause: "#lbl-pause", stop: "#lbl-stop", done: "#lbl-done" }[state];
      if (labelNow) to(labelNow, { opacity: 0, duration: 0.12 }, at);
      ft(id, { opacity: 0, scale: 0.94 }, { opacity: 1, scale: 1, duration: 0.2, ease: "power3.out" }, at);
      to("#glow", { "--st": COL[state], duration: 0.18, ease: "none" }, at);
      to("#zcur", { "--ring": COL[state], duration: 0.18, ease: "none" }, at);
      labelNow = id;
    };
    const glide = (p, at, dur, ease) => to("#zcur", { x: p.x, y: p.y, duration: dur, ease: ease || "power2.out" }, at);
    const zClick = (at, target) => {
      to("#zarrow", { scale: 0.84, svgOrigin: "0 0", duration: 0.06, ease: "power2.in", yoyo: true, repeat: 1 }, at);
      if (target) to(target, { scale: 0.94, duration: 0.06, ease: "power2.in", yoyo: true, repeat: 1 }, at);
      ft("#zrip", { attr: { r: 5, "stroke-width": 3 }, opacity: 1 }, { attr: { r: 27, "stroke-width": 1.5 }, opacity: 0, duration: 0.45, ease: "none" }, at);
      ft("#zripdot", { attr: { r: 4 }, opacity: 0.8 }, { attr: { r: 0 }, opacity: 0, duration: 0.45, ease: "none" }, at);
    };
    const flash = (at, peak, dur) => {
      ft("#flash", { opacity: peak }, { opacity: 0, duration: dur || 0.35, ease: "power2.out" }, at);
    };
    // distinct entrances for the kinetic words
    const slam = (id, at, from) => ft(id, Object.assign({ opacity: 1, scale: 1.35, filter: "blur(14px)" }, from || {}), { scale: 1, filter: "blur(0px)", duration: 0.22, ease: "power4.out" }, at);
    const snap = (id, at, x) => ft(id, { opacity: 1, x: x || -520 }, { x: 0, duration: 0.2, ease: "expo.out" }, at);
    const rise = (id, at) => ft(id, { opacity: 1, y: 160, rotation: -4 }, { y: 0, rotation: 0, duration: 0.24, ease: "power4.out" }, at);
    const chipIn = (id, at) => ft(id, { opacity: 0, x: 60 }, { opacity: 1, x: 0, duration: 0.18, ease: "expo.out" }, at);
    const chipOut = (id, at) => to(id, { opacity: 0, duration: 0.1 }, at);
    const wash = (color, op, at, dur) => {
      set("#wash", { backgroundColor: color }, at);
      ft("#wash", { opacity: op * 1.6 }, { opacity: op, duration: dur || 0.25, ease: "power2.out" }, at);
    };

    // ------------------------------------------------------------ measure
    const winB = box("#win");
    const del = center(box("#tb-delete"));
    const ok = center(box("#dlg-ok"));
    const search = center(box(".tb-search"));
    const card = box("#cardno");
    const treeB = S.tree.map((n) => box(n.el));
    const rowB = ["#row-1", "#row-2", "#row-3"].map(box);

    // ------------------------------------------------------------ initial states
    set("#camera", { x: 0, y: 0, scale: 1, opacity: 1, filter: "blur(0px)" }, 0);
    set("#dim", { opacity: 1 }, 0);
    set(["#fade", "#flash", "#wash", "#grid"], { opacity: 0 }, 0);
    set("#ucur", { x: 960, y: 540, opacity: 0 }, 0);
    set("#zcur", { x: 2100, y: 1200, opacity: 0, "--ring": COL.work }, 0);
    set("#glow", { opacity: 0, "--st": COL.work }, 0);
    set(".lbl", { xPercent: -50, opacity: 0 }, 0);
    set("#idlebar i", { scaleX: 0 }, 0);
    set([".k", ".m", "#chip .c", "#keys", "#ticker", "#cards", "#dd", "#tools", "#os3", "#clients", "#mark", "#wm-name", "#repo", "#credit", "#freedoms span"], { opacity: 0 }, 0);
    set("#os span", { y: 130 }, 0);
    set("#maskbar", { x: card.x - 3, y: card.y - 2, width: card.w + 6, height: card.h + 4, background: "#808080", scaleX: 0, opacity: 0 }, 0);

    // ------------------------------------------------------------ build: element boxes
    const marks = $("#marks");
    const mk = S.tree.map((n, i) => {
      const b = treeB[i];
      const g = el("g", { class: "mk" }, marks);
      el("rect", { class: "mbox", x: b.x + 1, y: b.y + 1, width: Math.max(2, b.w - 2), height: Math.max(2, b.h - 2) }, g);
      const tg = el("g", { class: "mtag" }, g);
      const label = String(n.ix);
      el("rect", { x: b.x, y: b.y, width: 12 + label.length * 10.5, height: 22 }, tg);
      const tx = el("text", { x: b.x + 6, y: b.y + 17 }, tg);
      tx.textContent = label;
      return g;
    });
    set(mk, { opacity: 0 }, 0);
    // tags that fly out when rows are removed, and old tags that come back
    const minus = rowB.map((b, i) => {
      const g = el("g", { class: "mk-tag" }, marks);
      el("rect", { x: b.x + 230, y: b.y + 26, width: 96, height: 34 }, g);
      const t = el("text", { x: b.x + 240, y: b.y + 51 }, g);
      t.textContent = `- ${14 + i}`;
      t.style.fontSize = "24px";
      return g;
    });
    set(minus, { opacity: 0 }, 0);
    const keep = [0, 1, 4, 6, 7, 12, 13, 17, 18, 19, 20];
    const back = keep.map((ix) => {
      const b = treeB[ix];
      const y = b.y + (ix >= 17 && ix <= 19 ? -3 * 84 : 0);
      const g = el("g", { class: "mk-tag" + (ix === 7 || ix === 20 ? " blue" : "") }, marks);
      const lbl = String(ix);
      el("rect", { x: b.x, y, width: 12 + lbl.length * 10.5, height: 22 }, g);
      const tx = el("text", { x: b.x + 6, y: y + 17 }, g);
      tx.textContent = lbl;
      return g;
    });
    set(back, { opacity: 0 }, 0);

    // ------------------------------------------------------------ build: tree ticker
    const col = $("#tk-col");
    S.tree.forEach((n) => {
      let body = esc(n.t);
      div("ln", `${" ".repeat(n.d)}<span class="ix">${n.ix}</span> ${body}`, col);
    });
    const lineY = (i) => 498 - i * 84;

    // ------------------------------------------------------------ build: call path pages
    const strip = $("#path-strip");
    const P = S.pipe;
    const pages = [
      `<div class="lane"></div><div class="t">computer-use-mcp</div><div class="s">MCP · JSON-RPC 2.0 · stdio</div>`,
      `<div class="lane"></div><div class="t">engine</div><div class="s">element 4 → its handle</div>`,
      `<div class="lane"></div><div class="tri"><div id="tri1">macOS<i>AXPress</i></div><div id="tri2">Windows<i>UIA Invoke</i></div><div id="tri3">Linux<i>AT-SPI DoAction</i></div></div>`,
      `<div class="lane"></div><div class="btn" id="bigdel">Delete</div><div class="s">accessibility press, not a mouse click</div>`,
      `<div class="lane"></div><div class="ret"><div id="r1"><b>settle</b> · re-read until two reads agree</div><div id="r2"><b>verify</b> · did the press take effect?</div><div id="r3"><b>diff</b> · against what the model saw</div><div id="r4"><b>State after the action:</b></div></div>`,
    ];
    pages.forEach((h, i) => div("pg", h, strip, { left: i * 1920 + "px" }));

    // ------------------------------------------------------------ build: montage
    const tools = $("#tools");
    S.tools.forEach((t) => div("", esc(t), tools));
    const toolEls = $$("#tools div");
    set(toolEls, { opacity: 0 }, 0);
    const clients = $("#clients");
    const csvg = el("svg", { width: 1920, height: 1080, viewBox: "0 0 1920 1080" }, clients);
    S.clients.forEach((c, i) => {
      const y = 250 + i * 135;
      div("cl", esc(c), clients, { top: y + "px" });
      el("path", { d: `M ${780} ${y + 35} C 980 ${y + 35}, 1000 ${538}, 1160 ${538}` }, csvg);
    });
    div("to", "computer-use-mcp", clients);
    const clEls = $$("#clients .cl");
    set(clEls, { opacity: 0 }, 0);
    set($$("#clients svg path"), { opacity: 0 }, 0);
    set($("#clients .to"), { opacity: 0 }, 0);

    // ====================================================================
    // INTRO — seeing isn't using                               B0 – B8
    // ====================================================================
    to("#ucur", { opacity: 1, duration: 0.1 }, C.intro_tick);
    slam("#k-aican", C.w_aican);
    cut("#k-aican", C.w_see0);
    snap("#k-see0", C.w_see0);
    cut("#k-see0", C.w_yourscreen);
    rise("#k-your", C.w_yourscreen);
    to("#dim", { opacity: 0.62, duration: 0.15 }, C.w_yourscreen);
    // the screenshot: the frame freezes into a picture
    ft("#shot1", { opacity: 1 }, { opacity: 0, duration: 0.4, ease: "power2.out" }, C.shot0);
    flash(C.shot0, 0.35, 0.3);
    cut("#k-your", C.w_seeing);
    ft("#k-seeing", { opacity: 1, fontStretch: "62%" }, { fontStretch: "125%", duration: 0.3, ease: "expo.out" }, C.w_seeing);
    cut("#k-seeing", C.w_isnt);
    slam("#k-isnt", C.w_isnt, { scale: 1.7 });
    cut("#k-isnt", C.w_using);
    ft(["#k-using-a", "#k-using-b"], { opacity: 1, scale: 1.15 }, { scale: 1, duration: 0.18, ease: "power4.out" }, C.w_using);
    // Zero's cursor slices through the word
    set("#zcur", { x: 2050, y: 980, opacity: 1 }, C.slice);
    glide({ x: 640, y: 300 }, C.slice, 0.2, "power3.in");
    to("#k-using-a", { x: -90, y: -50, rotation: -3, opacity: 0, duration: 0.32, ease: "power3.out" }, C.slice + 0.45);
    to("#k-using-b", { x: 90, y: 50, rotation: 3, opacity: 0, duration: 0.32, ease: "power3.out" }, C.slice + 0.45);
    to("#ucur", { opacity: 0, duration: 0.1 }, C.slice);
    to("#dim", { opacity: 1, duration: 0.12 }, C.gap);
    set("#zcur", { opacity: 0 }, C.gap);

    // ====================================================================
    // DROP — Zero's cursor, close enough to touch                B8 – B16
    // ====================================================================
    const tip = { x: 1180, y: 600 };
    const MACRO = { scale: 7, x: 960 - 7 * (tip.x + 32), y: 540 - 7 * (tip.y + 36) };
    set("#zcur", { x: tip.x, y: tip.y, opacity: 1 }, C.drop);
    set("#camera", MACRO, C.drop);
    set("#dim", { opacity: 0 }, C.drop);
    set("#glow", { opacity: 1 }, C.drop);
    label("work", C.drop);
    flash(C.drop, 0.55, 0.45);
    cam(F_MID, C.macro_out, B(2), "expo.inOut");
    to("#grid", { opacity: 1, duration: 0.6 }, C.drop + 1);
    // the user's request, typed big, then it becomes the chip
    // left-aligned so the caret can follow the typing; centred by measurement
    const askW = $("#ask-text").getBoundingClientRect().width / k;
    const askPad = Math.round((1920 - askW) / 2);
    $("#m-ask").style.paddingLeft = askPad + "px";
    set("#ask-caret", { x: askPad + 6 }, 0);
    to("#ask-caret", { x: askPad + askW + 6, duration: B(1.25), ease: "steps(29)" }, C.task);
    ft("#m-ask", { opacity: 1, scaleY: 0 }, { scaleY: 1, duration: 0.12, ease: "expo.out" }, C.task);
    ft("#ask-text", { clipPath: "inset(0 100% 0 0)" }, { clipPath: "inset(0 0% 0 0)", duration: B(1.25), ease: "steps(29)" }, C.task);
    ft("#ask-caret", { opacity: 1 }, { opacity: 0, duration: 0.01, repeat: 1, yoyo: true, repeatDelay: 0.2 }, C.task + 1.3);
    to("#m-ask", { y: 440, x: 330, scale: 0.42, opacity: 0, duration: B(0.5), ease: "power3.in" }, C.task + 1.6);
    chipIn("#c-task", C.task + 2);
    set("#task-text", { clipPath: "inset(0 0% 0 0)" }, C.task + 2);
    set("#caret", { opacity: 0 }, C.task + 2);
    label("think", C.think1);
    label("work", C.call_state);
    chipOut("#c-task", C.call_state);
    chipIn("#c-state", C.call_state);
    cam(F_PUSH, C.push, B(2), "power2.inOut");

    // ====================================================================
    // SEE / UNDERSTAND                                           B16 – B24
    // ====================================================================
    flash(C.capture, 0.6, 0.4);
    ft("#capframe", { opacity: 1 }, { opacity: 0, duration: 0.45 }, C.capture);
    slam("#k-see", C.capture, { scale: 1.5 });
    cut("#k-see", C.boxes + 0.25);
    to("#maskbar", { opacity: 1, scaleX: 1, duration: 0.1, ease: "power4.out" }, C.mask);
    mk.forEach((g, i) => ft(g, { opacity: 0, scale: 1.3, transformOrigin: "50% 50%" }, { opacity: 1, scale: 1, duration: 0.1, ease: "power3.out" }, C.boxes + i * 0.12));
    // whip into the tree
    const WHIP = { x: F_PUSH.x - 2400, y: F_PUSH.y, scale: F_PUSH.scale };
    cam(WHIP, C.whip_tree, B(0.5), "power3.in");
    ft("#camera", { filter: "blur(0px)" }, { filter: "blur(18px)", duration: B(0.5), ease: "power2.in" }, C.whip_tree);
    ft("#ticker", { opacity: 1, x: 1920 }, { x: 0, duration: B(0.5), ease: "power3.out" }, C.whip_tree + 0.25);
    ft("#tk-col", { y: lineY(20) - 900, filter: "blur(10px)" }, { y: lineY(4), filter: "blur(0px)", duration: B(2), ease: "expo.out" }, C.ticker);
    ft("#tk-bar", { scaleX: 0 }, { scaleX: 1, duration: 0.25, ease: "power3.out" }, C.land4);
    set("#tk-bar", { scaleX: 0 }, 0);
    to("#tk-col", { opacity: 0.35, duration: 0.15 }, C.w_und);
    slam("#k-und", C.w_und, { scale: 1.25 });
    wash(COL.think, 0.16, C.think2, 0.3);
    to("#wash", { opacity: 0, duration: 0.2 }, C.call_click - 0.2);

    // ====================================================================
    // ACT — the click becomes the call path                     B24 – B32
    // ====================================================================
    cut("#k-und", C.call_click);
    to("#ticker", { x: -1920, duration: B(0.5), ease: "power3.in" }, C.call_click);
    set("#camera", { x: F_MID.x + 2400, y: F_MID.y, scale: F_MID.scale, filter: "blur(18px)" }, C.call_click);
    cam(F_MID, C.call_click, B(0.5), "power3.out");
    to("#camera", { filter: "blur(0px)", duration: B(0.5), ease: "power2.out" }, C.call_click);
    chipOut("#c-state", C.call_click);
    chipIn("#c-click4", C.call_click);
    to(mk, { opacity: 0, duration: 0.1 }, C.call_click);
    to("#maskbar", { opacity: 0, duration: 0.1 }, C.call_click);
    glide(del, C.dart, B(0.45), "power2.out");
    zClick(C.click1, "#tb-delete");
    slam("#k-act", C.click1, { scale: 1.6 });
    cut("#k-act", C.n_mcp);
    const D = toScreen(F_MID, del);
    set("#path", { clipPath: `circle(0px at ${D.x}px ${D.y}px)` }, 0);
    to("#path", { clipPath: `circle(2400px at ${D.x}px ${D.y}px)`, duration: B(0.75), ease: "power2.in" }, C.iris);
    set("#camera", { opacity: 0 }, C.n_mcp + 0.1);
    // one stop per beat
    const stops = [C.n_mcp, C.n_engine, C.n_os, C.n_press, C.n_settle];
    stops.forEach((at, i) => {
      if (i > 0) {
        to("#path-strip", { x: -i * 1920, duration: B(0.42), ease: "expo.inOut" }, at - 0.21);
        ft("#path-strip", { filter: "blur(0px)" }, { filter: "blur(16px)", duration: B(0.21), ease: "power2.in", yoyo: true, repeat: 1 }, at - 0.21);
      }
      ft("#path-pkt", { x: 160, y: 300, opacity: 1 }, { x: 1760, duration: B(0.9), ease: "power2.in" }, at);
    });
    set("#path-pkt", { opacity: 0 }, 0);
    set("#path-strip", { x: 0 }, 0);
    ["#tri1", "#tri2", "#tri3"].forEach((t, i) => ft(t, { opacity: 0, y: 60 }, { opacity: 1, y: 0, duration: 0.15, ease: "expo.out" }, C.n_os + i * 0.33));
    to("#bigdel", { scale: 0.93, duration: 0.07, ease: "power2.in", yoyo: true, repeat: 1 }, C.n_press + 0.25);
    ft("#bigdel", { backgroundColor: "#20242a" }, { backgroundColor: "#2e3540", duration: 0.07, yoyo: true, repeat: 1 }, C.n_press + 0.25);
    slam("#k-ver", C.w_ver, { scale: 1.3 });
    cut("#k-ver", C.n_verify);
    [["#r1", C.n_verify], ["#r2", C.n_verify + 0.375], ["#r3", C.n_diff + 0.25], ["#r4", C.n_result + 0.125]].forEach(([r, at]) => to(r, { opacity: 1, duration: 0.08 }, at));
    set(["#r1", "#r2", "#r3", "#r4"], { opacity: 0.25 }, 0);

    // ====================================================================
    // VERIFY / REMEMBER                                          B32 – B40
    // ====================================================================
    set("#camera", { opacity: 1 }, C.snap_back - 0.4);
    to("#path", { clipPath: `circle(0px at ${D.x}px ${D.y}px)`, duration: B(0.4), ease: "power3.in" }, C.snap_back - 0.4);
    ft("#dlg-shade", { opacity: 0 }, { opacity: 1, duration: 0.1 }, C.snap_back);
    ft("#dlg", { opacity: 0, scale: 0.9 }, { opacity: 1, scale: 1, duration: 0.2, ease: "back.out(1.6)" }, C.snap_back);
    ft("#m-new", { opacity: 1, scaleY: 0 }, { scaleY: 1, duration: 0.12, ease: "expo.out" }, C.snap_back);
    cut("#m-new", C.dart2);
    chipOut("#c-click4", C.snap_back + 0.5);
    chipIn("#c-click25", C.snap_back + 0.5);
    glide(ok, C.dart2, B(0.45), "power2.out");
    zClick(C.click2, "#dlg-ok");
    to("#dlg", { opacity: 0, scale: 0.96, duration: 0.12 }, C.click2 + 0.3);
    to("#dlg-shade", { opacity: 0, duration: 0.15 }, C.click2 + 0.3);
    ["#rw-1", "#rw-2", "#rw-3"].forEach((r, i) => {
      to(r, { height: 0, duration: 0.2, ease: "power3.inOut" }, C.rows + i * 0.5);
      ft(minus[i], { opacity: 1, x: 0 }, { opacity: 0, x: 220, duration: 0.45, ease: "power2.out" }, C.rows + i * 0.5);
    });
    to("#inbox-a", { opacity: 0, duration: 0.08 }, C.rows);
    to("#inbox-b", { opacity: 1, duration: 0.08 }, C.rows);
    to("#read-a", { opacity: 0, duration: 0.08 }, C.rows);
    to("#read-b", { opacity: 1, duration: 0.1 }, C.rows);
    // rewind: screen #2 out, screen #1 back
    set("#card1", { x: -1700 }, 0);
    set("#cards", { opacity: 1 }, C.rewind);
    ft("#card2", { x: 0, filter: "blur(0px)" }, { x: 1700, filter: "blur(20px)", duration: B(0.5), ease: "power3.in" }, C.rewind);
    ft("#card1", { x: -1700, filter: "blur(20px)" }, { x: 0, filter: "blur(0px)", duration: B(0.5), ease: "power3.out" }, C.rewind + 0.5);
    ft("#m-seen", { opacity: 1, scaleY: 0 }, { scaleY: 1, duration: 0.12, ease: "expo.out" }, C.seen);
    cut("#m-seen", C.seen + 0.75);
    set("#cards", { opacity: 0 }, C.seen + 0.75);
    back.forEach((g, i) => ft(g, { opacity: 0, y: -40 }, { opacity: 1, y: 0, duration: 0.12, ease: "power3.out" }, C.seen + 0.75 + i * 0.07));
    slam("#k-rem", C.w_rem, { scale: 1.3 });
    cut("#k-rem", C.call_state2);
    to(back, { opacity: 0, duration: 0.15 }, C.call_state2);
    // the next look: no second picture
    chipOut("#c-click25", C.call_state2);
    chipIn("#c-state2", C.call_state2);
    set("#dd", { opacity: 1 }, C.call_state2);
    set("#camera", { opacity: 0 }, C.call_state2);
    set("#camera", { opacity: 1 }, C.call_search);
    const ddLine = $("#dd-line");
    const ddLen = ddLine.getTotalLength();
    ddLine.style.strokeDasharray = ddLen;
    ft("#dd-line", { strokeDashoffset: ddLen }, { strokeDashoffset: 0, duration: B(1), ease: "power1.inOut" }, C.call_state2);
    ft("#dd-ghost", { opacity: 0, scale: 0.9, svgOrigin: "1060 470" }, { opacity: 1, scale: 1, duration: 0.12, ease: "back.out(2)" }, C.call_state2 + 0.25);
    const ddKeys = [];
    for (let i = 0; i <= 30; i++) {
      const p = ddLine.getPointAtLength((ddLen * i) / 30);
      ddKeys.push({ x: p.x, y: p.y, duration: B(1) / 30, ease: "none" });
    }
    set("#dd-pkt", { x: 120, y: 470, opacity: 0 }, 0);
    set("#dd-pkt", { opacity: 1 }, C.call_state2);
    to("#dd-pkt", { keyframes: ddKeys }, C.call_state2);
    to("#dd-ghost", { opacity: 0.3, duration: 0.15 }, C.notattached);
    ft("#dd-out", { opacity: 0, x: -30 }, { opacity: 1, x: 0, duration: 0.15, ease: "expo.out" }, C.notattached);
    set("#dd", { opacity: 0 }, C.call_search);

    // ====================================================================
    // CONTROL                                                    B40 – B48
    // ====================================================================
    chipOut("#c-state2", C.call_search);
    chipIn("#c-click5", C.call_search);
    const half = { x: del.x + (search.x - del.x) * 0.35, y: del.y + (search.y - del.y) * 0.35 };
    const most = { x: del.x + (search.x - del.x) * 0.7, y: del.y + (search.y - del.y) * 0.7 };
    set("#zcur", { x: ok.x, y: ok.y }, C.call_search - 0.01);
    glide(half, C.call_search, B(0.5), "power1.in");
    // the user's own mouse moves: everything stops
    set("#ucur", { x: 1560, y: 880 }, C.user_in);
    to("#ucur", { opacity: 1, duration: 0.05 }, C.user_in);
    to("#ucur", { x: 1380, y: 720, duration: B(C.user_stop - C.user_in), ease: "power2.out" }, C.user_in);
    label("pause", C.pause);
    wash(COL.pause, 0.22, C.pause, 0.3);
    slam("#k-paused", C.pause, { scale: 1.2 });
    to("#idlebar i", { scaleX: 1, duration: B(C.resume - C.user_stop), ease: "none" }, C.user_stop);
    cut("#k-paused", C.resume);
    to("#wash", { opacity: 0, duration: 0.12 }, C.resume);
    to("#ucur", { opacity: 0, duration: 0.15 }, C.resume);
    label("work", C.resume);
    glide(most, C.resume, B(1), "power1.in");
    // the emergency stop key
    ft("#keys", { opacity: 0, y: 40 }, { opacity: 1, y: 0, duration: 0.12, ease: "expo.out" }, C.keys - 0.25);
    const press = (at) =>
      ["#k-ctrl", "#k-alt", "#k-esc"].forEach((kk, i) => {
        to(kk, { y: 9, borderBottomWidth: 3, duration: 0.04, ease: "power2.in" }, at + i * 0.25);
        to(kk, { y: 0, borderBottomWidth: 12, duration: 0.1, ease: "power2.out" }, at + 0.9 + i * 0.05);
      });
    press(C.keys);
    label("stop", C.stop);
    wash(COL.stop, 0.24, C.stop, 0.3);
    slam("#k-stopped", C.stop, { scale: 1.25 });
    press(C.keys2 - 0.5);
    cut("#k-stopped", C.go);
    to("#wash", { opacity: 0, duration: 0.12 }, C.go);
    to("#keys", { opacity: 0, duration: 0.12 }, C.go);
    label("work", C.go);
    glide(search, C.go, B(0.45), "power2.out");
    zClick(C.click3, ".tb-search");

    // ====================================================================
    // MONTAGE — everything it has                                B48 – B56
    // ====================================================================
    chipOut("#c-click5", C.tools);
    set("#tools", { opacity: 1 }, C.tools);
    set("#camera", { opacity: 0 }, C.tools);
    toolEls.forEach((t, i) => ft(t, { opacity: 0, y: 26 }, { opacity: 1, y: 0, duration: 0.08, ease: "expo.out" }, C.tools + i * 0.075));
    set("#tools", { opacity: 0 }, C.os3);
    set("#os3", { opacity: 1 }, C.os3);
    $$("#os3 .os").forEach((o, i) => ft(o, { opacity: 0, scale: 1.3 }, { opacity: 1, scale: 1, duration: 0.12, ease: "power4.out" }, C.os3 + i * 0.33));
    set($$("#os3 .os"), { opacity: 0 }, 0);
    set("#os3", { opacity: 0 }, C.clients);
    set("#clients", { opacity: 1 }, C.clients);
    clEls.forEach((c, i) => ft(c, { opacity: 0, x: -60 }, { opacity: 1, x: 0, duration: 0.08, ease: "expo.out" }, C.clients + i * 0.125));
    $$("#clients svg path").forEach((p, i) => to(p, { opacity: 1, duration: 0.06 }, C.clients + 0.5 + i * 0.05));
    to("#clients .to", { opacity: 1, duration: 0.08 }, C.clients + 0.75);
    set("#clients", { opacity: 0 }, C.build);
    // the screen again, far away, through every state it can show
    set("#camera", { x: 960 - 0.42 * 960, y: 540 - 0.42 * 540, scale: 0.42, opacity: 1 }, C.build);
    cam(F_FAR, C.build, B(3.5), "power1.inOut");
    set("#glow", { opacity: 1 }, C.build);
    set(".lbl", { opacity: 0 }, C.build);
    const cycle = [COL.work, COL.think, COL.work, COL.pause, COL.work, COL.stop, COL.work, COL.done];
    cycle.forEach((c, i) => {
      set("#glow", { "--st": c }, C.build + i * 0.5);
      set("#zcur", { "--ring": c }, C.build + i * 0.5);
    });
    // collapse into the mark
    const markB = box("#mark");
    const mc = { x: markB.x + markB.w / 2, y: markB.y + markB.h / 2 };
    to("#camera", { x: mc.x - 0.02 * 960, y: mc.y - 0.02 * 540, scale: 0.02, opacity: 0, duration: B(0.5), ease: "power3.in" }, C.collapse);
    set("#morph-r", { attr: { x: 672, y: 378, width: 576, height: 324, rx: 0 }, opacity: 0 }, 0);
    set("#morph-r", { opacity: 1 }, C.collapse);
    to("#morph-r", { attr: { x: mc.x - 100 * (256 / 300), y: mc.y - 100 * (256 / 300), width: 200 * (256 / 300), height: 200 * (256 / 300), rx: 100 * (256 / 300) }, duration: B(0.5), ease: "power3.inOut" }, C.collapse);
    set("#morph-r", { opacity: 0 }, C.hit);
    to("#grid", { opacity: 0.3, duration: 0.4 }, C.collapse);

    // ====================================================================
    // REVEAL                                                     B56 – end
    // ====================================================================
    flash(C.hit, 0.32, 0.35);
    set("#mark", { opacity: 1 }, C.hit);
    ft("#m-arrow", { scale: 0.3, opacity: 0, svgOrigin: "0 0" }, { scale: 1, opacity: 1, duration: 0.4, ease: "back.out(1.8)" }, C.hit);
    ft("#m-dot", { scale: 0, svgOrigin: "0 0" }, { scale: 1, duration: 0.3, ease: "back.out(2.5)" }, C.hit);
    ft("#m-glow", { opacity: 0, scale: 0.7, svgOrigin: "0 0" }, { opacity: 1, scale: 1.08, duration: 0.25, ease: "power2.out" }, C.hit);
    to("#m-glow", { opacity: 0.55, scale: 1, duration: 0.8, ease: "power2.out" }, C.hit + 0.6);
    ft("#wm-name", { opacity: 1, x: 120, filter: "blur(12px)" }, { x: 0, filter: "blur(0px)", duration: 0.35, ease: "expo.out" }, C.hit);
    to("#os span", { y: 0, duration: 0.3, ease: "expo.out" }, C.opensource);
    ["#f1", "#f2", "#f3", "#f4"].forEach((f, i) => ft(f, { opacity: 0, scale: 1.25 }, { opacity: 1, scale: 1, duration: 0.15, ease: "power4.out" }, [C.read, C.run, C.change, C.ship][i]));
    ft("#repo", { opacity: 0, y: 14 }, { opacity: 1, y: 0, duration: 0.3, ease: "power3.out" }, C.repo);
    // the music's licence asks for credit; it sits under the repository line
    ft("#credit", { opacity: 0 }, { opacity: 1, duration: 0.3, ease: "power1.out" }, C.repo + 0.5);
    to("#fade", { opacity: 1, duration: B(C.duration_beats - C.end_fade), ease: "power1.in" }, C.end_fade);

    // deliberate layering, marked for the layout audit
    const allow = (sel, attrs) => $$(sel).forEach((n) => attrs.forEach((a) => n.setAttribute(a, "")));
    allow(".k, .m, .ztag-text, #marks text, #chip .c, .key, .kplus", ["data-layout-allow-overlap", "data-layout-allow-occlusion"]);
    allow("#os span, #ticker, #tk-col, #tk-col .ln, #tk-col span", ["data-layout-allow-overflow", "data-layout-allow-overlap"]);
    return tl;
  }

  const fonts = ['900 300px "Archivo"', '400 50px "Archivo"', '500 24px "Archivo"', '400 60px "JetBrains Mono"', '700 60px "JetBrains Mono"'];
  // A GSAP timeline is a thenable: wrap it so the promise doesn't wait on it.
  window.__filmReady = Promise.all(fonts.map((f) => document.fonts.load(f)))
    .then(() => document.fonts.ready)
    .then(() => ({ tl: build() }));
})();
