// 編集して再送・再生成（M50、DESIGN_P5 §5）の本体。外枠（Shell）と下部ボタンの置き場所は Dialogs.tsx 側。
// 規則: 押したときだけ1回送る（自動送信・自動再送なし）。分岐が受理不明なら送らず、読取りだけの「分岐を確認」を出す。
//       送信の受理不明は再送しない（分岐先の「履歴と照合」に委ねる）。元の会話は変えない。結果は「受け付けた」までしか書かない。
import { useState } from "react";
import * as host from "../ipc/client";
import type { Chat, ChatKey, ForkPurpose, ForkReconcile, HostSnapshot, OpCapability, SendAttempt, TurnRecord } from "../ipc/types";
import { availability, commandContext } from "./commands";
import { chatAgents, chatName, chatRequests, chatStops, historyOutcomeOf, isRunning, rootView, stopOpen } from "./derive";
import { isActiveState } from "./chatState";
import { UnverifiedNote } from "./ChangesDialog";
import { About, FootActions } from "./DialogParts";
import { PURPOSE_SUFFIX, type ResendCtx, type ResendPlan, type ResendTarget } from "./resend";

const errText = (e: unknown) => host.asIpcError(e).message;

type Purpose = Exclude<ForkPurpose, "fork">;

// ───────────────────────────── 判断材料の組立て（snapshot → ResendCtx） ─────────────────────────────

/** メインのエージェントのturnだけ（子孫のturnは編集・再生成の対象にしない）。 */
export function rootTurns(snap: HostSnapshot, chat: Chat, turns: TurnRecord[]): TurnRecord[] {
  const rootId = rootView(snap, chat)?.agent.key.id;
  return rootId ? turns.filter((t) => t.key.agent.id === rootId) : turns;
}

/** チャット単位の可否の判断材料。表示のために接続・再開・読取りを増やさない。 */
export function resendCtxOf(snap: HostSnapshot, chat: Chat, connected: boolean, turns: TurnRecord[]): ResendCtx {
  const root = rootView(snap, chat);
  const base = commandContext(snap, chat, true, "fork");
  const cap = availability({ needsChat: false, blockedWhileBusy: false, needsGit: false }, { ...base, deletePending: false, externalRunning: false, busy: false });
  const stopOpenNow = chatStops(snap, chat).some(stopOpen);
  const busy = isRunning(snap, chat) || chatRequests(snap, chat).length > 0
    || chatAgents(snap, chat).some((a) => isActiveState(a.status.state))
    || snap.pendingOps.some((p) => p.chat.id === chat.key.id);
  return {
    capProblem: cap.kind === "disabled" ? cap.reason : null,
    connected,
    deletePending: base.deletePending,
    externalRunning: base.externalRunning,
    stopUnconfirmed: stopOpenNow,
    busy,
    // 状態不明でも、履歴で終端を確認できていれば使える（§4.2 と同じ考え方）。メインが見えないときは不明扱い。
    rootUnknown: !root || (root.status.state === "unknown" && !historyOutcomeOf(snap, root)),
    turns: rootTurns(snap, chat, turns).map((t) => ({ turnId: t.key.turnId, end: t.end, userCount: t.entries.filter((e) => e.kind.kind === "userMessage").length })),
  };
}

/** 対象（依頼の吹き出し＝編集、応答＝再生成）と、そのturnの依頼の文。 */
export function resendTargetOf(turns: TurnRecord[], turnId: string, purpose: Purpose, itemId: string | null): ResendTarget {
  const t = turns.find((x) => x.key.turnId === turnId);
  const users = t ? t.entries.filter((e) => e.kind.kind === "userMessage") : [];
  const idx = purpose === "editResend" && itemId ? users.findIndex((e) => e.key.itemId === itemId) : 0;
  const entry = users[idx < 0 ? 0 : idx];
  return {
    turnId, purpose,
    userIndex: purpose === "editResend" ? (idx < 0 ? 0 : idx) : null,
    requestText: entry && entry.text.kind === "value" ? entry.text.value : null,
  };
}

// ───────────────────────────── 本体 ─────────────────────────────

/** App から渡す操作。分岐先の選択・入力欄への反映・新しいチャットの作成は App が持つ。 */
export interface ResendHandlers {
  /** 分岐先を選び、入力欄の文を反映する（text=null は入力欄を変えない。attempt は送信の受理の案内用）。 */
  forked: (key: ChatKey, text: string | null, attempt: SendAttempt | null) => void;
  /** 最初のturn: 同じ種類・作業フォルダ・モデル・権限の新しいチャットを作り、文を入力欄に入れて選ぶ（送らない）。 */
  startFirst: (text: string) => Promise<ChatKey>;
  /** 分岐先の下書きへ文を保存し、入力欄に反映する（保存できなければ例外）。 */
  saveDraft: (key: ChatKey, text: string) => Promise<void>;
}

export interface ResendState {
  plan: ResendPlan;
  purpose: Purpose;
  requestText: string | null;
}

interface Result { tone: "info" | "err"; lines: string[]; unknownAt: number | null; finished: boolean }

const reconText = (r: ForkReconcile): string => {
  switch (r.kind) {
    case "notFound": return "該当する分岐先は見えていません（分岐されなかったとは断定できません）。一覧を更新してからもう一度確認してください。";
    case "ambiguous": return `分岐先を確認できません（候補が${r.count}件あり、1件に絞れません）。一覧を更新して確認してください。`;
    case "unreadable": return `一覧を読めませんでした: ${r.message}`;
    case "adopted": return "";
  }
};

/** 分岐が受理された後の結果の文。送信は「受け付けた」までしか書かない。 */
function forkedLines(draftSaved: boolean, send: SendAttempt | null, sent: boolean, note: string | null): string[] {
  const out: string[] = [];
  if (!draftSaved) out.push("分岐しましたが、入力欄への保存に失敗したため、送っていません。依頼の文は入力欄に表示しています。内容を確かめて、送信は入力欄から行ってください。");
  else if (!sent) out.push("分岐して、依頼の文を新しいチャットの入力欄に入れました。送信は入力欄から行ってください。");
  else if (!send) out.push("分岐しましたが、送信の結果を取得できませんでした。分岐先の会話を確認してください（再送はしません）。");
  else {
    switch (send.state.kind) {
      case "accepted": out.push("分岐して送信しました。Codex が受け付けました（完了は会話の表示で確認してください）。"); break;
      case "rejected": out.push(`分岐しましたが、送信は受け付けられませんでした（${send.state.message}）。依頼の文は入力欄に残っています。`); break;
      case "acceptanceUnknown": out.push("分岐しましたが、送信の受理を確認できません。再送はしません。分岐先の「履歴と照合」で確認してください。依頼の文は入力欄に残っています。"); break;
      default: out.push("分岐しましたが、送信の状態を確認できていません。分岐先の会話を確認してください（再送はしません）。"); break;
    }
  }
  if (note) out.push(note);
  return out;
}

export function ResendBody({ chat, live, cap, s, h, onClose }: { chat: Chat | undefined; live: boolean; cap: OpCapability | undefined; s: ResendState; h: ResendHandlers; onClose: () => void }) {
  // 開いた時点の判定を保つ（結果の表示中に、状態の変化で画面が差し替わらないようにする。最終判定はホスト）。
  const [plan] = useState(s.plan);
  const [text, setText] = useState(s.requestText ?? "");
  const [editing, setEditing] = useState(s.purpose === "editResend");
  const [running, setRunning] = useState(false);
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<Result | null>(null);
  if (!chat) return <div className="content"><p>チャットを選んでください。</p></div>;
  const name = chatName(chat);
  if (plan.kind === "blocked") {
    return (
      <div className="content">
        <p><b>{name}</b> では、いまは使えません。</p>
        <div className="banner info" role="status"><div>{plan.reason}</div></div>
        <p className="small muted">元の会話は変わりません。何も送っていません。</p>
      </div>
    );
  }
  // 再生成で文を編集に切り替えたら、目的は「編集」になる。
  const purpose: Purpose = editing ? "editResend" : s.purpose;
  const first = plan.kind === "first";
  const finished = result?.finished === true;
  const unknownAt = result?.unknownAt ?? null;
  const cannot = running || finished || unknownAt !== null || text.trim() === "";
  const guard = (): boolean => {
    if (live) return true;
    setResult({ tone: "err", lines: ["モックでは分岐も送信もしません（画面の確認用です）。"], unknownAt: null, finished: false });
    return false;
  };

  const runFork = async (send: boolean) => {
    if (plan.kind !== "fork" || cannot || !guard()) return;
    setRunning(true);
    const startedAt = Date.now();
    try {
      const r = await host.resendAsFork(chat.key, plan.throughTurn, plan.targetTurn, text, purpose, send);
      const ack = r.fork.ack;
      if (ack.kind === "rejected") {
        setResult({ tone: "err", lines: [`分岐は受け付けられませんでした（${ack.message}）。何も送っていません。`], unknownAt: null, finished: false });
      } else if (ack.kind === "unknown") {
        setResult({ tone: "err", lines: [`分岐の受理を確認できません（${ack.message}）。再送はしません。送信も入力欄への入力もしていません。「分岐を確認」で、分岐先ができているかを読取りだけで確認できます。`], unknownAt: r.fork.attemptedAt, finished: false });
      } else if (!r.fork.chat) {
        setResult({ tone: "err", lines: ["分岐を受け付けましたが、分岐先を特定できませんでした。一覧を更新して確認してください。送信はしていません。"], unknownAt: null, finished: true });
      } else {
        const accepted = r.send?.state.kind === "accepted";
        // 送信が受け付けられたときだけ、入力欄を空にしたまま（ホストが下書きを消している）。それ以外は文を入力欄に残す。
        h.forked(r.fork.chat.key, accepted ? null : text, r.send);
        setResult({ tone: "info", lines: forkedLines(r.draftSaved, r.send, send, r.fork.note), unknownAt: null, finished: true });
      }
    } catch (e) {
      // 通信断などで結果が分からない。再送しない。
      setResult({ tone: "err", lines: [`結果を確認できませんでした: ${errText(e)}。再送はしません。「分岐を確認」で、分岐先ができているかを読取りだけで確認できます。`], unknownAt: startedAt, finished: false });
    } finally { setRunning(false); }
  };

  const runFirst = async () => {
    if (cannot || !guard()) return;
    setRunning(true);
    try {
      await h.startFirst(text);
      setResult({ tone: "info", lines: ["新しいチャットを作り、依頼の文を入力欄に入れました。送信は入力欄から行ってください。"], unknownAt: null, finished: true });
    } catch (e) {
      setResult({ tone: "err", lines: [`新しいチャットへ入れられませんでした: ${errText(e)}。送信はしていません。`], unknownAt: null, finished: false });
    } finally { setRunning(false); }
  };

  const check = async () => {
    if (unknownAt === null || !guard()) return;
    setChecking(true);
    try {
      const r = await host.reconcileFork(chat.key, unknownAt);
      if (r.kind !== "adopted") { setResult({ tone: "err", lines: [reconText(r)], unknownAt, finished: false }); return; }
      try {
        await h.saveDraft(r.chat.key, text);
        h.forked(r.chat.key, text, null);
        setResult({ tone: "info", lines: ["分岐先を確認しました。依頼を入力欄に入れました。送信は入力欄から行ってください。"], unknownAt: null, finished: true });
      } catch (e) {
        h.forked(r.chat.key, text, null);
        setResult({ tone: "err", lines: [`分岐先を確認しましたが、入力欄へ保存できませんでした（${errText(e)}）。依頼の文は入力欄に表示しています。送信は入力欄から行ってください。`], unknownAt: null, finished: true });
      }
    } catch (e) { setResult({ tone: "err", lines: [`確認できませんでした: ${errText(e)}`], unknownAt, finished: false }); } finally { setChecking(false); }
  };

  const primary = first ? () => void runFirst() : () => void runFork(true);
  const suffix = purpose === "editResend" ? PURPOSE_SUFFIX.editResend : PURPOSE_SUFFIX.regenerate;
  return (
    <>
    <div className="content resend">
      {first ? (
        <p>これは会話の最初の依頼です。分岐はせず、<b>{name}</b> と同じ種類・作業フォルダ・モデル・権限の新しいチャットを作り、下の文を入力欄へ入れます（送信しません）。元の会話はそのまま残ります。</p>
      ) : (
        <p>元の会話はそのまま残ります。turn {plan.turnNo} の直前までを分岐した新しいチャット「{name}{suffix}」へ送ります。</p>
      )}
      <UnverifiedNote cap={first ? undefined : cap} />
      {editing ? (
        <label className="resend-field">
          <span>依頼の文</span>
          <textarea className="resend-text" rows={6} value={text} disabled={running || finished} aria-label="依頼の文"
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => { if (e.ctrlKey && e.key === "Enter") { e.preventDefault(); primary(); } }} />
        </label>
      ) : (
        <div className="resend-field">
          <span>依頼の文（そのまま送ります）</span>
          <div className="resend-quote" tabIndex={0} role="group" aria-label="依頼の文（読取り）">{text}</div>
          <div><button className="btn-line" disabled={running || finished} onClick={() => setEditing(true)}>文を編集する</button></div>
        </div>
      )}
      <p className="small">元の依頼の添付・Skill指定は引き継ぎません。必要なら新しいチャットで添付し直してください。</p>
      {first ? null : <About><p>分岐は Codex の会話の分岐機能で作られます。元の会話・添付は変わらず、分岐先は AgentDock の一覧に加わります。分岐先の名前は「{name}{suffix}」になり、ヘッダーに「{purpose === "editResend" ? "編集から" : "再生成"}」の印が付きます。</p></About>}
      {result ? <div className={`banner ${result.tone}`} role={result.tone === "err" ? "alert" : "status"}><div>{result.lines.map((l, i) => <p key={i} style={{ margin: i ? "4px 0 0" : 0 }}>{l}</p>)}</div></div> : null}
    </div>
    <FootActions
      left={unknownAt !== null
        ? <button className="btn" disabled={checking} onClick={() => void check()}>{checking ? "確認しています…" : "分岐を確認"}</button>
        : !first && !finished ? <button className="btn" disabled={cannot} onClick={() => void runFork(false)}>分岐して入力欄へ（送らない）</button> : null}
      main={finished
        ? <button className="btn primary" onClick={onClose}>閉じる</button>
        : unknownAt !== null ? null
        : <button className="btn primary" disabled={cannot} onClick={primary}>{running ? "処理しています…" : first ? "新しいチャットの入力欄へ" : "分岐して送信"}</button>} />
    </>
  );
}
