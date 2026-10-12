// 左一覧の「プロジェクトごと」表示のグルーピング。純粋関数。画面（LeftPane.tsx）から使う。
// 所属の決め方: ①チャットの projectId（バックエンドが示す確定値）、②無ければチャットのフォルダ（cwd＋分かっている追加フォルダ）と各プロジェクトのフォルダの一致による推測。
// 推測の順: (a) チャットのフォルダがプロジェクトの全フォルダ（roots）を含む（各rootに一致または内側）→ roots件数が多い方、同数なら一致rootが深い方、それも同数なら position（同じなら一覧順）最小。
// (b) 無ければ cwd が先頭root（プライマリ推定）の内側、(c) それも無ければ cwd がいずれかのrootの内側（深い方、同じ深さは position 順）。
// 先頭の1つの見出しにだけ置き、他の一致は multi で示す。
import type { Chat, ProjectSummary } from "../ipc/types";
import { UNKNOWN_KEY, UNKNOWN_LABEL, folderKey, usedTime, type FolderGroup } from "./folderGroups.ts";

export const PROJECT_KEY_PREFIX = "project:";
export const NO_PROJECT_KEY = "noproject";
export const NO_PROJECT_LABEL = "プロジェクトなし";

/** 推測の根拠。all＝すべてのフォルダを含む、primary＝先頭フォルダに一致、partial＝一部のフォルダに一致。 */
export type MatchBasis = "all" | "primary" | "partial";
export const BASIS_TEXT: Record<MatchBasis, string> = { all: "すべてのフォルダを含む", primary: "先頭フォルダに一致", partial: "一部のフォルダに一致" };

export interface ProjectMatch {
  /** 所属するプロジェクトのID（優先順）。空＝どのプロジェクトにも一致しない。2件以上＝推測で複数に一致。 */
  projectIds: string[];
  /** projectId による確定か（false＝フォルダ一致による推測、または一致なし）。 */
  confirmed: boolean;
  /** 推測の根拠（推測で一致したときだけ）。 */
  basis?: MatchBasis;
}

const inside = (k: string, rk: string) => k === rk || k.startsWith(rk.endsWith("\\") ? rk : rk + "\\");

/** チャットの所属。cwd が不明で projectId も無いときは判定できないので null（「なし」にしない）。`extraFolders` は分かっている追加フォルダ。 */
export function matchProjects(chat: Chat, projects: ProjectSummary[], extraFolders: string[] = []): ProjectMatch | null {
  if (chat.projectId && projects.some((p) => p.id === chat.projectId)) return { projectIds: [chat.projectId], confirmed: true };
  if (chat.kind === "general") return { projectIds: [], confirmed: false };
  const cwd = chat.cwd.kind === "value" && chat.cwd.value.trim() !== "" ? chat.cwd.value : null;
  if (!cwd) return null;
  const k = folderKey(cwd);
  const keys = [k, ...extraFolders.filter((f) => f.trim() !== "").map(folderKey)];
  const rootKeys = (p: ProjectSummary) => p.roots.filter((r) => r.trim() !== "").map(folderKey);
  const pos = (p: ProjectSummary) => p.position ?? 0;
  const idx = new Map(projects.map((p, i) => [p.id, i]));
  const byPos = (a: ProjectSummary, b: ProjectSummary) => (pos(a) - pos(b)) || (idx.get(a.id)! - idx.get(b.id)!);
  const out = (ps: ProjectSummary[], basis: MatchBasis): ProjectMatch => ({ projectIds: ps.map((p) => p.id), confirmed: false, basis });
  const depthOf = (p: ProjectSummary) => rootKeys(p).reduce((m, rk) => (inside(k, rk) && rk.length > m ? rk.length : m), -1);
  // (a) すべてのフォルダを含む
  const all = projects.filter((p) => { const rs = rootKeys(p); return rs.length > 0 && rs.every((rk) => keys.some((fk) => inside(fk, rk))); });
  if (all.length) return out([...all].sort((a, b) => (rootKeys(b).length - rootKeys(a).length) || (depthOf(b) - depthOf(a)) || byPos(a, b)), "all");
  // (b) 先頭rootの内側
  const prim = projects.filter((p) => { const r0 = rootKeys(p)[0]; return r0 !== undefined && inside(k, r0); });
  if (prim.length) {
    const d = (p: ProjectSummary) => rootKeys(p)[0].length;
    return out([...prim].sort((a, b) => (d(b) - d(a)) || byPos(a, b)), "primary");
  }
  // (c) いずれかのrootの内側（最も深い一致）
  const part = projects.filter((p) => depthOf(p) >= 0);
  if (part.length) return out([...part].sort((a, b) => (depthOf(b) - depthOf(a)) || byPos(a, b)), "partial");
  return { projectIds: [], confirmed: false };
}

export interface ProjectGrouping {
  groups: FolderGroup[];
  /** 複数のプロジェクトに一致したチャットのID → 見出しに置かなかった他の一致プロジェクト名。 */
  multi: Map<string, string[]>;
  /** 推測で入れたチャットのID → 一致の根拠。 */
  basis: Map<string, MatchBasis>;
}

/** グループ化。`chats` は表示する順（sortChats 済み・ピン留めチャットは除いたもの）。既定ではチャットが0件のプロジェクトの見出しも出す（件数0）。`includeEmpty=false`（検索中など）なら出さない。 */
export function groupByProject(chats: Chat[], projects: ProjectSummary[], pinnedKeys: string[], extraFoldersOf?: (c: Chat) => string[], includeEmpty = true): ProjectGrouping {
  const byId = new Map(projects.map((p) => [p.id, p]));
  const map = new Map<string, FolderGroup>();
  const multi = new Map<string, string[]>();
  const basis = new Map<string, MatchBasis>();
  const add = (key: string, make: () => Pick<FolderGroup, "kind" | "path" | "label" | "roots">, c: Chat, guessed: boolean) => {
    const t = usedTime(c);
    let g = map.get(key);
    if (!g) { g = { key, ...make(), chats: [], latest: null, pinned: pinnedKeys.includes(key) && key !== UNKNOWN_KEY }; map.set(key, g); }
    if (t !== null && (g.latest === null || t > g.latest)) g.latest = t;
    if (guessed) g.guessedCount = (g.guessedCount ?? 0) + 1;
    g.chats.push(c);
  };
  for (const c of chats) {
    const m = matchProjects(c, projects, extraFoldersOf?.(c) ?? []);
    if (!m) { add(UNKNOWN_KEY, () => ({ kind: "unknown", path: UNKNOWN_LABEL }), c, false); continue; }
    if (m.projectIds.length === 0) { add(NO_PROJECT_KEY, () => ({ kind: "noProject", path: NO_PROJECT_LABEL }), c, false); continue; }
    const ids = m.projectIds;
    if (m.basis) basis.set(c.key.id, m.basis);
    if (ids.length > 1) multi.set(c.key.id, ids.slice(1).map((id) => byId.get(id)!.name));
    const p = byId.get(ids[0])!;
    add(PROJECT_KEY_PREFIX + p.id, () => ({ kind: "project", path: p.name, label: p.name, roots: p.roots }), c, !m.confirmed);
  }
  if (includeEmpty) {
    for (const p of projects) {
      const key = PROJECT_KEY_PREFIX + p.id;
      if (!map.has(key)) map.set(key, { key, kind: "project", path: p.name, label: p.name, roots: p.roots, chats: [], latest: null, pinned: pinnedKeys.includes(key) });
    }
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
  return { groups, multi, basis };
}
