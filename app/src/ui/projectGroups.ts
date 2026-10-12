// 左一覧の「プロジェクトごと」表示のグルーピング。純粋関数。画面（LeftPane.tsx）から使う。
// 所属の決め方: ①チャットの projectId（バックエンドが示す確定値）、②無ければ cwd と各プロジェクトのフォルダの一致による推測。
// 推測は、フォルダ自体または内側で一致するもののうち最も深いフォルダのプロジェクト。同じ深さで複数一致したら、プロジェクト順（position、同じなら一覧順）で最初の1つの見出しにだけ置き、他の一致は multi で示す。
import type { Chat, ProjectSummary } from "../ipc/types";
import { UNKNOWN_KEY, UNKNOWN_LABEL, folderKey, usedTime, type FolderGroup } from "./folderGroups.ts";

export const PROJECT_KEY_PREFIX = "project:";
export const NO_PROJECT_KEY = "noproject";
export const NO_PROJECT_LABEL = "プロジェクトなし";

export interface ProjectMatch {
  /** 所属するプロジェクトのID。空＝どのプロジェクトにも一致しない。2件以上＝推測で複数に一致。 */
  projectIds: string[];
  /** projectId による確定か（false＝フォルダ一致による推測、または一致なし）。 */
  confirmed: boolean;
}

const inside = (k: string, rk: string) => k === rk || k.startsWith(rk.endsWith("\\") ? rk : rk + "\\");

/** チャットの所属。cwd が不明で projectId も無いときは判定できないので null（「なし」にしない）。 */
export function matchProjects(chat: Chat, projects: ProjectSummary[]): ProjectMatch | null {
  if (chat.projectId && projects.some((p) => p.id === chat.projectId)) return { projectIds: [chat.projectId], confirmed: true };
  if (chat.kind === "general") return { projectIds: [], confirmed: false };
  const cwd = chat.cwd.kind === "value" && chat.cwd.value.trim() !== "" ? chat.cwd.value : null;
  if (!cwd) return null;
  const k = folderKey(cwd);
  let best = -1;
  let ids: string[] = [];
  for (const p of projects) {
    let depth = -1;
    for (const r of p.roots) {
      if (r.trim() === "") continue;
      const rk = folderKey(r);
      if (inside(k, rk) && rk.length > depth) depth = rk.length;
    }
    if (depth < 0) continue;
    if (depth > best) { best = depth; ids = [p.id]; }
    else if (depth === best) ids.push(p.id);
  }
  return { projectIds: ids, confirmed: false };
}

export interface ProjectGrouping {
  groups: FolderGroup[];
  /** 複数のプロジェクトに一致したチャットのID → 見出しに置かなかった他の一致プロジェクト名。 */
  multi: Map<string, string[]>;
}

/** グループ化。`chats` は表示する順（sortChats 済み・ピン留めチャットは除いたもの）。チャットが空のプロジェクトの見出しは作らない。 */
export function groupByProject(chats: Chat[], projects: ProjectSummary[], pinnedKeys: string[]): ProjectGrouping {
  const byId = new Map(projects.map((p) => [p.id, p]));
  const map = new Map<string, FolderGroup>();
  const multi = new Map<string, string[]>();
  const order = new Map(projects.map((p) => [p.id, p.position ?? 0]));
  const idx = new Map(projects.map((p, i) => [p.id, i]));
  const add = (key: string, make: () => Pick<FolderGroup, "kind" | "path" | "label" | "roots">, c: Chat, guessed: boolean) => {
    const t = usedTime(c);
    let g = map.get(key);
    if (!g) { g = { key, ...make(), chats: [], latest: null, pinned: pinnedKeys.includes(key) && key !== UNKNOWN_KEY }; map.set(key, g); }
    if (t !== null && (g.latest === null || t > g.latest)) g.latest = t;
    if (guessed) g.guessedCount = (g.guessedCount ?? 0) + 1;
    g.chats.push(c);
  };
  for (const c of chats) {
    const m = matchProjects(c, projects);
    if (!m) { add(UNKNOWN_KEY, () => ({ kind: "unknown", path: UNKNOWN_LABEL }), c, false); continue; }
    if (m.projectIds.length === 0) { add(NO_PROJECT_KEY, () => ({ kind: "noProject", path: NO_PROJECT_LABEL }), c, false); continue; }
    const ids = m.projectIds.length > 1 ? [...m.projectIds].sort((a, b) => (order.get(a)! - order.get(b)!) || (idx.get(a)! - idx.get(b)!)) : m.projectIds;
    if (ids.length > 1) multi.set(c.key.id, ids.slice(1).map((id) => byId.get(id)!.name));
    const p = byId.get(ids[0])!;
    add(PROJECT_KEY_PREFIX + p.id, () => ({ kind: "project", path: p.name, label: p.name, roots: p.roots }), c, !m.confirmed);
  }
  const position = (g: FolderGroup) => byId.get(g.key.slice(PROJECT_KEY_PREFIX.length))?.position ?? 0;
  const rank = (g: FolderGroup) => (g.kind === "unknown" ? 3 : g.kind === "noProject" ? 2 : g.pinned ? 0 : 1);
  const groups = [...map.values()].map((g, i) => ({ g, i })).sort((a, b) => {
    const ra = rank(a.g), rb = rank(b.g);
    if (ra !== rb) return ra - rb;
    if (ra === 0) return pinnedKeys.indexOf(a.g.key) - pinnedKeys.indexOf(b.g.key);
    if (ra === 1) {
      const ta = a.g.latest, tb = b.g.latest;
      const d = ta === null ? (tb === null ? 0 : 1) : tb === null ? -1 : tb - ta;
      if (d) return d;
      if (position(a.g) !== position(b.g)) return position(a.g) - position(b.g);
    }
    return a.i - b.i;
  }).map((x) => x.g);
  return { groups, multi };
}
