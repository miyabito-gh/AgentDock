// 所有者違い（dubious ownership）のリポジトリの案内と、リポジトリ別の「変更を記録するか」の選択（変更ダイアログ内）。
// 規則: トーストにしない。最初は「記録するか」だけ聞く。記録しないを選んだら以後Gitを呼ばず、ここに中立の一行だけ出す。
//       AgentDockはユーザーのgit設定・.gitの所有者を変えない（コマンド例はユーザーが自分で実行する）。許可は確認の後だけ。
import { useState, type ReactNode } from "react";
import type { Chat, RepoBaselinePref, RepoRecordChoice, SegmentStatus } from "../ipc/types";
import { useCopy } from "./markdown/parts";
import { askConfirm } from "./appConfirm";
import { ownerAllowSpec, RECORD_CHOICE_NOTE, RECORD_SKIP_NOTE } from "./ownerAllow";
import { cleanRepoPath, currentOwnerProblem, ownerCardProblem, ownerCommands, ownerFailedCount, prefFor, safeDirectoryCommand } from "./revertView";

export type SetRepoFn = (repo: string, choice: RepoRecordChoice | null, allowGit: boolean) => Promise<void>;

export function CopyLine({ text }: { text: string }) {
  const { state, copy } = useCopy();
  return (
    <div className="acts" style={{ alignItems: "center" }}>
      <code className="mono" style={{ overflowWrap: "anywhere" }}>{text}</code>
      <button className="btn-line" onClick={() => copy(text)}>{state === "ok" ? "コピーしました" : state === "ng" ? "コピーできません" : "コピー"}</button>
    </div>
  );
}

/** 案内①〜④。compact のときだけ「対処のしかた」に畳む。 */
function Guide({ compact, children }: { compact?: boolean; children: ReactNode }) {
  return compact ? <details className="about"><summary>対処のしかた</summary><div>{children}</div></details> : <>{children}</>;
}

/** 許可の確認（行き先のパスを表示。文面は組織実行の開始前と共通）。 */
const confirmAllow = (repo: string): Promise<boolean> => askConfirm(ownerAllowSpec(cleanRepoPath(repo)));

/** `ownerRepo`: 作業フォルダのリポジトリが、いま所有者違いでGitに拒否されているときの場所（`git_info` の所有者違い。控えの失敗記録がなくても案内を出す）。 */
/** `compact`: 変更を確認の状態スロット用。見出し1行＋ボタンにし、案内①〜④は「対処のしかた」に畳む（文は同じ）。 */
export function RepoRecordPanel({ chat, segs, prefs, onSet, ownerRepo, compact }: { chat: Chat | undefined; segs: SegmentStatus[] | null; prefs: RepoBaselinePref[] | undefined; onSet: SetRepoFn | undefined; ownerRepo?: string | null; compact?: boolean }) {
  const [busy, setBusy] = useState(false);
  const [guide, setGuide] = useState(false);
  const cwd = chat?.cwd.kind === "value" ? chat.cwd.value : null;
  const segProblem = currentOwnerProblem(segs);
  const problem = ownerCardProblem(segs, ownerRepo);
  const pref = prefFor(prefs, problem, cwd);
  const repo = problem || pref?.path || cwd;
  const failedCount = ownerFailedCount(segs);
  if (!repo || (!problem && !pref)) return null;
  const set = async (choice: RepoRecordChoice | null, allow: boolean, then?: () => void) => {
    if (!onSet) return;
    setBusy(true);
    try { await onSet(repo, choice, allow); then?.(); } finally { setBusy(false); }
  };

  /** 確認の後だけ、AgentDockのgit操作に許可して記録する。 */
  const allowAndRecord = async () => { if (await confirmAllow(repo)) await set("Record", true); };

  if (pref?.choice === "Skip") {
    return (
      <div className="small" role="status">
        <p className="muted">このリポジトリの変更は記録しない設定です（{cleanRepoPath(pref.path)}）。</p>
        <div className="acts"><button className="btn-line" disabled={busy || !onSet} onClick={() => void set("Record", false, () => setGuide(true))}>このリポジトリの記録を有効にする</button></div>
      </div>
    );
  }
  if (problem && !pref) {
    return (
      <div className="gbanner warn" role="status" style={{ display: "block" }}>
        <p><b>このリポジトリ（{cleanRepoPath(repo)}）のGit管理が、別のユーザー（Codexのサンドボックスなど）の所有になっています。</b></p>
        <p>そのため、差分の確認と変更の記録ができません。記録しますか？</p>
        <p className="small muted">{RECORD_CHOICE_NOTE}</p>
        <p className="small muted">{RECORD_SKIP_NOTE}</p>
        <div className="acts">
          <button className="btn-line" disabled={busy || !onSet} onClick={() => void allowAndRecord()}>AgentDockのgit操作だけに許可して記録する…</button>
          <button className="btn-line" disabled={busy || !onSet} title="所有者が違うままでは記録できません。対処の案内を表示します" onClick={() => void set("Record", false, () => setGuide(true))}>記録する（対処の案内へ）</button>
          <button className="btn-line" disabled={busy || !onSet} onClick={() => void set("Skip", false)}>記録しない</button>
        </div>
      </div>
    );
  }
  const allowed = !!pref?.allowGit;
  return (
    <div className="small" role="status">
      {problem || guide ? (
        <div className="gbanner warn" style={{ display: "block" }}>
          <p><b>{segProblem ? "この発言の前の状態を保存できなかったため、この発言の変更は戻せません。" : problem ? "このフォルダのGit管理が別のユーザーの所有になっているため、差分の確認や変更の記録ができません。" : "このリポジトリの変更を記録します。"}</b>{problem ? `原因: このフォルダのGit管理（.git）が別のユーザー（Codexのサンドボックス）の所有になっていて、Gitが安全のため拒否しています。AgentDockは設定を勝手に変えません。次のどれかを選んでください。` : "所有者の問題があれば、ここに案内を出します。"}</p>
          {problem ? (
            <Guide compact={compact}>
              <p>① 先に自分で <code className="mono">git init</code> する（Codexに作らせない）。今後のリポジトリはこれで防げます。</p>
              <p>② .git の所有者を自分に戻す（管理者のコマンドプロンプトで、自分で実行）:</p>
              {ownerCommands(repo).map((c) => <CopyLine key={c} text={c} />)}
              <p>③ Gitに安全なフォルダとして登録する（あなたのgit設定を変更します。自分で実行）:</p>
              <CopyLine text={safeDirectoryCommand(repo)} />
              <p>④ またはAgentDockが呼ぶgitにだけ許可する（あなたのgit設定は変えません）:</p>
              <div className="acts"><button className="btn-line" disabled={busy || !onSet || allowed} onClick={() => void allowAndRecord()}>{allowed ? "許可済み" : "AgentDockのgit操作だけに許可…"}</button></div>
            </Guide>
          ) : null}
        </div>
      ) : null}
      {allowed ? (
        <div className="acts" style={{ alignItems: "center" }}>
          <span className="muted">このリポジトリ（{cleanRepoPath(repo)}）は、AgentDockのgit操作だけに許可しています。</span>
          <button className="btn-line" disabled={busy || !onSet} onClick={() => void set("Record", false)}>許可を解除</button>
        </div>
      ) : null}
      {failedCount > 1 ? <p className="muted">所有者の問題で記録できなかった送信: {failedCount} 回（その発言の変更は戻せません）。</p> : null}
    </div>
  );
}
