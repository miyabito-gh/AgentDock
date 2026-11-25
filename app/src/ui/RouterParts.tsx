// 受付係（DESIGN_ROUTER §7・§8）の画面部品: 振り分けカード、ルール更新の行、送り先の選択、ルール帳の本文。
// 規則: AIは決めるだけ。送るのは利用者が［送る］を押したときの1回。失敗・受理不明でも自動で再送しない（再送ボタンも置かない）。
//       状態はホストの RouterView が正本。操作のあとは取り直す（routerUpdated の合図でも取り直される）。
import { useEffect, useMemo, useState } from "react";
import * as host from "../ipc/client";
import type { Chat, ChatKey, ChatLocalView, DispatchView, RouterView, RouterTurnView, RuleChangeView, RuleOp, RuleTargetInput, RulesView, RuleView } from "../ipc/types";
import { askConfirm } from "./appConfirm";
import { chatName, sortChats } from "./derive";
import { knownValue } from "./format";
import { baseText, canToggleOriginal, chipNeedsAttention, dispatchChip, dispatchesOfTurn, foldedLine, noticeView, pickableChats, proposedProvider, providerLabel, resultBody, targetLabel } from "./router";
import { Icon } from "./Icon";

const errText = (e: unknown) => host.asIpcError(e).message;

/** 中央ペインの受付係の画面へ渡す材料と操作。 */
export interface RouterCtx {
  view: RouterView;
  chats: Chat[];
  locals: ChatLocalView[];
  /** カード・ルールの状態を取り直す（読み取りのみ）。 */
  refresh: () => void;
  /** ［始める］: 新規チャット作成のダイアログを、案の値を入れた状態で開く（作成と送信は既存の経路）。 */
  startNew: (d: DispatchView, text: string) => void;
  openChat: (key: ChatKey) => void;
}

const folderOf = (c: Chat): string | null => knownValue(c.cwd) ?? null;
const ruleDate = (ms: number) => { const d = new Date(ms); const p = (n: number) => String(n).padStart(2, "0"); return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`; };

/** ルール変更の取り消し。その後の変更があるときはホストが拒否するので、確認のうえ force で1回だけやり直す。失敗の文（成功は null）を返す。 */
export async function undoRule(revision: number): Promise<string | null> {
  try {
    await host.routerRuleUndo(revision);
    return null;
  } catch (e) {
    const er = host.asIpcError(e);
    if (er.blocked?.kind !== "routerRuleUndoNeedsConfirm") return `取り消せませんでした: ${er.message}`;
  }
  const ok = await askConfirm({ title: "ルールの変更を取り消しますか", body: "この変更のあとに、同じルールがさらに変更されています。\n取り消すと、そのルールをこの変更の前の状態へ戻します。", okLabel: "取り消す", cancelLabel: "やめる" });
  if (!ok) return null;
  try { await host.routerRuleUndo(revision, true); return null; } catch (e) { return `取り消せませんでした: ${errText(e)}`; }
}

// ───────────────────────────── 送り先の選択 ─────────────────────────────

/** 全チャットから検索して選ぶ（アーカイブ・組織所有・引き継ぎ済み・受付係自身は除く。最終の判定はホスト）。 */
export function ChatPicker({ chats, locals, routerKey, onPick, onCancel }: { chats: Chat[]; locals: ChatLocalView[]; routerKey: ChatKey | null; onPick: (c: Chat) => void; onCancel?: () => void }) {
  const [q, setQ] = useState("");
  const list = useMemo(
    () => pickableChats(sortChats(chats), (c) => locals.find((l) => l.chat.id === c.key.id), routerKey, chatName, folderOf, q),
    [chats, locals, routerKey, q]);
  const MAX = 40;
  return (
    <div className="rpicker">
      <div className="rpicker-q">
        <input type="search" className="grow" autoFocus value={q} onChange={(e) => setQ(e.target.value)} placeholder="名前・フォルダで探す" aria-label="送り先の会話を探す" />
        {onCancel ? <button type="button" className="btn sm" onClick={onCancel}>やめる</button> : null}
      </div>
      <ul className="rpicker-list" aria-label="会話の一覧">
        {list.slice(0, MAX).map((c) => (
          <li key={c.key.id}>
            <button type="button" onClick={() => onPick(c)} title={folderOf(c) ?? "一般チャット"}>
              <span className="nm">{chatName(c)}</span>
              {c.key.backend === "antigravity" ? <span className="tag" title="Antigravity（Gemini）の会話">Gemini</span> : null}
              <span className="small muted fl">{folderOf(c) ?? "一般"}</span>
            </button>
          </li>
        ))}
        {list.length === 0 ? <li className="small muted rpicker-none">一致する会話はありません</li> : null}
        {list.length > MAX ? <li className="small muted rpicker-none">ほかに {list.length - MAX} 件あります。語を足して絞ってください。</li> : null}
      </ul>
    </div>
  );
}

// ───────────────────────────── 振り分けカード ─────────────────────────────

function DispatchCard({ d, original, ctx }: { d: DispatchView; original: string | null; ctx: RouterCtx }) {
  const [useOrig, setUseOrig] = useState(false);
  const [text, setText] = useState(() => baseText(d, false, original));
  const [picking, setPicking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const folded = foldedLine(d);
  if (folded) {
    const key = d.target.chat;
    // 状態は色だけでなく、形＋文字の印（.st）でも示す。失敗・受理不明・要確認は枠も警告色（緑の「送りました」に埋もれさせない）
    const chip = dispatchChip(d);
    const kind = chipNeedsAttention(chip) ? "warn" : folded.kind;
    return (
      <div className={`rcard folded ${kind}`}>
        <div className="grow"><div className="rline">{chip ? <span className={`st ${chip.tone}`}><Icon name={chip.icon} />{chip.label}</span> : null}<span>{folded.text}</span></div>{d.executionResult && <div style={{ whiteSpace: "pre-wrap" }}>{resultBody(d.executionResult)}</div>}<small>受付 {d.requestId ?? d.sourceTurn} ／ 振り分け {d.id}{d.executionTurn && ` ／ 担当 ${d.executionTurn}`}</small></div>
        {folded.canOpen && key ? <button type="button" className="btn sm" onClick={() => ctx.openChat(key)}>開く</button> : null}
      </div>
    );
  }
  const isNew = d.target.kind === "new" && d.target.chat === null;
  const provider = proposedProvider(d);
  const p = d.proposal;
  const empty = !text.trim();
  const toggle = (next: boolean) => { setUseOrig(next); setText(baseText(d, next, original)); };
  /** 送る（利用者が押した1回）。失敗しても自動では再送しない。 */
  const send = async (override: ChatKey | null) => {
    if (busy || empty) return;
    setBusy(true); setErr(null);
    try { await host.routerDispatch(d.id, text, override); setPicking(false); }
    catch (e) { setErr(`送れませんでした: ${errText(e)}`); }
    finally { setBusy(false); ctx.refresh(); }
  };
  const dismiss = async () => {
    if (busy) return;
    setBusy(true); setErr(null);
    try { await host.routerDismiss(d.id); } catch (e) { setErr(`見送れませんでした: ${errText(e)}`); } finally { setBusy(false); ctx.refresh(); }
  };
  return (
    <div className="rcard">
      <div className="rcard-h"><span className="tag ai">{isNew ? "新規" : "送り先"}</span><b className="grow nm">{targetLabel(d)}</b>{provider ? <span className="tag" title="このチャットを担当するAI">AI: {providerLabel(provider)}</span> : null}</div>
      <small className="muted">受付 {d.requestId ?? d.sourceTurn} ／ 振り分け {d.id}{d.target.chat && ` ／ 送り先ID ${d.target.chat.id}`}</small>
      {p?.reason ? <div className="small muted">{p.reason}</div> : null}
      {p?.confidence === "low" ? <div className="small rwarn">この案はあまり自信がありません。送り先をよく確認してください。</div> : null}
      {isNew && p?.newChat?.purpose ? <div className="small muted">目的メモの案: {p.newChat.purpose}</div> : null}
      {canToggleOriginal(d, original) ? (
        <div className="rseg" role="group" aria-label="送る文">
          <button type="button" className={useOrig ? "" : "on"} aria-pressed={!useOrig} onClick={() => toggle(false)}>整えた文</button>
          <button type="button" className={useOrig ? "on" : ""} aria-pressed={useOrig} onClick={() => toggle(true)}>原文</button>
        </div>
      ) : null}
      <textarea value={text} onChange={(e) => setText(e.target.value)} rows={3} aria-label="送る文" />
      {!isNew && d.targetRunning && !d.targetProblem ? <div className="small muted">実行中です。順番待ちとして送ります</div> : null}
      {d.targetProblem ? <div className="small rwarn" role="alert">{d.targetProblem}</div> : null}
      {err ? <div className="small why err" role="alert">{err}</div> : null}
      <div className="rcard-acts">
        {isNew
          ? <button type="button" className="btn primary sm" disabled={busy || empty} title="新しいチャットの作成画面を、この案の値を入れて開きます（作成と送信はそこで押したときだけ）" onClick={() => ctx.startNew(d, text)}>始める</button>
          : <button type="button" className="btn primary sm" disabled={busy || empty || !!d.targetProblem} onClick={() => void send(d.target.kind === "new" ? d.target.chat : null)}>送る</button>}
        <button type="button" className="btn sm" disabled={busy} aria-expanded={picking} onClick={() => setPicking((v) => !v)}>別の会話へ</button>
        <button type="button" className="btn sm" disabled={busy} onClick={() => void dismiss()}>見送る</button>
      </div>
      {picking ? (
        <ChatPicker chats={ctx.chats} locals={ctx.locals} routerKey={ctx.view.chat} onCancel={() => setPicking(false)}
          onPick={(c) => { if (!empty) void send(c.key); }} />
      ) : null}
    </div>
  );
}

/** 受付係の返答（turn）の下: 振り分けカード、ルール更新の行、落とした案と解析失敗の静かな行。 */
export function RouterTurnExtras({ tv, original, ctx }: { tv: RouterTurnView | null; original: string | null; ctx: RouterCtx }) {
  const [busyRev, setBusyRev] = useState<number | null>(null);
  const [err, setErr] = useState<string | null>(null);
  if (!tv) return null;
  const cards = dispatchesOfTurn(ctx.view, tv.turn);
  const undo = async (rev: number) => {
    if (busyRev !== null) return;
    setBusyRev(rev); setErr(null);
    const m = await undoRule(rev);
    setBusyRev(null);
    if (m) setErr(m);
    ctx.refresh();
  };
  return (
    <div className="rextra">
      {tv.parseError ? <div className="small muted rquiet">振り分け案を読み取れませんでした</div> : null}
      {cards.map((d) => <DispatchCard key={d.id} d={d} original={original} ctx={ctx} />)}
      {tv.notices.map((n, i) => {
        const v = noticeView(n);
        return (
          <div key={`${n.revision ?? "x"}:${i}`} className={`small rnotice${v.undone || n.revision === null ? " muted" : ""}`}>
            <span>{v.text}</span>
            {v.canUndo && n.revision !== null ? <button type="button" className="btn-text" disabled={busyRev !== null} onClick={() => void undo(n.revision as number)}>取り消す</button> : null}
          </div>
        );
      })}
      {tv.routeRejects.map((r, i) => <div key={`rj${i}`} className="small muted rquiet">振り分け案を出さなかった理由: {r}</div>)}
      {err ? <div className="small why err" role="alert">{err}</div> : null}
    </div>
  );
}

// ───────────────────────────── ルール帳 ─────────────────────────────

type TargetMode = "keep" | "none" | "chat" | "newIn";

function RuleForm({ initial, chats, locals, routerKey, onSave, onCancel }: {
  /** null＝新しく追加。 */
  initial: RuleView | null; chats: Chat[]; locals: ChatLocalView[]; routerKey: ChatKey | null;
  onSave: (text: string, target: RuleTargetInput | null) => Promise<string | null>; onCancel: () => void;
}) {
  const adding = initial === null;
  const [text, setText] = useState(initial?.rule.text ?? "");
  const [mode, setMode] = useState<TargetMode>(adding ? "none" : "keep");
  const [chat, setChat] = useState<Chat | null>(null);
  const [folder, setFolder] = useState(initial?.rule.target?.kind === "newIn" ? initial.rule.target.folder : "");
  const [picking, setPicking] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const target = (): RuleTargetInput | null => (mode === "chat" && chat ? { kind: "chat", chat: chat.key } : mode === "newIn" ? { kind: "newIn", folder: folder.trim() } : null);
  const blocked = !text.trim() ? "ルールの文を入れてください" : mode === "chat" && !chat ? "送り先の会話を選んでください" : mode === "newIn" && !folder.trim() ? "フォルダを入れてください" : null;
  const save = async () => {
    if (blocked || busy) return;
    setBusy(true); setErr(null);
    const m = await onSave(text.trim(), target());
    setBusy(false);
    if (m) setErr(m);
  };
  const current = initial ? (initial.rule.target?.kind === "chat" ? `いまの送り先: ${initial.targetTitle ?? "送り先なし"}` : initial.rule.target?.kind === "newIn" ? `いまの送り先: 新規（${initial.rule.target.folder}）` : "いまの送り先: なし（条件だけ）") : null;
  return (
    <div className="rform">
      <label className="small">ルール（利用者の言葉で、短く）<textarea value={text} onChange={(e) => setText(e.target.value)} rows={2} /></label>
      <div className="small">
        <span className="muted">送り先: </span>
        {!adding ? <label><input type="radio" name="rtm" checked={mode === "keep"} onChange={() => setMode("keep")} /> 変えない</label> : null}{" "}
        <label><input type="radio" name="rtm" checked={mode === "none"} onChange={() => setMode("none")} /> なし（条件だけ）</label>{" "}
        <label><input type="radio" name="rtm" checked={mode === "chat"} onChange={() => setMode("chat")} /> 会話</label>{" "}
        <label><input type="radio" name="rtm" checked={mode === "newIn"} onChange={() => setMode("newIn")} /> 新規（フォルダ）</label>
      </div>
      {current && mode === "keep" ? <div className="small muted">{current}</div> : null}
      {mode === "chat" ? (
        <div className="small">
          {chat ? <span>選んだ会話: <b>{chatName(chat)}</b>　</span> : <span className="muted">会話を選んでいません　</span>}
          <button type="button" className="btn sm" onClick={() => setPicking((v) => !v)} aria-expanded={picking}>{chat ? "選び直す" : "選ぶ"}</button>
          {picking ? <ChatPicker chats={chats} locals={locals} routerKey={routerKey} onCancel={() => setPicking(false)} onPick={(c) => { setChat(c); setPicking(false); }} /> : null}
        </div>
      ) : null}
      {mode === "newIn" ? <label className="small">作業フォルダ（新しいチャットの置き場所。フルパス）<input type="text" className="mono" value={folder} onChange={(e) => setFolder(e.target.value)} placeholder="例: C:\work\minutes" /></label> : null}
      {err ? <div className="small why err" role="alert">{err}</div> : null}
      <div className="rcard-acts">
        <button type="button" className="btn primary sm" disabled={!!blocked || busy} title={blocked ?? undefined} onClick={() => void save()}>{adding ? "追加" : "保存"}</button>
        <button type="button" className="btn sm" disabled={busy} onClick={onCancel}>やめる</button>
      </div>
    </div>
  );
}

function ruleTargetText(r: RuleView): string {
  const t = r.rule.target;
  if (!t) return "送り先: なし（条件だけ）";
  if (t.kind === "newIn") return `送り先: 新規（${t.folder}）`;
  return `送り先: ${r.targetTitle ?? "送り先なし"}`;
}

/** ルール帳の本文（外枠は Dialogs の Shell）。一覧・手での追加／編集／削除・履歴からの取り消し。手での編集の出所は「手動」。 */
export function RouterRulesBody({ chats, locals, routerKey, live }: { chats: Chat[]; locals: ChatLocalView[]; routerKey: ChatKey | null; live: boolean }) {
  const [rv, setRv] = useState<RulesView | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [form, setForm] = useState<{ id: string | null } | null>(null);
  const [note, setNote] = useState<string | null>(null);
  useEffect(() => {
    if (!live) return;
    let alive = true;
    const load = () => host.routerRules().then((v) => { if (alive) { setRv(v); setErr(null); } }, (e) => { if (alive) setErr(`ルール帳を読み込めませんでした: ${errText(e)}`); });
    void load();
    const un = host.subscribeHostEvents((e) => { if (e.event.kind === "routerUpdated") void load(); });
    return () => { alive = false; void un.then((f) => f()); };
  }, [live]);
  const edit = async (op: RuleOp, ruleId: string | null, text: string, target: RuleTargetInput | null): Promise<string | null> => {
    try { setRv(await host.routerRuleEdit({ op, ruleId, text, target })); setNote(null); return null; } catch (e) { return errText(e); }
  };
  const remove = async (r: RuleView) => {
    if (!(await askConfirm({ title: "ルールを削除しますか", body: `「${r.rule.text}」\n\n削除しても、下の履歴から取り消せます。`, okLabel: "削除する", danger: true }))) return;
    const m = await edit("remove", r.rule.id, "", null);
    if (m) setNote(`削除できませんでした: ${m}`);
  };
  const [busyRev, setBusyRev] = useState<number | null>(null);
  const undo = async (c: RuleChangeView) => {
    if (busyRev !== null) return;
    setBusyRev(c.change.revision);
    const m = await undoRule(c.change.revision);
    setBusyRev(null);
    setNote(m);
    try { setRv(await host.routerRules()); } catch { /* 合図で取り直される */ }
  };
  if (!live) return <div className="content"><p className="muted">モックでは動きません。</p></div>;
  const editing = form && form.id !== null ? rv?.rules.find((r) => r.rule.id === form.id) ?? null : null;
  return (
    <div className="content">
      {rv?.warning ? <div className="gbanner warn" role="status"><span className="grow">{rv.warning}</span></div> : null}
      {err ? <div className="why err" role="alert">{err}</div> : null}
      {note ? <div className="why err" role="alert">{note}</div> : null}
      <p className="small muted">受付係が振り分けに使うルールです。会話の中で「今後は…」と伝えても更新されます。手で直した内容は「手動」として残ります。</p>
      <h4 className="rh">ルール{rv ? `（${rv.rules.length}件）` : ""}</h4>
      {rv && rv.rules.length === 0 ? <div className="small muted">まだルールはありません</div> : null}
      <ul className="rrules">
        {rv?.rules.map((r) => (
          <li key={r.rule.id}>
            {form?.id === r.rule.id && editing
              ? <RuleForm initial={editing} chats={chats} locals={locals} routerKey={routerKey} onCancel={() => setForm(null)} onSave={async (t, tg) => { const m = await edit("update", r.rule.id, t, tg); if (!m) setForm(null); return m; }} />
              : (
                <div className="rrule">
                  <div className="grow"><div>{r.rule.text}</div><div className="small muted">{ruleTargetText(r)}　{r.rule.source.kind === "manual" ? "手動" : "会話から"}</div></div>
                  <button type="button" className="btn sm" disabled={form !== null} onClick={() => setForm({ id: r.rule.id })}>編集</button>
                  <button type="button" className="btn sm" disabled={form !== null} onClick={() => void remove(r)}>削除</button>
                </div>
              )}
          </li>
        ))}
      </ul>
      {form && form.id === null
        ? <RuleForm initial={null} chats={chats} locals={locals} routerKey={routerKey} onCancel={() => setForm(null)} onSave={async (t, tg) => { const m = await edit("add", null, t, tg); if (!m) setForm(null); return m; }} />
        : <button type="button" className="btn sm" disabled={form !== null} onClick={() => setForm({ id: null })}>ルールを追加…</button>}
      <h4 className="rh">履歴</h4>
      {rv && rv.history.length === 0 ? <div className="small muted">まだ履歴はありません</div> : null}
      <ul className="rhist">
        {rv?.history.map((c) => (
          <li key={c.change.revision} className="small">
            <span className="muted">{ruleDate(c.change.at)}</span>
            <span className="grow">{c.summary}{c.change.undone ? "（取り消し済み）" : ""}</span>
            {!c.change.undone && c.change.undoes === undefined ? <button type="button" className="btn-text" disabled={busyRev !== null} onClick={() => void undo(c)}>取り消す</button> : null}
          </li>
        ))}
      </ul>
    </div>
  );
}
