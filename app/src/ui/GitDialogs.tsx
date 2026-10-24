// Gitの準備（git init＋最初のコミット）とアプリからのプッシュの画面（DESIGN_ORG_INTAKE §2・§3）。外枠（Shell）と下部ボタンは Dialogs.tsx 側。
// 規則: 開く・プレビューするだけでは何も作らない・送らない。実行は確認ダイアログの主操作を押したときだけ（自動では行わない）。
//       結果が分からないものは「確認できません」と書き、成功にしない。強制プッシュはできない。資格情報は扱わない（Git の設定に任せる）。
//       AgentDock が作るのは `.git` フォルダだけ。除外ファイルへの追記は、選んだときだけ（内容を示し、追記のみ）。
import { useCallback, useEffect, useRef, useState } from "react";
import * as host from "../ipc/client";
import type { DirtySummaryView, PrepOutcome, PushBlocker, PushPreview, PushRejectReason, PushResult, PushVerify, RepoPrepPreview, RepoStatusView } from "../ipc/types";
import { ConfirmBlock, FootActions } from "./DialogParts";
import { fmtBytes } from "./ManageDialogs";
import { OwnerAllowPanel } from "./OwnerAllowPanel";
import { ownerRepoOf } from "./ownerAllow";
import { dirtyCheckApplies, dirtyNoticeText } from "./dirtyNotice";

const errText = (e: unknown) => host.asIpcError(e).message;

/** Gitの準備が要る状態か（開始前の案内・準備ダイアログの入口の判定）。 */
export function needsPrep(s: RepoStatusView | null): boolean {
  return s?.kind === "notRepo" || s?.kind === "noCommit" || s?.kind === "prepInterrupted";
}

/** Gitの状態の1行の説明。 */
export function repoStatusText(s: RepoStatusView): string {
  switch (s.kind) {
    case "ready": return `Git のリポジトリです（ブランチ: ${s.branch}）。`;
    case "nested": return `上位のリポジトリの一部です（${s.root}）。そのまま使えます。`;
    case "notRepo": return "Git のリポジトリではありません。Worker が専用 worktree で作業するには、先に準備が要ります。";
    case "noCommit": return "Git のリポジトリですが、まだコミットがありません。Worker が作業するには、最初のコミットが要ります。";
    case "prepInterrupted": return "前回の Git の準備が途中で止まっています（コミットがありません）。";
    case "ownerMismatch": return "Git がこのフォルダの所有者の違いを理由に拒否しています。";
    case "unavailable": return `Git の状態を確認できません（${s.message}）。`;
  }
}

/** 開始ダイアログの Git の状態行。フォルダが変わるたびに読取りだけで調べる。`tick` を変えると読み直す。 */
export function GitStatusRow({ folder, tick, onPrepare, onStatus }: { folder: string; tick: number; onPrepare: () => void; onStatus?: (s: RepoStatusView | null) => void }) {
  const [st, setSt] = useState<RepoStatusView | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [again, setAgain] = useState(0);
  const [dirty, setDirty] = useState<DirtySummaryView | null>(null);
  const [dirtyErr, setDirtyErr] = useState<string | null>(null);
  const cb = useRef(onStatus);
  cb.current = onStatus;
  useEffect(() => {
    setSt(null); setErr(null); cb.current?.(null);
    const f = folder.trim();
    if (!f) return;
    let stale = false;
    const t = setTimeout(() => {
      host.gitRepoStatus(f).then((s) => { if (!stale) { setSt(s); cb.current?.(s); } }).catch((e) => { if (!stale) setErr(errText(e)); });
    }, 300);
    return () => { stale = true; clearTimeout(t); };
  }, [folder, tick, again]);
  // 未コミットの変更の確認（読取りのみ）。コミットのあるリポジトリのときだけ調べる
  useEffect(() => {
    setDirty(null); setDirtyErr(null);
    const f = folder.trim();
    if (!f || !dirtyCheckApplies(st)) return;
    let stale = false;
    host.gitDirtySummary(f).then((d) => { if (!stale) setDirty(d); }).catch((e) => { if (!stale) setDirtyErr(errText(e)); });
    return () => { stale = true; };
  }, [folder, st, tick]);
  if (!folder.trim()) return null;
  const owner = ownerRepoOf(st);
  const dirtyText = dirtyNoticeText(dirty);
  return (
    <div className="small" role="status" style={{ marginTop: 4 }}>
      {err ? <span className="why err">Git の状態を確認できませんでした（{err}）。</span> : st ? <span>{repoStatusText(st)}</span> : <span className="muted">Git の状態を確認しています…</span>}
      {needsPrep(st) ? <> <button type="button" className="btn sm" onClick={onPrepare}>Gitを準備…</button></> : null}
      {/* 許可したら状態を読み直す（開始前チェックのやり直し） */}
      {owner ? <OwnerAllowPanel repo={owner} onAllowed={() => setAgain((n) => n + 1)} /> : null}
      {dirtyText ? <div className="why warn" role="status" style={{ marginTop: 4, color: "var(--wait-ink)" }}>{dirtyText}</div> : null}
      {dirtyErr ? <div className="muted" style={{ marginTop: 4 }}>未コミットの変更は確認できませんでした（{dirtyErr}）。</div> : null}
    </div>
  );
}

// ───────────────────────────── Gitを準備 ─────────────────────────────

function outcomeView(o: PrepOutcome): { ok: boolean; text: string; extra?: string } {
  switch (o.kind) {
    case "prepared": return { ok: true, text: `Git を準備しました（最初のコミット ${o.commit.slice(0, 8)}）。` };
    case "rolledBack": return { ok: false, text: `準備に失敗しました: ${o.reason}` };
    case "failed": return { ok: false, text: `準備に失敗しました: ${o.reason}`, extra: o.leftover ? `途中まで作った .git が残っています: ${o.leftover}` : undefined };
    case "unknown": return { ok: false, text: `準備が完了したか確認できません。実際の状態: ${repoStatusText(o.status)}` };
  }
}

/** 「Gitを準備」。内容を見せ、押したときだけ実行する。`onDone(prepared)` は閉じるときに呼ぶ（準備できたかどうか）。 */
export function GitPrepBody({ folder, onDone }: { folder: string; onDone: (prepared: boolean) => void }) {
  const [apply, setApply] = useState(false);
  const [pv, setPv] = useState<RepoPrepPreview | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [out, setOut] = useState<PrepOutcome | null>(null);
  const [removing, setRemoving] = useState(false);
  const [owner, setOwner] = useState<string | null>(null);
  const load = useCallback(() => {
    setPv(null); setErr(null); setOwner(null);
    let stale = false;
    host.gitPrepPreview({ folder, applyExcludes: apply }).then((p) => { if (!stale) setPv(p); }).catch((e) => {
      if (stale) return;
      setErr(errText(e));
      // 所有者違いで断られたときは、許可の選択肢を出す（読取りだけで確かめる）
      host.gitRepoStatus(folder).then((s) => { if (!stale) setOwner(ownerRepoOf(s)); }).catch(() => {});
    });
    return () => { stale = true; };
  }, [folder, apply]);
  useEffect(() => load(), [load]);
  const done = out?.kind === "prepared";
  const run = async () => {
    if (!pv || busy || pv.refusal) return;
    setBusy(true); setErr(null);
    try { const r = await host.gitPrepareRepo({ folder, previewToken: pv.previewToken, applyExcludes: apply }); setOut(r.outcome); }
    catch (e) { setErr(errText(e)); }
    setBusy(false);
  };
  const removeInterrupted = async () => {
    setBusy(true); setErr(null);
    try { await host.gitPrepRemoveInterrupted(folder); setRemoving(false); setOut(null); setPv(null); load(); }
    catch (e) { setErr(errText(e)); }
    setBusy(false);
  };
  const ov = out ? outcomeView(out) : null;
  const interrupted = pv?.kind === "interrupted";
  return (
    <div className="content">
      <div className="field"><span>対象のフォルダ</span><div className="mono">{folder}</div></div>
      {err ? <div className="why err" role="alert">{err}</div> : null}
      {owner ? <OwnerAllowPanel repo={owner} onAllowed={() => { load(); }} /> : null}
      {!pv && !err ? <p className="small muted">フォルダの中身を調べています（何も作りません）…</p> : null}
      {pv ? (
        <>
          <div className="banner info" role="note"><div>
            {interrupted
              ? <>前回の準備が途中で止まっています。<b>続きから</b>（ファイルの追加と最初のコミットだけ）行うか、作りかけの <span className="mono">.git</span> を取り除けます。</>
              : <>作るもの: <b><span className="mono">.git</span> フォルダ</b>（AgentDock はこれ以外のファイルを作りません）。このフォルダの既存のファイルは移動・変更しません。</>}
          </div></div>
          <div className="field"><span>最初のコミットに入るファイル</span>
            <div><b>{pv.fileCount}</b> 件・合計 <b>{fmtBytes(pv.totalBytes)}</b></div>
            {pv.largest.length > 0 ? (
              <details className="about"><summary>大きいファイル（上位 {pv.largest.length} 件）</summary>
                <ul className="check-list">{pv.largest.map((f) => <li key={f.path}><span className="mono">{f.path}</span>（{fmtBytes(f.size)}）</li>)}</ul></details>
            ) : null}
          </div>
          {pv.suggestedExcludes.length > 0 ? (
            <div className="field"><span>外す候補（巨大なファイル・一時ファイル）</span>
              <label><input type="checkbox" checked={apply} disabled={busy || done} onChange={(e) => setApply(e.target.checked)} /> これらをコミットから外す（<span className="mono">.git\info\exclude</span> に追記します）</label>
              <ul className="check-list">{pv.suggestedExcludes.map((x) => <li key={x.path}><span className="mono">{x.path}</span>（{fmtBytes(x.size)}・{x.reason}）</li>)}</ul>
              {apply ? <div className="note">追記する内容（既存の内容は変えず、末尾に足すだけです）: <pre className="mono">{pv.excludeLines.join("\n")}</pre>外したファイルは Worker の作業場所に入りません。</div>
                : <div className="note">外さないと、これらも最初のコミットに入ります（既定は外しません）。</div>}
            </div>
          ) : null}
          <div className="field"><span>コミットの作者</span>
            <div>{pv.author.name} &lt;{pv.author.email}&gt;{pv.author.isAgentdock ? <span className="note">Git のユーザー設定が見つからないため、AgentDock 名義で作ります（設定は書き換えません）。後で <span className="mono">git commit --amend --reset-author</span> で変えられます。</span> : null}</div>
            <div className="note">署名なし・フックは走らせません。ブランチ名は Git の既定（init.defaultBranch）に従います。</div></div>
          {pv.notes.map((n, i) => <div key={i} className="banner warn" role="status"><div>{n}</div></div>)}
          {pv.refusal ? <div className="why err" role="alert">{pv.refusal}</div> : null}
        </>
      ) : null}
      {ov ? (
        <div className={ov.ok ? "banner info" : "banner warn"} role={ov.ok ? "status" : "alert"}><div>{ov.text}{ov.extra ? <div className="mono small">{ov.extra}</div> : null}</div></div>
      ) : null}
      {removing ? (
        <ConfirmBlock label="途中の .git を取り除く確認" actions={<>
          <button className="btn" disabled={busy} onClick={() => setRemoving(false)}>やめる</button>
          <button className="btn danger" disabled={busy} onClick={() => void removeInterrupted()}>{busy ? "取り除いています…" : "取り除く"}</button></>}>
          <p>AgentDock が作った印のある <span className="mono">.git</span> フォルダ（コミットなし）だけを取り除きます。ほかのファイルには触れません。</p>
        </ConfirmBlock>
      ) : null}
      <FootActions
        left={interrupted && !done ? <button className="btn" disabled={busy} onClick={() => setRemoving(true)}>取り除く…</button> : undefined}
        main={done
          ? <button className="btn primary" onClick={() => onDone(true)}>閉じる</button>
          : <button className="btn primary" disabled={!pv || !!pv.refusal || busy} onClick={() => void run()}>{busy ? "準備しています…" : interrupted ? "続きから行う" : "Gitを準備する"}</button>} />
    </div>
  );
}

// ───────────────────────────── プッシュ ─────────────────────────────

const BLOCKER_FALLBACK: Record<PushBlocker, string> = {
  noRemote: "リモートがありません。", detached: "ブランチを指していません。", internalBranch: "AgentDock の作業用ブランチは送れません。",
  nothingToPush: "送るコミットがありません。", unavailable: "Git の状態を確認できません。",
};

const REJECT_TEXT = (r: PushRejectReason): string => {
  switch (r.kind) {
    case "nonFastForward": return "リモートに新しい変更があります。Git で取り込んでから、もう一度送ってください（AgentDock には取り込み＝pull の操作がありません）。強制プッシュはしません。";
    case "hookDeclined": return "プッシュ前のフックまたはリモートのフックが断りました。";
    case "other": return `拒否されました: ${r.text}`;
  }
};

function pushResultView(r: PushResult): { ok: boolean; unknown: boolean; text: string } {
  switch (r.kind) {
    case "pushed": return { ok: true, unknown: false, text: `送りました（コミット ${r.sha.slice(0, 8)}）。${r.upstreamNote ? ` ${r.upstreamNote}` : ""}` };
    case "rejected": return { ok: false, unknown: false, text: REJECT_TEXT(r.reason) };
    case "authFailed": return { ok: false, unknown: false, text: "認証に失敗しました。Git の資格情報（資格情報マネージャなど）を確認してください。AgentDock は資格情報を扱いません。" };
    case "networkError": return { ok: false, unknown: false, text: "ネットワークに接続できず、送れませんでした。" };
    case "unknown": return { ok: false, unknown: true, text: `送れたか確認できません。${r.message}` };
  }
}

const verifyText = (v: PushVerify): string => {
  switch (v.kind) {
    case "confirmed": return "リモートのブランチは、送ろうとしたコミットを指しています（送れています）。";
    case "notThere": return "リモートのブランチが、送ろうとしたコミットを指していません（送れていません）。";
    case "unavailable": return `リモートを確認できませんでした（${v.message}）。`;
  }
};

/** 「プッシュ」。作業フォルダの今のブランチだけを、確認時のコミットで送る。 */
export function GitPushBody({ folder, onDone }: { folder: string; onDone: () => void }) {
  const [remote, setRemote] = useState<string | null>(null);
  const [pv, setPv] = useState<PushPreview | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [setUp, setSetUp] = useState(true);
  const [busy, setBusy] = useState(false);
  const [res, setRes] = useState<PushResult | null>(null);
  const [verify, setVerify] = useState<PushVerify | null>(null);
  useEffect(() => {
    let stale = false;
    setPv(null); setErr(null);
    host.gitPushPreview(folder, remote).then((p) => { if (!stale) setPv(p); }).catch((e) => { if (!stale) setErr(errText(e)); });
    return () => { stale = true; };
  }, [folder, remote]);
  const ready = !!pv && !pv.blocker && !!pv.remote && !!pv.branch && !!pv.headSha;
  const rv = res ? pushResultView(res) : null;
  const run = async () => {
    if (!pv || !ready || busy || res) return;
    setBusy(true); setErr(null);
    try { setRes(await host.gitPush({ folder, remote: pv.remote!, branch: pv.branch!, expectedSha: pv.headSha!, setUpstream: !pv.upstream && setUp })); }
    catch (e) { setErr(errText(e)); }
    setBusy(false);
  };
  const check = async () => {
    if (!pv?.remote || !pv.branch || !pv.headSha) return;
    setBusy(true);
    try { setVerify(await host.gitPushVerify({ folder, remote: pv.remote, branch: pv.branch, expectedSha: pv.headSha })); }
    catch (e) { setVerify({ kind: "unavailable", message: errText(e) }); }
    setBusy(false);
  };
  return (
    <div className="content">
      <p><b>リモートへ送ると、AgentDock からは取り消せません。</b>内容を確認してから押してください。</p>
      <div className="field"><span>作業フォルダ</span><div className="mono">{folder}</div></div>
      {err ? <div className="why err" role="alert">{err}</div> : null}
      {!pv && !err ? <p className="small muted">送る内容を調べています（ネットワークは使いません）…</p> : null}
      {pv ? (
        <>
          {pv.blocker ? <div className="banner warn" role="alert"><div>{pv.blocker.message || BLOCKER_FALLBACK[pv.blocker.kind]}</div></div> : null}
          {pv.remote && pv.branch ? (
            <div className="field"><span>送り先</span>
              <div><b className="mono">{pv.branch}</b> → <b className="mono">{pv.remote}/{pv.branch}</b></div>
              {pv.url ? <div className="mono small">{pv.url}</div> : null}
              {pv.remotes.length > 1 && !pv.upstream ? (
                <label className="small">リモート: <select value={pv.remote} disabled={busy || !!res} onChange={(e) => setRemote(e.target.value)}>{pv.remotes.map((r) => <option key={r} value={r}>{r}</option>)}</select></label>
              ) : null}
              {pv.upstream ? <div className="note">追跡先: <span className="mono">{pv.upstream}</span></div> : null}
            </div>
          ) : null}
          {pv.commits.length > 0 ? (
            <div className="field"><span>送るコミット（{pv.truncated ? `${pv.commits.length} 件以上` : `${pv.commitTotal} 件`}）</span>
              <ul className="check-list">{pv.commits.map((c) => <li key={c.sha}><span className="mono">{c.short}</span> {c.subject}<span className="muted">（{c.author}）</span></li>)}</ul>
              {pv.truncated ? <div className="note">先頭の {pv.commits.length} 件だけを表示しています。</div> : null}
            </div>
          ) : null}
          {pv.dirtyCount > 0 ? <div className="note">未コミットの変更 {pv.dirtyCount} 件は送られません。</div> : null}
          {ready && !pv.upstream ? (
            <label className="small"><input type="checkbox" checked={setUp} disabled={busy || !!res} onChange={(e) => setSetUp(e.target.checked)} /> 追跡先として設定する（リポジトリの設定に追跡先を書きます）</label>
          ) : null}
          <div className="note">強制プッシュはしません。認証は Git の設定（資格情報マネージャ）を使い、AgentDock は資格情報を扱いません。送るのは、確認したこのコミットだけです。</div>
        </>
      ) : null}
      {rv ? <div className={rv.ok ? "banner info" : "banner warn"} role={rv.ok ? "status" : "alert"}><div>{rv.text}</div></div> : null}
      {rv?.unknown ? (
        <div className="small">
          <button type="button" className="btn sm" disabled={busy} onClick={() => void check()}>リモートを確認</button>
          {verify ? <div role="status">{verifyText(verify)}</div> : null}
          <div className="muted">自動では送り直しません。もう一度送るときは、閉じて開き直し、内容を確認してから押してください。</div>
        </div>
      ) : null}
      <FootActions main={res
        ? <button className="btn primary" onClick={onDone}>閉じる</button>
        : <button className="btn primary" disabled={!ready || busy} onClick={() => void run()}>{busy ? "送っています…" : "プッシュする"}</button>} />
    </div>
  );
}
