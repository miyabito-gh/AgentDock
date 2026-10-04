// Standalone adoption probe; no product code or external packages.
import { spawn, execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { createInterface } from 'node:readline';
import { mkdirSync, writeFileSync, readFileSync, existsSync } from 'node:fs';
import { resolve, join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const root = dirname(fileURLToPath(import.meta.url));
const exe = process.argv[2];
if (!exe) throw new Error('Supply an absolute path to the 0.160.0 codex.exe');
const version = execFileSync(exe, ['--version'], { encoding: 'utf8' }).trim();
if (version !== 'codex-cli 0.160.0') throw new Error(`Wrong version: ${version}`);
const run = join(root, 'runs', new Date().toISOString().replace(/[:.]/g, '-'));
const home = join(run, 'codex-home');
const work = join(run, 'work');
mkdirSync(home, { recursive: true }); mkdirSync(work, { recursive: true });
const originalConfig = join(process.env.USERPROFILE, '.codex', 'config.toml');
const fingerprint = () => existsSync(originalConfig)
  ? createHash('sha256').update(readFileSync(originalConfig)).digest('hex') : null;
const before = fingerprint();
const results = []; const clients = []; const calls = [];
let mode = 'complete'; let requestCount = 0;
const sockets = new Set();
const model = createServer(async (req, res) => {
  if (req.method !== 'POST' || !req.url.endsWith('/responses')) {
    res.writeHead(404); res.end(); return;
  }
  const chunks = []; for await (const chunk of req) chunks.push(chunk);
  const body = JSON.parse(Buffer.concat(chunks).toString());
  requestCount++;
  // Store request shape only; never persist instructions, messages, or headers.
  calls.push({ mode, model: body.model, serviceTier: body.service_tier ?? null,
    tools: (body.tools ?? []).map(t => t.name ?? t.type) });
  res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' });
  const send = data => res.write(`data: ${JSON.stringify(data)}\n\n`);
  const id = `probe_response_${requestCount}`;
  send({ type: 'response.created', response: { id } });
  if (mode === 'hold') {
    const timer = setInterval(() => res.write(': probe heartbeat\n\n'), 200);
    res.on('close', () => clearInterval(timer)); return;
  }
  const item = { id: `msg_${requestCount}`, type: 'message', role: 'assistant',
    content: [{ type: 'output_text', text: 'PROBE_OK', annotations: [] }] };
  send({ type: 'response.output_item.added', output_index: 0,
    item: { ...item, content: [] } });
  send({ type: 'response.output_text.delta', item_id: item.id,
    output_index: 0, content_index: 0, delta: 'PROBE_OK' });
  send({ type: 'response.output_item.done', output_index: 0, item });
  send({ type: 'response.completed', response: { id, status: 'completed', output: [item],
    usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } }); res.end();
});
model.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)); });
await new Promise(r => model.listen(0, '127.0.0.1', r));
const port = model.address().port;
writeFileSync(join(home, 'config.toml'), `model = "gpt-5.4"\nmodel_provider = "agentdock_probe"\ncli_auth_credentials_store = "file"\napproval_policy = "never"\nsandbox_mode = "read-only"\n[analytics]\nenabled = false\n[model_providers.agentdock_probe]\nname = "AgentDock local fixture"\nbase_url = "http://127.0.0.1:${port}/v1"\nwire_api = "responses"\nrequires_openai_auth = false\nsupports_websockets = false\nrequest_max_retries = 0\nstream_max_retries = 0\nstream_idle_timeout_ms = 10000\n`);
// Remove inherited auth/network overrides from the child environment.
const baseConfig = readFileSync(join(home, 'config.toml'), 'utf8');
const configureMcp = enabled => writeFileSync(join(home, 'config.toml'), baseConfig +
  `\n[mcp_servers.agentdock_fixture]\ncommand = ${JSON.stringify(process.execPath.replaceAll('\\', '/'))}\nargs = [${JSON.stringify(join(root, 'mcp-fixture.mjs').replaceAll('\\', '/'))}]\nenabled = ${enabled}\nstartup_timeout_sec = 5\n`);
configureMcp(true);
const env = { ...process.env, CODEX_HOME: home, HOME: work };
for (const key of Object.keys(env)) if (/^(OPENAI_|CODEX_API_KEY|CODEX_ACCESS_TOKEN|HTTP_PROXY|HTTPS_PROXY|ALL_PROXY)/i.test(key)) delete env[key];
env.NO_PROXY = '127.0.0.1,localhost';
const sleep = ms => new Promise(r => setTimeout(r, ms));
class Client {
  constructor() {
    this.next = 1; this.pending = new Map(); this.events = []; this.stderr = '';
    this.proc = spawn(exe, ['app-server', '--stdio'], { env, cwd: work, windowsHide: true });
    this.send = m => this.proc.stdin.write(JSON.stringify(m) + '\n');
    this.closed = new Promise(r => this.proc.on('exit', (code, signal) => r({ code, signal })));
    this.proc.stderr.on('data', b => { this.stderr = (this.stderr + b.toString()).slice(-6000); });
    createInterface({ input: this.proc.stdout }).on('line', line => {
      let m; try { m = JSON.parse(line); } catch { return; }
      if (m.id !== undefined && this.pending.has(m.id)) {
        const p = this.pending.get(m.id); this.pending.delete(m.id); clearTimeout(p.timer); p.resolve(m);
      } else if (m.id !== undefined) {
        // No approval or dynamic tool is authorized by the fixture.
        this.proc.stdin.write(JSON.stringify({ id: m.id, error: { code: -32000, message: 'Probe declines server request' } }) + '\n');
      } else this.events.push(m);
    }); clients.push(this);
  }
  rpc(method, params = {}, timeout = 20000) {
    const id = this.next++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`Timeout ${method}; ${this.stderr.slice(-1000)}`)); }, timeout);
      this.pending.set(id, { resolve, timer });
      this.send({ id, method, params });
    });
  }
  async ok(method, params) {
    const r = await this.rpc(method, params); if (r.error) throw new Error(`${method}: ${JSON.stringify(r.error)}`); return r.result;
  }
  async init(experimental = true) {
    const r = await this.ok('initialize', { clientInfo: { name: 'agentdock_probe', title: 'AgentDock Probe', version: '0.1' },
      capabilities: { experimentalApi: experimental } });
    this.send({ method: 'initialized', params: {} }); return r;
  }
  async event(method, predicate = () => true, from = 0, timeout = 12000) {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
      const m = this.events.slice(from).find(e => e.method === method && predicate(e.params));
      if (m) return m.params; await sleep(30);
    } throw new Error(`Missing notification ${method}; methods=${this.events.slice(from).map(x=>x.method).join(',')}`);
  }
  async close() {
    if (this.proc.exitCode !== null || this.proc.signalCode !== null) return this.closed;
    this.proc.stdin.end(); const timer = setTimeout(() => this.proc.kill(), 2000);
    try { return await this.closed; } finally { clearTimeout(timer); }
  }
}
const assert = (condition, text) => { if (!condition) throw new Error(text); };
async function check(name, fn) {
  try { const evidence = await fn(); results.push({ name, status: 'pass', evidence }); }
  catch (e) { results.push({ name, status: 'fail', error: e.message }); }
  console.log(`${results.at(-1).status}: ${name}`);
}
let client; let threadId; let wsProc;
async function connectWs(url) {
  const c = Object.create(Client.prototype);
  c.next = 1; c.pending = new Map(); c.events = []; c.stderr = '';
  const ws = new WebSocket(url);
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { ws.close(); reject(new Error('WebSocket connection timeout')); }, 5000);
    ws.addEventListener('open', () => { clearTimeout(timer); resolve(); }, { once: true });
    ws.addEventListener('error', () => { clearTimeout(timer); reject(new Error('WebSocket connection failed')); }, { once: true });
  });
  c.send = m => ws.send(JSON.stringify(m));
  ws.addEventListener('message', event => {
    const m = JSON.parse(event.data);
    if (c.pending.has(m.id)) { const p = c.pending.get(m.id); c.pending.delete(m.id); clearTimeout(p.timer); p.resolve(m); }
    else if (m.id !== undefined) c.send({ id: m.id, error: { code: -32000, message: 'Probe declines server request' } });
    else c.events.push(m);
  });
  c.close = async () => { if (ws.readyState >= 2) return; const closed = new Promise(r => ws.addEventListener('close', r, { once: true })); ws.close(); await closed; };
  clients.push(c); return c;
}
try {
  client = new Client();
  await check('handshake_required', async () => {
    const r = await client.rpc('thread/loaded/list'); assert(r.error, 'Request accepted before initialize'); return r.error;
  });
  await check('initialize_0_160_0', () => client.init());
  await check('thread_start_local_provider', async () => {
    const r = await client.ok('thread/start', { cwd: work, approvalPolicy: 'never', sandbox: 'read-only', personality: 'friendly' });
    threadId = r.thread.id; assert(threadId, 'No thread ID'); return { threadId, provider: r.modelProvider, personality: r.personality };
  });
  if (!threadId) throw new Error('Thread prerequisite failed');
  await check('disabled_plugin_ids_saved', async () => {
    const from = client.events.length;
    const r = await client.ok('thread/settings/update', { threadId, disabledPluginIds: ['probe@local'] });
    const e = await client.event('thread/settings/updated', p => p.threadId === threadId, from);
    assert(e.threadSettings?.disabledPluginIds?.includes('probe@local'), 'Saved list missing');
    return { acknowledgment: r, disabledPluginIds: e.threadSettings.disabledPluginIds };
  });
  await check('empty_background_terminal_list', async () => {
    const r = await client.ok('thread/backgroundTerminals/list', { threadId });
    assert(Array.isArray(r.data) && r.data.length === 0, 'Expected empty list'); return r;
  });
  await check('mcp_connected_and_tool_available', async () => {
    let server; const until = Date.now() + 10000;
    while (Date.now() < until) {
      const r = await client.ok('mcpServerStatus/list', { threadId });
      server = r.data.find(s => s.name === 'agentdock_fixture');
      if (server?.runtimeStatus === 'connected' && Object.keys(server.tools).some(n => n.includes('probe_ping'))) break;
      await sleep(100);
    }
    assert(server?.runtimeStatus === 'connected', `MCP not connected: ${JSON.stringify(server)}`);
    assert(Object.keys(server.tools).some(n => n.includes('probe_ping')), 'MCP tool missing');
    return { runtimeStatus: server.runtimeStatus, authStatus: server.authStatus, toolNames: Object.keys(server.tools) };
  });
  await check('model_response_and_completion', async () => {
    const from = client.events.length;
    const r = await client.ok('turn/start', { threadId, input: [{ type: 'text', text: 'PROBE: return fixture response.' }] });
    const e = await client.event('turn/completed', p => p.turn.id === r.turn.id, from);
    assert(e.turn.status === 'completed', `Unexpected status ${e.turn.status}`);
    assert(requestCount > 0, 'Local fixture not reached'); return { turnId: r.turn.id, status: e.turn.status, requestCount };
  });
  await check('mcp_disable_reload_removes_model_tool', async () => {
    assert(calls.at(-1).tools.some(n => n.startsWith('mcp__agentdock_fixture')), 'Enabled MCP namespace not sent to model');
    configureMcp(false); const ack = await client.ok('config/mcpServer/reload');
    await sleep(300); const from = client.events.length;
    const r = await client.ok('turn/start', { threadId, input: [{ type: 'text', text: 'PROBE: after MCP disable.' }] });
    await client.event('turn/completed', p => p.turn.id === r.turn.id && p.turn.status === 'completed', from);
    assert(!calls.at(-1).tools.some(n => n.startsWith('mcp__agentdock_fixture')), 'Disabled MCP still sent to model');
    const status = await client.ok('mcpServerStatus/list', { threadId });
    return { acknowledgment: ack, removedFromModelTools: true,
      serverStatus: status.data.find(s => s.name === 'agentdock_fixture') ?? null };
  });
  await check('active_turn_interrupt', async () => {
    mode = 'hold'; const from = client.events.length; const previous = requestCount;
    const r = await client.ok('turn/start', { threadId, input: [{ type: 'text', text: 'PROBE: hold response.' }] });
    const until = Date.now() + 10000; while (requestCount === previous && Date.now() < until) await sleep(30);
    assert(requestCount > previous, 'Active local stream not reached');
    const started = Date.now(); const ack = await client.ok('turn/interrupt', { threadId, turnId: r.turn.id });
    const e = await client.event('turn/completed', p => p.turn.id === r.turn.id, from);
    assert(e.turn.status === 'interrupted', `Unexpected status ${e.turn.status}`);
    mode = 'complete'; return { acknowledgment: ack, status: e.turn.status, elapsedMs: Date.now() - started };
  });
  await check('restart_read_does_not_load', async () => {
    await client.close(); client = new Client(); await client.init();
    const before = requestCount; const r = await client.ok('thread/read', { threadId, includeTurns: true });
    const loaded = await client.ok('thread/loaded/list');
    assert(!loaded.data.includes(threadId), 'read loaded thread');
    assert(requestCount === before, 'read initiated inference');
    return { turnCount: r.thread.turns.length, loaded: loaded.data, requestCount };
  });
  await check('resume_idle_thread_without_queue', async () => {
    const before = requestCount; const r = await client.ok('thread/resume', { threadId });
    const loaded = await client.ok('thread/loaded/list'); await sleep(500);
    assert(loaded.data.includes(threadId), 'resume did not load');
    assert(requestCount === before, 'idle resume initiated inference'); return { loaded: true, requestCount, disabledPluginIds: r.disabledPluginIds };
  });
  await check('experimental_gate', async () => {
    const stable = new Client(); await stable.init(false);
    try {
      const r = await stable.rpc('thread/backgroundTerminals/list', { threadId });
      assert(r.error?.message?.includes('experimentalApi'), 'Missing experimental rejection'); return r.error;
    } finally { await stable.close(); }
  });
  await check('live_websocket_reconnect_read_vs_resume', async () => {
    const reserve = createServer(); await new Promise(r => reserve.listen(0, '127.0.0.1', r));
    const wsPort = reserve.address().port; await new Promise(r => reserve.close(r));
    wsProc = spawn(exe, ['app-server', '--listen', `ws://127.0.0.1:${wsPort}`], { env, cwd: work, windowsHide: true });
    let errors = ''; wsProc.stderr.on('data', b => { errors = (errors + b).slice(-2000); }); wsProc.stdout.resume();
    const until = Date.now() + 12000; let ready = false;
    while (Date.now() < until && !ready) {
      try { ready = (await fetch(`http://127.0.0.1:${wsPort}/readyz`, { signal: AbortSignal.timeout(500) })).ok; } catch {}
      if (!ready) await sleep(50);
    }
    assert(ready, `WebSocket server not ready: ${errors}`);
    const first = await connectWs(`ws://127.0.0.1:${wsPort}`); await first.init();
    // Keep the server connected while replacing the observer connection.
    const witness = await connectWs(`ws://127.0.0.1:${wsPort}`); await witness.init();
    const start = await first.ok('thread/start', { cwd: work, approvalPolicy: 'never', sandbox: 'read-only' });
    const id = start.thread.id; mode = 'hold'; const count = requestCount;
    const turn = await first.ok('turn/start', { threadId: id, input: [{ type: 'text', text: 'PROBE: hold for reconnect.' }] });
    const wait = Date.now() + 8000; while (requestCount === count && Date.now() < wait) await sleep(30);
    assert(requestCount > count, 'Stream not active'); await first.close();
    const second = await connectWs(`ws://127.0.0.1:${wsPort}`); await second.init();
    const loaded = await second.ok('thread/loaded/list'); assert(loaded.data.includes(id), 'Disconnected thread not loaded');
    await second.ok('thread/read', { threadId: id, includeTurns: true });
    let from = second.events.length;
    await second.ok('thread/settings/update', { threadId: id, disabledPluginIds: ['read-observer@probe'] });
    await sleep(300);
    const readSubscribed = second.events.slice(from).some(e => e.method === 'thread/settings/updated' && e.params.threadId === id);
    assert(!readSubscribed, 'read unexpectedly subscribed');
    const beforeResume = requestCount; await second.ok('thread/resume', { threadId: id });
    from = second.events.length;
    await second.ok('thread/settings/update', { threadId: id, disabledPluginIds: [] });
    await second.event('thread/settings/updated', p => p.threadId === id, from);
    const beforeStop = second.events.length;
    await second.ok('turn/interrupt', { threadId: id, turnId: turn.turn.id });
    const completed = await second.event('turn/completed', p => p.turn.id === turn.turn.id, beforeStop);
    assert(completed.turn.status === 'interrupted', 'Reconnected turn did not interrupt');
    assert(requestCount === beforeResume, 'resume restarted inference'); mode = 'complete';
    // Observe the separate case where ALL clients disconnect. Do not require
    // persistence: this is an adoption condition to measure, not assume.
    mode = 'hold'; const previous = requestCount;
    const lastTurn = await second.ok('turn/start', { threadId: id, input: [{ type: 'text', text: 'PROBE: last-client disconnect.' }] });
    const holdUntil = Date.now() + 8000;
    while (requestCount === previous && Date.now() < holdUntil) await sleep(30);
    assert(requestCount > previous, 'Last-client scenario did not reach local stream');
    await witness.close(); await second.close(); await sleep(500);
    const third = await connectWs(`ws://127.0.0.1:${wsPort}`); await third.init();
    const afterLast = await third.ok('thread/loaded/list');
    const history = await third.ok('thread/read', { threadId: id, includeTurns: true });
    const lastState = history.thread.turns.find(t => t.id === lastTurn.turn.id)?.status ?? null;
    const retained = afterLast.data.includes(id);
    if (retained) {
      await third.ok('thread/resume', { threadId: id });
      await third.ok('turn/interrupt', { threadId: id, turnId: lastTurn.turn.id });
    }
    mode = 'complete';
    return { loadedAfterDisconnect: true, readSubscribed, resumeSubscribed: true, duplicateModelRequest: false,
      stopStatus: completed.turn.status, queuedInput: false, otherConnectionKeptOpen: true,
      lastConnectionDisconnect: { delayMs: 500, retainedLoadedThread: retained,
        persistedTurnStatus: lastState, runtimeThreadStatus: history.thread.status },
      transport: 'loopback experimental websocket' };
  });
} catch (e) { results.push({ name: 'suite_prerequisite', status: 'fail', error: e.message }); }
finally {
  for (const c of clients) await c.close();
  if (wsProc && wsProc.exitCode === null && wsProc.signalCode === null) {
    const exit = new Promise(r => wsProc.once('exit', r)); wsProc.kill(); await exit;
  }
  for (const socket of sockets) socket.destroy(); await new Promise(r => model.close(r));
  results.push({ name: 'existing_config_unchanged', status: before === fingerprint() ? 'pass' : 'fail' });
  const report = { createdAt: new Date().toISOString(), executable: resolve(exe), version,
    mode: 'Real App Server with loopback model fixture; no cloud authentication',
    run, results, modelRequests: calls,
    limitations: ['No real cloud inference/authentication', 'No subagent or OS process termination tested',
      'Plugin ID persistence is not capability filtering', 'Reconnect without queue does not establish queued-input safety',
      'Loopback WebSocket fixture is unauthenticated and experimental, not a production transport decision'] };
  writeFileSync(join(run, 'result.json'), JSON.stringify(report, null, 2));
  writeFileSync(join(root, 'latest-result.json'), JSON.stringify(report, null, 2));
  console.log(`Result: ${join(run, 'result.json')}`);
}
// All owned servers and clients were closed above. Bound lingering platform handles.
process.exit(results.some(r => r.status === 'fail') ? 1 : 0);
