// Capture the two "screens the model has seen" as stills, the way a
// get_app_state screenshot would look: no overlay (the helper's windows are
// excluded from captures) and the card number filled grey (privacy "fill").
// Usage: PUPPETEER=/path/to/puppeteer-core node tools/capture-thumbs.cjs
const puppeteer = require(process.env.PUPPETEER || "puppeteer-core");
const http = require("http"), fs = require("fs"), path = require("path");
const root = path.resolve(__dirname, "..");
const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".woff2": "font/woff2", ".png": "image/png" };
const srv = http.createServer((q, r) => {
  let f = path.join(root, decodeURIComponent(q.url.split("?")[0]));
  if (f.endsWith("/")) f += "index.html";
  fs.readFile(f, (e, d) => {
    if (e) { r.writeHead(404); r.end(); return; }
    r.writeHead(200, { "content-type": types[path.extname(f)] || "application/octet-stream" });
    r.end(d);
  });
}).listen(8766);
(async () => {
  const b = await puppeteer.launch({ executablePath: process.env.CHROME, args: ["--no-sandbox"] });
  const p = await b.newPage();
  await p.setViewport({ width: 1920, height: 1080 });
  await p.evaluateOnNewDocument(() => { window.__timelines = {}; });
  await p.goto("http://localhost:8766/index.html", { waitUntil: "networkidle0" });
  await p.evaluate(async () => { await window.__filmReady; });
  const shot = async (name, dialog) => {
    await p.evaluate((dialog) => {
      const hide = ["#glow", "#label", "#zcur", "#ucur", "#dim", "#marks", "#grain", "#fade", "#hook", "#words", "#chip", "#keys", "#final", "#tagline", "#tgscrim", "#morph", "#pipe", "#iris", "#annot", "#pkt-s"];
      hide.forEach((s) => document.querySelectorAll(s).forEach((n) => (n.style.visibility = "hidden")));
      // outside the HyperFrames runtime nothing sizes the root
      Object.assign(document.querySelector("#root").style, { width: "1920px", height: "1080px" });
      const cam = document.querySelector("#camera");
      cam.style.transform = "none";
      const card = document.querySelector("#cardno").getBoundingClientRect();
      const m = document.querySelector("#maskbar");
      Object.assign(m.style, { visibility: "visible", opacity: 1, transform: "none", left: card.left - 3 + "px", top: card.top - 2 + "px", width: card.width + 6 + "px", height: card.height + 4 + "px", background: "#808080" });
      document.querySelector("#dlg").style.opacity = dialog ? 1 : 0;
      document.querySelector("#dlg").style.transform = "none";
      document.querySelector("#dlg-shade").style.opacity = dialog ? 1 : 0;
    }, dialog);
    await p.screenshot({ path: path.join(root, "assets/thumbs", name), clip: { x: 0, y: 0, width: 1920, height: 1080 } });
  };
  await shot("screen1.png", false);
  await shot("screen2.png", true);
  await b.close();
  srv.close();
  console.log("wrote assets/thumbs/screen1.png, screen2.png");
})();
