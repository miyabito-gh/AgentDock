import { invoke } from "@tauri-apps/api/core";
import type { ChatKey } from "./gen/ChatKey";
import type { ModelChoice } from "./gen/ModelChoice";
import type { OrgArtifactFile } from "./gen/OrgArtifactFile";
import type { OrgImportOutcome } from "./gen/OrgImportOutcome";
import type { SubagentImportRecord } from "./gen/SubagentImportRecord";
import type { SubagentRunView } from "./gen/SubagentRunView";
import type { SubagentImportPreview } from "./gen/SubagentImportPreview";
import { asIpcError } from "./client";

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try { return await invoke<T>(command, args === undefined ? undefined : { args }); }
  catch (error) { throw asIpcError(error); }
}

export const subagentRuns = (): Promise<SubagentRunView[]> => call("subagent_runs");
export const subagentStart = (repo: string, request: string, model: ModelChoice | null): Promise<SubagentRunView> =>
  call("subagent_start", { repo, request, model });
export const subagentRecover = (id: string, parent: ChatKey): Promise<SubagentRunView> =>
  call("subagent_recover", { id, parent });
export const subagentImportPreview = (id: string): Promise<SubagentImportPreview> => call("subagent_import_preview", { id });
export const subagentImport = (id: string, token: string): Promise<OrgImportOutcome> => call("subagent_import", { id, token });
export const subagentVerifyImport = (id: string): Promise<SubagentImportRecord[]> => call("subagent_verify_import", { id });

export type { ChatKey, ModelChoice, OrgArtifactFile, OrgImportOutcome, SubagentImportRecord, SubagentImportPreview, SubagentRunView };
