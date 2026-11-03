// app-server の起動と読み取りだけを確かめる小さなスモーク試験（版を問わない）。
// 使い方: node probes/app-server/handshake-smoke.mjs <codex.exe の絶対パス>
// 隔離: CODEX_HOME は使い捨ての空フォルダ（利用者の ~/.codex・認証には触れない。実行の前後で config.toml のハッシュを比べる）。
// しないこと: ログイン、推論、会話の作成、ファイルの書込み（使い捨てフォルダ以外）。
// 確かめること: initialize（experimentalApi 有無の両方）、initialized、thread/list、skills/list、model/list が JSON-RPC として成立し、
//               応答の形（トップレベルのキー）が取れること。結果は標準出力に、形と真偽だけを出す（本文・秘密は出さない）。
import { spawn, execFileSync } from "node:child_process";
import { createInterface } from "node:readline";
import { mkdtempSync, existsSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";

const exe = process.argv[2];
if (!exe) { console.error("使い方: node handshake-smoke.mjs <codex.exe>"); process.exit(2); }
const version = execFileSync(exe, ["--version"], { encoding: "utf8" }).trim();
const original = join(process.env.USERPROFILE ?? "", ".codex", "config.toml");
const fp = () => (existsSync(original) ? createHash("sha256").update(readFileSync(original)).digest("hex") : null);
const before = fp();

async function session(experimental) {
  const home = mkdtempSync(join(tmpdir(), "agentdock-smoke-"));
  const child = spawn(exe, ["app-server"], { env: { ...process.env, CODEX_HOME: home }, stdio: ["pipe", "pipe", "pipe"] });
  const pending = new Map();
  let next = 1;
  createInterface({ input: child.stdout }).on("line", (line) => {
    let msg; try { msg = JSON.parse(line); } catch { return; }
    if (msg.id !== undefined && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
  });
  const call = (method, params) => new Promise((resolve) => {
    const id = next++;
    const timer = setTimeout(() => { pending.delete(id); resolve({ timeout: true }); }, 15000);
    pending.set(id, (m) => { clearTimeout(timer); resolve(m); });
    child.stdin.write(JSON.stringify({ id, method, params }) + "\n");
  });
  const out = { experimental, steps: {} };
  const shape = (m) => (m.timeout ? "timeout" : m.error ? `error:${m.error.code}` : `ok:${Object.keys(m.result ?? {}).sort().join(",")}`);
  const init = await call("initialize", { clientInfo: { name: "agentdock_smoke", title: "AgentDock Smoke", version: "0.1" }, capabilities: experimental ? { experimentalApi: true } : {} });
  out.steps.initialize = shape(init);
  child.stdin.write(JSON.stringify({ method: "initialized" }) + "\n");
  for (const [method, params] of [["thread/list", { limit: 1 }], ["skills/list", {}], ["model/list", {}]]) out.steps[method] = shape(await call(method, params));
  child.kill();
  await new Promise((r) => setTimeout(r, 300));
  try { rmSync(home, { recursive: true, force: true }); } catch { /* 使い捨てなので失敗しても続ける */ }
  return out;
}

const results = [await session(false), await session(true)];
const after = fp();
const pass = results.every((r) => Object.values(r.steps).every((s) => !String(s).startsWith("timeout")) && String(r.steps.initialize).startsWith("ok:"));
console.log(JSON.stringify({ version, configUnchanged: before === after, pass, results }, null, 2));
process.exit(pass && before === after ? 0 : 1);
