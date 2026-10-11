// 「変更を戻す」の表示用の純粋ロジック（区分・理由の並び・控えの状況の文）。画面（ChangesDialog.tsx）から使う。
// 規則: 判定そのものはホスト（rules/baseline.rs）が行う。ここは受け取った判定を並べ替えて文にするだけで、判定を足したり変えたりしない。
import type { BaselineFailure, GitInfo, RepoBaselinePref, RevertBlockCode, RevertItem, RevertVerdict, SegmentState, SegmentStatus } from "../ipc/types";

export const BLOCK_LABEL: Record<RevertBlockCode, string> = {
  gitBusy: "Gitの操作の途中",
  notARepository: "Gitリポジトリではない",
  gitUnavailable: "Gitを実行できない",
  noBaseline: "控えがない",
  outsideWorkFolder: "作業フォルダの外",
  notSnapshotted: "控えの対象外",
  headMoved: "HEADが変わった（コミット・ブランチ切替など）",
  alreadyReverted: "戻し済み",
  endUnknown: "終了時の控えがない",
  changedBetween: "turnの間に別の変更",
  changedAfter: "turnの後に別の変更",
  concurrentChange: "同時作業の可能性",
  pathOutside: "作業フォルダの外",
  readFailed: "読み取れない",
};

export type VerdictGroup = "revertible" | "needsOverride" | "blocked";
export const groupOf = (v: RevertVerdict): VerdictGroup => v.kind;

/** 要確認の理由を並べる固定順（ホストが返す順に依らず表示を安定させる）。 */
const OVERRIDE_ORDER: RevertBlockCode[] = ["endUnknown", "changedBetween", "changedAfter", "concurrentChange"];
const orderIndex = (c: RevertBlockCode) => { const i = OVERRIDE_ORDER.indexOf(c); return i < 0 ? OVERRIDE_ORDER.length : i; };

export interface ReasonLine { code: RevertBlockCode; label: string; message: string }

/** 区分が要確認・止めるのとき、すべての理由を（落とさずに）固定順で返す。戻せる場合は空。 */
export function reasonLines(v: RevertVerdict): ReasonLine[] {
  switch (v.kind) {
    case "revertible": return [];
    case "blocked": return [{ code: v.code, label: BLOCK_LABEL[v.code] ?? v.code, message: v.message }];
    case "needsOverride": return [...v.reasons].sort((a, b) => orderIndex(a.code) - orderIndex(b.code))
      .map((r) => ({ code: r.code, label: BLOCK_LABEL[r.code] ?? r.code, message: r.message }));
  }
}

/** 一覧の並び: 戻せる → 要確認 → 止める（同区分内は元の順）。 */
export function sortedItems(items: RevertItem[]): RevertItem[] {
  const rank: Record<VerdictGroup, number> = { revertible: 0, needsOverride: 1, blocked: 2 };
  return items.map((it, i) => [it, i] as const).sort((a, b) => rank[groupOf(a[0].verdict)] - rank[groupOf(b[0].verdict)] || a[1] - b[1]).map(([it]) => it);
}

/** 計画を受け取った直後の選択。戻せるものだけオン。要確認（強制）は必ずオフ、止めるは選べない。 */
export function initialChosen(items: RevertItem[]): Record<string, boolean> {
  return Object.fromEntries(items.filter((i) => i.verdict.kind === "revertible").map((i) => [i.path, true]));
}

/** 通常の戻し対象（戻せる区分でチェック済み）。 */
export const pickedPaths = (items: RevertItem[], chosen: Record<string, boolean>): string[] =>
  items.filter((i) => i.verdict.kind === "revertible" && chosen[i.path]).map((i) => i.path);

/** 強制の対象（要確認の区分でチェック済み）。チェックは利用者が1件ずつ入れたものだけ。 */
export const forcedPaths = (items: RevertItem[], chosen: Record<string, boolean>): string[] =>
  items.filter((i) => i.verdict.kind === "needsOverride" && chosen[i.path]).map((i) => i.path);

/** 結果の見出し区分。失敗が1件でもあれば「一部のみ」（完了とは言わない）。 */
export type ResultKind = "partial" | "allReverted" | "nothing";
export function resultKind(r: { reverted: string[]; failed: unknown[] }): ResultKind {
  if (r.failed.length > 0) return "partial";
  return r.reverted.length > 0 ? "allReverted" : "nothing";
}

export function failureText(f: BaselineFailure): string {
  switch (f.kind) {
    case "notARepository": return "作業フォルダがGitリポジトリではありません";
    case "gitUnavailable": return "Gitを実行できません";
    case "gitTooOld": return `Gitのバージョンが古すぎます（${f.found}）`;
    case "workFolderUnknown": return "作業フォルダを確認できません";
    case "timeout": return "取得に時間がかかりすぎました";
    case "insufficientSpace": return "空き容量が足りません";
    case "gitBusy": return "Gitの操作の途中でした";
    case "disabled": return "設定で控えの記録がオフです";
    case "git": return `Gitがエラーを返しました: ${f.message}`;
    case "dubiousOwnership": return "このフォルダのGit管理が別のユーザー（Codexのサンドボックスなど）の所有になっているため、Gitが拒否しました";
    case "skipped": return "このリポジトリの変更は記録しない設定です";
  }
}

/** 所有者違い（dubious ownership）の失敗なら、そのリポジトリの場所。保存済みの古い形（`git` のメッセージ）も拾う（場所が分からなければ空文字）。 */
export function ownerProblemOf(f: BaselineFailure): string | null {
  if (f.kind === "dubiousOwnership") return f.repo;
  if (f.kind === "git" && /dubious ownership/i.test(f.message)) return /repository at '([^']*)'/.exec(f.message)?.[1] ?? "";
  return null;
}

/** いま所有者違いで控えを取れていないリポジトリ（直近の区間が所有者違いで失敗しているときだけ。後の送信で取れていれば null）。 */
export function currentOwnerProblem(status: SegmentStatus[] | null): string | null {
  const last = status && status.length > 0 ? status[status.length - 1] : null;
  return last && last.state.kind === "failed" ? ownerProblemOf(last.state.reason) : null;
}

export function stateText(s: SegmentState): string {
  switch (s.kind) {
    case "ready": return "";
    case "running": return "作業中（途中の状態）";
    case "endUnknown": return "終了時の控えなし";
    case "failed": return ownerProblemOf(s.reason) !== null ? "戻せない（控えなし：所有者の問題）" : `控えを取れませんでした: ${failureText(s.reason)}`;
    case "abandoned": return "送信されなかったため控えなし";
  }
}

/** turn ID は先頭8文字＋「…」（全文は title・コピーで見せる）。 */
export const shortId = (id: string): string => (id.length > 8 ? `${id.slice(0, 8)}…` : id);

export interface TurnOption { turn: string; label: string }

/** 「戻す範囲」「差分の対象」の選択肢。turn に対応づいた区間だけ（受理不明の区間は対応づくまで出さない）。控えを取れなかった・送信されなかった区間は選べない。 */
export function turnOptions(status: SegmentStatus[]): TurnOption[] {
  const out: TurnOption[] = [];
  for (const s of status) {
    if (!s.turn || s.state.kind === "failed" || s.state.kind === "abandoned") continue;
    if (out.some((o) => o.turn === s.turn)) continue;
    const st = stateText(s.state);
    out.push({ turn: s.turn, label: `turn ${shortId(s.turn)}${st ? `（${st}）` : ""}${s.concurrent ? "［同時作業あり］" : ""}` });
  }
  return out;
}

/** 作業フォルダのGit。`git_info` で確認できたときだけ repo／notRepo、取得前・失敗は unknown（無効にしない）。 */
export type GitKind = "repo" | "notRepo" | "unknown";
/** ホスト（git_info）は NotSupported を「フォルダなし」「Gitを実行できない」「リポジトリではない」に使う。コードはなくメッセージだけなので、リポジトリではないと言い切れる文だけを notRepo にする。 */
const NOT_REPO_MESSAGE = /Gitのリポジトリではありません/;
export function gitKindOf(info: Pick<GitInfo, "status"> | null): GitKind {
  if (!info) return "unknown";
  return info.status.kind === "ready" ? "repo" : info.status.kind === "notSupported" && NOT_REPO_MESSAGE.test(info.status.message) ? "notRepo" : "unknown";
}

/** ホストの理由文と控えがない帯の文が同じときだけ重複（1本にする）。原因が違えば両方出す。 */
export const sameReason = (hostMessage: string | null | undefined, reason: string | null): boolean => !!reason && !!hostMessage && hostMessage.trim() === reason.trim();

export interface BaselineContext { baselinesEnabled: boolean; git: GitKind }

/**
 * 控えがない・使えない理由を1つだけ返す。status が null（未取得）のときは null（取得中。理由を作らない）。
 * ホストが返した失敗の理由（直近の控えの失敗）があればそれを優先し、なければ ①設定オフ ②Gitリポジトリではない ③Gitを確認できていない ④控えを取る前のturn の順。
 * ctx を渡さないときは④の一般文のまま。
 */
export function noBaselineReason(status: SegmentStatus[] | null, ctx?: BaselineContext): string | null {
  if (!status) return null;
  const usable = status.some((s) => s.state.kind === "ready" || s.state.kind === "running" || s.state.kind === "endUnknown");
  if (usable) return null;
  if (status.length === 0) {
    if (ctx) {
      if (!ctx.baselinesEnabled) return "設定で控えの記録がオフのため、このチャットには変更の控えがありません。";
      if (ctx.git === "notRepo") return "作業フォルダがGitリポジトリではないため、変更の控えがありません。";
      if (ctx.git === "unknown") return "作業フォルダのGitの状態を確認できていないため、控えがない理由を特定できません。";
      return "このチャットには変更の控えがありません（控えを取る前のturnだけです）。";
    }
    return "このチャットには変更の控えがありません（控えを取る前のturn、Gitリポジトリではない、または設定で控えの記録がオフです）。";
  }
  const failed = [...status].reverse().find((s) => s.state.kind === "failed");
  if (failed && failed.state.kind === "failed" && failed.state.reason.kind === "skipped") return failureText(failed.state.reason);
  if (failed && failed.state.kind === "failed" && ownerProblemOf(failed.state.reason) !== null) return `この発言の前の状態を保存できなかったため、この発言の変更は戻せません（${failureText({ kind: "dubiousOwnership", repo: "" })}）。`;
  if (failed && failed.state.kind === "failed") return `直近の控えを取れませんでした（${failureText(failed.state.reason)}）。戻せる控えがありません。`;
  return "戻せる控えがありません（送信されなかった区間のみ）。";
}

// ───────────────────────────── リポジトリ別の選択（所有者違い） ─────────────────────────────

/** ホスト（rules/baseline.rs `clean_repo_path`）と同じ正規化。`\\?\` を除き `/` 区切り・末尾の区切りなし。比較は小文字で行う。 */
export function cleanRepoPath(p: string): string {
  const s = p.trim().replace(/\\/g, "/").replace(/^\/\/\?\//, "");
  return s.replace(/\/+$/, "") || s;
}
const cmpPath = (p: string) => cleanRepoPath(p).toLowerCase();
const insidePath = (outer: string, inner: string) => inner === outer || inner.startsWith(`${outer}/`);

/** 場所（リポジトリ・作業フォルダ）を含む選択。入れ子なら最も深いもの。 */
export function prefFor(prefs: readonly RepoBaselinePref[] | undefined, ...paths: (string | null | undefined)[]): RepoBaselinePref | undefined {
  let best: RepoBaselinePref | undefined;
  for (const raw of paths) {
    if (!raw) continue;
    const c = cmpPath(raw);
    for (const p of prefs ?? []) {
      const k = cmpPath(p.path);
      if (insidePath(k, c) && (!best || k.length > cmpPath(best.path).length)) best = p;
    }
  }
  return best;
}

/** 所有者違いで控えを取れなかった送信の数（その発言の変更は戻せない）。 */
export const ownerFailedCount = (status: SegmentStatus[] | null): number =>
  status ? status.filter((s) => s.state.kind === "failed" && ownerProblemOf(s.state.reason) !== null).length : 0;

/** Windowsのパス（`\` 区切り）。コマンド例に使う。 */
export const winPath = (p: string): string => cleanRepoPath(p).replace(/\//g, "\\");

/** 所有者を戻すコマンド例（ユーザーが管理者のコマンドプロンプトで自分で実行する。AgentDockは実行しない）。 */
export function ownerCommands(repo: string): string[] {
  const git = `${winPath(repo)}\\.git`;
  return [`takeown /F "${git}" /R /D Y`, `icacls "${git}" /setowner "%USERNAME%" /T`];
}

/** `safe.directory` 登録のコマンド例（ユーザーのgit設定を変えるので、ユーザーが自分で実行する）。 */
export const safeDirectoryCommand = (repo: string): string => `git config --global --add safe.directory ${cleanRepoPath(repo)}`;
