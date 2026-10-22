// 継続チャット（DESIGN_CONT §7）のダイアログ本文: 目的メモの編集、引き継ぎ資料の確認、設定「継続チャット」。外枠（Shell）は Dialogs.tsx 側。
// 規則: 資料の確認は読取りだけ（何も送らない）。目的メモの保存は「保存」を押したときの1回。未取得・失敗を成功に見せない。
//       文言・判定は contUsage.ts（純関数）。Esc・IME の規則は Shell が持つ（変換中は閉じない）。
import { useEffect, useRef, useState } from "react";
import * as host from "../ipc/client";
import type { ChatLocalView, ContinuationSettings, PurposeNoteView } from "../ipc/types";
import type { HandoffPreview } from "../ipc/types";
import { FootActions } from "./DialogParts";
import { copyText } from "./markdown/parts";
import { PURPOSE_MAX, charCount, ledgerText, observedText, purposeSaveBlock, statsText } from "./contUsage";

const errText = (e: unknown) => host.asIpcError(e).message;

// ───────────────────────────── 目的メモ ─────────────────────────────

/** 目的メモの編集。空にして保存＝削除。`save` は失敗の文（成功なら null）を返し、成功したら `onDone` で閉じる。 */
export function PurposeBody({ chatName, current, live, orgOwned, save, onDone }: {
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
  const ta = useRef<HTMLTextAreaElement>(null);
  useEffect(() => { ta.current?.focus(); }, []);
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
      <p className="small muted">この会話の送信に資料として添えます（この thread で最初の送信・メモを変更した後・文脈の圧縮を観測した後の、手動の送信のとき）。キューや追加指示には添えません。空にして保存すると、目的メモを削除します（記録は残ります）。</p>
      {orgOwned ? <div className="why">組織実行が所有するチャットでは、目的メモを編集できません。</div> : null}
      {current ? <p className="small muted">現在の版: {current.revision}{observedText(current.updatedAt) ? `（${observedText(current.updatedAt)} に保存）` : ""}</p> : null}
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
