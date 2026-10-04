// Extended, fixture-only adoption measurements; no product code or external packages.
import { spawn, execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { createServer as createTcpServer, connect as connectTcp } from 'node:net';
import { createInterface } from 'node:readline';
import { mkdirSync, writeFileSync, readFileSync, existsSync } from 'node:fs';
import { resolve, join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const root = dirname(fileURLToPath(import.meta.url));
const exe = process.argv[2];
const selectedChecks = process.argv[3]?.split(',');
if (!exe) throw new Error('Supply an absolute path to the 0.160.0 codex.exe');
const version = execFileSync(exe, ['--version'], { encoding: 'utf8' }).trim();
if (version !== 'codex-cli 0.160.0') throw new Error(`Wrong version: ${version}`);
const run = join(root, 'runs', new Date().toISOString().replace(/[:.]/g, '-'));
const home = join(run, 'codex-home');
const work = join(run, 'work');
mkdirSync(home, { recursive: true }); mkdirSync(work, { recursive: true });
writeFileSync(join(run,'extended-probe-snapshot.mjs'),readFileSync(fileURLToPath(import.meta.url)));
const originalConfig = join(process.env.USERPROFILE, '.codex', 'config.toml');
const fingerprint = () => existsSync(originalConfig)
  ? createHash('sha256').update(readFileSync(originalConfig)).digest('hex') : null;
const before = fingerprint();
const results = []; const clients = []; const calls = [];
let permittedFixtureCommand=null;
const serverReply = m => {
  const p=m.params??{};
  if(m.method==='item/commandExecution/requestApproval'&&permittedFixtureCommand&&
     (p.command===permittedFixtureCommand||p.commandActions?.some(a=>a.command===permittedFixtureCommand))&&
     p.cwd===work)return {id:m.id,result:{decision:'accept'}};
  return {id:m.id,error:{code:-32000,message:'Probe declines server request'}};
};
let mode = 'complete'; let requestCount = 0;
let toolPlan = null;
const responseModes=new Map(); const heldResponses=new Map();
const fixturePids=[];
const sockets = new Set();
const model = createServer(async (req, res) => {
  if (req.method !== 'POST' || !req.url.endsWith('/responses')) {
    res.writeHead(404); res.end(); return;
  }
  const chunks = []; for await (const chunk of req) chunks.push(chunk);
  const body = JSON.parse(Buffer.concat(chunks).toString());
  const userText=(body.input??[]).filter(x=>x.role==='user').at(-1)?.content;
  const lastUserText=typeof userText==='string'?userText:(userText??[]).map(x=>x.text??'').join(' ');
  const effectiveMode=[...responseModes].find(([marker])=>lastUserText.includes(marker))?.[1]??mode;
  for(const item of body.input??[]) if(item.type==='function_call_output'){
    const output=typeof item.output==='string'?item.output:JSON.stringify(item.output);
    for(const match of output.matchAll(/PROBE_PID=(\d+)/g))fixturePids.push(Number(match[1]));
  }
  requestCount++; writeFileSync(join(run, "tool-schema.json"), JSON.stringify(body.tools, null, 2));
  // Store request shape only; never persist instructions, messages, or headers.
  calls.push({ mode:effectiveMode, model: body.model, serviceTier: body.service_tier ?? null,
    skillBodyInjected:JSON.stringify(body.input??[]).includes('AGENTDOCK_UNIQUE_SKILL_BODY_20261004'),
    fixtureMarker:lastUserText.match(/PROBE[^\n]*/)?.[0]??null,
    fixtureSkillMentioned:JSON.stringify([body.instructions,...(body.input??[]).filter(x=>x.role==='developer')]).includes('fixture-probe'),
    fixtureSkillInInstructions:JSON.stringify(body.instructions??'').includes('fixture-probe'),
    fixtureSkillInLastDeveloper:JSON.stringify((body.input??[]).filter(x=>x.role==='developer').at(-1)??'').includes('fixture-probe'),
    fixtureToolOutputs:(body.input??[]).filter(x=>x.type==='function_call_output').map(x=>x.output),
    tools: (body.tools ?? []).map(t => t.name ?? t.type) });
  if(effectiveMode==='fail'){res.writeHead(500,{'Content-Type':'application/json'});res.end(JSON.stringify({error:{message:'Authorized fixture failure',type:'server_error'}}));return;}
  res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' });
  const send = data => res.write(`data: ${JSON.stringify(data)}\n\n`);
  const id = `probe_response_${requestCount}`;
  send({ type: 'response.created', response: { id } });
  if (toolPlan && lastUserText.includes(toolPlan.marker)) {
    const plan = toolPlan; toolPlan = null;
    const item = { type: 'function_call', id: `fc_${requestCount}`, call_id: `call_${requestCount}`,
      name: plan.name.includes('.') ? plan.name.split('.').at(-1) : plan.name,
      ...(plan.name.includes('.') ? {namespace:plan.name.split('.')[0]} : {}),
      arguments: JSON.stringify(plan.args) };
    send({ type: 'response.output_item.added', output_index: 0, item: { ...item, arguments: '' } });
    send({ type: 'response.function_call_arguments.delta', item_id: item.id, output_index: 0, delta: item.arguments });
    send({ type: 'response.output_item.done', output_index: 0, item });
    send({ type: 'response.completed', response: { id, status: 'completed', output: [item],
      usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } }); res.end(); return;
  }
  const completeResponse=()=>{
    const item={id:`msg_${id}`,type:'message',role:'assistant',content:[{type:'output_text',text:'PROBE_OK',annotations:[]}]};
    send({type:'response.output_item.added',output_index:0,item:{...item,content:[]}});
    send({type:'response.output_text.delta',item_id:item.id,output_index:0,content_index:0,delta:'PROBE_OK'});
    send({type:'response.output_item.done',output_index:0,item});
    send({type:'response.completed',response:{id,status:'completed',output:[item],usage:{input_tokens:1,output_tokens:1,total_tokens:2}}});res.end();
  };
  if (effectiveMode === 'hold') {
    const timer = setInterval(() => res.write(': probe heartbeat\n\n'), 200);
    heldResponses.set(lastUserText,()=>{clearInterval(timer);completeResponse();});
    heldResponses.get(lastUserText).fail=()=>{clearInterval(timer);res.destroy();};
    res.on('close',()=>heldResponses.delete(lastUserText));
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
writeFileSync(join(home, 'config.toml'), `model = "gpt-5.4"\nmodel_provider = "agentdock_probe"\ncli_auth_credentials_store = "file"\napproval_policy = "never"\nsandbox_mode = "read-only"\n[analytics]\nenabled = false\n[model_providers.agentdock_probe]\nname = "AgentDock local fixture"\nbase_url = "http://127.0.0.1:${port}/v1"\nwire_api = "responses"\nrequires_openai_auth = false\nsupports_websockets = false\nrequest_max_retries = 0\nstream_max_retries = 0\nstream_idle_timeout_ms = 60000\n[agents]\nmax_depth = 2\n`);
// Remove inherited auth/network overrides from the child environment.
const baseConfig = readFileSync(join(home, 'config.toml'), 'utf8');
const configureMcp = enabled => writeFileSync(join(home, 'config.toml'), baseConfig +
  `\n[mcp_servers.agentdock_fixture]\ncommand = ${JSON.stringify(process.execPath.replaceAll('\\', '/'))}\nargs = [${JSON.stringify(join(root, 'mcp-fixture.mjs').replaceAll('\\', '/'))}]\nenabled = ${enabled}\nstartup_timeout_sec = 5\n`);
configureMcp(true);
const env = { ...process.env, CODEX_HOME: home };
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
        // Accept only the explicitly scoped bounded command fixture; reject other requests.
        writeFileSync(join(run,'server-request.json'),JSON.stringify(m,null,2));
        this.proc.stdin.write(JSON.stringify(serverReply(m)) + '\n');
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
  if(selectedChecks&&!selectedChecks.includes(name))return;
  try { const evidence = await fn(); results.push({ name, status: 'pass', evidence }); }
  catch (e) { results.push({ name, status: 'fail', error: e.message }); }
  console.log(`${results.at(-1).status}: ${name}`);
  writeFileSync(join(run,'partial-result.json'),JSON.stringify({results,modelRequests:calls},null,2));
  if(results.at(-1).status==='fail') writeFileSync(join(run,`${name}-events.json`),JSON.stringify(clients.flatMap(c=>c.events),null,2));
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
  c.close = async () => {
    if (ws.readyState >= 2) return;
    const closed = new Promise(r => ws.addEventListener('close',()=>r(true), { once: true }));
    ws.close(); c.closeAcknowledged=await Promise.race([closed,sleep(2000).then(()=>false)]);
  };
  clients.push(c); return c;
}
async function connectDroppable(wsPort) {
  const pipes=new Set();
  const proxy=createTcpServer(socket=>{
    const upstream=connectTcp(wsPort,'127.0.0.1'); pipes.add(socket);pipes.add(upstream);
    socket.pipe(upstream);upstream.pipe(socket);
    socket.on('error',()=>{});upstream.on('error',()=>{});
    socket.on('close',()=>{pipes.delete(socket);upstream.destroy();});
    upstream.on('close',()=>{pipes.delete(upstream);socket.destroy();});
  });
  await new Promise(r=>proxy.listen(0,'127.0.0.1',r));
  const c=await connectWs(`ws://127.0.0.1:${proxy.address().port}`);
  c.drop=async()=>{for(const socket of pipes)socket.destroy();await new Promise(r=>proxy.close(r));await sleep(100);};
  const gracefulClose=c.close;c.close=async()=>{await c.drop();await gracefulClose();};return c;
}
const input = text => [{type:'text',text}];
const waitFor = async (test, label, ms=10000) => {
  const end=Date.now()+ms; while(Date.now()<end) { const value=await test(); if(value) return value; await sleep(50); }
  throw new Error(`Timeout: ${label}`);
};
const cli = args => execFileSync(exe,args,{env,cwd:work,windowsHide:true,encoding:'utf8',timeout:15000,stdio:['ignore','pipe','pipe']});
const startThread = c => c.ok('thread/start',{cwd:work,approvalPolicy:'never',sandbox:'read-only'});
const startHeld = async (c,id,text='PROBE: held') => {
  mode='hold'; const before=requestCount;
  const r=await c.ok('turn/start',{threadId:id,input:input(text)});
  await waitFor(()=>requestCount>before,'fixture reached'); return r.turn.id;
};
const finish = async (c,id) => {
  mode='complete'; const from=c.events.length;
  const r=await c.ok('turn/start',{threadId:id,input:input('PROBE: completed baseline')});
  await c.event('turn/completed',p=>p.turn.id===r.turn.id&&p.turn.status==='completed',from);
};
try {
  client = new Client(); await client.init();
  await check('deep_plugin_skill_hook_visibility',async()=>{
    const marketplace=join(run,'marketplace');const plugin=join(marketplace,'fixture');
    for(const dir of [join(marketplace,'.claude-plugin'),join(plugin,'.claude-plugin'),join(plugin,'skills','probe'),join(plugin,'hooks')])mkdirSync(dir,{recursive:true});
    writeFileSync(join(marketplace,'.claude-plugin','marketplace.json'),JSON.stringify({name:'agentdock-probe',plugins:[{name:'fixture',source:'./fixture'}]}));
    writeFileSync(join(plugin,'.claude-plugin','plugin.json'),JSON.stringify({name:'fixture',version:'0.1.0',skills:'./skills',hooks:'./hooks/hooks.json'}));
    writeFileSync(join(plugin,'skills','probe','SKILL.md'),'---\nname: fixture-probe\ndescription: Authorized fixture only.\n---\nAGENTDOCK_UNIQUE_SKILL_BODY_20261004\nReturn PROBE_OK.\n');
    const hookScript=join(plugin,'hooks','fixture.mjs');const hookLog=join(work,'hook-invocations.txt');
    writeFileSync(hookScript,`import {appendFileSync} from 'node:fs';appendFileSync(${JSON.stringify(hookLog)},'PROBE_HOOK\\n');console.log('{}');`);
    const hookCommand=`\"${process.execPath}\" \"${hookScript}\"`;
    writeFileSync(join(plugin,'hooks','hooks.json'),JSON.stringify({hooks:{UserPromptSubmit:[{hooks:[{type:'command',command:hookCommand,timeout:3}]}]}}));
    cli(['plugin','marketplace','add',marketplace,'--json']);const installed=JSON.parse(cli(['plugin','add','fixture@agentdock-probe','--json']));
    await client.close();client=new Client();await client.init();mode='complete';
    const skills=await client.ok('skills/list',{cwds:[work],forceReload:true});
    writeFileSync(join(run,'skill-list.json'),JSON.stringify(skills,null,2));
    const skill=skills.data.flatMap(x=>x.skills??[]).find(x=>x.pluginId==='fixture@agentdock-probe');assert(skill,'Installed fixture skill absent');
    const rows=[];
    const discovered=await client.ok('hooks/list',{cwds:[work]});const hook=discovered.data.flatMap(x=>x.hooks).find(x=>x.pluginId==='fixture@agentdock-probe');assert(hook?.currentHash,'Fixture hook hash absent');
    for(const state of [{trusted:false,disabled:false},{trusted:false,disabled:true},{trusted:true,disabled:false},{trusted:true,disabled:true}]){
      const {trusted,disabled}=state;
      if(trusted&& !rows.some(x=>x.trusted)){
        const configPath=join(home,'config.toml');
        writeFileSync(configPath,readFileSync(configPath,'utf8')+`\n[hooks.state.${JSON.stringify(hook.key)}]\ntrusted_hash = ${JSON.stringify(hook.currentHash)}\nenabled = true\n`);
        await client.close();client=new Client();await client.init();
      }
      const id=(await startThread(client)).thread.id;
      if(disabled)await client.ok('thread/settings/update',{threadId:id,disabledPluginIds:['fixture@agentdock-probe']});
      const hooks=await client.rpc('hooks/list',{cwds:[work]});const from=client.events.length;
      const turn=await client.ok('turn/start',{threadId:id,input:[...input(`PROBE: explicit $${skill.name} invocation`),{type:'skill',name:skill.name,path:skill.path}]});
      const ended=await client.event('turn/completed',p=>p.turn.id===turn.turn.id,from);
      rows.push({trusted,disabled,threadId:id,skillPath:skill.path,skillBodyInjected:calls.at(-1).skillBodyInjected,hooks,turnStatus:ended.turn.status,
        hookLogExists:existsSync(hookLog),hookInvocationCount:existsSync(hookLog)?readFileSync(hookLog,'utf8').split('PROBE_HOOK').length-1:0,
        hookNotifications:client.events.slice(from).filter(e=>e.method.startsWith('hook/'))});
    }
    return {installed,rows,scope:'Fresh threads; explicit skill body injection; only reviewed fixture exact hook hash trusted in isolated CODEX_HOME, no global bypass.'};
  });
  await check('deep_managed_os_descendants',async()=>{
    const fixture=join(work,'managed-tree.mjs');
    writeFileSync(fixture,"import {spawn} from 'node:child_process';console.log(`PROBE_PID=${process.pid}`);const depth=Number(process.argv[2]??0);if(depth<2)spawn(process.execPath,[process.argv[1],String(depth+1)],{stdio:['ignore','inherit','inherit'],windowsHide:true});const tick=setInterval(()=>{},1000);setTimeout(()=>{clearInterval(tick);process.exit(0)},45000);\n");
    const alive=pid=>{try{process.kill(pid,0);return true;}catch(e){return e.code==='ESRCH'?false:'unknown:'+e.code;}};const rows=[];
    for(const operation of ['terminate','clean']){
      const id=(await client.ok('thread/start',{cwd:work,approvalPolicy:'on-request',sandbox:'read-only'})).thread.id;
      mode='hold';const pidFrom=fixturePids.length;const marker=`PROBE: OS tree ${operation}`;
      permittedFixtureCommand=`& '${process.execPath.replaceAll("'","''")}' '${fixture.replaceAll("'","''")}'`;
      toolPlan={name:'exec_command',marker,args:{cmd:permittedFixtureCommand,shell:'powershell.exe',login:false,workdir:work,sandbox_permissions:'require_escalated',justification:'Run only the authorized bounded AgentDock OS descendant fixture.',yield_time_ms:1000,max_output_tokens:1000}};
      const turn=await client.ok('turn/start',{threadId:id,input:input(marker)});
      await waitFor(()=>new Set(fixturePids.slice(pidFrom)).size===3,'three bounded OS fixture PIDs',18000);
      const pids=[...new Set(fixturePids.slice(pidFrom))];const info=(await client.ok('thread/backgroundTerminals/list',{threadId:id})).data.find(x=>x.status!=='exited');assert(info,'Managed tree absent');
      const before=pids.map(pid=>({pid,alive:alive(pid)}));
      const ack=await client.ok(operation==='terminate'?'thread/backgroundTerminals/terminate':'thread/backgroundTerminals/clean',operation==='terminate'?{threadId:id,processId:info.processId}:{threadId:id});
      await sleep(500);const after=pids.map(pid=>({pid,alive:alive(pid)}));const list=await client.ok('thread/backgroundTerminals/list',{threadId:id});
      rows.push({operation,threadId:id,info,ack,before,after,list,observationMs:500});
      await client.ok('turn/interrupt',{threadId:id,turnId:turn.turn.id});
      // Only PIDs printed by this exact 45-second fixture may be cleaned up.
      for(const pid of pids)if(alive(pid)===true)process.kill(pid);
      writeFileSync(join(run,'os-descendants.json'),JSON.stringify(rows,null,2));
    }
    return rows;
  });
  await check('deep_queue_completion_failure_and_child',async()=>{
    const rows=[];mode='hold';
    for(const scenario of ['normal','failure','interrupt','active-child']){
      const id=(await startThread(client)).thread.id;
      const marker=`PROBE: queue boundary ${scenario}`;const queuedMarker=`PROBE: remaining ${scenario}`;
      responseModes.set(queuedMarker,'hold');
      if(scenario==='active-child')toolPlan={name:'multi_agent_v1.spawn_agent',marker,args:{message:'PROBE_CHILD_QUEUE: held child only'}};
      if(scenario==='failure')responseModes.set(marker,'hold');
      const from=client.events.length;const turnId=await startHeld(client,id,marker);
      let child=null;
      if(scenario==='active-child'){
        const e=await client.event('item/completed',p=>p.threadId===id&&p.item.type==='collabAgentToolCall'&&p.item.tool==='spawnAgent',from);
        child=e.item.receiverThreadIds[0];
      }
      await waitFor(()=>[...heldResponses.keys()].some(k=>k.includes(marker)),'parent held response');
      cli(['queue','--thread',id,'--message',queuedMarker]);
      const queueBefore=await client.ok('thread/queue/list',{threadId:id});const before=requestCount;
      if(scenario==='interrupt'||scenario==='failure'){
        if(scenario==='failure')[...heldResponses].find(([k])=>k.includes(marker))[1].fail();
        else await client.ok('turn/interrupt',{threadId:id,turnId});
        await client.event('turn/completed',p=>p.turn.id===turnId&&p.turn.status===(scenario==='failure'?'failed':'interrupted'),from);
      }else{[...heldResponses].find(([k])=>k.includes(marker))[1]();await client.event('turn/completed',p=>p.turn.id===turnId&&p.turn.status==='completed',from);}
      await sleep(1500);
      const queueAfter=await client.ok('thread/queue/list',{threadId:id});
      const state=await client.ok('thread/read',{threadId:id,includeTurns:true});
      const childState=child?(await client.ok('thread/read',{threadId:child,includeTurns:true})).thread:null;
      rows.push({scenario,threadId:id,turnId,queueBefore,queueAfter,requestsBefore:before,requestsAfter:requestCount,
        turns:state.thread.turns.map(t=>({id:t.id,status:t.status})),child:childState&&{id:child,status:childState.status,turns:childState.turns.map(t=>({id:t.id,status:t.status}))},observationMs:1500});
      for(const target of [id,child].filter(Boolean)){
        const r=await client.ok('thread/read',{threadId:target,includeTurns:true});
        for(const t of r.thread.turns.filter(t=>t.status==='inProgress'))await client.ok('turn/interrupt',{threadId:target,turnId:t.id});
      }
      writeFileSync(join(run,'queue-boundaries.json'),JSON.stringify(rows,null,2));
    }
    return rows;
  });
  await check('deep_tree_disconnect_recovery',async()=>{
    const reserve=createServer();await new Promise(r=>reserve.listen(0,'127.0.0.1',r));const wsPort=reserve.address().port;await new Promise(r=>reserve.close(r));
    wsProc=spawn(exe,['app-server','--listen',`ws://127.0.0.1:${wsPort}`],{env,cwd:work,windowsHide:true});wsProc.stdout.resume();wsProc.stderr.resume();
    await waitFor(async()=>{try{return(await fetch(`http://127.0.0.1:${wsPort}/readyz`,{signal:AbortSignal.timeout(300)})).ok;}catch{return false;}},'tree ws ready');
    const first=await connectDroppable(wsPort);await first.init();mode='hold';
    const parent=(await startThread(first)).thread.id;const from=first.events.length;
    toolPlan={name:'multi_agent_v1.spawn_agent',marker:'PROBE: recovery parent',args:{message:'PROBE_CHILD_RECOVERY: hold'}};
    await startHeld(first,parent,'PROBE: recovery parent');
    const child=(await first.event('item/completed',p=>p.threadId===parent&&p.item.tool==='spawnAgent',from)).item.receiverThreadIds[0];
    const c=await waitFor(async()=>{const r=await first.ok('thread/read',{threadId:child,includeTurns:true});return r.thread.turns.find(t=>t.status==='inProgress');},'recovery child active');
    await first.ok('turn/interrupt',{threadId:child,turnId:c.id});
    const childFrom=first.events.length;
    toolPlan={name:'multi_agent_v1.spawn_agent',marker:'PROBE: recovery child',args:{message:'PROBE_GRANDCHILD_RECOVERY: hold'}};
    await startHeld(first,child,'PROBE: recovery child');
    const grandchild=(await first.event('item/completed',p=>p.threadId===child&&p.item.tool==='spawnAgent',childFrom)).item.receiverThreadIds[0];
    await waitFor(()=>[...heldResponses.keys()].some(k=>k.includes('PROBE_GRANDCHILD_RECOVERY')),'grandchild held');
    const before=requestCount;await first.drop();
    [...heldResponses].find(([k])=>k.includes('PROBE_GRANDCHILD_RECOVERY'))[1]();await sleep(500);
    const next=await connectDroppable(wsPort);await next.init();const loaded=await next.ok('thread/loaded/list');
    const rows=[];for(const id of [parent,child,grandchild]){
      const r=await next.ok('thread/read',{threadId:id,includeTurns:true});
      rows.push({id,loaded:loaded.data.includes(id),source:r.thread.source,status:r.thread.status,turns:r.thread.turns.map(t=>({id:t.id,status:t.status}))});
    }
    const afterRead=requestCount;const eventsBeforeResume=next.events.map(e=>e.method);
    await next.ok('thread/resume',{threadId:parent});await next.ok('thread/resume',{threadId:child});await next.ok('thread/resume',{threadId:grandchild});
    await sleep(300);const afterResume=requestCount;const notifications=[];
    for(const id of [grandchild,child,parent]){
      const r=await next.ok('thread/read',{threadId:id,includeTurns:true});
      for(const t of r.thread.turns.filter(t=>t.status==='inProgress')){
        const offset=next.events.length;await next.ok('turn/interrupt',{threadId:id,turnId:t.id});
        const ended=await next.event('turn/completed',p=>p.threadId===id&&p.turn.id===t.id,offset);notifications.push({threadId:id,status:ended.turn.status});
      }
    }
    assert(rows[1].source.subAgent.thread_spawn.parent_thread_id===parent,'Recovered child ancestry wrong');
    assert(rows[2].source.subAgent.thread_spawn.parent_thread_id===child,'Recovered grandchild ancestry wrong');
    assert(rows[2].turns.some(t=>t.status==='completed'),'Missed grandchild completion not recovered by read');
    assert(before===afterRead&&afterRead===afterResume,'Queue-free recovery unexpectedly inferred');
    return {parent,child,grandchild,rows,before,afterRead,afterResume,eventsBeforeResume,notifications,knownIdsRecoveryOnly:true};
  });
  await check('fixture_plugin_capability_filter',async()=>{
    const marketplace=join(run,'marketplace'); const plugin=join(marketplace,'fixture');
    mkdirSync(join(marketplace,'.claude-plugin'),{recursive:true});
    mkdirSync(join(plugin,'.claude-plugin'),{recursive:true});
    mkdirSync(join(plugin,'skills','probe'),{recursive:true});
    writeFileSync(join(marketplace,'.claude-plugin','marketplace.json'),JSON.stringify({name:'agentdock-probe',plugins:[{name:'fixture',source:'./fixture'}]}));
    writeFileSync(join(plugin,'.claude-plugin','plugin.json'),JSON.stringify({name:'fixture',version:'0.1.0',skills:'./skills',mcpServers:'./.mcp.json'}));
    writeFileSync(join(plugin,'skills','probe','SKILL.md'),'---\nname: fixture-probe\ndescription: Deterministic AgentDock probe only.\n---\nReturn PROBE_SKILL. No general tasks.\n');
    writeFileSync(join(plugin,'.mcp.json'),JSON.stringify({mcpServers:{plugin_fixture:{command:process.execPath,args:[join(root,'mcp-fixture.mjs')]}}}));
    const added=JSON.parse(cli(['plugin','marketplace','add',marketplace,'--json']));
    const installed=JSON.parse(cli(['plugin','add','fixture@agentdock-probe','--json']));
    writeFileSync(join(run,'plugin-cli.json'),JSON.stringify({added,installed},null,2));
    await client.close(); client=new Client(); await client.init();
    const t=await startThread(client); const id=t.thread.id;
    const status=await waitFor(async()=>{
      const s=await client.ok('mcpServerStatus/list',{threadId:id});
      return s.data.find(x=>x.pluginId==='fixture@agentdock-probe'&&x.runtimeStatus==='connected');
    },'plugin MCP connected');
    await finish(client,id); const enabledTools=calls.at(-1).tools;
    const namespace=enabledTools.find(x=>x.includes('plugin_fixture'));
    assert(namespace,'Installed plugin MCP namespace absent');
    await client.ok('thread/settings/update',{threadId:id,disabledPluginIds:['fixture@agentdock-probe']});
    await finish(client,id); const disabledTools=calls.at(-1).tools;
    const after=await client.ok('mcpServerStatus/list',{threadId:id});
    const skills=await client.ok('skills/list',{cwds:[work],forceReload:true});
    const disabledSkillMentioned=calls.at(-1).fixtureSkillMentioned;
    const disabledSkillExposure={inInstructions:calls.at(-1).fixtureSkillInInstructions,
      inLastDeveloper:calls.at(-1).fixtureSkillInLastDeveloper,
      inRequestContext:disabledSkillMentioned};
    assert(!disabledTools.includes(namespace),'Thread-disabled plugin MCP still advertised');
    assert(!after.data.some(s=>s.pluginId==='fixture@agentdock-probe'),'Thread-disabled plugin still in MCP runtime list');
    await client.ok('thread/settings/update',{threadId:id,disabledPluginIds:[]});await finish(client,id);
    const reenabledTools=calls.at(-1).tools;
    assert(reenabledTools.includes(namespace),'Reenabled plugin MCP namespace absent');
    const removed=JSON.parse(cli(['plugin','remove','fixture@agentdock-probe','--json']));
    await client.ok('config/mcpServer/reload');await sleep(300);await finish(client,id);
    const afterRemoveTools=calls.at(-1).tools;
    const afterRemoveStatus=await client.ok('mcpServerStatus/list',{threadId:id});
    const afterRemoveSkills=await client.ok('skills/list',{cwds:[work],forceReload:true});
    assert(!afterRemoveTools.includes(namespace),'Removed plugin MCP namespace still advertised after reload');
    return {installed,status,namespace,enabledTools,disabledTools,
      capabilityStillAdvertised:disabledTools.includes(namespace),after,skills,
      disabledSkillMentioned,disabledSkillExposure,reenabledTools,removed,afterRemoveTools,afterRemoveStatus,afterRemoveSkills,
      scope:'Actual local plugin MCP advertisement; not all Skills/Hooks/Apps execution'};
  });
  await check('saved_queue_cold_resume_side_effect',async()=>{
    const t=await startThread(client); const id=t.thread.id; await finish(client,id);
    await client.close();
    const before=requestCount; const queued=cli(['queue','--thread',id,'--message','PROBE: saved queue payload']);
    client=new Client(); await client.init();
    const list=await client.ok('thread/queue/list',{threadId:id});
    await client.ok('thread/read',{threadId:id,includeTurns:true}); await sleep(500);
    const afterRead=requestCount;
    const from=client.events.length; mode='hold';
    const resumed=await client.ok('thread/resume',{threadId:id});
    await sleep(1200); const afterResume=requestCount;
    const remaining=await client.ok('thread/queue/list',{threadId:id});
    const starts=client.events.slice(from).filter(e=>e.method==='turn/started');
    const history=await client.ok('thread/read',{threadId:id,includeTurns:true});
    for(const turn of history.thread.turns.filter(t=>t.status==='inProgress'))
      await client.ok('turn/interrupt',{threadId:id,turnId:turn.id});
    return {threadId:id,queued,list,remaining,before,afterRead,afterResume,
      readInitiatedInference:afterRead>before,resumeInitiatedInference:afterResume>afterRead,
      starts:starts.map(x=>x.params),resumedStatus:resumed.thread.status};
  });
  await check('all_connection_disconnect_matrix',async()=>{
    const reserve=createServer(); await new Promise(r=>reserve.listen(0,'127.0.0.1',r));
    const wsPort=reserve.address().port; await new Promise(r=>reserve.close(r));
    wsProc=spawn(exe,['app-server','--listen',`ws://127.0.0.1:${wsPort}`],{env,cwd:work,windowsHide:true});
    wsProc.stdout.resume(); wsProc.stderr.on('data',()=>{});
    await waitFor(async()=>{try{return(await fetch(`http://127.0.0.1:${wsPort}/readyz`,{signal:AbortSignal.timeout(300)})).ok;}catch{return false;}},'ws ready');
    const rows=[];
    for(const connectionCount of [1,2]) for(const delayMs of [0,500,2500]) for(let repeat=1;repeat<=2;repeat++){
      const c=await connectDroppable(wsPort); await c.init();
      const other=connectionCount===2?await connectDroppable(wsPort):null; if(other)await other.init();
      const t=await startThread(c); const id=t.thread.id; const turnId=await startHeld(c,id);
      if(other)await other.drop(); await c.drop(); await sleep(delayMs);
      const next=await connectWs(`ws://127.0.0.1:${wsPort}`); await next.init();
      const loaded=await next.ok('thread/loaded/list');
      const read=await next.ok('thread/read',{threadId:id,includeTurns:true});
      const retained=loaded.data.includes(id); const beforeResume=requestCount;
      await next.ok('thread/resume',{threadId:id}); await sleep(150);
      const state=await next.ok('thread/read',{threadId:id,includeTurns:true});
      const active=state.thread.turns.filter(t=>t.status==='inProgress');
      for(const turn of active) await next.ok('turn/interrupt',{threadId:id,turnId:turn.id});
      rows.push({connectionCount,delayMs,repeat,threadId:id,turnId,retained,
        persistedStatus:read.thread.turns.find(t=>t.id===turnId)?.status,
        runtimeStatus:read.thread.status,requestsDuringResume:requestCount-beforeResume});
      writeFileSync(join(run,'disconnect-matrix.json'),JSON.stringify(rows,null,2));
      await next.close(); console.log(`matrix ${connectionCount}/${delayMs}/${repeat}: retained=${retained}`);
    }
    return rows;
  });
  await check('warm_queue_reconnect_and_graceful_close',async()=>{
    const reserve=createServer();await new Promise(r=>reserve.listen(0,'127.0.0.1',r));
    const wsPort=reserve.address().port;await new Promise(r=>reserve.close(r));
    if(wsProc&&wsProc.exitCode===null){wsProc.kill();await new Promise(r=>wsProc.once('exit',r));}
    wsProc=spawn(exe,['app-server','--listen',`ws://127.0.0.1:${wsPort}`],{env,cwd:work,windowsHide:true});
    wsProc.stdout.resume();wsProc.stderr.on('data',()=>{});
    await waitFor(async()=>{try{return(await fetch(`http://127.0.0.1:${wsPort}/readyz`,{signal:AbortSignal.timeout(300)})).ok;}catch{return false;}},'queue ws ready');
    const first=await connectDroppable(wsPort);await first.init();
    const id=(await startThread(first)).thread.id;const turnId=await startHeld(first,id);
    const queued=cli(['queue','--thread',id,'--message','PROBE: warm queued payload']);
    const before=requestCount;await first.drop();await sleep(500);
    const next=await connectDroppable(wsPort);await next.init();
    const loaded=await next.ok('thread/loaded/list');
    const read=await next.ok('thread/read',{threadId:id,includeTurns:true});
    const queueBeforeResume=await next.ok('thread/queue/list',{threadId:id});
    await next.ok('thread/resume',{threadId:id});await sleep(500);
    const afterActiveResume=requestCount;
    const from=next.events.length;await next.ok('turn/interrupt',{threadId:id,turnId});
    await next.event('turn/completed',p=>p.turn.id===turnId&&p.turn.status==='interrupted',from);
    await sleep(500);const afterInterrupt=requestCount;
    const queueAfterInterrupt=await next.ok('thread/queue/list',{threadId:id});
    await next.ok('thread/resume',{threadId:id});await sleep(500);
    const afterIdleResume=requestCount;const queueAfterIdleResume=await next.ok('thread/queue/list',{threadId:id});
    const history=await next.ok('thread/read',{threadId:id,includeTurns:true});
    for(const turn of history.thread.turns.filter(t=>t.status==='inProgress'))await next.ok('turn/interrupt',{threadId:id,turnId:turn.id});
    await next.close();
    // Separately measure the WebSocket close handshake without assuming TCP has dropped.
    const graceful=await connectWs(`ws://127.0.0.1:${wsPort}`);await graceful.init();
    const graceId=(await startThread(graceful)).thread.id;const graceTurn=await startHeld(graceful,graceId);
    const closedAt=Date.now();await graceful.close();const closeElapsedMs=Date.now()-closedAt;
    const observer=await connectDroppable(wsPort);await observer.init();
    const loadedAfterClose=await observer.ok('thread/loaded/list');
    const graceRead=await observer.ok('thread/read',{threadId:graceId,includeTurns:true});
    if(loadedAfterClose.data.includes(graceId)){
      await observer.ok('thread/resume',{threadId:graceId});
      const r=await observer.ok('thread/read',{threadId:graceId,includeTurns:true});
      for(const t of r.thread.turns.filter(t=>t.status==='inProgress'))await observer.ok('turn/interrupt',{threadId:graceId,turnId:t.id});
    }
    await observer.close();
    return {threadId:id,queued,retainedLoaded:loaded.data.includes(id),readStatus:read.thread.status,
      queueBeforeResume,queueAfterInterrupt,queueAfterIdleResume,before,afterActiveResume,afterInterrupt,afterIdleResume,
      graceful:{threadId:graceId,turnId:graceTurn,closeAcknowledged:graceful.closeAcknowledged,
        closeElapsedMs,retainedLoaded:loadedAfterClose.data.includes(graceId),readStatus:graceRead.thread.status,
        persistedStatus:graceRead.thread.turns.find(t=>t.id===graceTurn)?.status}};
  });
  await check('parent_child_grandchild_interrupt',async()=>{
    const parent=(await startThread(client)).thread.id;
    mode='hold'; toolPlan={name:'multi_agent_v1.spawn_agent',marker:'PROBE: spawn fixture child',args:{message:'PROBE_CHILD: hold fixture stream only. No general work.'}};
    const from=client.events.length; const rootTurn=await startHeld(client,parent,'PROBE: spawn fixture child');
    const childSpawn=await client.event('item/completed',p=>p.threadId===parent&&p.item.type==='collabAgentToolCall'&&p.item.tool==='spawnAgent',from);
    const child=childSpawn.item.receiverThreadIds[0];
    await waitFor(async()=>{const r=await client.ok('thread/read',{threadId:child,includeTurns:true});return r.thread.turns.find(t=>t.status==='inProgress');},'child active');
    let childState=await client.ok('thread/read',{threadId:child,includeTurns:true});
    for(const t of childState.thread.turns.filter(t=>t.status==='inProgress'))await client.ok('turn/interrupt',{threadId:child,turnId:t.id});
    toolPlan={name:'multi_agent_v1.spawn_agent',marker:'PROBE: spawn fixture grandchild',args:{message:'PROBE_GRANDCHILD: hold fixture stream only. No general work.'}};
    const childFrom=client.events.length; const childTurn=await startHeld(client,child,'PROBE: spawn fixture grandchild');
    const grandSpawn=await client.event('item/completed',p=>p.threadId===child&&p.item.type==='collabAgentToolCall'&&p.item.tool==='spawnAgent',childFrom);
    const grandchild=grandSpawn.item.receiverThreadIds[0];
    await waitFor(async()=>{const r=await client.ok('thread/read',{threadId:grandchild,includeTurns:true});return r.thread.turns.some(t=>t.status==='inProgress');},'grandchild active');
    const stopFrom=client.events.length; await client.ok('turn/interrupt',{threadId:parent,turnId:rootTurn});
    await client.event('turn/completed',p=>p.threadId===parent&&p.turn.id===rootTurn&&p.turn.status==='interrupted',stopFrom);
    await sleep(300);
    const snapshot=[];
    for(const id of [parent,child,grandchild]){
      const r=await client.ok('thread/read',{threadId:id,includeTurns:true});
      snapshot.push({threadId:id,parentThreadId:r.thread.parentThreadId,source:r.thread.source,status:r.thread.status,turns:r.thread.turns.map(t=>({id:t.id,status:t.status}))});
    }
    assert(snapshot[1].source.subAgent.thread_spawn.parent_thread_id===parent,'Wrong child parent');
    assert(snapshot[2].source.subAgent.thread_spawn.parent_thread_id===child,'Wrong grandchild parent');
    const cleanup=[];
    for(const id of [grandchild,child]){
      const r=await client.ok('thread/read',{threadId:id,includeTurns:true});
      for(const t of r.thread.turns.filter(t=>t.status==='inProgress')){
        await client.ok('turn/interrupt',{threadId:id,turnId:t.id});
        const stopped=await client.ok('thread/read',{threadId:id,includeTurns:true});
        const status=stopped.thread.turns.find(x=>x.id===t.id)?.status;
        assert(status==='interrupted','Individual descendant interruption unconfirmed');
        cleanup.push({threadId:id,turnId:t.id,status});
      }
    }
    return {parent,child,grandchild,childTurn,snapshot,cleanup,
      childSpawn,grandSpawn,
      childInternalNotifications:client.events.slice(from).filter(e=>e.params?.threadId===child).map(e=>e.method),
      grandchildInternalNotifications:client.events.slice(childFrom).filter(e=>e.params?.threadId===grandchild).map(e=>e.method)};
  });
  await check('managed_execution_interrupt_terminate_clean',async()=>{
    const fixture=join(work,'managed-process-fixture.mjs');
    writeFileSync(fixture,'console.log(`PROBE_PID=${process.pid}`);\nconst tick=setInterval(()=>{},1000);\nsetTimeout(()=>{clearInterval(tick);process.exit(0);},45000);\n');
    const rows=[];
    const alive=pid=>{try{process.kill(pid,0);return true;}catch(e){return e.code==='ESRCH'?false:'unknown:'+e.code;}};
    for(const operation of ['interrupt','terminate','clean']){
      const id=(await client.ok('thread/start',{cwd:work,approvalPolicy:'on-request',sandbox:'read-only'})).thread.id;
      mode='hold';const before=requestCount;const pidFrom=fixturePids.length;
      permittedFixtureCommand=`& '${process.execPath.replaceAll("'","''")}' '${fixture.replaceAll("'","''")}'`;
      toolPlan={name:'exec_command',marker:`PROBE: bounded managed execution ${operation}`,args:{cmd:permittedFixtureCommand,shell:'powershell.exe',login:false,workdir:work,sandbox_permissions:'require_escalated',justification:'Run the authorized, bounded AgentDock stop-test fixture only.',yield_time_ms:1000,max_output_tokens:1000}};
      const turn=await client.ok('turn/start',{threadId:id,input:input(`PROBE: bounded managed execution ${operation}`)});
      const processInfo=await waitFor(async()=>{
        const r=await client.ok('thread/backgroundTerminals/list',{threadId:id});
        return r.data.find(x=>x.status!=='exited');
      },'managed terminal visible',15000);
      await waitFor(()=>fixturePids.length>pidFrom&&requestCount>before+1,'fixture PID and yielded tool',18000);
      const pid=fixturePids.at(-1);const beforeAlive=alive(pid);const from=client.events.length;
      assert(beforeAlive===true,'Fixture OS PID not alive at baseline');
      let ack;
      if(operation==='interrupt')ack=await client.ok('turn/interrupt',{threadId:id,turnId:turn.turn.id});
      else if(operation==='terminate')ack=await client.ok('thread/backgroundTerminals/terminate',{threadId:id,processId:processInfo.processId});
      else ack=await client.ok('thread/backgroundTerminals/clean',{threadId:id});
      await sleep(300);const afterList=await client.ok('thread/backgroundTerminals/list',{threadId:id});
      const afterAlive=alive(pid);
      const ended=client.events.slice(from).find(e=>e.method==='turn/completed'&&e.params.turn.id===turn.turn.id);
      rows.push({operation,threadId:id,turnId:turn.turn.id,processInfo,pid,beforeAlive,ack,afterList,afterAlive,turnStatus:ended?.params.turn.status??null});
      if(operation!=='interrupt')await client.ok('turn/interrupt',{threadId:id,turnId:turn.turn.id});
      writeFileSync(join(run,'managed-execution.json'),JSON.stringify(rows,null,2));
      if(afterList.data.some(p=>p.processId===processInfo.processId))
        await client.ok('thread/backgroundTerminals/terminate',{threadId:id,processId:processInfo.processId});
      // Cleanup is limited to a PID printed by the exact bounded fixture.
      if(alive(pid)===true)process.kill(pid);
    }
    return rows;
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
    run, selectedChecks:selectedChecks??'all', results, modelRequests: calls,
    limitations: ['No real cloud inference/authentication', 'Fixture agent observations are not universal recursive or OS descendant termination guarantees',
      'Plugin MCP advertisement is not all plugin capability filtering', 'Queue observations do not implement AgentDock queue policy',
      'Loopback WebSocket fixture is unauthenticated and experimental, not a production transport decision'] };
  writeFileSync(join(run, 'result.json'), JSON.stringify(report, null, 2));
  writeFileSync(join(root, 'latest-extended-result.json'), JSON.stringify(report, null, 2));
  console.log(`Result: ${join(run, 'result.json')}`);
}
// All owned servers and clients were closed above. Bound lingering platform handles.
process.exit(results.some(r => r.status === 'fail') ? 1 : 0);
