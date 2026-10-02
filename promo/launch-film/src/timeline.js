// Zero Use Computer launch film — one paused, seekable GSAP timeline built from cues.json.
// Layout is measured once (after fonts load, camera at identity); every tween
// then uses those constants, never tween-time DOM reads.
(function () {
  const C = window.CUES;
  const S = window.SCENE;
  const NS = "http://www.w3.org/2000/svg";
  const $ = (s, r) => (r || document).querySelector(s);
  const $$ = (s, r) => Array.from((r || document).querySelectorAll(s));

  // camera framings: screen point = (x + s * worldX, y + s * worldY)
  const F_FULL = { x: 0, y: 0, scale: 1 };
  const F_MID = { x: 153.6, y: 26, scale: 0.84 };
  const F_LEFT = { x: 60, y: 150, scale: 0.55 };
  const F_FAR = { x: 672, y: 378, scale: 0.3 };
  const toScreen = (f, p) => ({ x: f.x + f.scale * p.x, y: f.y + f.scale * p.y });

  // overlay state colours (config_template.toml [overlay])
  const COL = { work: "#1e88e5", think: "#d4a017", pause: "#78909c", stop: "#ff6d00", done: "#2e7d32" };

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
    const k = scr.width / 1920; // preview may scale the page
    const box = (sel) => {
      const r = $(sel).getBoundingClientRect();
      return { x: (r.left - scr.left) / k, y: (r.top - scr.top) / k, w: r.width / k, h: r.height / k };
    };
    const center = (b) => ({ x: b.x + b.w / 2, y: b.y + b.h / 2 });

    // ------------------------------------------------------------ helpers
    const set = (t, v, at) => tl.set(t, v, at);
    const to = (t, v, at) => tl.to(t, v, at);
    const ft = (t, a, b, at) => tl.fromTo(t, a, Object.assign({ immediateRender: false }, b), at);
    const cam = (f, at, dur, ease) =>
      to("#camera", { x: f.x, y: f.y, scale: f.scale, duration: dur, ease: ease || "power2.inOut" }, at);
    const wordIn = (id, at) => ft(id, { y: 125, opacity: 1 }, { y: 0, duration: 0.5, ease: "power4.out" }, at);
    const wordOut = (id, at) => to(id, { y: -125, opacity: 0, duration: 0.32, ease: "power3.in" }, at);
    const chipIn = (id, at) => ft(id, { y: 14, opacity: 0 }, { y: 0, opacity: 1, duration: 0.3, ease: "power3.out" }, at);
    const chipOut = (id, at) => to(id, { y: -10, opacity: 0, duration: 0.2, ease: "power2.in" }, at);

    let labelNow = null;
    const label = (state, at) => {
      const id = { work: "#lbl-work", think: "#lbl-think", pause: "#lbl-pause", stop: "#lbl-stop", done: "#lbl-done" }[state];
      if (labelNow) to(labelNow, { opacity: 0, duration: 0.18, ease: "power1.out" }, at);
      ft(id, { opacity: 0, scale: 0.97 }, { opacity: 1, scale: 1, duration: 0.25, ease: "power2.out" }, at + 0.05);
      // transition_ms = 300: the state colour blends
      to("#glow", { "--st": COL[state], duration: 0.3, ease: "none" }, at);
      to("#zcur", { "--ring": COL[state], duration: 0.3, ease: "none" }, at);
      labelNow = id;
    };
    const glide = (p, at, dur, ease) => to("#zcur", { x: p.x, y: p.y, duration: dur, ease: ease || "power2.out" }, at);
    // draw.rs ripple: r (5 + 22t)·S, width 3·S·(1 − t/2), opacity 1 − t, 450 ms
    const zClick = (at, target) => {
      to("#zarrow", { scale: 0.86, svgOrigin: "0 0", duration: 0.07, ease: "power2.in", yoyo: true, repeat: 1 }, at);
      if (target) to(target, { scale: 0.95, duration: 0.07, ease: "power2.in", yoyo: true, repeat: 1 }, at);
      ft("#zrip", { attr: { r: 5, "stroke-width": 3 }, opacity: 1 }, { attr: { r: 27, "stroke-width": 1.5 }, opacity: 0, duration: 0.45, ease: "none" }, at + 0.02);
      ft("#zripdot", { attr: { r: 4 }, opacity: 0.8 }, { attr: { r: 0 }, opacity: 0, duration: 0.45, ease: "none" }, at + 0.02);
    };

    // ------------------------------------------------------------ measure
    const winB = box("#win");
    const del = center(box("#tb-delete"));
    const okB = box("#dlg-ok");
    const ok = center(okB);
    const card = box("#cardno");
    const treeB = S.tree.map((n) => box(n.el));
    const dlgEls = ["#dlg", "#dlg .dlg-text", "#dlg .dlg-sub", "#dlg-cancel", "#dlg-ok"].map(box);

    // ------------------------------------------------------------ initial states (t = 0)
    set("#camera", { x: 0, y: 0, scale: 1 }, 0);
    set("#dim", { opacity: 1 }, 0);
    set("#fade", { opacity: 0 }, 0);
    set("#topfade", { opacity: 0 }, 0);
    set("#grid", { opacity: 0 }, 0);
    set("#ucur", { x: 1560, y: 650, opacity: 0 }, 0);
    set("#zcur", { x: 2010, y: 780, opacity: 0, "--ring": COL.work }, 0);
    set("#glow", { opacity: 0, "--st": COL.work }, 0);
    set(".lbl", { xPercent: -50, opacity: 0 }, 0);
    set("#idlebar i", { scaleX: 0 }, 0);
    set([".hook-line span", ".tg-line span"], { y: 140 }, 0);
    set([".word"], { y: 125, opacity: 0 }, 0);
    set("#chip .c", { opacity: 0 }, 0);
    set(["#tree", "#rep2", "#rep3", "#mem"], { opacity: 0 }, 0);
    set("#pipe", { clipPath: "circle(0px at 794px 199px)" }, 0);
    set(["#os span"], { y: 124 }, 0);
    set(["#freedoms span"], { y: 64 }, 0);
    set("#repo", { opacity: 0, y: 12 }, 0);
    set("#wm-name", { opacity: 0 }, 0);
    set("#mark", { opacity: 0 }, 0);
    set("#maskbar", { x: card.x - 3, y: card.y - 2, width: card.w + 6, height: card.h + 4, background: "#808080", scaleX: 0, opacity: 0 }, 0);

    // ------------------------------------------------------------ build: detection marks (screen #1)
    const marks = $("#marks");
    const mk = S.tree.map((n, i) => {
      const b = treeB[i];
      const g = el("g", { class: "mk" + (n.target ? " target" : "") }, marks);
      const r = el("rect", { class: "mbox", x: b.x + 1, y: b.y + 1, width: Math.max(2, b.w - 2), height: Math.max(2, b.h - 2) }, g);
      const per = 2 * (b.w + b.h);
      r.style.strokeDasharray = per;
      r.style.strokeDashoffset = per;
      const label = String(n.ix);
      const tw = 12 + label.length * 10.5;
      const tg = el("g", { class: "mtag" }, g);
      el("rect", { x: b.x, y: b.y, width: tw, height: 22 }, tg);
      const tx = el("text", { x: b.x + 6, y: b.y + 17 }, tg);
      tx.textContent = label;
      return { g, r, per, tg, b };
    });
    // the target's hot overlay
    const tb = treeB[4];
    const hot = el("g", { id: "mk-hot" }, marks);
    el("rect", { x: tb.x - 3, y: tb.y - 3, width: tb.w + 6, height: tb.h + 6, fill: "none", stroke: COL.work, "stroke-width": 3.5 }, hot);
    set("#mk-hot", { opacity: 0 }, 0);

    // dialog marks (screen #2, indices 21-25)
    const dmarks = el("g", { id: "dmarks" }, marks);
    dlgEls.forEach((b, i) => {
      const g = el("g", { class: "mk" }, dmarks);
      el("rect", { class: "mbox", x: b.x + 1, y: b.y + 1, width: b.w - 2, height: b.h - 2 }, g);
      const tg = el("g", { class: "mtag" }, g);
      const label = String(21 + i);
      el("rect", { x: b.x, y: b.y, width: 12 + label.length * 10.5, height: 22 }, tg);
      const tx = el("text", { x: b.x + 6, y: b.y + 17 }, tg);
      tx.textContent = label;
    });
    set("#dmarks .mk", { opacity: 0 }, 0);

    // ------------------------------------------------------------ build: tree panel + leaders
    const treeLines = $("#tree-lines");
    S.tree.forEach((n) => {
      const pre = " ".repeat(n.d);
      let body = esc(n.t);
      if (n.masked) body = body.replace("•••• •••• •••• 1111", '<span class="mask">•••• •••• •••• 1111</span>');
      div("ln", `${pre}<span class="ix">${n.ix}</span> ${body}${n.target ? '<i class="hl"></i>' : ""}`, treeLines);
    });
    const rep2Lines = $("#rep2-lines");
    S.rep2.forEach((t) => {
      const m = t.match(/^(\s*)(\d+)(.*)$/);
      div("ln", `${m[1]}<span class="ix">${m[2]}</span>${esc(m[3])}`, rep2Lines);
    });
    const rep3Lines = $("#rep3-lines");
    rep3Lines.classList.add("wrap");
    S.rep3.forEach((d) => {
      const was = d.was ? `  <span class="was">(was: ${esc(d.was)})</span>` : "";
      div("ln", `<span class="mk">${d.mk}</span> <span class="ix">${d.ix}</span> ${esc(d.t)}${was}`, rep3Lines);
    });

    // positions of tree lines (world units; annot is at the world origin)
    const annot = $("#annot").getBoundingClientRect();
    const lineB = $$("#tree-lines .ln").map((n) => {
      const r = n.getBoundingClientRect();
      return { x: (r.left - annot.left) / k, y: (r.top - annot.top) / k, w: r.width / k, h: r.height / k };
    });
    // leaders: a fan from the screen's right edge to each line of the tree
    const leaders = $("#leaders");
    const edgeY = (b) => Math.max(10, Math.min(1070, b.y + Math.min(b.h / 2, 40)));
    const lead = S.tree.map((n, i) => {
      const y0 = edgeY(treeB[i]);
      const L = lineB[i];
      const x0 = 1922;
      const x1 = L.x - 22;
      const y1 = L.y + L.h / 2;
      const p = el("path", { d: `M${x0} ${y0} C ${x0 + 120} ${y0}, ${x1 - 120} ${y1}, ${x1} ${y1}` }, leaders);
      const len = p.getTotalLength();
      p.style.strokeDasharray = len;
      p.style.strokeDashoffset = len;
      return { p, len };
    });
    // the chosen element: from its box, across the screen, into its line
    const tbh = treeB[4];
    const hy = tbh.y + tbh.h / 2;
    const L4 = lineB[4];
    const hotLead = el("path", { class: "hot", d: `M${tbh.x + tbh.w + 4} ${hy} L1922 ${hy} C 2042 ${hy}, ${L4.x - 142} ${L4.y + L4.h / 2}, ${L4.x - 22} ${L4.y + L4.h / 2}` }, leaders);
    const hotLen = hotLead.getTotalLength();
    hotLead.style.strokeDasharray = hotLen;
    hotLead.style.strokeDashoffset = hotLen;

    // ------------------------------------------------------------ build: the call path
    const P = S.pipe;
    const psvg = $("#pipe-svg");
    const pnodes = $("#pipe-nodes");
    const IRIS = toScreen(F_MID, del);
    // construction circles left by the ripple
    const cons = [180, 360, 620].map((r) => el("circle", { class: "construct", cx: IRIS.x, cy: IRIS.y, r }, psvg));
    const xs = P.xs;
    const topL = el("path", { class: "edge", d: `M${xs[0]} ${P.top} L${xs[4]} ${P.top}` }, psvg);
    const rightL = el("path", { class: "edge", d: `M${xs[4]} ${P.top} L${xs[4]} ${P.bottom}` }, psvg);
    const botL = el("path", { class: "edge", d: `M${xs[4]} ${P.bottom} L${xs[0]} ${P.bottom}` }, psvg);
    const leftL = el("path", { class: "edge", d: `M${xs[0]} ${P.bottom} L${xs[0]} ${P.top}` }, psvg);
    [topL, rightL, botL, leftL].forEach((p) => {
      const len = p.getTotalLength();
      p.style.strokeDasharray = len;
      p.style.strokeDashoffset = len;
      p._len = len;
    });
    // node markers
    const marker = (x, y) => {
      const g = el("g", { class: "pm" }, psvg);
      el("rect", { x: x - 8, y: y - 8, width: 16, height: 16, fill: "#0a0b0d", stroke: "#eceff2", "stroke-width": 2.4 }, g);
      return g;
    };
    const topM = xs.map((x) => marker(x, P.top));
    const botM = xs.map((x, i) => (P.retNodes[i] ? marker(x, P.bottom) : null));
    // labels
    const callN = P.callNodes.map((n, i) =>
      div("pn", `<span class="t">${esc(n.t)}</span><span class="s">${n.s.join("<br>")}</span>`, pnodes, {
        left: xs[i] - 8 + "px",
        top: P.top + 26 + "px",
      }),
    );
    callN[4].style.left = xs[4] - 8 + "px";
    const retN = P.retNodes.map((n, i) =>
      n
        ? div("pn", `<span class="t">${esc(n.t)}</span><span class="s">${n.s.join("<br>")}</span>`, pnodes, {
            left: xs[i] - 8 + "px",
            top: P.bottom + 26 + "px",
          })
        : null,
    );
    const edgeL = P.callEdges.map((e, i) =>
      div("el" + (e.call ? " call" : ""), esc(e.t), pnodes, { left: xs[i] + 22 + "px", top: P.top - 38 + "px" }),
    );
    const result = div("el call", esc(P.result), pnodes, { left: xs[0] - 8 + "px", top: P.bottom + 150 + "px" });
    result.style.fontSize = "26px";
    const laneA = div("lane-title", "the call →", pnodes, { left: xs[0] - 8 + "px", top: P.top - 90 + "px" });
    const laneB = div("lane-title", "← the result", pnodes, { left: xs[3] + "px", top: P.bottom - 50 + "px" });
    const pkt = el("rect", { class: "pkt", x: -8, y: -8, width: 16, height: 16 }, psvg);
    set([topM, botM.filter(Boolean)].flat(), { opacity: 0 }, 0);
    set(pkt, { x: xs[0], y: P.top, opacity: 0 }, 0);

    // ------------------------------------------------------------ build: system map (world units)
    const sm = $("#sysmap");
    const smsvg = el("svg", { class: "sm-svg", width: 7000, height: 4200, viewBox: "-2600 -1500 7000 4200" }, sm);
    smsvg.style.left = "-2600px";
    smsvg.style.top = "-1500px";
    const T = (cls, text, x, y) => div("t " + cls, esc(text), sm, { left: x + "px", top: y + "px" });
    T("cap", "MCP clients", -2160, 130);
    S.clients.forEach((c, i) => {
      const y = 260 + i * 150;
      T("", c, -2160, y);
      el("line", { x1: -1700, y1: y + 29, x2: -1290, y2: 560 }, smsvg);
    });
    T("big", "computer-use-mcp", -1260, 520);
    T("sub", "stdio · HTTP · JSON-RPC 2.0", -1260, 620);
    el("line", { x1: -500, y1: 560, x2: -400, y2: 560 }, smsvg);
    T("big", "engine", -380, 520);
    T("sub", "tree · diffs · memory", -380, 620);
    el("line", { x1: -60, y1: 560, x2: 0, y2: 560 }, smsvg);
    T("cap", "tools/list", -650, -1230);
    S.tools.forEach((t, i) => {
      const col = i % 5;
      const row = Math.floor(i / 5);
      T("tool", t, -650 + col * 760, -1110 + row * 120);
    });
    [["macOS · AX", 60], ["Windows · UI Automation", 700], ["Linux · AT-SPI2", 1480]].forEach(([t, x]) => {
      T("os", t, x, 1330);
      el("line", { x1: x + 120, y1: 1300, x2: x + 120, y2: 1090 }, smsvg);
    });

    // ------------------------------------------------------------ dedupe path samples (for the swerve)
    const ddAround = $("#dd-around");
    const ddLen = ddAround.getTotalLength();
    const ddKeys = [];
    for (let i = 0; i <= 24; i++) {
      const p = ddAround.getPointAtLength((ddLen * i) / 24);
      ddKeys.push({ x: p.x, y: p.y });
    }
    const ddStraight = $("#dd-straight");
    ddStraight.style.strokeDasharray = ddStraight.getTotalLength();
    ddStraight.style.strokeDashoffset = ddStraight.getTotalLength();
    ddAround.style.strokeDasharray = ddLen;
    ddAround.style.strokeDashoffset = ddLen;
    set("#dd-ghost", { opacity: 0 }, 0);
    set("#dd-pkt", { x: 40, y: 60, opacity: 0 }, 0);
    set(".dd-call", { opacity: 0 }, 0);

    // ====================================================================
    // SCENE 1 — hook: seeing isn't using                       0.0 – 4.2
    // ====================================================================
    to("#ucur", { opacity: 1, duration: 0.25 }, C.user_in);
    to("#dim", { opacity: 0.6, duration: 1.0, ease: "power1.inOut" }, C.desk_in);
    to("#hook-1 span", { y: 0, duration: 0.75, ease: "power4.out" }, C.h1_in);
    // a capture: the screen freezes into a picture
    ft("#shot1", { opacity: 0 }, { opacity: 1, duration: 0.04 }, C.capture1);
    to("#shot1", { opacity: 0, duration: 0.45, ease: "power2.out" }, C.capture1 + 0.12);
    ft("#flash1", { opacity: 0 }, { opacity: 0.16, duration: 0.03 }, C.capture1);
    to("#flash1", { opacity: 0, duration: 0.35 }, C.capture1 + 0.05);
    to("#dim", { opacity: 0.74, duration: 0.2 }, C.capture1 + 0.03);
    to("#hook-1 span", { y: -140, duration: 0.42, ease: "power3.in" }, C.h2_in - 0.12);
    to("#hook-2 span", { y: 0, duration: 0.7, ease: "power4.out" }, C.h2_in + 0.12);
    // the user's own pointer drifts away
    to("#ucur", { x: 1640, y: 760, duration: 0.6, ease: "power2.inOut" }, C.zero_enter - 0.35);
    // Zero's own cursor arrives (cubic ease-out, as helper.rs glides)
    to("#zcur", { opacity: 1, duration: 0.25 }, C.zero_enter);
    glide({ x: 1470, y: 520 }, C.zero_enter, 0.55, "power2.out");
    // HIT: the screen becomes operable
    to("#hook-2 span", { y: -140, duration: 0.22, ease: "power3.in" }, C.hit1 - 0.24);
    to("#glow", { opacity: 1, duration: 0.25, ease: "none" }, C.hit1);
    label("work", C.hit1);
    to("#dim", { opacity: 0, duration: 0.55, ease: "power2.out" }, C.hit1);
    ft("#flash1", { opacity: 0 }, { opacity: 0.1, duration: 0.03 }, C.hit1);
    to("#flash1", { opacity: 0, duration: 0.5 }, C.hit1 + 0.04);
    to("#ucur", { opacity: 0, duration: 0.3 }, C.hit1);
    // the screen becomes an object on the drafting table
    cam(F_MID, C.cam_mid, 0.9);
    to("#grid", { opacity: 1, duration: 1.2, ease: "power1.inOut" }, C.cam_mid);

    // ====================================================================
    // SCENE 2 — SEE / UNDERSTAND                              4.2 – 10.6
    // ====================================================================
    chipIn("#c-task", C.task_chip);
    label("think", C.think1);
    label("work", C.call_state);
    chipOut("#c-task", C.call_state - 0.15);
    chipIn("#c-state", C.call_state);
    // the call travels from the agent to the window
    const winS = toScreen(F_MID, center(winB));
    ft("#pkt-s", { x: 1782, y: 1004, opacity: 1 }, { x: winS.x, y: winS.y, duration: 0.3, ease: "power2.in" }, C.call_state + 0.05);
    to("#pkt-s", { opacity: 0, duration: 0.05 }, C.call_state + 0.35);
    // capture
    ft("#capframe", { opacity: 0 }, { opacity: 1, duration: 0.04 }, C.capture2);
    to("#capframe", { opacity: 0, duration: 0.5, ease: "power2.out" }, C.capture2 + 0.1);
    ft("#flash2", { opacity: 0 }, { opacity: 0.14, duration: 0.03 }, C.capture2);
    to("#flash2", { opacity: 0, duration: 0.35 }, C.capture2 + 0.04);
    // privacy: the card number is filled grey before anything is sent
    to("#maskbar", { opacity: 1, scaleX: 1, duration: 0.14, ease: "power4.out" }, C.mask);
    // detection, in tree order
    mk.forEach((m, i) => {
      const at = C.marks + i * 0.045;
      to(m.r, { strokeDashoffset: 0, duration: 0.32, ease: "power2.out" }, at);
      ft(m.tg, { opacity: 0, scale: 0.6, transformOrigin: `${m.b.x}px ${m.b.y}px` }, { opacity: 1, scale: 1, duration: 0.18, ease: "back.out(2)" }, at + 0.04);
    });
    set(mk.map((m) => m.tg), { opacity: 0 }, 0);
    // camera: the screen moves aside for the model's view of it
    cam(F_LEFT, C.cam_left, 0.95);
    wordIn("#w-see", C.w_see);
    set("#tree", { opacity: 1 }, C.tree_lines - 0.25);
    ft("#tree .p-head", { opacity: 0, x: -20 }, { opacity: 1, x: 0, duration: 0.3, ease: "power3.out" }, C.tree_lines - 0.25);
    ft("#tree .p-sub", { opacity: 0 }, { opacity: 1, duration: 0.2 }, C.tree_lines - 0.12);
    $$("#tree-lines .ln").forEach((n, i) => {
      const at = C.tree_lines + i * 0.062;
      ft(n, { opacity: 0, x: -18 }, { opacity: 1, x: 0, duration: 0.24, ease: "power3.out" }, at);
      to(lead[i].p, { strokeDashoffset: 0, duration: 0.45, ease: "power2.inOut" }, at - 0.05);
    });
    set($$("#tree-lines .ln"), { opacity: 0 }, 0);
    wordOut("#w-see", C.w_und - 0.3);
    wordIn("#w-und", C.w_und);
    // the decision: line 4, its box, its leader
    ft("#tree-lines .hl", { scaleX: 0 }, { scaleX: 1, duration: 0.35, ease: "power3.out" }, C.target_hot);
    ft("#mk-hot", { opacity: 0 }, { opacity: 1, duration: 0.2 }, C.target_hot);
    to(hotLead, { strokeDashoffset: 0, duration: 0.5, ease: "power2.inOut" }, C.target_hot);
    set($$("#tree-lines .hl"), { scaleX: 0 }, 0);
    label("think", C.think2);

    // ====================================================================
    // SCENE 3 — ACT: the click becomes the call path          10.6 – 19.3
    // ====================================================================
    label("work", C.call_click);
    chipOut("#c-state", C.call_click - 0.15);
    chipIn("#c-click4", C.call_click);
    wordOut("#w-und", C.call_click);
    to(["#tree", "#leaders"], { opacity: 0, duration: 0.4 }, C.cam_mid2);
    to(["#marks .mk", "#maskbar"], { opacity: 0, duration: 0.35 }, C.cam_mid2 + 0.1);
    to("#mk-hot", { opacity: 0, duration: 0.35 }, C.click1 - 0.1);
    cam(F_MID, C.cam_mid2, 0.8);
    glide(del, C.zero_glide1, 0.62, "power2.out");
    zClick(C.click1, "#tb-delete");
    // the ripple keeps going: the camera dives into it
    const R = 2300;
    to("#pipe", { clipPath: `circle(${R}px at ${IRIS.x}px ${IRIS.y}px)`, duration: 0.85, ease: "power2.in" }, C.iris_open);
    set("#pipe", { clipPath: `circle(0px at ${IRIS.x}px ${IRIS.y}px)` }, 0);
    set(["#iris-ring", "#iris-ring2"], { attr: { cx: IRIS.x, cy: IRIS.y, r: 10 }, opacity: 0 }, 0);
    ft("#iris-ring", { attr: { r: 14 }, opacity: 1 }, { attr: { r: R }, duration: 0.85, ease: "power2.in" }, C.iris_open);
    ft("#iris-ring2", { attr: { r: 8 }, opacity: 1 }, { attr: { r: R * 0.8 }, duration: 0.9, ease: "power2.in" }, C.iris_open + 0.06);
    to(["#iris-ring", "#iris-ring2"], { opacity: 0, duration: 0.15 }, C.iris_open + 0.85);
    const DIVE = { x: IRIS.x - 1.18 * del.x, y: IRIS.y - 1.18 * del.y, scale: 1.18 };
    cam(DIVE, C.iris_open, 0.85, "power2.in");
    // construction circles settle
    cons.forEach((c, i) => ft(c, { attr: { r: 12 }, opacity: 0.9 }, { attr: { r: [180, 360, 620][i] }, opacity: 0.55, duration: 0.9, ease: "expo.out" }, C.iris_open + 0.25 + i * 0.05));
    set("#camera", { opacity: 0 }, C.iris_open + 0.86);
    set("#camera", { opacity: 1 }, C.iris_close);
    wordIn("#w-act", C.w_act);
    ft([laneA], { opacity: 0 }, { opacity: 1, duration: 0.3 }, C.pipe_build);
    to(topL, { strokeDashoffset: 0, duration: 0.9, ease: "power2.inOut" }, C.pipe_build);
    // nodes and the packet along the call
    const nodeAt = [C.pipe_build + 0.1, C.pkt_engine - 0.05, C.pkt_backend - 0.05, C.lanes - 0.15, C.app_press - 0.05];
    topM.forEach((m, i) => ft(m, { opacity: 0, scale: 0.4, svgOrigin: `${xs[i]} ${P.top}` }, { opacity: 1, scale: 1, duration: 0.22, ease: "back.out(2)" }, nodeAt[i]));
    callN.forEach((n, i) => ft(n, { opacity: 0, y: 10 }, { opacity: 1, y: 0, duration: 0.3, ease: "power3.out" }, nodeAt[i] + 0.05));
    set(pkt, { opacity: 1 }, C.pkt_mcp);
    const legs = [C.pkt_mcp, C.pkt_engine, C.pkt_backend, C.pkt_app];
    legs.forEach((at, i) => {
      ft(edgeL[i], { opacity: 0 }, { opacity: 1, duration: 0.2 }, at);
      to(pkt, { x: xs[i + 1], y: P.top, duration: i === 3 ? 0.38 : 0.42, ease: "power1.inOut" }, at);
    });
    // the backend: three operating systems, one press
    // the app is pressed
    ft(topM[4].firstChild, { attr: { fill: "#0a0b0d" } }, { attr: { fill: "#eceff2" }, duration: 0.08, yoyo: true, repeat: 1 }, C.app_press);
    // the result comes back along the bottom lane
    to(rightL, { strokeDashoffset: 0, duration: 0.2, ease: "none" }, C.app_press);
    to(pkt, { y: P.bottom, duration: 0.2, ease: "none" }, C.app_press);
    ft([laneB], { opacity: 0 }, { opacity: 1, duration: 0.3 }, C.ret_settle);
    const retLegs = [
      [C.ret_settle, 3],
      [C.ret_verify, 2],
      [C.ret_diff, 1],
      [C.ret_agent, 0],
    ];
    retLegs.forEach(([at, i]) => {
      to(pkt, { x: xs[i], duration: 0.35, ease: "power1.inOut" }, at);
      if (botM[i]) ft(botM[i], { opacity: 0, scale: 0.4, svgOrigin: `${xs[i]} ${P.bottom}` }, { opacity: 1, scale: 1, duration: 0.22, ease: "back.out(2)" }, at + 0.3);
      if (retN[i]) ft(retN[i], { opacity: 0, y: 10 }, { opacity: 1, y: 0, duration: 0.3, ease: "power3.out" }, at + 0.32);
    });
    // the bottom lane draws behind the packet
    const botLen = botL._len;
    to(botL, { strokeDashoffset: 0, duration: C.ret_agent + 0.35 - C.ret_settle, ease: "none" }, C.ret_settle);
    to(leftL, { strokeDashoffset: 0, duration: 0.3, ease: "power1.inOut" }, C.ret_agent + 0.35);
    ft(result, { opacity: 0, x: -14 }, { opacity: 1, x: 0, duration: 0.35, ease: "power3.out" }, C.ret_agent + 0.25);
    wordOut("#w-act", C.w_ver - 0.32);
    wordIn("#w-ver", C.w_ver);
    // fold back into the button
    to("#pipe", { clipPath: `circle(0px at ${IRIS.x}px ${IRIS.y}px)`, duration: 0.6, ease: "power3.in" }, C.iris_close);
    ft("#iris-ring", { attr: { r: R }, opacity: 1 }, { attr: { r: 6 }, duration: 0.6, ease: "power3.in" }, C.iris_close);
    to("#iris-ring", { opacity: 0, duration: 0.12 }, C.iris_close + 0.6);
    cam(F_MID, C.iris_close, 0.6, "power3.in");
    wordOut("#w-ver", C.iris_close - 0.05);
    // the app answers: its own confirm dialog (a new window)
    ft("#dlg-shade", { opacity: 0 }, { opacity: 1, duration: 0.25 }, C.dialog_in);
    ft("#dlg", { opacity: 0, scale: 0.96 }, { opacity: 1, scale: 1, duration: 0.32, ease: "power3.out" }, C.dialog_in);
    cam(F_LEFT, C.cam_left2, 0.8);
    set("#rep2", { opacity: 1 }, C.rep2_in - 0.1);
    ft("#rep2 .p-head", { opacity: 0, x: -20 }, { opacity: 1, x: 0, duration: 0.3, ease: "power3.out" }, C.rep2_in - 0.1);
    $$("#rep2-lines .ln").forEach((n, i) => ft(n, { opacity: 0, x: -18 }, { opacity: 1, x: 0, duration: 0.24, ease: "power3.out" }, C.rep2_in + 0.12 + i * 0.09));
    set($$("#rep2-lines .ln"), { opacity: 0 }, 0);
    $$("#dmarks .mk").forEach((g, i) => ft(g, { opacity: 0 }, { opacity: 1, duration: 0.2 }, C.rep2_in + 0.12 + i * 0.09));

    // ====================================================================
    // SCENE 4 — CONTROL                                      19.3 – 26.6
    // ====================================================================
    label("think", C.think3);
    cam(F_MID, C.cam_mid3, 0.75);
    to(["#rep2", "#dmarks .mk"], { opacity: 0, duration: 0.35 }, C.cam_mid3);
    chipOut("#c-click4", C.think3);
    label("work", C.call_ok);
    chipIn("#c-click25", C.call_ok);
    // Zero sets off for OK…
    const lerp = (a, b, t) => ({ x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t });
    const p1 = lerp(del, ok, 0.36);
    const p2 = lerp(del, ok, 0.7);
    glide(p1, C.zero_go1, C.pause_on - C.zero_go1, "power1.in");
    // …the user's own mouse moves: everything stops
    set("#ucur", { x: 1530, y: 930 }, C.user_move - 0.01);
    to("#ucur", { opacity: 1, duration: 0.12 }, C.user_move);
    const upath = [
      [1430, 840, 0.4],
      [1490, 770, 0.45],
      [1380, 720, 0.5],
      [1405, 700, 0.55],
    ];
    let ut = C.user_move;
    upath.forEach(([x, y, d]) => {
      to("#ucur", { x, y, duration: d, ease: "sine.inOut" }, ut);
      ut += d;
    });
    label("pause", C.pause_on);
    wordIn("#w-ctl", C.w_ctl);
    // resume_after_idle_ms = 1500: the idle time fills, then it goes on
    to("#idlebar i", { scaleX: 1, duration: C.resume - C.user_stop, ease: "none" }, C.user_stop);
    to("#ucur", { opacity: 0, duration: 0.3 }, C.resume + 0.05);
    label("work", C.resume);
    glide(p2, C.zero_go2, C.stop_key - C.zero_go2, "power1.in");
    // the emergency stop key
    ft("#keys", { opacity: 0, y: 16 }, { opacity: 1, y: 0, duration: 0.18, ease: "power3.out" }, C.stop_key - 0.22);
    const press = (at) => {
      ["#k-ctrl", "#k-alt", "#k-esc"].forEach((kk, i) => {
        to(kk, { y: 5, borderBottomWidth: 2, duration: 0.05, ease: "power2.in" }, at + i * 0.05);
        to(kk, { y: 0, borderBottomWidth: 7, duration: 0.12, ease: "power2.out" }, at + 0.3 + i * 0.03);
      });
    };
    press(C.stop_key - 0.1);
    label("stop", C.stop_key);
    chipOut("#c-click25", C.stop_key);
    chipIn("#c-stop", C.stop_key + 0.1);
    press(C.stop_release - 0.1);
    label("work", C.stop_release);
    chipOut("#c-stop", C.stop_release);
    chipIn("#c-click25", C.stop_release + 0.1);
    to("#keys", { opacity: 0, y: 10, duration: 0.25 }, C.stop_release + 0.45);
    wordOut("#w-ctl", C.click2 - 0.2);
    glide(ok, C.zero_go3, C.click2 - C.zero_go3 - 0.05, "power2.out");
    zClick(C.click2, "#dlg-ok");

    // ====================================================================
    // SCENE 5 — REMEMBER / what changed                     26.6 – 33.2
    // ====================================================================
    to("#dlg", { opacity: 0, scale: 0.97, duration: 0.2, ease: "power2.in" }, C.dialog_out);
    to("#dlg-shade", { opacity: 0, duration: 0.25 }, C.dialog_out);
    ["#rw-1", "#rw-2", "#rw-3"].forEach((r, i) => to(r, { height: 0, duration: 0.42, ease: "power2.inOut" }, C.rows_gone + i * 0.05));
    to("#inbox-a", { opacity: 0, duration: 0.15 }, C.rows_gone);
    to("#inbox-b", { opacity: 1, duration: 0.15 }, C.rows_gone + 0.1);
    to("#read-a", { opacity: 0, duration: 0.15 }, C.rows_gone);
    to("#read-b", { opacity: 1, duration: 0.2 }, C.rows_gone + 0.12);
    chipOut("#c-click25", C.rows_gone + 0.3);
    cam(F_LEFT, C.cam_left3, 0.85);
    // screen memory: what the model has seen
    set("#mem", { opacity: 1 }, C.mem_in);
    ft(["#mem1", "#mem2"], { opacity: 0, x: 40 }, { opacity: 1, x: 0, duration: 0.4, ease: "power3.out", stagger: 0.1 }, C.mem_in);
    const sel = div("memsel", null, $("#mem"));
    sel.id = "memsel";
    set("#memsel", { x: 600, opacity: 0 }, 0);
    to("#memsel", { opacity: 1, duration: 0.2 }, C.mem_in + 0.3);
    // back to screen #1: the selection travels back
    to("#memsel", { x: 0, duration: 0.5, ease: "power3.inOut" }, C.rewind);
    to("#mem2", { opacity: 0.45, duration: 0.3 }, C.rewind + 0.2);
    wordIn("#w-rem", C.w_rem);
    // recognition: a sweep across the live screen as it is matched to screen #1
    const sweep = el("g", { id: "sweep" }, marks);
    el("rect", { x: -2, y: winB.y, width: 4, height: winB.h, fill: "#eceff2" }, sweep);
    el("rect", { x: 2, y: winB.y, width: 90, height: winB.h, fill: "url(#sweepfade)" }, sweep);
    const defs = el("defs", {}, marks);
    const lg = el("linearGradient", { id: "sweepfade", x1: "0", x2: "1", y1: "0", y2: "0" }, defs);
    el("stop", { offset: "0", "stop-color": "#eceff2", "stop-opacity": "0.22" }, lg);
    el("stop", { offset: "1", "stop-color": "#eceff2", "stop-opacity": "0" }, lg);
    set("#sweep", { x: winB.x + winB.w, opacity: 0 }, 0);
    to("#sweep", { opacity: 1, duration: 0.08 }, C.rewind + 0.05);
    to("#sweep", { x: winB.x, duration: 0.55, ease: "power2.inOut" }, C.rewind + 0.05);
    to("#sweep", { opacity: 0, duration: 0.12 }, C.rewind + 0.55);
    // the old indices come back to their places
    const keep = [0, 1, 4, 6, 7, 12, 13, 17, 18, 19, 20];
    const shift = (i) => (i >= 17 ? -3 * 84 : 0); // rows 17-19 moved up by three rows
    const back = el("g", { id: "backtags" }, marks);
    keep.forEach((ix) => {
      const b = treeB[ix];
      const g = el("g", { class: "mk-tag" + (ix === 7 || ix === 20 ? " blue" : "") }, back);
      const lbl = String(ix);
      const y = b.y + (ix >= 17 && ix <= 19 ? shift(ix) : 0);
      el("rect", { x: b.x, y, width: 12 + lbl.length * 10.5, height: 22 }, g);
      const tx = el("text", { x: b.x + 6, y: y + 17 }, g);
      tx.textContent = lbl;
    });
    const btags = $$("#backtags g");
    set(btags, { opacity: 0 }, 0);
    btags.forEach((g, i) => ft(g, { opacity: 0, y: -14 }, { opacity: 1, y: 0, duration: 0.2, ease: "power3.out" }, C.rewind + 0.45 + i * 0.03));
    // the change report: only what changed since then
    set("#rep3", { opacity: 1 }, C.rep3_in - 0.1);
    ft("#rep3 .p-head", { opacity: 0, x: -20 }, { opacity: 1, x: 0, duration: 0.3, ease: "power3.out" }, C.rep3_in - 0.1);
    ft("#rep3 .p-sub", { opacity: 0 }, { opacity: 1, duration: 0.25 }, C.rep3_in + 0.1);
    $$("#rep3-lines .ln").forEach((n, i) => ft(n, { opacity: 0, x: -18 }, { opacity: 1, x: 0, duration: 0.24, ease: "power3.out" }, C.rep3_in + 0.35 + i * 0.1));
    set($$("#rep3-lines .ln"), { opacity: 0 }, 0);
    // the delta on the screen itself
    const gap = { x: treeB[13].x, y: treeB[13].y + treeB[13].h };
    const dl = el("g", { id: "deltamark" }, marks);
    el("line", { x1: gap.x + 8, y1: gap.y, x2: gap.x + 552, y2: gap.y, stroke: "#eceff2", "stroke-width": 3, "stroke-dasharray": "8 6" }, dl);
    const dtg = el("g", { class: "mk-tag" }, dl);
    el("rect", { x: gap.x + 360, y: gap.y - 11, width: 186, height: 22 }, dtg);
    const dtx = el("text", { x: gap.x + 368, y: gap.y + 6 }, dtg);
    dtx.textContent = "- 14 · 15 · 16";
    set("#deltamark", { opacity: 0 }, 0);
    ft("#deltamark", { opacity: 0 }, { opacity: 1, duration: 0.25 }, C.delta);
    // the next look: no second picture of the same screen. The camera goes to it.
    const ddB = (() => {
      const r = $("#dedupe").getBoundingClientRect();
      return { x: (r.left - annot.left) / k, y: (r.top - annot.top) / k, w: r.width / k, h: r.height / k };
    })();
    const F_DD = { scale: 0.86, x: 0, y: 0 };
    F_DD.x = 980 - F_DD.scale * (ddB.x + 560);
    F_DD.y = 560 - F_DD.scale * (ddB.y + ddB.h / 2);
    cam(F_DD, C.dd_cam, 0.6, "power3.inOut");
    to("#topfade", { opacity: 1, duration: 0.5, ease: "power1.inOut" }, C.dd_cam + 0.1);
    to("#topfade", { opacity: 0, duration: 0.4, ease: "power1.inOut" }, C.dd_back);
    cam(F_LEFT, C.dd_back, 0.55, "power3.inOut");
    wordOut("#w-rem", C.dd_cam - 0.1);
    chipIn("#c-state2", C.call_state2);
    ft(".dd-call", { opacity: 0 }, { opacity: 1, duration: 0.25 }, C.call_state2);
    to("#dd-straight", { strokeDashoffset: 0, duration: 0.3, ease: "none" }, C.dd_pkt);
    ft("#dd-pkt", { opacity: 0, x: 40, y: 60 }, { opacity: 1, x: 250, y: 60, duration: 0.25, ease: "power1.in" }, C.dd_pkt);
    ft("#dd-ghost", { opacity: 0 }, { opacity: 1, duration: 0.12 }, C.dd_pkt - 0.05);
    to("#dd-around", { strokeDashoffset: 0, duration: 0.4, ease: "none" }, C.dd_swerve);
    to("#dd-straight", { opacity: 0.25, duration: 0.2 }, C.dd_swerve);
    to("#dd-pkt", { keyframes: ddKeys.slice(6).map((p) => ({ x: p.x, y: p.y, duration: 0.4 / (ddKeys.length - 6), ease: "none" })) }, C.dd_swerve);
    to("#dd-ghost", { opacity: 0.32, duration: 0.3 }, C.dd_swerve + 0.25);
    ft("#dd-out1", { opacity: 0, x: -14 }, { opacity: 1, x: 0, duration: 0.3, ease: "power3.out" }, C.dd_out);
    ft("#dd-out2", { opacity: 0 }, { opacity: 1, duration: 0.3 }, C.dd_out + 0.25);
    // done: green, then everything fades out slowly (fade_out_ms = 1200)
    label("done", C.done);
    chipOut("#c-state2", C.done);
    to(["#glow", "#label", "#zcur"], { opacity: 0, duration: 1.2, ease: "power1.inOut" }, C.glow_out);
    to(["#backtags", "#deltamark"], { opacity: 0, duration: 0.6 }, C.glow_out);

    // ====================================================================
    // SCENE 6 — pull back: the whole system                 33.2 – 35.6
    // ====================================================================
    cam(F_FAR, C.pull, 2.2, "power3.inOut");
    ft("#sysmap", { opacity: 0 }, { opacity: 1, duration: 1.1, ease: "power1.inOut" }, C.pull + 0.35);
    ft("#tgscrim", { opacity: 0 }, { opacity: 1, duration: 0.5 }, C.tg1 - 0.1);
    to("#tg-1 span", { y: 0, duration: 0.7, ease: "power4.out" }, C.tg1);
    to("#tg-2 span", { y: 0, duration: 0.7, ease: "power4.out" }, C.tg2);
    to(["#tg-1 span", "#tg-2 span"], { y: -130, duration: 0.32, ease: "power3.in", stagger: 0.05 }, C.tg_out);
    to("#tgscrim", { opacity: 0, duration: 0.35 }, C.tg_out + 0.15);

    // ====================================================================
    // SCENE 7 — everything collapses into the mark          35.6 – 44.0
    // ====================================================================
    const markB = box("#mark");
    const markC = { x: markB.x + markB.w / 2, y: markB.y + markB.h / 2 };
    const centerShift = 960 - markC.x;
    set("#lockup", { x: centerShift }, 0);
    const mc = { x: 960, y: markC.y };
    to("#camera", { x: 960 - 0.02 * 960, y: mc.y - 0.02 * 540, scale: 0.02, opacity: 0, duration: 0.65, ease: "power3.in" }, C.collapse);
    to("#grid", { opacity: 0.35, duration: 0.6 }, C.collapse);
    set("#morph-r", { attr: { x: 672, y: 378, width: 576, height: 324, rx: 0 }, opacity: 0 }, 0);
    to("#morph-r", { opacity: 1, duration: 0.1 }, C.collapse);
    to("#morph-r", { attr: { x: mc.x - 100, y: mc.y - 100, width: 200, height: 200, rx: 100 }, strokeWidth: 9, duration: 0.7, ease: "power3.inOut" }, C.collapse);
    to("#morph-r", { opacity: 0, duration: 0.05 }, C.hit_final);
    // the mark: the screen's glow became a ring, Zero's cursor docks in it
    ft("#mark", { opacity: 0 }, { opacity: 1, duration: 0.04 }, C.hit_final - 0.02);
    ft("#m-arrow", { scale: 0.3, opacity: 0, svgOrigin: "0 0" }, { scale: 1, opacity: 1, duration: 0.45, ease: "back.out(1.8)" }, C.hit_final - 0.05);
    ft("#m-dot", { scale: 0, svgOrigin: "0 0" }, { scale: 1, duration: 0.3, ease: "back.out(2.5)" }, C.hit_final);
    ft("#m-glow", { opacity: 0, scale: 0.7, svgOrigin: "0 0" }, { opacity: 1, scale: 1.08, duration: 0.25, ease: "power2.out" }, C.hit_final);
    to("#m-glow", { opacity: 0.55, scale: 1, duration: 0.8, ease: "power2.out" }, C.hit_final + 0.25);
    to("#lockup", { x: 0, duration: 0.75, ease: "power3.inOut" }, C.lock_slide);
    ft("#wm-name", { opacity: 0, x: 50 }, { opacity: 1, x: 0, duration: 0.6, ease: "power3.out" }, C.lock_slide + 0.12);
    to("#os span", { y: 0, duration: 0.7, ease: "power4.out" }, C.os);
    to("#freedoms span", { y: 0, duration: 0.6, ease: "power4.out" }, C.freedoms);
    to("#repo", { opacity: 1, y: 0, duration: 0.6, ease: "power3.out" }, C.repo);
    to("#fade", { opacity: 1, duration: C.duration - C.end_fade, ease: "power1.in" }, C.end_fade);

    // Deliberate layering (text over the dimmed app, set-of-marks tags over
    // app text, the call path over the covered screen, the tagline over the
    // map): mark exactly those text blocks for the layout audit.
    const allow = (sel, attrs) => $$(sel).forEach((n) => attrs.forEach((a) => n.setAttribute(a, "")));
    allow(
      ".hook-line span, .tg-line span, .ztag-text, #marks text, #pipe-nodes .pn, #pipe-nodes .pn span, #pipe-nodes .el, #pipe-nodes .lane-title, #sysmap .t",
      ["data-layout-allow-overlap"],
    );
    allow(".word", ["data-layout-allow-overlap", "data-layout-allow-occlusion", "data-layout-allow-overflow"]);
    // masked reveals: text waits outside its clipping line until it slides in
    allow(".hook-line span, .tg-line span, #os span, #freedoms span", ["data-layout-allow-overflow"]);
    return tl;
  }

  const fonts = [
    '300 112px "Archivo"',
    '900 108px "Archivo"',
    '500 24px "Archivo"',
    '400 33px "JetBrains Mono"',
    '700 33px "JetBrains Mono"',
  ];
  window.__filmReady = Promise.all(fonts.map((f) => document.fonts.load(f)))
    .then(() => document.fonts.ready)
    // A GSAP timeline is a thenable: wrap it, or the promise would wait for
    // the paused timeline to finish playing.
    .then(() => ({ tl: build() }));
})();
