// 組織実行の開始前・Gitの準備で、作業フォルダのGit管理が別のユーザーの所有でGitが拒否したときの案内と「AgentDockのgit操作だけに許可」。
// 規則: 許可は押したときの確認の後だけ（自動で付けない）。あなたのgit設定・.gitの所有者は変えない（コマンド例は自分で実行する）。
//       許可を保存したら `onAllowed` を呼び、呼び出し側が開始前チェックをやり直す。
import { useState } from "react";
import * as host from "../ipc/client";
import { CopyLine } from "./RepoRecordPanel";
import { ownerAllowConfirmText } from "./ownerAllow";
import { cleanRepoPath, ownerCommands, safeDirectoryCommand } from "./revertView";

export function OwnerAllowPanel({ repo, onAllowed }: { repo: string; onAllowed: () => void }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const shown = cleanRepoPath(repo);
  const allow = async () => {
    if (busy || !window.confirm(ownerAllowConfirmText(shown))) return;
    setBusy(true); setErr(null);
    try { await host.setRepoBaseline(repo, "Record", true); onAllowed(); }
    catch (e) { setErr(host.asIpcError(e).message); }
    setBusy(false);
  };
  return (
    <div className="gbanner warn" role="status" style={{ display: "block" }}>
      <p><b>このフォルダのGit管理が別のユーザー（Codexのサンドボックスなど）の所有です。</b>AgentDockのGit操作だけに許可しますか？（あなたのGit設定は変更しません）</p>
      <p className="small muted mono" style={{ overflowWrap: "anywhere" }}>{shown}</p>
      <div className="acts"><button type="button" className="btn-line" disabled={busy} onClick={() => void allow()}>{busy ? "許可しています…" : "AgentDockのGit操作だけに許可…"}</button></div>
      {err ? <div className="why err" role="alert">許可を保存できませんでした（{err}）。</div> : null}
      <details className="about"><summary>自分で対処するには</summary><div>
        <p>.git の所有者を自分に戻す（管理者のコマンドプロンプトで、自分で実行）:</p>
        {ownerCommands(repo).map((c) => <CopyLine key={c} text={c} />)}
        <p>Gitに安全なフォルダとして登録する（あなたのgit設定を変更します。自分で実行）:</p>
        <CopyLine text={safeDirectoryCommand(repo)} />
      </div></details>
    </div>
  );
}
