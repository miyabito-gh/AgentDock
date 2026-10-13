// 作業フォルダのセット（名前つきの組。先頭＝プライマリ、順序つき）の純粋ロジック。React・IPCには依存しない。
// セットの中身は検査済みの保証ではない。使うたびに check_work_folder を通し、使えないものは理由つきで残す（黙って外さない）。
// 実行: node --test src/ui/workFolderSets.test.ts （tscの対象外）
import type { FolderCheck, WorkFolderSet, WorkFoldersView } from "../ipc/types";
import { folderKey, MAX_FOLDERS, moveItem, REJECT_TEXT, type FolderItem } from "./workFolders.ts";

/** セットの上限件数・名前の上限文字数（ホストの `MAX_FOLDER_SETS`・`MAX_SET_NAME_CHARS` と同じ）。 */
export const MAX_SETS = 50;
export const MAX_SET_NAME = 60;

const nameKey = (n: string): string => n.trim().toLowerCase();

export function nameProblem(name: string): string | null {
  const n = name.trim();
  if (!n) return "名前を入力してください";
  if ([...n].length > MAX_SET_NAME) return `名前は${MAX_SET_NAME}文字までです`;
  return null;
}

/** 保存できないフォルダの並び（できるときは null）。 */
export function folderListProblem(folders: string[]): string | null {
  if (folders.length === 0) return "作業フォルダ（プライマリ）が必要です";
  if (folders.length > MAX_FOLDERS) return REJECT_TEXT.tooMany;
  if (folders.some((f) => !f.trim())) return "空のパスがあります";
  const seen = new Set<string>();
  for (const f of folders) {
    const k = folderKey(f);
    if (seen.has(k)) return `${REJECT_TEXT.duplicate}: ${f}`;
    seen.add(k);
  }
  return null;
}

export type SetEdit =
  | { kind: "ok"; sets: WorkFolderSet[]; saved: WorkFolderSet }
  /** 同名のセットがある。上書きするかはユーザーが決める。 */
  | { kind: "needsOverwrite"; existing: WorkFolderSet }
  | { kind: "error"; message: string };

/** 並びをセットとして保存する。同名（大文字小文字を区別しない）は `overwrite` のときだけ置き換える（位置とIDは保つ）。 */
export function saveSet(sets: WorkFolderSet[], name: string, folders: string[], o: { overwrite: boolean; now: number; newId: () => string }): SetEdit {
  const np = nameProblem(name);
  if (np) return { kind: "error", message: np };
  const fp = folderListProblem(folders);
  if (fp) return { kind: "error", message: fp };
  const n = name.trim();
  const list = folders.map((f) => f.trim());
  const existing = sets.find((s) => nameKey(s.name) === nameKey(n));
  if (existing) {
    if (!o.overwrite) return { kind: "needsOverwrite", existing };
    const saved: WorkFolderSet = { ...existing, name: n, folders: list, updatedAt: o.now };
    return { kind: "ok", sets: sets.map((s) => (s.id === existing.id ? saved : s)), saved };
  }
  if (sets.length >= MAX_SETS) return { kind: "error", message: `作業フォルダのセットは${MAX_SETS}件までです` };
  const saved: WorkFolderSet = { id: o.newId(), name: n, folders: list, updatedAt: o.now };
  return { kind: "ok", sets: [...sets, saved], saved };
}

/** 名前の変更。他のセットと同名（大文字小文字を区別しない）にはできない。 */
export function renameSet(sets: WorkFolderSet[], id: string, name: string, now: number): SetEdit {
  const np = nameProblem(name);
  if (np) return { kind: "error", message: np };
  const cur = sets.find((s) => s.id === id);
  if (!cur) return { kind: "error", message: "セットが見つかりません" };
  const n = name.trim();
  if (sets.some((s) => s.id !== id && nameKey(s.name) === nameKey(n))) return { kind: "error", message: `同じ名前のセットがあります: ${n}` };
  const saved: WorkFolderSet = { ...cur, name: n, updatedAt: now };
  return { kind: "ok", sets: sets.map((s) => (s.id === id ? saved : s)), saved };
}

export function moveSet(sets: WorkFolderSet[], id: string, dir: -1 | 1): WorkFolderSet[] {
  return moveItem(sets, sets.findIndex((s) => s.id === id), dir);
}

export const removeSet = (sets: WorkFolderSet[], id: string): WorkFolderSet[] => sets.filter((s) => s.id !== id);

/** 1件の検査結果（null＝検査できなかった）から並びの1行にする。使えないものは理由つきで残す。 */
export function itemFromCheck(raw: string, c: FolderCheck | null): FolderItem {
  if (c === null) return { path: raw, strong: [], confirmed: false, rejected: "検査できませんでした" };
  if (c.kind === "rejected") return { path: raw, strong: [], confirmed: false, rejected: REJECT_TEXT[c.reason] };
  if (c.kind === "ok") return { path: c.normalized, strong: [], confirmed: false };
  return { path: c.normalized, strong: c.reasons, confirmed: false };
}

/** 並び全体を検査して行にする（各件は自分以外の全件に対して検査する。ホストの verify_folder_set と同じ）。強い確認は未確認のまま。 */
export async function inspectFolders(folders: string[], check: (path: string, current: string[]) => Promise<FolderCheck>): Promise<FolderItem[]> {
  return Promise.all(folders.map(async (f, i) => {
    let c: FolderCheck | null;
    try { c = await check(f, folders.filter((_, j) => j !== i)); } catch { c = null; }
    return itemFromCheck(f, c);
  }));
}

/** 検査した行を、新しいチャットの「プライマリ＋追加分」へ分ける。 */
export function splitForNewChat(items: FolderItem[]): { primary: FolderItem | null; extras: FolderItem[] } {
  return { primary: items[0] ?? null, extras: items.slice(1) };
}

/** 検査結果の行へ別のフォルダを置き換える。拒否は理由を返して置き換えない。 */
export function replaceItemAt(items: FolderItem[], i: number, c: FolderCheck): { items: FolderItem[]; error: string | null } {
  if (i < 0 || i >= items.length) return { items, error: null };
  if (c.kind === "rejected") return { items, error: REJECT_TEXT[c.reason] };
  const next = itemFromCheck(c.normalized, c);
  return { items: items.map((x, j) => (j === i ? next : x)), error: null };
}

export interface SameFolderInit { folders: string[]; /** 追加フォルダを取得できず、プライマリだけで始める。 */ extrasUnconfirmed: boolean }

/** 「同じ作業フォルダで新しいチャット」の初期値。優先順は 次の送信から使う値 → 直接取得 → 送信記録（受理済み）（ホストの pick_fork_folders と同じ）。
 *  どれも取れなければ cwd のみで「追加フォルダは未確認」。cwd も無ければ null。 */
export function sameFolderInit(view: WorkFoldersView | null, cwd: string | null): SameFolderInit | null {
  const fromView = view?.next?.folders ?? (view?.current.kind === "value" ? view.current.value.folders : null) ?? view?.lastAccepted?.folders.folders ?? null;
  const list = fromView?.filter((f) => f.trim() !== "") ?? [];
  if (list.length > 0) return { folders: list.slice(0, MAX_FOLDERS), extrasUnconfirmed: false };
  const c = cwd?.trim();
  return c ? { folders: [c], extrasUnconfirmed: true } : null;
}

export const SAME_FOLDER_UNCONFIRMED_NOTE = "追加フォルダは未確認です（このチャットの作業フォルダ一式を取得できませんでした）";
