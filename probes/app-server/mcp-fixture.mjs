import { createInterface } from 'node:readline';
const input = createInterface({ input: process.stdin });
input.on('line', line => {
  const m = JSON.parse(line); if (m.id === undefined) return;
  let result;
  if (m.method === 'initialize') result = {
    protocolVersion: m.params.protocolVersion, capabilities: { tools: {} },
    serverInfo: { name: 'agentdock-probe-mcp', version: '0.1' }
  };
  else if (m.method === 'tools/list') result = { tools: [{ name: 'probe_ping',
    description: 'Deterministic local adoption-test fixture.', inputSchema: { type: 'object', properties: {} } }] };
  else if (m.method === 'tools/call') result = { content: [{ type: 'text', text: 'PROBE_PONG' }] };
  else if (m.method === 'resources/list') result = { resources: [] };
  else if (m.method === 'resources/templates/list') result = { resourceTemplates: [] };
  else if (m.method === 'ping') result = {};
  else { process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: m.id,
    error: { code: -32601, message: 'Unsupported fixture method' } }) + '\n'); return; }
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: m.id, result }) + '\n');
});
input.on('close', () => process.exit(0));
