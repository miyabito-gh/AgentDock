// 所有者違い（dubious ownership）のリポジトリの案内と、リポジトリ別の「変更を記録するか」の選択（変更ダイアログ内）。
// 規則: トーストにしない。最初は「記録するか」だけ聞く。記録しないを選んだら以後Gitを呼ばず、ここに中立の一行だけ出す。
//       AgentDockはユーザーのgit設定・.gitの所有者を変えない（コマンド例はユーザーが自分で実行する）。許可は確認の後だけ。
import { useState } from "react";
import type { Chat, RepoBaselinePref, RepoRecordChoice, SegmentStatus } from "../ipc/types";
import { useCopy } from "./markdown/parts";
import { cleanRepoPath, currentOwnerProblem, ownerCommands, ownerFailedCount, prefFor, safeDirectoryCommand } from "./revertView";

export type SetRepoFn = (repo: string, choice: RepoRecordChoice | null, allowGit: boolean) => Promise<void>;

function CopyLine({ text }: { text: string }) {
  const { state, copy } = useCopy();
  return (
    <div className="acts" style={{ alignItems: "center" }}>
      <code className="mono" style={{ overflowWrap: "anywhere" }}>{text}</code>
      <button className="btn-line" onClick={() => copy(text)}>{state === "ok" ? "コピーしました" : state === "ng" ? "コピーできません" : "コピー"}</button>
    </div>
  );
}

/** 許可の確認（行き先のパスを表示）。 */
const confirmAllow = (repo: string): boolean =>
  window.confirm(`次のリポジトリを、AgentDockのgit操作だけに許可します。\n\n${cleanRepoPath(repo)}\n\nAgentDockが呼ぶgitにだけ safe.directory として渡します。あなたのgit設定（.gitconfig）は変更しません。このフォルダのGit管理は別のユーザーの所有です。内容を信頼できる場合だけ許可してください。許可はいつでも解除できます。`);

export function RepoRecordPanel({ chat, segs, prefs, onSet }: { chat: Chat | undefined; segs: SegmentStatus[] | null; prefs: RepoBaselinePref[] | undefined; onSet: SetRepoFn | undefined }) {
  const [busy, setBusy] = useState(false);
  const [guide, setGuide] = useState(false);
  const cwd = chat?.cwd.kind === "value" ? chat.cwd.value : null;
  const problem = currentOwnerProblem(segs);
  const pref = prefFor(prefs, problem, cwd);
  const repo = problem || pref?.path || cwd;
  const failedCount = ownerFailedCount(segs);
  if (!repo || (!problem && !pref)) return null;
  const set = async (choice: RepoRecordChoice | null, allow: boolean, then?: () => void) => {
    if (!onSet) return;
    setBusy(true);
    try { await onSet(repo, choice, allow); then?.(); } finally { setBusy(false); }
  };

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
      <div className="gbanner warn" role="status" style={{ flexWrap: "wrap" }}>
        <span className="grow">このリポジトリ（{cleanRepoPath(repo)}）の変更を記録しますか？ Git管理が別のユーザー（Codexのサンドボックスなど）の所有になっているため、いまは記録できません。</span>
        <button className="btn-line" disabled={busy || !onSet} onClick={() => void set("Record", false, () => setGuide(true))}>記録する</button>
        <button className="btn-line" disabled={busy || !onSet} onClick={() => void set("Skip", false)}>記録しない</button>
      </div>
    );
  }
  const allowed = !!pref?.allowGit;
  return (
    <div className="small" role="status">
      {problem || guide ? (
        <div className="gbanner warn" style={{ display: "block" }}>
          <p><b>{problem ? "この発言の前の状態を保存できなかったため、この発言の変更は戻せません。" : "このリポジトリの変更を記録します。"}</b>{problem ? `原因: このフォルダのGit管理（.git）が別のユーザー（Codexのサンドボックス）の所有になっていて、Gitが安全のため拒否しています。AgentDockは設定を勝手に変えません。次のどれかを選んでください。` : "所有者の問題があれば、ここに案内を出します。"}</p>
          {problem ? (
            <>
              <p>① 先に自分で <code className="mono">git init</code> する（Codexに作らせない）。今後のリポジトリはこれで防げます。</p>
              <p>② .git の所有者を自分に戻す（管理者のコマンドプロンプトで、自分で実行）:</p>
              {ownerCommands(repo).map((c) => <CopyLine key={c} text={c} />)}
              <p>③ Gitに安全なフォルダとして登録する（あなたのgit設定を変更します。自分で実行）:</p>
              <CopyLine text={safeDirectoryCommand(repo)} />
              <p>④ またはAgentDockが呼ぶgitにだけ許可する（あなたのgit設定は変えません）:</p>
              <div className="acts"><button className="btn-line" disabled={busy || !onSet || allowed} onClick={() => { if (confirmAllow(repo)) void set("Record", true); }}>{allowed ? "許可済み" : "AgentDockのgit操作だけに許可…"}</button></div>
            </>
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
