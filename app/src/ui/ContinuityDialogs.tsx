// 継続チャット（DESIGN_CONT §7）のダイアログ本文: 目的メモの編集、引き継ぎ資料の確認、設定「継続チャット」。外枠（Shell）は Dialogs.tsx 側。
// 規則: 資料の確認は読取りだけ（何も送らない）。目的メモの保存は「保存」を押したときの1回。未取得・失敗を成功に見せない。
//       文言・判定は contUsage.ts（純関数）。Esc・IME の規則は Shell が持つ（変換中は閉じない）。
import { useEffect, useMemo, useRef, useState } from "react";
import * as host from "../ipc/client";
import type { ChatKey, ChatLocalView, ContinuationSettings, DraftProposal, PurposeDraftEstimate, PurposeDraftView, PurposeNoteView } from "../ipc/types";
import type { HandoffPreview } from "../ipc/types";
import { FootActions } from "./DialogParts";
import { copyText } from "./markdown/parts";
import { PURPOSE_MAX, charCount, ledgerText, observedText, purposeSaveBlock, statsText } from "./contUsage";
import { KIND_LABELS, KIND_ORDER, actionText, assembleMemo, cite, diffMark, elapsedText, estimateText, issueText, lineDiff, needsReview, stateHeadline } from "./purposeDraft";

const errText = (e: unknown) => host.asIpcError(e).message;

// ───────────────────────────── 目的メモ ─────────────────────────────

/** 目的メモの編集。空にして保存＝削除。`save` は失敗の文（成功なら null）を返し、成功したら `onDone` で閉じる。 */
export function PurposeBody({ chat, chatName, current, live, orgOwned, save, onDone }: {
  /** 更新案の作成に使う会話（先頭のチャット）。 */
  chat: ChatKey | null;
  chatName: string;
  current: PurposeNoteView | null;
  live: boolean;
  orgOwned: boolean;
  /** 保存する。expectedRevision は開いた時点の版（別の画面で変えられていたら拒否される）。 */
  save: (text: string, expectedRevision: number | null) => Promise<string | null>;
  onDone: () => void;
}) {
  const original = current?.text ?? "";
  // 開いた時点の版を固定する（開いている間に他の画面で変わっても、上書きせず拒否される側に倒す）
  const rev = useRef<number | null>(current?.revision ?? null);
  const [text, setText] = useState(original);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [view, setView] = useState<"edit" | "draft">("edit");
  const [ask, setAsk] = useState(false);
  const ta = useRef<HTMLTextAreaElement>(null);
  useEffect(() => { if (view === "edit") ta.current?.focus(); }, [view]);
  const n = charCount(text);
  const block = purposeSaveBlock(text, original, orgOwned);
  const go = async () => {
    if (block || busy) return;
    setBusy(true); setErr(null);
    try {
      const m = await save(text, rev.current);
      if (m === null) onDone(); else setErr(m);
    } catch (e) { setErr(errText(e)); } finally { setBusy(false); }
  };
  // 更新案の作成は未保存の編集を持ち込まない（保存するか破棄してから）。作成は押したときだけで、自動では始めない。
  const dirty = text !== original;
  const openDraft = () => { if (dirty) setAsk(true); else setView("draft"); };
  const saveThenDraft = async () => {
    if (block || busy) return;
    setBusy(true); setErr(null);
    try {
      const m = await save(text, rev.current);
      if (m === null) { setAsk(false); setView("draft"); } else setErr(m);
    } catch (e) { setErr(errText(e)); } finally { setBusy(false); }
  };
  const discardThenDraft = () => { setText(original); setAsk(false); setView("draft"); };
  if (view === "draft" && chat) {
    return <PurposeDraftBody chat={chat} chatName={chatName} current={current} live={live} save={save} onBack={() => setView("edit")} onDone={onDone} />;
  }
  const main = <button className="btn primary" disabled={!live || !!block || busy} title={!live ? "モックでは保存できません" : block ?? undefined} onClick={() => void go()}>{busy ? "保存しています…" : text.trim() === "" && original !== "" ? "削除して保存" : "保存"}</button>;
  return (
    <div className="content">
      <FootActions main={main} />
      <p className="small muted">対象: {chatName}</p>
      <div className="field"><span>目的メモ</span>
        <textarea ref={ta} rows={8} style={{ width: "100%" }} value={text} disabled={orgOwned || busy} aria-label="目的メモ" aria-describedby="purpose-count"
          onChange={(e) => setText(e.target.value)} placeholder="この会話の目的・前提・守ってほしいことなど" />
        <span className={`note ${n > PURPOSE_MAX ? "over" : ""}`} id="purpose-count">{n.toLocaleString("en-US")} / {PURPOSE_MAX.toLocaleString("en-US")} 字{n > PURPOSE_MAX ? "（超えています。保存できません）" : ""}</span>
      </div>
      {chat && !orgOwned ? (
        <div className="acts">
          <button className="btn" disabled={!live || busy} title={!live ? "モックでは動きません" : undefined} onClick={openDraft}>会話から更新案を作る…</button>
          <span className="small muted">会話の内容から、目的メモへの追加・修正の案を作ります（押したときだけ。保存は確認のあと）</span>
        </div>
      ) : null}
      {ask ? (
        <div className="why" role="alert">
          <b>未保存の編集があります。</b>更新案は保存済みの目的メモを元に作ります。
          <div className="acts">
            <button className="btn" disabled={!!block || busy} title={block ?? undefined} onClick={() => void saveThenDraft()}>保存してから作る</button>
            <button className="btn" disabled={busy} onClick={discardThenDraft}>破棄して作る</button>
            <button className="btn" disabled={busy} onClick={() => setAsk(false)}>やめる</button>
          </div>
        </div>
      ) : null}
      <p className="small muted">この会話の送信に資料として添えます（この thread で最初の送信・メモを変更した後・文脈の圧縮を観測した後の、手動の送信のとき）。キューや追加指示には添えません。空にして保存すると、目的メモを削除します（記録は残ります）。</p>
      {orgOwned ? <div className="why">組織実行が所有するチャットでは、目的メモを編集できません。</div> : null}
      {current ? <p className="small muted">現在の版: {current.revision}{observedText(current.updatedAt) ? `（${observedText(current.updatedAt)} に保存）` : ""}</p> : null}
      {err ? <div className="why" role="alert">{err}</div> : null}
    </div>
  );
}

// ───────────────────────────── 目的メモの更新案（DESIGN_CONT_DRAFT §4） ─────────────────────────────

const ACTIVE = new Set(["preparing", "running", "acceptanceUnknown"]);

/**
 * 更新案の作成と確認。作成は「作成する」を押したときの1回だけ（自動実行・自動再送なし）。結果は常に「確認待ち／要確認」で、
 * 保存は利用者が採用を選んで「保存」を押したときだけ（既存の目的メモの保存。版が違えば拒否される）。
 */
export function PurposeDraftBody({ chat, chatName, current, live, save, onBack, onDone }: {
  chat: ChatKey;
  chatName: string;
  current: PurposeNoteView | null;
  live: boolean;
  save: (text: string, expectedRevision: number | null) => Promise<string | null>;
  /** 目的メモ編集から開いたときだけ渡す（直接開いたときは「編集に戻る」を出さない）。 */
  onBack?: () => void;
  onDone: () => void;
}) {
  const [view, setView] = useState<PurposeDraftView | null | undefined>(undefined);
  const [est, setEst] = useState<PurposeDraftEstimate | null>(null);
  const [headOnly, setHeadOnly] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [picks, setPicks] = useState<Set<number>>(new Set());
  const [manual, setManual] = useState<string | null>(null);
  const [glance, setGlance] = useState<number | null>(null);
  const [now, setNow] = useState(Date.now());
  const [tick, setTick] = useState(0);
  const memo = current?.text ?? "";
  const rev = current?.revision ?? null;

  // 最後の結果の読込みと、更新の合図での取り直し
  useEffect(() => {
    if (!live) return;
    let alive = true;
    const load = () => host.readPurposeDraft(chat).then((v) => { if (alive) setView(v); }, (e) => { if (alive) { setView(null); setErr(errText(e)); } });
    void load();
    const un = host.subscribeHostEvents((e) => { if (e.event.kind === "purposeDraftUpdated" && e.event.chat.id === chat.id) void load(); });
    return () => { alive = false; void un.then((f) => f()); };
  }, [chat.id, live]);
  const state = view?.state;
  const active = !!state && ACTIVE.has(state.kind);
  const loaded = view !== undefined;
  // 作成前の見積り（読取りのみ）。作成中は取らない
  useEffect(() => {
    if (!live || !loaded || active) return;
    let alive = true;
    host.previewPurposeDraft(chat, rev, headOnly).then((e) => { if (alive) setEst(e); }, (e) => { if (alive) { setEst(null); setErr(errText(e)); } });
    return () => { alive = false; };
  }, [chat.id, live, loaded, active, headOnly, rev, tick]);
  // 新しい結果が来たら採用の既定に戻す
  const viewId = view?.id;
  useEffect(() => {
    if (view && view.state.kind === "ready") setPicks(new Set(view.state.proposals.filter((p) => p.defaultPick).map((p) => p.index)));
    else setPicks(new Set());
    setManual(null); setGlance(null);
    // 結果の入れ替わり（id・種類）のときだけ
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewId, state?.kind]);
  useEffect(() => {
    if (state?.kind !== "running") return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [state?.kind]);

  const proposals: DraftProposal[] = state?.kind === "ready" ? state.proposals : [];
  const mode = view?.mode ?? "global";
  const built = useMemo(() => assembleMemo(memo, proposals, picks, mode), [memo, proposals, picks, mode]);
  const text = manual ?? built.text;
  const n = charCount(text);
  const stale = !!view && state?.kind === "ready" && view.baseRevision !== rev;

  const run = async (f: () => Promise<void>) => {
    if (busy) return;
    setBusy(true); setErr(null);
    try { await f(); } catch (e) { setErr(errText(e)); } finally { setBusy(false); }
  };
  const start = (ho: boolean) => run(async () => { setView(await host.startPurposeDraft(chat, rev, ho)); });
  const cancel = () => run(async () => { setView(await host.cancelPurposeDraft(chat)); });
  const discard = () => run(async () => { await host.discardPurposeDraft(chat); setView(null); setTick((t) => t + 1); });
  const saveBlock = !live ? "モックでは保存できません" : stale ? "目的メモが作成後に変わりました" : n > PURPOSE_MAX ? `${PURPOSE_MAX.toLocaleString("en-US")} 字を超えています` : text === memo ? "変更がありません" : null;
  const doSave = () => run(async () => {
    if (saveBlock || !view) return;
    const m = await save(text, view.baseRevision);
    if (m === null) onDone(); else setErr(m);
  });

  const head = state ? stateHeadline(state) : null;
  const main = state?.kind === "ready"
    ? <button className="btn primary" disabled={!!saveBlock || busy} title={saveBlock ?? undefined} onClick={() => void doSave()}>{busy ? "保存しています…" : "この内容で保存"}</button>
    : undefined;
  if (!live) return <div className="content"><p>この操作はモックでは動きません。</p></div>;
  const toggle = (i: number) => setPicks((s) => { const x = new Set(s); if (x.has(i)) x.delete(i); else x.add(i); return x; });
  const g = glance === null ? null : view?.glances.find((x) => x.number === glance) ?? null;
  const item = (p: DraftProposal) => (
    <li key={p.index} className="pd-item">
      <label>
        <input type="checkbox" checked={picks.has(p.index)} disabled={p.action === null || p.kind === null} onChange={() => toggle(p.index)} />{" "}
        <b>{actionText(p.action)}</b>{" "}
        {p.action === "replace" ? <><span className="muted">現在: {p.replaces}</span> → {p.text}</> : p.action === "remove" ? <s>{p.replaces}</s> : p.text}
      </label>
      <div className="small">
        {p.sources.map((s) => <button key={s} className="linkish" onClick={() => setGlance(glance === s ? null : s)} aria-expanded={glance === s}>{cite(s, mode)}</button>)}
        {p.issues.map((i) => <span key={i} className={`pd-issue ${needsReview(i) ? "review" : ""}`}>{needsReview(i) ? "要確認: " : ""}{issueText(i)}</span>)}
        {p.note ? <span className="muted">{p.note}</span> : null}
      </div>
    </li>
  );
  const diff = state?.kind === "ready" ? lineDiff(memo, text) : [];

  return (
    <div className="content">
      <FootActions left={onBack ? <button className="btn" disabled={busy} onClick={onBack}>編集に戻る</button> : undefined} main={main} />
      <p className="small muted">対象: {chatName}。会話とは別の一時 thread で、読み取り専用（承認を求めない）で作ります。会話本体には何も送りません。目的メモは、あなたが保存するまで変わりません。</p>
      {head ? <div className={head.tone === "ok" || head.tone === "run" ? "" : "why"} role="status"><b>{head.text}</b>{state?.kind === "running" ? `（経過 ${elapsedText(state.startedAt, now)}）` : ""}</div> : null}
      {view && state?.kind !== "preparing" ? (
        <p className="small muted">
          {view.excludedRunning ? "実行中の往復は含めていません。" : ""}
          {view.mode === "headOnly" ? "今の thread だけで作っています（出典は [今#n]）。" : ""}
          {view.usageTokens !== null ? ` 使用: ${view.usageTokens.toLocaleString("en-US")} トークン` : " 使用量: 未取得"}
        </p>
      ) : null}
      {state?.kind === "running" ? <div className="acts"><button className="btn" disabled={busy} onClick={() => void cancel()}>中止</button></div> : null}
      {state?.kind === "acceptanceUnknown" ? <div className="acts"><button className="btn" disabled={busy} onClick={() => void discard()}>この表示を閉じる（結果が届いても表示しません）</button></div> : null}

      {state?.kind === "ready" ? (
        <>
          {stale ? <div className="why" role="alert">目的メモが作成後に変わりました。この結果は保存できません。<button className="btn" disabled={busy} onClick={() => void discard()}>結果を破棄</button></div> : null}
          {state.dropped > 0 ? <p className="small muted">提案が多すぎたため、{state.dropped} 件を捨てました。</p> : null}
          {state.omitted ? <p className="small muted">モデルが省いたもの: {state.omitted}</p> : null}
          {proposals.length === 0 ? <p className="muted">提案はありませんでした。</p> : null}
          {[...KIND_ORDER, null].map((k) => {
            const list = proposals.filter((p) => p.kind === k);
            return list.length === 0 ? null : (
              <div key={k ?? "none"} className="field"><span>{k === null ? "種類が不明" : KIND_LABELS[k]}</span><ul className="pd-list">{list.map(item)}</ul></div>
            );
          })}
          {glance !== null ? (
            <div className="pd-glance" role="region" aria-label={`往復${glance}の抜粋`}>
              {g ? <><b>{cite(glance, mode)}</b> {g.time ?? "日時不明"}<br />依頼: {g.userHead}<br />応答: {g.aiHead}</> : <span className="muted">{cite(glance, mode)} の抜粋は返されませんでした（台帳で確かめてください）。</span>}
            </div>
          ) : null}
          {built.skipped.length > 0 && manual === null ? <p className="why small" role="status">対象の行が一意に見つからないなどで、適用できなかった提案があります（{built.skipped.length} 件）。</p> : null}
          <div className="field"><span>保存されるメモ</span>
            <textarea rows={8} style={{ width: "100%" }} value={text} aria-label="保存される目的メモ" onChange={(e) => setManual(e.target.value)} />
            <span className={`note ${n > PURPOSE_MAX ? "over" : ""}`}>{n.toLocaleString("en-US")} / {PURPOSE_MAX.toLocaleString("en-US")} 字{n > PURPOSE_MAX ? "（超えています。保存できません。採用を外してください）" : ""}</span>
            {manual !== null ? <span className="note">手で直しています（採用の選択は反映されません）。 <button className="btn" onClick={() => setManual(null)}>チェックから組み立て直す</button></span> : null}
          </div>
          <div className="field"><span>現在のメモとの差分</span>
            <pre className="mono cont-env pd-diff" aria-label="現在のメモとの差分">{diff.length === 0 ? "（差分なし）" : diff.map((d, i) => <div key={i} className={`pd-${d.kind}`}>{diffMark(d.kind)} {d.text}</div>)}</pre>
          </div>
        </>
      ) : null}

      {state?.kind === "unreadable" ? (
        <>
          <p className="small muted">項目にできなかったため、受け取った本文をそのまま示します（保存はできません）。</p>
          <pre className="mono cont-env" aria-label="受け取った本文">{state.raw || "（本文なし）"}</pre>
          {state.raw ? <div className="acts"><button className="btn" onClick={() => void copyText(state.raw)}>コピー</button></div> : null}
        </>
      ) : null}

      {!active && loaded ? (
        <div className="field"><span>{view ? "もう一度作る" : "更新案を作る"}</span>
          {est ? (
            <>
              <p className="small">{estimateText(est)}</p>
              {est.excludedRunning ? <p className="small muted">実行中の往復は含めません。</p> : null}
              {est.notes.map((t, i) => <p key={i} className="small muted">{t}</p>)}
              {est.blockers.length > 0 ? <div className="why" role="alert"><ul>{est.blockers.map((b, i) => <li key={i}>{b}</li>)}</ul></div> : null}
              <div className="acts">
                <button className="btn primary" disabled={busy || est.blockers.length > 0} onClick={() => void start(headOnly)}>{headOnly ? "今の thread だけで作成する" : "作成する"}</button>
                {est.headOnlyAvailable && !headOnly ? <button className="btn" disabled={busy} onClick={() => setHeadOnly(true)}>今の thread だけで作る…</button> : null}
                {headOnly ? <button className="btn" disabled={busy} onClick={() => setHeadOnly(false)}>全体で作る</button> : null}
              </div>
              <p className="small muted">押したときだけ、1回作ります（自動では作りません・やり直しも自動ではしません）。トークンを消費します。</p>
            </>
          ) : <p className="muted">見積りを確認しています…</p>}
          {view ? <div className="acts"><button className="btn" disabled={busy} onClick={() => void discard()}>この結果を破棄</button></div> : null}
        </div>
      ) : null}
      {err ? <div className="why" role="alert">{err}</div> : null}
    </div>
  );
}

// ───────────────────────────── 引き継ぎ資料の確認 ─────────────────────────────

/** 資料の確認（読取りのみ。`preview_handoff` の結果をそのまま表示する。送らない）。 */
export function HandoffPreviewBody({ chatName, load, live }: { chatName: string; load: () => Promise<HandoffPreview>; live: boolean }) {
  const [state, setState] = useState<{ kind: "loading" } | { kind: "error"; message: string } | { kind: "ok"; p: HandoffPreview }>({ kind: "loading" });
  const [tick, setTick] = useState(0);
  const [copied, setCopied] = useState<boolean | null>(null);
  useEffect(() => {
    if (!live) return;
    let alive = true;
    setState({ kind: "loading" });
    load().then((p) => { if (alive) setState({ kind: "ok", p }); }).catch((e) => { if (alive) setState({ kind: "error", message: errText(e) }); });
    return () => { alive = false; };
    // 開いたときと「再確認」のときだけ読む
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tick, live]);
  if (!live) return <div className="content"><p>この操作はモックでは動きません。</p></div>;
  return (
    <div className="content">
      <FootActions left={<button className="btn" disabled={state.kind === "loading"} onClick={() => setTick((t) => t + 1)}>再確認</button>} />
      <p className="small muted">対象: {chatName}。ここで見られるのは、引き継ぐときに新しい thread へ最初に添える資料です（読み取りのみ。何も送りません）。</p>
      {state.kind === "loading" ? <p>資料を確認しています…</p> : null}
      {state.kind === "error" ? <div className="why" role="alert">資料を確認できませんでした: {state.message}</div> : null}
      {state.kind === "ok" ? (
        <>
          {state.p.blockers.length > 0 ? (
            <div className="why" role="alert"><b>いまは引き継げません</b>
              <ul>{state.p.blockers.map((b, i) => <li key={i}>{b.message}</li>)}</ul>
              <span className="small">引き継がずに送るか、状況が変わるまで待つかは、あなたが選びます。</span>
            </div>
          ) : null}
          {state.p.notes.length > 0 ? <ul className="small muted">{state.p.notes.map((t, i) => <li key={i}>{t}</li>)}</ul> : null}
          <p className="small">{statsText(state.p)}</p>
          <p className="small muted" style={{ overflowWrap: "anywhere" }}>{ledgerText(state.p)}</p>
          {state.p.envelope ? (
            <>
              <pre className="mono cont-env" aria-label="引き継ぎ資料の本文">{state.p.envelope}</pre>
              <div className="acts">
                <button className="btn" onClick={() => void copyText(state.p.envelope).then(setCopied)}>資料をコピー</button>
                {copied === true ? <span className="small muted" role="status">コピーしました</span> : copied === false ? <span className="small why" role="status">コピーできませんでした</span> : null}
              </div>
            </>
          ) : <p className="muted">添える資料はありません。</p>}
        </>
      ) : null}
    </div>
  );
}

// ───────────────────────────── 設定「継続チャット」 ─────────────────────────────

const THRESHOLDS = [50, 55, 60, 65, 70, 75, 80, 85, 90, 95];
const ROUNDS = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
/** 選択肢に無い保存値（手で書き換えた settings.json など）も、そのまま選択肢に出す。 */
const withCurrent = (list: number[], v: number | null): number[] => (v === null || list.includes(v) ? list : [...list, v].sort((a, b) => a - b));

export function ContinuationSettingsBody({ value, set }: { value: ContinuationSettings; set: (c: ContinuationSettings) => void }) {
  return (
    <>
      <div className="field"><span>継続チャット</span>
        <label><input type="checkbox" checked={value.enabled} onChange={(e) => set({ ...value, enabled: e.target.checked })} />継続チャットを使う</label>
        <span className="note">初期値はオフです。オンにすると、使用量チップ・目的メモ・引き継ぎの案内（と「目的メモ」「新しい thread へ引き継ぐ」の操作）が使えるようになります。オフのあいだはこれらを出さず、使用量も取り込みません。保存済みの目的メモなどの記録は消しません。</span>
      </div>
      <div className="field"><span>引き継ぎの案内</span>
        <select aria-label="文脈の使用率が何%以上で案内するか" value={value.suggestAtPercent ?? ""}
          onChange={(e) => set({ ...value, suggestAtPercent: e.target.value === "" ? null : Number(e.target.value) })}>
          <option value="">案内しない</option>
          {withCurrent(THRESHOLDS, value.suggestAtPercent).map((n) => <option key={n} value={n}>{n}% 以上で案内</option>)}
        </select>
        <span className="note">文脈の使用率がこの値以上になったら（この接続で受け取った値のときだけ）、入力欄の上に「次の送信で新しい thread へ引き継ぎます」と案内します。案内だけで、自動では引き継ぎません。</span>
      </div>
      <div className="field"><span>資料に入れる往復数</span>
        <select aria-label="引き継ぎ資料に入れる直近の往復数" value={value.rounds} onChange={(e) => set({ ...value, rounds: Number(e.target.value) })}>
          {withCurrent(ROUNDS, value.rounds).map((n) => <option key={n} value={n}>直近 {n} 往復</option>)}
        </select>
        <span className="note">引き継ぎのときに新しい thread へ添える、直近の往復の数です（目的メモも添えます）。</span>
      </div>
    </>
  );
}

/** Dialogs.tsx へ渡す、継続チャットの値と操作（ホストの値のコピーと操作の入口）。 */
export interface ContinuityDialogProps {
  value: ContinuationSettings;
  setValue: (c: ContinuationSettings) => void;
  /** 目的メモを保存する。戻り値は失敗の文（成功なら null）。 */
  savePurpose: (chatId: string, text: string, expectedRevision: number | null) => Promise<string | null>;
  loadPreview: (chatId: string) => Promise<HandoffPreview>;
  /** チャット別の補足情報（目的メモの現在値・組織実行所有の判定に使う）。 */
  locals: ChatLocalView[];
}
