// Markdown プレビューの安全処理の確認（Edge headless）。手順: app で `npx vite --port 5199` を起動してから `node verification/markdown-sanitize/run.cjs`。
const { spawn } = require("child_process");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
(async () => {
  const edge = spawn("C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe", ["--headless=new", "--remote-debugging-port=9334", "--user-data-dir=" + require("os").tmpdir() + "/md-san-profile", "about:blank"], { stdio: "ignore" });
  let t; for (let i = 0; i < 50; i++) { try { t = (await (await fetch("http://127.0.0.1:9334/json")).json()).find((x) => x.type === "page"); if (t) break; } catch {} await sleep(200); }
  const ws = new WebSocket(t.webSocketDebuggerUrl); await new Promise((r) => (ws.onopen = r));
  let id = 0; const p = new Map(); ws.onmessage = (m) => { const d = JSON.parse(m.data); p.get(d.id)?.(d); };
  const send = (method, params = {}) => new Promise((res) => { const i = ++id; p.set(i, res); ws.send(JSON.stringify({ id: i, method, params })); });
  const ev = async (e) => (await send("Runtime.evaluate", { expression: e, returnByValue: true })).result.result.value;
  await send("Page.enable"); await send("Page.navigate", { url: "http://localhost:5199/verification/markdown-sanitize/harness.html" }); await sleep(7000);
  let bad = 0; const chk = (n, ok, d) => { console.log((ok ? "ok   " : "FAIL ") + n + (ok ? "" : " " + JSON.stringify(d))); if (!ok) bad++; };
  const html = await ev("window.__html"); if (!html) { console.log("NO HTML", await ev("document.body.innerHTML")); process.exit(3); } console.log(html);
  chk("area/map/usemap なし", !/<area|<map|usemap|ismap/i.test(html), html);
  chk("id/name なし", !/\sid=|\sname=/i.test(html), html);
  chk("a以外にURL属性なし", !/<div[^>]*(href|background|poster)/i.test(html), html);
  chk("全aにhrefなし", await ev("[...document.querySelectorAll('a')].every(a=>!a.hasAttribute('href'))"));
  chk("#root が置き換わっていない", await ev("document.getElementById('root') && document.getElementById('root').tagName==='DIV' && !document.querySelector('img#root')"));
  await ev("window.__nav=0; window.addEventListener('beforeunload',()=>{window.__nav=1}); window.__u=location.href");
  await ev("document.querySelectorAll('a').forEach(a=>a.click())"); await sleep(700);
  chk("リンクのクリックで遷移しない", (await ev("location.href===window.__u && window.__nav===0")));
  console.log(bad ? "FAILED" : "ALL OK"); ws.close(); edge.kill(); process.exit(bad ? 1 : 0);
})();
