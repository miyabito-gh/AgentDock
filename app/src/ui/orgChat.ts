// 組織実行の役割チャットの表示用変換（純粋関数）。保存済みの名前・本文は書き換えず、表示だけを整える。
// 構造化出力の形は org-engine/src/schemas.rs の出力スキーマ（計画・Workerの報告・Reviewerの判定）に合わせる。
import type { Chat, ChatLocalView, HostSnapshot, OrgWorkflowView } from "../ipc/types";
import type { OrgBindingView } from "../ipc/gen/OrgBindingView";

/** 組織が最初の送信に付ける役割の指示の先頭（org-engine の with_instructions）。 */
export const ORG_INSTRUCTION_MARK = "【この役割の指示】";

const ROLE_NAME: Record<string, string> = { planner: "Planner", worker: "Worker", reviewer: "Reviewer", verifier: "Verifier" };
export const roleLabel = (role: string): string => ROLE_NAME[role] ?? role;

/** 役割チャットの表示名「<役割名> <タスクID>: <タスク名>」。組織が所有していない（無い・手動に切替済み）か、タスクが見つからなければ null（従来の名前のまま）。 */
export function orgChatTitle(org: OrgBindingView | null | undefined, wf: OrgWorkflowView | null | undefined): string | null {
  if (!org || org.released || !wf || wf.workflowId !== org.workflowId) return null;
  const t = wf.tasks.find((x) => x.id === org.taskId);
  if (!t) return null;
  return `${roleLabel(org.roleId)} ${t.id}: ${t.title}`;
}

/**
 * 組織が自分で作った役割チャットの根のエージェントの表示名（「Worker・build-1『build』」）。
 * 要求元などの名前が空のときだけ使う代わりの名前（組織の所有と分かっているチャットに限る）。
 */
export function orgAgentName(org: OrgBindingView | null | undefined, wf: OrgWorkflowView | null | undefined): string | null {
  if (!org || org.released || !wf || wf.workflowId !== org.workflowId) return null;
  const t = wf.tasks.find((x) => x.id === org.taskId);
  if (!t) return null;
  return `${roleLabel(org.roleId)}・${t.id}${t.title ? `『${t.title}』` : ""}`;
}

/**
 * スナップショットのチャット名を、組織の役割チャットだけ上書きして返す（表示用。ホストの保存は変えない）。
 * 根のエージェントの表示名が空・欠損のときは、組織のタスク情報から補う。上書きがなければ同じ参照を返す。
 */
export function withOrgTitles(snap: HostSnapshot, wf: OrgWorkflowView | null | undefined): HostSnapshot {
  if (!wf) return snap;
  const names = new Map<string, string>();
  const agentNames = new Map<string, string>();
  for (const l of snap.chatLocals as ChatLocalView[]) {
    const n = orgChatTitle(l.org, wf);
    if (n) names.set(l.chat.id, n);
    const a = orgAgentName(l.org, wf);
    if (a) agentNames.set(l.chat.id, a);
  }
  if (names.size === 0 && agentNames.size === 0) return snap;
  const chats: Chat[] = snap.chats.map((c) => {
    const n = names.get(c.key.id);
    return n ? { ...c, name: { kind: "value", value: n, basis: "derived" } } : c;
  });
  const agents = snap.agents.map((v) => {
    const a = agentNames.get(v.agent.key.id);
    const dn = v.agent.displayName;
    if (!a || (dn.kind === "value" && dn.value.trim() !== "")) return v;
    return { ...v, agent: { ...v.agent, displayName: { kind: "value" as const, value: a, basis: "derived" as const } } };
  });
  return { ...snap, chats, agents };
}

/** 組織が自動送信した最初の指示か（先頭が目印で始まる）。利用者の通常の発話は対象外。 */
export const isOrgInstruction = (text: string): boolean => text.startsWith(ORG_INSTRUCTION_MARK);

// ---- 構造化JSONの最終返答 ----

export type OrgReport =
  | { kind: "verdict"; pass: boolean; findings: string[] }
  | { kind: "plan"; tasks: Array<{ id: string; role: string; title: string; deps: string[]; effects: string[]; instructions: string }> }
  | { kind: "worker"; summary: string; changedFiles: string[]; tests: { ran: boolean; passed: boolean } };

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const strs = (v: unknown): string[] | null => (Array.isArray(v) && v.every((x) => typeof x === "string") ? (v as string[]) : null);

/** 返答本文を既知の形として読む。JSONでない・形が違う・余計な項目があるときは null（生テキストのまま表示する）。 */
export function parseOrgReport(text: string): OrgReport | null {
  const s = text.trim();
  if (!s.startsWith("{")) return null;
  let v: unknown;
  try { v = JSON.parse(s); } catch { return null; }
  if (!isObj(v)) return null;
  const keys = Object.keys(v).sort().join(",");
  if (keys === "findings,verdict") {
    const f = strs(v.findings);
    if (!f || (v.verdict !== "pass" && v.verdict !== "rework")) return null;
    return { kind: "verdict", pass: v.verdict === "pass", findings: f };
  }
  if (keys === "tasks" && Array.isArray(v.tasks)) {
    const tasks: Extract<OrgReport, { kind: "plan" }>["tasks"] = [];
    for (const t of v.tasks) {
      if (!isObj(t)) return null;
      const deps = strs(t.deps);
      const effects = strs(t.effects);
      if (typeof t.id !== "string" || typeof t.role !== "string" || typeof t.title !== "string" || typeof t.instructions !== "string" || !deps || !effects) return null;
      tasks.push({ id: t.id, role: t.role, title: t.title, deps, effects, instructions: t.instructions });
    }
    return { kind: "plan", tasks };
  }
  if (keys === "changedFiles,summary,tests") {
    const files = strs(v.changedFiles);
    const tt = v.tests;
    if (!files || typeof v.summary !== "string" || !isObj(tt) || typeof tt.ran !== "boolean" || typeof tt.passed !== "boolean") return null;
    return { kind: "worker", summary: v.summary, changedFiles: files, tests: { ran: tt.ran, passed: tt.passed } };
  }
  return null;
}
