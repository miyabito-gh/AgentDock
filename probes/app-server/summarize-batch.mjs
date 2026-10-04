// Combine selected observations without hiding incomplete or unsuccessful runs.
import { readFileSync, writeFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
const root=dirname(fileURLToPath(import.meta.url));
const selections=[
  ['idle_31min_read_recovery','2026-10-04T10-43-12-306Z'],
  ['plugin_concurrent_cli_config','2026-10-04T10-45-08-956Z'],
  ['plugin_version_refresh_remove_existing','2026-10-04T10-46-33-494Z'],
  ['managed_owner_scope_reconciliation','2026-10-04T10-49-50-044Z'],
  ['plugin_git_marketplace_upgrade_loopback','2026-10-04T10-49-01-537Z'],
  ['plugin_cli_api_config_version_conflict','2026-10-04T10-53-30-021Z'],
  ['read_subscription_and_explicit_continue','2026-10-04T10-54-59-697Z'],
  ['read_only_active_partial_polling','2026-10-04T11-09-15-132Z'],
  ['plugin_live_add_reload_capabilities','2026-10-04T10-36-18-033Z'],
  ['plugin_hook_definition_and_script_retrust','2026-10-04T10-36-18-033Z'],
  ['plugin_cli_partial_config_write_failure','2026-10-04T10-33-01-637Z'],
  ['fixture_plugin_capability_filter','2026-10-04T08-51-20-222Z'],
  ['saved_queue_cold_resume_side_effect','2026-10-04T08-42-30-412Z'],
  ['all_connection_disconnect_matrix','2026-10-04T08-42-30-412Z'],
  ['warm_queue_reconnect_and_graceful_close','2026-10-04T08-48-28-110Z'],
  ['parent_child_grandchild_interrupt','2026-10-04T08-49-44-364Z'],
  ['managed_execution_interrupt_terminate_clean','2026-10-04T08-47-29-135Z'],
  ['deep_tree_disconnect_recovery','2026-10-04T09-15-42-486Z'],
  ['deep_queue_completion_failure_and_child','2026-10-04T09-18-06-457Z'],
  ['deep_managed_os_descendants','2026-10-04T09-16-58-097Z'],
  ['deep_plugin_skill_hook_visibility','2026-10-04T09-19-58-962Z'],
  ['unseen_descendants_restart_read_only','2026-10-04T09-32-48-928Z'],
  ['reconnected_stop_completion_reconciliation','2026-10-04T09-32-06-429Z'],
  ['pagination_generation_and_rescan','2026-10-04T10-15-30-823Z'],
  ['stop_natural_completion_and_generation_race','2026-10-04T10-17-08-956Z'],
  ['longer_disconnect_queue_read_resume','2026-10-04T10-18-22-565Z'],
];
const observations=selections.map(([name,run])=>{
  const resultPath=join(root,'runs',run,'result.json');
  const report=JSON.parse(readFileSync(resultPath,'utf8'));
  const result=report.results.find(r=>r.name===name);
  if(!result||result.status!=='pass')throw new Error(`Unconfirmed selection ${name}`);
  if(report.version!=='codex-cli 0.160.0')throw new Error('Version mismatch');
  if(!report.results.some(r=>r.name==='existing_config_unchanged'&&r.status==='pass'))throw new Error('Original config changed');
  return {source:resultPath,version:report.version,existingConfigUnchanged:true,...result};
});
const trials=readdirSync(join(root,'runs')).map(run=>{
  try {
    const path=join(root,'runs',run,'result.json');
    const r=JSON.parse(readFileSync(path,'utf8'));
    let evidenceAssessment=null;
    try{evidenceAssessment=JSON.parse(readFileSync(join(root,'runs',run,'evidence-assessment.json'),'utf8'));}catch{}
    return {run,path,checks:r.results.map(x=>({name:x.name,status:x.status,error:x.error??null})),evidenceAssessment};
  }catch{return {run,status:'No final report; inspect files without inferring success'};}
});
const sourceCache=readdirSync(join(root,'source-cache')).map(name=>({name,
  url:`https://raw.githubusercontent.com/openai/codex/rust-v0.160.0/codex-rs/${({
    'common.rs':'app-server-protocol/src/protocol/common.rs',
    'marketplace.rs':'core-plugins/src/marketplace.rs',
    'multi_agents.rs':'core/src/tools/handlers/multi_agents.rs',
    'plugin_cmd.rs':'cli/src/plugin_cmd.rs',
    'queue_cmd.rs':'cli/src/queue_cmd.rs',
    'router.rs':'core/src/tools/router.rs',
    'spawn.rs':'core/src/tools/handlers/multi_agents/spawn.rs',
  })[name]}`,
  sha256:createHash('sha256').update(readFileSync(join(root,'source-cache',name))).digest('hex')}));
const followupSource=JSON.parse(readFileSync(join(root,'source-followup','manifest.json'),'utf8'));
followupSource.files=followupSource.files.map(entry=>{
  const bytes=readFileSync(join(root,'source-followup',entry.file));
  const gitBlobSha=createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex');
  if(gitBlobSha!==entry.sha)throw new Error(`Fixed-source byte mismatch: ${entry.file}`);
  return {...entry,gitBlobVerified:true,sha256:createHash('sha256').update(bytes).digest('hex')};
});
writeFileSync(join(root,'batch-result.json'),JSON.stringify({createdAt:new Date().toISOString(),
  interpretation:`${selections.length} selected measurement procedures completed across ${new Set(selections.map(x=>x[1])).size} runs. Not a single full-suite pass or AgentDock acceptance. Includes observed failures of desired native queue policy.`,
  observations,trials,sourceCache,
  followupSource,
  limitations:['No cloud inference/authentication','No universal OS descendant termination guarantee',
    'Graceful WebSocket close was not acknowledged within 2 seconds; cannot call it a proven all-client TCP disconnect',
    'Earlier unloaded-thread observation remains unresolved','Fixture MCP, explicit Skill and trusted UserPromptSubmit Hook filtering do not prove all Skills/Hooks/Apps filtering',
    'A newer descendant inserted between cursor pages was missed by one pass and recovered by a full rescan; no atomic snapshot guarantee',
    'Unknown children created with no live connection showed interrupt RPC timeouts despite persisted interrupted turns; underlying cause is not established',
    '120-second disconnect does not test the 30-minute idle expiry or Windows sleep; native idle queues can start while the UI is disconnected',
    'Separate 31-minute unconnected completed/queue-empty trial observed unloaded state and saved-history recovery without inference; exact unload timing, queued/active idle recovery and actual OS sleep remain untested',
    'Concurrent CLI and CLI/API trials did not force internal read/write interleaving; successful observations are not a cross-process transaction guarantee',
    'Skill token presence in historical model input is distinct from new injection; removal does not erase prior conversation instructions or effects',
    'Explicit turn/start after read did not restore turn/item subscription in the measured loaded root; status notifications do not prove full subscription',
    'Read-only polling returned active status but not in-flight assistant text chunks that were delivered to the original subscriber; it is not a proven full streaming substitute',
    'Queue policy and production implementation remain undecided']},null,2));
console.log('Saved batch-result.json');
