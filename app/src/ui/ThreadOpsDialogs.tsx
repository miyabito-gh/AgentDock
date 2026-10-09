// コードレビュー（#3・#4）・会話の分岐（#6）・文脈の圧縮（#7）の本体（段階③ P3-2）。外枠（Shell）と下部ボタンは Dialogs.tsx 側。
// 規則: 状態を変える操作は確認の後だけ。結果は「受け付けた」までしか書かない（レビュー結果・圧縮の完了は、会話の記録を観測したときだけ表示される）。
//       受理不明は再送せず、読取りだけの「状態を確認」を出す。取れていない値は「未確認」と書き、0や空で代用しない。
import { useEffect, useState } from "react";
import * as host from "../ipc/client";
import type { Chat, ChatKey, ExternalId, ForkReconcile, OpAck, OpCapability, OpReconcile, ParityOp, PendingOp, ReviewChoices, ReviewDelivery, ReviewOutcome, ReviewTarget, TurnRecord } from "../ipc/types";
import { chatName } from "./derive";
import { UnverifiedNote } from "./ChangesDialog";
import { About, ConfirmBlock, FootActions, ShortId } from "./DialogParts";
import { ackText } from "./PrefsDialogs";

const errText = (e: unknown) => host.asIpcError(e).message;
const when = (ms: number) => new Date(ms).toLocaleString("ja-JP");

export interface ThreadOpProps {
  chat: Chat | undefined;
  live: boolean;
  cap: OpCapability | undefined;
  /** このチャットの、結果が未確認の操作。 */
  pending: PendingOp[];
  /** 送信待ちの件数（この操作の完了後に送られる）。 */
  waiting: number;
  /** 別の会話（分岐先・レビュー用の会話）を開く。 */
  onOpenChat: (chat: ChatKey) => void;
}

const reconcileText = (r: OpReconcile): string => {
  switch (r.kind) {
    case "observed": return "履歴に、この操作の痕跡を観測しました（受け付けられていました）。未確認の記録を解除しました。";
    case "notObserved": return "履歴にこの操作の痕跡はありませんでした（十分な時間が経ち、履歴は完全に読めています）。未確認の記録を解除しました。再実行するかはあなたが決めてください。";
    case "undetermined": return `まだ判断できません: ${r.reason}`;
    case "unreadable": return `履歴を読めませんでした: ${r.message}`;
  }
};

/** 結果が未確認の操作の表示と、読取りだけの照合。照合しても操作は再送しない。 */
function PendingNote({ chat, pending, ops }: { chat: Chat; pending: PendingOp[]; ops: ParityOp[] }) {
  const [busy, setBusy] = useState(false);
  const [text, setText] = useState<string | null>(null);
  const mine = pending.filter((p) => ops.includes(p.op));
  if (mine.length === 0 && !text) return null;
  const check = async (op: ParityOp) => {
    setBusy(true);
    try { setText(reconcileText(await host.reconcileOp(chat.key, op))); } catch (e) { setText(errText(e)); } finally { setBusy(false); }
  };
  return (
    <div className="gbanner warn" role="status">
      <span className="grow">
        {mine.length > 0 ? `結果が未確認の操作があります（送った時刻 ${mine.map((p) => when(p.since)).join("、")}）。確認できるまで、同じ操作と送信待ちの自動送信を止めています。再送はしません。` : null}
        {text ? <div className="small">{text}</div> : null}
      </span>
      {mine.length > 0 ? <button className="btn-line" disabled={busy} onClick={() => void check(mine[0].op)}>{busy ? "確認しています…" : "状態を確認（読取りのみ）"}</button> : null}
    </div>
  );
}

// ───────────────────────────── コードレビュー ─────────────────────────────

type TargetKind = "uncommitted" | "base" | "commit" | "custom";

export function ReviewBody({ chat, live, cap, pending, waiting, onOpenChat }: ThreadOpProps) {
  const [choices, setChoices] = useState<ReviewChoices | null>(null);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [kind, setKind] = useState<TargetKind>("custom");
  const [branch, setBranch] = useState("");
  const [sha, setSha] = useState("");
  const [instructions, setInstructions] = useState("");
  const [delivery, setDelivery] = useState<ReviewDelivery>("currentChat");
  const [confirming, setConfirming] = useState(false);
  const [running, setRunning] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<{ o: ReviewOutcome; target: ReviewTarget } | null>(null);
  const [retrying, setRetrying] = useState(false);
  const chatKey = chat?.key;
  const chatId = chatKey?.id;

  useEffect(() => {
    if (!live || !chatKey) return;
    let alive = true;
    host.getReviewChoices(chatKey).then((c) => {
      if (!alive) return;
      setChoices(c);
      if (c.git.kind === "ready") {
        setKind("uncommitted");
        setBranch(c.branches[0] ?? "");
        setSha(c.commits[0]?.sha ?? "");
      }
    }).catch((e) => { if (alive) setLoadErr(errText(e)); });
    return () => { alive = false; };
  }, [live, chatId]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!chat) return <div className="content"><p>チャットを選んでください。</p></div>;
  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;

  const gitOk = choices?.git.kind === "ready";
  const gitReason = choices && choices.git.kind !== "ready" ? choices.git.message : null;
  const target = (): ReviewTarget | null => {
    switch (kind) {
      case "uncommitted": return { kind: "uncommittedChanges" };
      case "base": return branch ? { kind: "baseBranch", branch } : null;
      case "commit": {
        if (!sha) return null;
        const title = choices?.commits.find((c) => c.sha === sha)?.title ?? null;
        return { kind: "commit", sha, title };
      }
      case "custom": return instructions.trim() ? { kind: "custom", instructions } : null;
    }
  };
  const t = target();
  const targetLabel = (x: ReviewTarget): string => {
    switch (x.kind) {
      case "uncommittedChanges": return "未コミットの変更";
      case "baseBranch": return `基準ブランチ「${x.branch}」との差分`;
      case "commit": return `コミット ${x.sha.slice(0, 8)}${x.title ? `（${x.title}）` : ""}`;
      case "custom": return "指示による依頼";
    }
  };
  const run = async () => {
    if (!t) return;
    setRunning(true); setErr(null);
    try { setOutcome({ o: await host.startReview(chat.key, t, delivery), target: t }); setConfirming(false); } catch (e) { setErr(errText(e)); setConfirming(false); } finally { setRunning(false); }
  };
  const retryHere = async (c: ChatKey, tg: ReviewTarget) => {
    setRetrying(true); setErr(null);
    try { setOutcome({ o: await host.startReview(c, tg, "currentChat"), target: tg }); } catch (e) { setErr(errText(e)); } finally { setRetrying(false); }
  };
  const ackLine = (o: Extract<ReviewOutcome, { kind: "started" }>) =>
    `${ackText(o.ack, "レビュー")}${o.ack.kind === "accepted" ? " レビュー結果は、会話に「レビュー結果」の記録が現れたときに表示されます。" : ""}`;

  return (
    <>
    <div className="content">
      <p><b>{chatName(chat)}</b> の作業フォルダのコードをレビューします。</p>
      <UnverifiedNote cap={cap} />
      <PendingNote chat={chat} pending={pending} ops={["codeReview"]} />
      {loadErr ? <p className="gbanner err" role="alert">レビュー対象の候補を取得できませんでした: {loadErr}</p> : null}
      {!choices && !loadErr ? <p>レビュー対象の候補を確認しています…（読取りのみ）</p> : null}
      {choices && !outcome ? (
        <>
          {gitReason ? <p className="gbanner warn" role="status">Gitの対象は選べません（{choices.git.kind === "notSupported" ? "非対応" : "未取得"}: {gitReason}）。指示を書いて依頼する方式だけ使えます。</p> : null}
          <div className="field"><span>レビュー対象</span>
            <div>
              <label><input type="radio" name="rv-kind" checked={kind === "uncommitted"} disabled={!gitOk} onChange={() => setKind("uncommitted")} /> 未コミットの変更</label><br />
              <label><input type="radio" name="rv-kind" checked={kind === "base"} disabled={!gitOk || choices.branches.length === 0} onChange={() => setKind("base")} /> 基準ブランチとの差分
                <select value={branch} disabled={kind !== "base"} onChange={(e) => setBranch(e.target.value)} aria-label="基準ブランチ">{choices.branches.map((b) => <option key={b} value={b}>{b}</option>)}</select></label><br />
              <label><input type="radio" name="rv-kind" checked={kind === "commit"} disabled={!gitOk || choices.commits.length === 0} onChange={() => setKind("commit")} /> コミット
                <select value={sha} disabled={kind !== "commit"} onChange={(e) => setSha(e.target.value)} aria-label="コミット">{choices.commits.map((c) => <option key={c.sha} value={c.sha}>{c.sha.slice(0, 8)} {c.title}</option>)}</select></label><br />
              <label><input type="radio" name="rv-kind" checked={kind === "custom"} onChange={() => setKind("custom")} /> 指示を書いて依頼する</label>
            </div></div>
          {kind === "custom" ? <div className="field"><span>指示</span><textarea rows={3} value={instructions} onChange={(e) => setInstructions(e.target.value)} aria-label="レビューの指示" /></div> : null}
          <div className="field"><span>結果の置き場所</span>
            <div>
              <label><input type="radio" name="rv-del" checked={delivery === "currentChat"} onChange={() => setDelivery("currentChat")} /> この会話の中</label><br />
              <label><input type="radio" name="rv-del" checked={delivery === "newChat"} onChange={() => setDelivery("newChat")} /> 別チャット（同じ作業フォルダで新しい会話を作って実行します。元の会話は変えません）</label>
            </div></div>
          {err ? <p className="gbanner err" role="alert">{err}</p> : null}
          {confirming && t ? (
            <div className="gbanner warn" role="alertdialog" aria-label="レビューの確認">
              <span className="grow">
                {targetLabel(t)} のレビューを{delivery === "newChat" ? "新しい会話で" : "この会話で"}始めます。{delivery === "currentChat" ? "この会話にレビューの作業が加わります。" : "新しい会話を作ります。"}
                {waiting > 0 ? `この操作の完了後に、送信待ち ${waiting} 件が送られます。` : null}
              </span>
              <button className="btn" disabled={running} onClick={() => setConfirming(false)}>やめる</button>
              <button className="btn primary" disabled={running} onClick={() => void run()}>{running ? "依頼しています…" : "レビューを始める"}</button>
            </div>
          ) : null}
        </>
      ) : null}
      {outcome ? (
        <div role="status">
          {outcome.o.kind === "started" ? (
            <>
              <p className={outcome.o.ack.kind === "accepted" ? "banner info" : "banner err"}>{ackLine(outcome.o)}</p>
              {outcome.o.createdChat ? <p className="small">新しい会話を作りました（レビュー元として、この会話を記録しています）。</p> : null}
              {outcome.o.ack.kind === "unknown" ? <p className="small muted">受理を確認できないため、再送していません。上の「状態を確認」で履歴を読んで確かめてください。</p> : null}
              {outcome.o.createdChat && outcome.o.ack.kind !== "rejected" ? <div className="acts"><button className="btn-line" onClick={() => onOpenChat((outcome.o as Extract<ReviewOutcome, { kind: "started" }>).chat)}>新しい会話を開く</button></div> : null}
            </>
          ) : (
            <>
              <p className="gbanner err">新しい会話は作りましたが、レビューは始まっていません: {outcome.o.message}</p>
              <p className="small muted">作った会話は残っています。自動では再試行しません。</p>
              <div className="acts">
                <button className="btn-main" disabled={retrying} onClick={() => void retryHere((outcome.o as Extract<ReviewOutcome, { kind: "newChatCreatedReviewFailed" }>).chat, outcome.target)}>{retrying ? "依頼しています…" : "作った会話でレビューを開始"}</button>
                <button className="btn-line" onClick={() => onOpenChat((outcome.o as Extract<ReviewOutcome, { kind: "newChatCreatedReviewFailed" }>).chat)}>作った会話を開く</button>
              </div>
              {err ? <p className="gbanner err" role="alert">{err}</p> : null}
            </>
          )}
        </div>
      ) : null}
    </div>
    {choices && !outcome ? <FootActions main={<button className="btn primary" disabled={!t || running || confirming || pending.some((p) => p.op === "codeReview")} onClick={() => setConfirming(true)}>確認へ…</button>} /> : null}
    </>
  );
}

// ───────────────────────────── 会話の分岐 ─────────────────────────────

const forkText = (r: ForkReconcile): string => {
  switch (r.kind) {
    case "adopted": return "分岐先を1件に絞れたため、分岐先として採用しました。";
    case "notFound": return "該当する分岐先は見えていません（分岐されなかったとは断定できません）。一覧を更新してからもう一度確認してください。";
    case "ambiguous": return `分岐先を確認できません（候補が${r.count}件あり、1件に絞れません）。一覧を更新して確認してください。`;
    case "unreadable": return `一覧を読めませんでした: ${r.message}`;
  }
};

export function ForkBody({ chat, live, cap, throughTurn, onOpenChat }: Pick<ThreadOpProps, "chat" | "live" | "cap" | "onOpenChat"> & { throughTurn: ExternalId | null }) {
  const [confirming, setConfirming] = useState(false);
  const [running, setRunning] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [result, setResult] = useState<{ ack: OpAck; chat: Chat | null; at: number; note: string | null } | null>(null);
  const [recon, setRecon] = useState<ForkReconcile | null>(null);
  const [checking, setChecking] = useState(false);
  if (!chat) return <div className="content"><p>チャットを選んでください。</p></div>;
  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;
  const run = async () => {
    setRunning(true); setErr(null);
    try {
      const r = await host.forkChat(chat.key, throughTurn);
      setResult({ ack: r.ack, chat: r.chat, at: r.attemptedAt, note: r.note }); setConfirming(false);
    } catch (e) { setErr(errText(e)); setConfirming(false); } finally { setRunning(false); }
  };
  const check = async () => {
    if (!result) return;
    setChecking(true);
    try { setRecon(await host.reconcileFork(chat.key, result.at)); } catch (e) { setErr(errText(e)); } finally { setChecking(false); }
  };
  const adopted = result?.chat ?? (recon?.kind === "adopted" ? recon.chat : null);
  return (
    <>
    <div className="content">
      <p><b>{chatName(chat)}</b> を{throughTurn ? <>「turn <ShortId id={throughTurn} />」まで</> : "最新の終了したturnまで"}引き継いだ、新しい会話に分岐します。元の会話は変わりません。</p>
      <UnverifiedNote cap={cap} />
      <p className="small">元の会話を削除すると、分岐先が参照する添付は欠損表示になります。</p>
      <About><p>分岐先はAgentDockで管理する会話として一覧に加わります。分岐先の履歴は、元の会話の添付のコピーを参照します。</p></About>
      {err ? <p className="gbanner err" role="alert">{err}</p> : null}
      {!result ? (
        confirming ? (
          <div className="gbanner warn" role="alertdialog" aria-label="分岐の確認">
            <span className="grow">新しい会話を作って分岐します。よろしいですか？</span>
            <button className="btn" disabled={running} onClick={() => setConfirming(false)}>やめる</button>
            <button className="btn primary" disabled={running} onClick={() => void run()}>{running ? "分岐しています…" : "分岐する"}</button>
          </div>
        ) : null
      ) : (
        <div role="status">
          {result.ack.kind === "accepted" ? <p className="banner info">分岐を要求し、Codex が受け付けました。{adopted ? `分岐先: ${chatName(adopted)}` : ""}</p> : null}
          {result.ack.kind === "rejected" ? <p className="gbanner err">分岐は受け付けられませんでした（{result.ack.message}）。</p> : null}
          {result.ack.kind === "unknown" ? <p className="gbanner err">分岐の受理を確認できません（{result.ack.message}）。再送はしません。分岐先を確認（読取りのみ）できます。</p> : null}
          {result.note ? <p className="small muted">{result.note}</p> : null}
          {recon ? <p className="small">{forkText(recon)}</p> : null}
        </div>
      )}
    </div>
    <FootActions
      left={result?.ack.kind === "unknown" ? <button className="btn" disabled={checking} onClick={() => void check()}>{checking ? "確認しています…" : "分岐先を確認"}</button> : null}
      main={!result ? <button className="btn primary" disabled={confirming} onClick={() => setConfirming(true)}>確認へ…</button> : adopted ? <button className="btn primary" onClick={() => onOpenChat(adopted.key)}>分岐先を開く</button> : null} />
    </>
  );
}

// ───────────────────────────── 文脈の圧縮 ─────────────────────────────

const entryLabel = (k: TurnRecord["entries"][number]["kind"]): string => {
  switch (k.kind) {
    case "userMessage": return "依頼";
    case "agentMessage": return "応答";
    case "other": return k.raw;
    default: return k.kind;
  }
};

function SnapshotView({ chat }: { chat: Chat }) {
  const [ids, setIds] = useState<number[] | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const [turns, setTurns] = useState<TurnRecord[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    host.listCompactionSnapshots(chat.key).then((l) => { if (alive) setIds(l); }).catch((e) => { if (alive) setErr(errText(e)); });
    return () => { alive = false; };
  }, [chat.key.id]); // eslint-disable-line react-hooks/exhaustive-deps
  const show = async (id: number) => {
    setOpen(id); setTurns(null); setErr(null);
    try { setTurns((await host.readCompactionSnapshot(chat.key, id)).turns); } catch (e) { setErr(errText(e)); }
  };
  return (
    <div>
      <h4 style={{ margin: "10px 0 4px" }}>圧縮前の本文の控え</h4>
      <About><p>圧縮を送る前に AgentDock が保存した、表示専用の控えです（Codex には戻しません。自動では削除せず、チャットの削除と一緒に消えます）。Codex が自動で行った圧縮の前の本文は、控えがなく、Codex の履歴に残っていなければ取得できません。</p></About>
      {err ? <p className="gbanner err" role="alert">{err}</p> : null}
      {ids === null && !err ? <p className="small">読み込んでいます…</p> : null}
      {ids && ids.length === 0 ? <p className="small">保存された控えはありません。</p> : null}
      {ids && ids.length > 0 ? (
        <ul className="check-list" style={{ listStyle: "none", paddingLeft: 0 }}>
          {ids.map((id) => <li key={id}><button className="btn-line" onClick={() => void show(id)} aria-pressed={open === id}>{when(id)} の控え</button></li>)}
        </ul>
      ) : null}
      {open !== null && !turns && !err ? <p className="small">控えを読んでいます…</p> : null}
      {turns ? (
        <div className="diffview" role="region" aria-label="圧縮前の本文" tabIndex={0} style={{ whiteSpace: "pre-wrap" }}>
          {turns.length === 0 ? <div className="ln">（この控えに本文はありません）</div> : turns.map((t) => (
            <div key={t.key.turnId} className="ln" style={{ marginBottom: 6 }}>
              <div className="meta">turn <ShortId id={t.key.turnId} />{t.complete ? "" : "（部分的にしか読めていません）"}</div>
              {t.entries.map((e) => <div key={e.key.itemId}><b>{entryLabel(e.kind)}</b>　{e.text.kind === "value" ? e.text.value : "（内容の記載なし）"}</div>)}
            </div>))}
        </div>
      ) : null}
    </div>
  );
}

export function CompactBody({ chat, live, cap, pending, waiting }: ThreadOpProps) {
  const [confirming, setConfirming] = useState(false);
  const [running, setRunning] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [result, setResult] = useState<Awaited<ReturnType<typeof host.compactChat>> | null>(null);
  if (!chat) return <div className="content"><p>チャットを選んでください。</p></div>;
  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;
  const run = async () => {
    setRunning(true); setErr(null);
    try { setResult(await host.compactChat(chat.key)); setConfirming(false); } catch (e) { setErr(errText(e)); setConfirming(false); } finally { setRunning(false); }
  };
  return (
    <>
    <div className="content">
      <p><b>{chatName(chat)}</b> の文脈を圧縮します。</p>
      <UnverifiedNote cap={cap} />
      <About><p>圧縮の前に、現在の本文をこのチャット用の領域へ控えとして保存します。保存できないときは圧縮を送りません。圧縮が終わったかどうかは、会話に「文脈の圧縮」の記録が現れたときだけ表示されます。</p></About>
      <PendingNote chat={chat} pending={pending} ops={["compact"]} />
      {err ? <p className="gbanner err" role="alert">{err}</p> : null}
      {!result ? (
        confirming ? (
          <ConfirmBlock label="圧縮の確認" actions={<>
            <button className="btn" disabled={running} onClick={() => setConfirming(false)}>やめる</button>
            <button className="btn danger" disabled={running} onClick={() => void run()}>{running ? "控えを保存して依頼しています…" : "圧縮する"}</button></>}>
            <p>
              文脈を圧縮します。元に戻せません（圧縮前の本文は控えとして残ります）。
              {waiting > 0 ? `この操作の完了後に、送信待ち ${waiting} 件が送られます。` : null}
            </p>
          </ConfirmBlock>
        ) : null
      ) : (
        <div role="status">
          <p className={result.ack.kind === "accepted" ? "banner info" : "banner err"}>{ackText(result.ack, "文脈の圧縮")}</p>
          {result.ack.kind === "unknown" ? <p className="small muted">受理を確認できないため、再送していません。上の「状態を確認」で履歴を読んで確かめてください。</p> : null}
          <p className="small muted">圧縮前の本文の控え: <span className="mono">{result.snapshotPath}</span></p>
        </div>
      )}
      <SnapshotView key={result?.snapshot ?? 0} chat={chat} />
    </div>
    {!result ? <FootActions main={<button className="btn primary" disabled={confirming || pending.some((p) => p.op === "compact")} onClick={() => setConfirming(true)}>確認へ…</button>} /> : null}
    </>
  );
}
