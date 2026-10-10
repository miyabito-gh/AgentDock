// 左一覧の「作業フォルダごと」表示のグルーピング（P5-1・M48）。純粋関数。画面（LeftPane.tsx）から使う。
// 規則: パスの正規化と所属の判定だけ。Git は実行せず、親フォルダへのまとめや外部 worktree の推定はしない。
import type { Chat, WorktreeRecord } from "../ipc/types";

export const GENERAL_KEY = "general";
export const UNKNOWN_KEY = "unknown";
export const GENERAL_LABEL = "一般チャット";
export const UNKNOWN_LABEL = "作業フォルダ不明";

/** フォルダのキー: `/`→`\`、先頭の `\\?\` を外す、末尾の `\` を外す（`C:\` は残す）、ASCII の大文字小文字を同一視。 */
export function folderKey(path: string): string {
  let p = path.replace(/\//g, "\\");
  if (p.startsWith("\\\\?\\")) p = p.slice(4);
  if (p.startsWith("UNC\\")) p = "\\\\" + p.slice(4);
  while (p.length > 1 && p.endsWith("\\") && !/^[A-Za-z]:\\$/.test(p)) p = p.slice(0, -1);
  return p.replace(/[A-Z]/g, (ch) => ch.toLowerCase());
}

/** 最近利用の根拠（sortChats と同じ）: 利用時刻、無ければ作成時刻。 */
export const usedTime = (c: Chat): number | null =>
  c.lastUsedAt ?? (c.createdAt.kind === "value" ? c.createdAt.value : null);

/** AgentDock が作った worktree 台帳のうち、cwd がその path と同じか配下のもの。無ければ null（外部 worktree は推定しない）。 */
export function worktreeOf(cwd: string | null, worktrees: WorktreeRecord[]): WorktreeRecord | null {
  if (!cwd) return null;
  const k = folderKey(cwd);
  let best: WorktreeRecord | null = null;
  let bestLen = -1;
  for (const w of worktrees) {
    const wk = folderKey(w.path);
    const hit = k === wk || k.startsWith(wk.endsWith("\\") ? wk : wk + "\\");
    if (hit && wk.length > bestLen) { best = w; bestLen = wk.length; }
  }
  return best;
}

export interface FolderGroup {
  key: string;
  kind: "folder" | "general" | "unknown";
  /** 全文のパス（一般・不明は表示名）。最近利用が最も新しいチャットの元の文字列。 */
  path: string;
  chats: Chat[];
  /** 中のチャットの最近利用の最大値 */
  latest: number | null;
  pinned: boolean;
}

/** グループ化。`chats` は表示する順（sortChats 済み・ピン留めチャットは除いたもの）。グループ内はその順のまま。 */
export function groupChats(chats: Chat[], worktrees: WorktreeRecord[], pinnedFolders: string[]): FolderGroup[] {
  const map = new Map<string, FolderGroup>();
  for (const c of chats) {
    let key: string, kind: FolderGroup["kind"], path: string;
    if (c.kind === "general") { key = GENERAL_KEY; kind = "general"; path = GENERAL_LABEL; }
    else {
      const cwd = c.cwd.kind === "value" && c.cwd.value.trim() !== "" ? c.cwd.value : null;
      if (!cwd) { key = UNKNOWN_KEY; kind = "unknown"; path = UNKNOWN_LABEL; }
      else {
        const wt = worktreeOf(cwd, worktrees);
        path = wt ? wt.repoRoot : cwd;
        key = folderKey(path);
        kind = "folder";
      }
    }
    const t = usedTime(c);
    let g = map.get(key);
    if (!g) { g = { key, kind, path, chats: [], latest: null, pinned: pinnedFolders.includes(key) && kind !== "unknown" }; map.set(key, g); }
    else if (kind === "folder" && t !== null && (g.latest === null || t > g.latest)) g.path = path;
    if (t !== null && (g.latest === null || t > g.latest)) g.latest = t;
    g.chats.push(c);
  }
  const gs = [...map.values()];
  const rank = (g: FolderGroup) => (g.kind === "unknown" ? 2 : g.pinned ? 0 : 1);
  return gs.map((g, i) => ({ g, i })).sort((a, b) => {
    const ra = rank(a.g), rb = rank(b.g);
    if (ra !== rb) return ra - rb;
    if (ra === 0) return pinnedFolders.indexOf(a.g.key) - pinnedFolders.indexOf(b.g.key);
    if (ra === 1) {
      const ta = a.g.latest, tb = b.g.latest;
      const d = ta === null ? (tb === null ? 0 : 1) : tb === null ? -1 : tb - ta;
      if (d) return d;
    }
    return a.i - b.i;
  }).map((x) => x.g);
}

function shortAt(display: string, n: number): string {
  let p = display.replace(/\//g, "\\");
  const ext = p.startsWith("\\\\?\\");
  if (ext) p = p.slice(4);
  const unc = p.startsWith("\\\\");
  const parts = p.split("\\").filter((s) => s !== "");
  const drive = parts.length > 0 && /^[A-Za-z]:$/.test(parts[0]);
  const tail = parts.slice(-n);
  const whole = tail.length === parts.length;
  let s = tail.join("\\");
  if (whole && drive && parts.length === 1) s += "\\";
  if (whole && unc && !drive) s = "\\\\" + s;
  return s || display;
}

/** 見出し用の短い表示（末尾2階層）。同じ短縮名が別グループにあれば、該当するものだけ階層を延ばして区別する。 */
export function shortNames(items: { key: string; path: string }[]): Map<string, string> {
  const depth = new Map(items.map((i) => [i.key, 2]));
  const max = (p: string) => p.replace(/^\\\\\?\\/, "").split(/[\\/]/).filter((s) => s !== "").length;
  for (let guard = 0; guard < 64; guard++) {
    const by = new Map<string, typeof items>();
    for (const it of items) {
      const s = shortAt(it.path, depth.get(it.key)!);
      by.set(s, [...(by.get(s) ?? []), it]);
    }
    let changed = false;
    for (const same of by.values()) {
      if (same.length < 2) continue;
      for (const it of same) if (depth.get(it.key)! < max(it.path)) { depth.set(it.key, depth.get(it.key)! + 1); changed = true; }
    }
    if (!changed) break;
  }
  return new Map(items.map((i) => [i.key, shortAt(i.path, depth.get(i.key)!)]));
}

/** 窓表示・描画用の平らな並び。見出しと行を交互に並べる（折りたたみ中の見出しは行を持たない）。 */
export type FlatItem = { type: "group"; group: FolderGroup } | { type: "chat"; chat: Chat; group: FolderGroup };
export function flattenGroups(groups: FolderGroup[], collapsed: (key: string) => boolean): FlatItem[] {
  const out: FlatItem[] = [];
  for (const g of groups) {
    out.push({ type: "group", group: g });
    if (!collapsed(g.key)) for (const c of g.chats) out.push({ type: "chat", chat: c, group: g });
  }
  return out;
}
