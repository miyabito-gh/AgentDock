// Git worktree（#14）の画面（段階③ P3-7）。外枠（Shell）と下部ボタンは Dialogs.tsx 側。
// 規則: worktreeは「分離」を選んだときだけ作る（初期値は通常。チャット作成のたびには作らない）。削除はAgentDockが作った記録のあるものだけ。
//       未コミット変更があれば止め、「変更を破棄して削除」は2段目の確認。ブランチは残す。結果は応答の観測値だけを書く。
import { useEffect, useState } from "react";
import * as host from "../ipc/client";
import type { Chat, GitInfo, WorktreeEntry, WorktreeRecord, WorktreeRemoveOutcome, WorktreeRemovePreview, WorktreeRemoveVerdict, RemoveBlockReason } from "../ipc/types";
import { chatName } from "./derive";
import { showKnown } from "./format";
import { About } from "./DialogParts";

const errText = (e: unknown) => host.asIpcError(e).message;

const ORIGIN_TEXT = (w: WorktreeEntry): string => (w.origin.kind === "main" ? "メイン" : w.origin.kind === "agentDock" ? "AgentDock作成" : "外部");

const BLOCK_TEXT: Record<RemoveBlockReason, string> = {
  outsideRoot: "記録されたパスがAgentDockの置き場所の外（またはリンク先が外）のため、削除できません。",
  inUse: "このworktreeを使っているチャットに、作業中・停止未確認・送信待ち・受理不明があるため、削除できません。完了または停止の確認後にやり直してください。",
  mainWorktree: "リポジトリ本体の作業ツリーのため、削除できません。",
  notListed: "フォルダは存在しますが、Gitのworktree一覧にありません。別の用途のフォルダかもしれないため、削除しません。",
  unverified: "Gitの状態を確認できないため、削除できません。",
};

const stateText = (r: WorktreeRecord): string => {
  switch (r.state.kind) {
    case "creating": return "作成中、または作成が中断された可能性があります（削除の確認で実在を照合します）";
    case "ready": return "作成済み";
    case "failed": return `作成に失敗: ${r.state.message}`;
  }
};

/** 新しいチャットの作業場所の選択（通常／分離）と、既存のworktree。cwd のGit情報は読取りだけで取得する。 */
export function WorkspaceChoice({ live, cwd, isolate, setIsolate, onUse }: {
  live: boolean; cwd: string; isolate: boolean; setIsolate: (b: boolean) => void;
  /** 既存のworktree（またはメイン）を作業フォルダにする。record はAgentDockが作ったものの記録ID。 */
  onUse: (path: string, record: string | null) => void;
}) {
  const [info, setInfo] = useState<GitInfo | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    setInfo(null); setErr(null);
    const folder = cwd.trim();
    if (!live || !folder) return;
    let stale = false;
    const t = setTimeout(() => {
      host.gitInfo(folder).then((i) => { if (!stale) setInfo(i); }).catch((e) => { if (!stale) setErr(errText(e)); });
    }, 400);
    return () => { stale = true; clearTimeout(t); };
  }, [cwd, live]);
  if (!live || !cwd.trim()) return null;
  if (err) return <div className="field"><span>作業場所</span><div className="small">Gitの状態を取得できませんでした: {err}（通常の作業場所で開始します）</div></div>;
  if (!info) return <div className="field"><span>作業場所</span><div className="small muted">Gitの状態を確認しています…</div></div>;
  if (info.status.kind !== "ready") {
    return (
      <div className="field"><span>作業場所</span>
        <div className="small">{info.status.kind === "notSupported" ? "worktreeは使えません" : "Gitの状態を取得できませんでした"}: {info.status.message}（通常の作業場所で開始します）</div>
      </div>
    );
  }
  const others = info.worktrees.filter((w) => !w.prunable);
  return (
    <div className="field"><span>作業場所</span>
      <div>
        <label><input type="radio" name="iso" checked={!isolate} onChange={() => setIsolate(false)} />通常（このフォルダで作業する）</label><br />
        <label><input type="radio" name="iso" checked={isolate} onChange={() => setIsolate(true)} />分離（worktreeを作成して、そこで作業する）</label>
        {isolate ? (
          <p className="small" style={{ marginTop: 4 }}>
            専用領域に新しいworktreeと新しいブランチ（agentdock/チャット名-短いID）を作ります。基点は現在のコミットで、元の作業ツリーの未コミット変更
            （{showKnown(info.dirtyCount, (n) => `${n}件`)}）は持ち込まれません。作ったworktreeとブランチは自動では削除しません（メニュー「作業 → worktree を管理」で削除できます）。
          </p>
        ) : null}
        {others.length > 1 ? (
          <details style={{ marginTop: 4 }}>
            <summary className="small">このリポジトリの既存のworktree（{others.length}）</summary>
            <ul className="check-list">
              {others.map((w) => (
                <li key={w.path}>
                  <span className="mono">{w.path}</span> <span className="small muted">{ORIGIN_TEXT(w)}・{w.branch ?? (w.detached ? "detached" : "ブランチなし")}</span>{" "}
                  <button className="btn-line" onClick={() => onUse(w.path, w.origin.kind === "agentDock" ? w.origin.record : null)}>このフォルダを使う</button>
                </li>
              ))}
            </ul>
          </details>
        ) : null}
      </div>
    </div>
  );
}

function VerdictView({ pv, discard, setDiscard }: { pv: WorktreeRemovePreview; discard: boolean; setDiscard: (b: boolean) => void }) {
  const v: WorktreeRemoveVerdict = pv.verdict;
  return (
    <>
      {pv.usingChats.length > 0 ? <p className="small">このworktreeを使っているチャット: {pv.usingChats.length} 件（削除してもチャットは消えません）</p> : null}
      {v.kind === "blocked" ? <p className="gbanner err" role="alert">{BLOCK_TEXT[v.reason]}{pv.detail ? `（${pv.detail}）` : ""}</p> : null}
      {v.kind === "alreadyGone" ? <p className="small">Gitの一覧にもフォルダにも見つかりません。台帳の記録だけを外します（ファイルとブランチには触りません）。</p> : null}
      {v.kind === "needsDiscard" ? (
        <div className="gbanner warn" role="status">
          <span className="grow">
            未コミットの変更（未追跡を含む）が {v.dirty} 件あるため、通常の削除は止めています。削除すると、これらの変更は失われます（ブランチ上のコミットは残ります）。
            <br /><label><input type="checkbox" checked={discard} onChange={(e) => setDiscard(e.target.checked)} />変更を破棄してよいことを確認した</label>
          </span>
        </div>
      ) : null}
    </>
  );
}

const outcomeText = (o: WorktreeRemoveOutcome): string => {
  if (o.kind === "recordDropped") return "台帳の記録を外しました（実体はすでにありませんでした）。ブランチは残っています。";
  switch (o.branch.kind) {
    case "kept": return "worktreeを削除しました。ブランチは残っています。";
    case "deleted": return "worktreeを削除し、マージ済みのブランチも削除しました。";
    case "notDeleted": return `worktreeを削除しました。ブランチは削除されず、残っています（${o.branch.message}）。`;
  }
};

/** worktreeの管理。AgentDockが作ったものだけ削除できる。外部のworktreeは一覧に出すだけ。 */
export function WorktreesBody({ live, records, cwd, chats }: { live: boolean; records: WorktreeRecord[]; cwd: string | null; chats: Chat[] }) {
  const [target, setTarget] = useState<string | null>(null);
  const [pv, setPv] = useState<WorktreeRemovePreview | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [discard, setDiscard] = useState(false);
  const [deleteBranch, setDeleteBranch] = useState(false);
  const [outcome, setOutcome] = useState<WorktreeRemoveOutcome | null>(null);
  const [repo, setRepo] = useState<GitInfo | null>(null);
  const [repoErr, setRepoErr] = useState<string | null>(null);
  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;

  const open = async (id: string) => {
    setTarget(id); setPv(null); setErr(null); setOutcome(null); setDiscard(false); setDeleteBranch(false); setBusy(true);
    try { setPv(await host.previewRemoveWorktree(id)); } catch (e) { setErr(errText(e)); } finally { setBusy(false); }
  };
  const run = async (force: boolean) => {
    if (!target) return;
    setBusy(true); setErr(null);
    try { setOutcome(await host.removeWorktree(target, force, deleteBranch)); setPv(null); } catch (e) { setErr(errText(e)); } finally { setBusy(false); }
  };
  const loadRepo = async () => {
    if (!cwd) return;
    setRepo(null); setRepoErr(null);
    try { setRepo(await host.gitInfo(cwd)); } catch (e) { setRepoErr(errText(e)); }
  };
  const v = pv?.verdict;
  const canRun = !!pv && !busy && (v?.kind === "allowed" || v?.kind === "alreadyGone" || (v?.kind === "needsDiscard" && discard));
  const chatOf = (r: WorktreeRecord) => (r.createdForChat ? chats.find((c) => c.key.id === r.createdForChat!.id) : undefined);
  return (
    <div className="content">
      <About><p>AgentDockが作ったworktreeです。削除はここに記録があるものだけで、ブランチは残します。チャットを削除してもworktreeは消えません。</p></About>
      {records.length === 0 ? <p>AgentDockが作ったworktreeはありません。</p> : (
        <ul className="check-list">
          {records.map((r) => (
            <li key={r.id}>
              <span className="mono">{r.path}</span><br />
              <span className="small muted">ブランチ {r.branch}・基点 {r.baseCommit.slice(0, 8)}・{stateText(r)}{chatOf(r) ? `・作成したチャット: ${chatName(chatOf(r)!)}` : ""}</span>{" "}
              <button className="btn-line" disabled={busy} onClick={() => void open(r.id)}>削除…</button>
            </li>
          ))}
        </ul>
      )}
      {target ? (
        <div className="confirm" role="alertdialog" aria-label="worktreeの削除の確認">
          {busy && !pv && !outcome ? <p>削除の可否を確認しています…</p> : null}
          {pv ? (
            <>
              <p><b>削除するworktree:</b> <span className="mono">{pv.record.path}</span></p>
              <VerdictView pv={pv} discard={discard} setDiscard={setDiscard} />
              {v?.kind === "allowed" || v?.kind === "needsDiscard" ? (
                <p><label><input type="checkbox" checked={deleteBranch} onChange={(e) => setDeleteBranch(e.target.checked)} />マージ済みならブランチ（{pv.record.branch}）も削除する（マージされていなければ残ります）</label></p>
              ) : null}
              <div className="acts end">
                <button className="btn" disabled={busy} onClick={() => { setTarget(null); setPv(null); }}>やめる</button>
                {v?.kind === "blocked" ? null : (
                  <button className={v?.kind === "needsDiscard" ? "btn danger" : "btn primary"} disabled={!canRun} onClick={() => void run(v?.kind === "needsDiscard")}>
                    {busy ? "実行しています…" : v?.kind === "needsDiscard" ? "変更を破棄して削除" : v?.kind === "alreadyGone" ? "記録だけを外す" : "削除"}
                  </button>
                )}
              </div>
            </>
          ) : null}
          {outcome ? <p role="status">{outcomeText(outcome)}</p> : null}
          {err ? <p style={{ color: "var(--fail)" }} role="alert">{err}</p> : null}
          {outcome ? <div className="acts end"><button className="btn" onClick={() => { setTarget(null); setOutcome(null); }}>閉じる</button></div> : null}
        </div>
      ) : null}
      <hr />
      <p><b>選択中のチャットのリポジトリ</b></p>
      {cwd ? <div className="acts"><button className="btn-line" onClick={() => void loadRepo()}>worktreeの一覧を取得（読取りのみ）</button></div> : <p className="small muted">チャットを選ぶと、そのリポジトリのworktreeを一覧できます。</p>}
      {repoErr ? <p style={{ color: "var(--fail)" }}>{repoErr}</p> : null}
      {repo ? (
        repo.status.kind !== "ready" ? <p className="small">{repo.status.kind === "notSupported" ? "非対応" : "取得できませんでした"}: {repo.status.message}</p> : (
          <ul className="check-list">
            {repo.worktrees.map((w) => (
              <li key={w.path}>
                <span className="mono">{w.path}</span> <span className="small muted">{ORIGIN_TEXT(w)}・{w.branch ?? (w.detached ? "detached" : "ブランチなし")}{w.locked ? "・ロック中" : ""}{w.prunable ? "・実体なし" : ""}</span>
                {w.origin.kind === "external" ? <span className="small muted">（AgentDockが作ったものではないため、ここでは削除できません）</span> : null}
              </li>
            ))}
          </ul>
        )
      ) : null}
    </div>
  );
}
