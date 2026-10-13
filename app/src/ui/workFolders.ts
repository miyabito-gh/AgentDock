// 作業フォルダ（複数）の画面ロジック（純粋。DESIGN_WORKFOLDERS.md）。React・IPCには依存しない。
// 実行: node --test src/ui/workFolders.test.ts （tscの対象外）
import type { FolderCheck, FolderReject, StrongReason, Support, WorkFolders, WorkFoldersView } from "../ipc/types";

/** プライマリを含めた上限件数（ホストの `MAX_FOLDERS` と同じ）。 */
export const MAX_FOLDERS = 8;

/** 並べているフォルダ1件。`strong` は強い確認が要る理由、`confirmed` はユーザーが確認したか。 */
export interface FolderItem {
  path: string; strong: StrongReason[]; confirmed: boolean;
  /** 保存済みのセットなどを復元して検査したとき、いまは使えない理由（外すか置き換えるまで保存・作成できない）。 */
  rejected?: string;
}

/** 比較用の鍵（区切り統一・小文字・末尾の区切りなし）。重複判定の目安で、最終判定はホスト。 */
export const folderKey = (p: string): string => p.trim().replace(/^\\\\\?\\/, "").replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();

export const sameFolders = (a: string[], b: string[]): boolean => a.length === b.length && a.every((x, i) => folderKey(x) === folderKey(b[i]));

export const toItems = (paths: string[]): FolderItem[] => paths.map((path) => ({ path, strong: [], confirmed: false }));

/** 位置 i を dir（-1＝上、+1＝下）へ動かす。範囲外は動かさない（新しい配列を返す）。 */
export function moveItem<T>(items: T[], i: number, dir: -1 | 1): T[] {
  const j = i + dir;
  if (i < 0 || i >= items.length || j < 0 || j >= items.length) return items;
  const out = items.slice();
  [out[i], out[j]] = [out[j], out[i]];
  return out;
}

/** 位置 i をプライマリ（先頭）にする。 */
export function makePrimary<T>(items: T[], i: number): T[] {
  if (i <= 0 || i >= items.length) return items;
  return [items[i], ...items.slice(0, i), ...items.slice(i + 1)];
}

export const REJECT_TEXT: Record<FolderReject, string> = {
  unc: "ネットワーク共有（UNC）のフォルダは追加できません",
  networkDrive: "ネットワークドライブのフォルダは追加できません",
  notAbsolute: "絶対パス（C:\\… の形）で指定してください",
  notFound: "フォルダが見つかりません",
  notDirectory: "フォルダではありません（ファイルです）",
  duplicate: "すでに並んでいるフォルダです",
  systemFolder: "Windows・Program Files 以下のフォルダは追加できません",
  appDataOfAgentDock: "AgentDockのデータ領域は追加できません",
  codexHome: "~/.codex 以下のフォルダは追加できません",
  tooMany: `作業フォルダは${MAX_FOLDERS}件（プライマリ含む）までです`,
};

export const STRONG_TEXT: Record<StrongReason, string> = {
  driveRoot: "ドライブ全体です",
  homeRoot: "ユーザーフォルダ全体です",
  appDataRoot: "AppData の直下です",
  programData: "ProgramData 以下です",
  containsOtherFolder: "並んでいる他のフォルダを含みます（範囲が実質広がります）",
};

export const strongText = (reasons: StrongReason[]): string => reasons.map((r) => STRONG_TEXT[r]).join("／");

/** 追加・並べ替えができない理由（できるときは null）。experimental が無い接続では複数フォルダは使えない。 */
export function addBlockReason(support: Support): string | null {
  switch (support) {
    case "supported": return null;
    case "experimental": return "追加の作業フォルダには experimental API が必要です（設定で有効にして再接続すると使えます）";
    case "unsupported": return "この Codex 接続では、追加の作業フォルダを使えません";
    default: return "接続を確認できていないため、追加の作業フォルダは使えません";
  }
}

/** 検査結果を並びへ足す。拒否は理由つきで足さない。強い確認は未確認のまま足す。 */
export function applyCheck(items: FolderItem[], c: FolderCheck, reserved = 0): { items: FolderItem[]; error: string | null } {
  if (c.kind === "rejected") return { items, error: REJECT_TEXT[c.reason] };
  if (items.length + reserved >= MAX_FOLDERS) return { items, error: REJECT_TEXT.tooMany };
  const item: FolderItem = c.kind === "ok" ? { path: c.normalized, strong: [], confirmed: false } : { path: c.normalized, strong: c.reasons, confirmed: false };
  return { items: [...items, item], error: null };
}

/** 確認が済んだ強い確認の対象のパス（ホストへ `confirmedStrong` として渡す）。 */
export const confirmedStrongPaths = (items: FolderItem[]): string[] => items.filter((i) => i.strong.length > 0 && i.confirmed).map((i) => i.path);

/** 強い確認が済んでいない件。 */
export const unconfirmedStrong = (items: FolderItem[]): FolderItem[] => items.filter((i) => i.strong.length > 0 && !i.confirmed);

/** 追加・プライマリ変更があるか（確認ダイアログが要る。削除・並べ替えだけなら不要）。 */
export function needsAddConfirm(before: string[], after: string[]): boolean {
  if (after.length === 0) return false;
  const had = new Set(before.map(folderKey));
  return after.some((p) => !had.has(folderKey(p))) || before.length === 0 || folderKey(before[0]) !== folderKey(after[0]);
}

/** 保存できない理由（できるときは null）。 */
export function saveBlockReason(items: FolderItem[], support: Support): string | null {
  if (items.length === 0) return "作業フォルダ（プライマリ）が必要です";
  if (items.length > MAX_FOLDERS) return REJECT_TEXT.tooMany;
  const bad = items.find((i) => i.rejected);
  if (bad) return `使えないフォルダがあります（外すか置き換えてください）: ${bad.path}（${bad.rejected}）`;
  if (items.length > 1 && support !== "supported") return addBlockReason(support);
  const u = unconfirmedStrong(items);
  if (u.length) return `確認が必要なフォルダがあります: ${u[0].path}`;
  return null;
}

/** 確認表示の1行。`how` は根拠の文言（固定）。 */
export interface ViewRow { key: "current" | "accepted" | "unconfirmed" | "next"; title: string; how: string; folders: string[] | null; note?: string }

const timeText = (ms: number): string => {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}`;
};

/** 確認表示の行を組み立てる。送信記録は直接取得と区別し、取得できないものは「未確認」のまま出す（記録を現在値とは言わない）。 */
export function viewRows(v: WorkFoldersView, fetchedAtMs: number | null): ViewRow[] {
  const rows: ViewRow[] = [];
  const c = v.current;
  if (c.kind === "value") rows.push({ key: "current", title: "現在", how: `直接取得${fetchedAtMs ? ` ${timeText(fetchedAtMs)}` : ""}`, folders: c.value.folders });
  else if (c.kind === "unsupported") rows.push({ key: "current", title: "現在", how: "非対応（この接続では複数の作業フォルダを確認できません）", folders: null });
  else if (c.kind === "missing") rows.push({ key: "current", title: "現在", how: "未確認（取得できましたが、作業フォルダを読み取れませんでした）", folders: null });
  else rows.push({ key: "current", title: "現在", how: "未確認（このチャットは読み込まれていません）", folders: null });
  if (v.lastAccepted) {
    const same = c.kind === "value" && sameFolders(c.value.folders, v.lastAccepted.folders.folders);
    rows.push({
      key: "accepted", title: "AgentDockの送信記録", how: `受理済み ${timeText(v.lastAccepted.acceptedAt)}`, folders: v.lastAccepted.folders.folders,
      note: same ? "直接取得した値と一致しています（反映を確認）" : "AgentDockが送って受理された指定です。バックエンドの現在値とは断定できません",
    });
  }
  if (v.unconfirmed) rows.push({ key: "unconfirmed", title: "受理不明の送信の指定", how: "未確認（受理を確認できていません）", folders: v.unconfirmed.folders, note: "履歴との照合で確定するまで、送信記録にはしません" });
  if (v.next) rows.push({ key: "next", title: "次の送信から（未送信）", how: "まだ適用していません", folders: v.next.folders });
  return rows;
}

/** 一致の表示文。推定の印を消さない。 */
export const projectMatchText = (m: NonNullable<WorkFoldersView["projectMatch"]>): string =>
  `プロジェクト「${m.projectName}」${m.inferred ? "(推定)" : ""}: 一致 ${m.matched}/${m.total}${m.missing.length ? `（不足: ${m.missing.join(", ")}）` : ""}`;

/** プロジェクトのフォルダを新しいチャットの既定で並べる（先頭＝プライマリ、上限まで）。超えた分は案内用に数を返す。 */
export function projectDefaults(choices: string[]): { primary: string; extras: string[]; omitted: number } {
  const seen = new Set<string>();
  const uniq = choices.filter((c) => { const k = folderKey(c); if (!c.trim() || seen.has(k)) return false; seen.add(k); return true; });
  const kept = uniq.slice(0, MAX_FOLDERS);
  return { primary: kept[0] ?? "", extras: kept.slice(1), omitted: uniq.length - kept.length };
}

/** 次の送信から使うことを、非対応の接続で保存済みの追加分が送られない旨で案内する（黙って捨てない）。 */
export function droppedExtrasNote(next: WorkFolders | null, support: Support): string | null {
  if (!next || next.folders.length < 2 || support === "supported") return null;
  return "非対応のため追加フォルダは送られません。プライマリだけにしてから送信してください";
}
