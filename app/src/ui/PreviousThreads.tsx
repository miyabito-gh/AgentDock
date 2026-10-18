// 継続チャットの「以前の会話」（DESIGN_CONT §7.4）。以前の thread は読取り専用で表示する（コピーだけ。resume・送信・アーカイブ・削除は出さない）。
// 読込みは利用者が押したときだけ、直前の thread から1本ずつ（保存履歴の読取りのみ。resume しない）。
import { useCallback, useId, useRef, useState } from "react";
import { readSegmentHistory } from "../ipc/client";
import type { AgentHistory, ChatKey, ThreadSegmentView } from "../ipc/types";
import { Icon } from "./Icon";
import { Markdown, COPY_LABEL, useCopy } from "./Markdown";
import { envelopeLabel, splitEnvelope, type SplitEnvelope } from "./continuity";
import { pad } from "./format";

type Seg = { status: "loading" } | { status: "error"; message: string } | { status: "ok"; history: AgentHistory };

export interface PrevThreads {
  /** 古い順。未読込みの thread は null（読み込み済みは直前から連続している）。 */
  segs: (Seg | null)[];
  /** 未読込みの thread がある。 */
  unloaded: boolean;
  loadNext: () => void;
}

/** 以前の thread を直前から1本ずつ読む。表示中の依頼ごとに1回だけ（押したときだけ）。 */
export function usePrevThreads(head: ChatKey, threads: ThreadSegmentView[]): PrevThreads {
  const [segs, setSegs] = useState<Record<number, Seg>>({});
  const busy = useRef(false);
  // 読み込み済みの先頭（最も古い）の次の、さらに前の thread
  const loadedCount = Object.values(segs).filter((s) => s.status === "ok").length;
  const idx = threads.length - 1 - loadedCount;
  const loadNext = useCallback(() => {
    if (busy.current || idx < 0) return;
    busy.current = true;
    setSegs((m) => ({ ...m, [idx]: { status: "loading" } }));
    readSegmentHistory(head, threads[idx].chat).then(
      (history) => setSegs((m) => ({ ...m, [idx]: { status: "ok", history } })),
      (e) => setSegs((m) => ({ ...m, [idx]: { status: "error", message: e instanceof Error ? e.message : typeof e === "object" && e && "message" in e ? String((e as { message: unknown }).message) : String(e) } })),
    ).finally(() => { busy.current = false; });
  }, [head, threads, idx]);
  return { segs: threads.map((_, i) => segs[i] ?? null), unloaded: idx >= 0, loadNext };
}

const stamp = (ms: number) => { const d = new Date(ms); return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`; };

function CopyBtn({ text }: { text: string }) {
  const { state, copy } = useCopy();
  return <button onClick={() => copy(text)} aria-live="polite"><Icon name="copy" />{state === "idle" ? COPY_LABEL.idle : COPY_LABEL[state]}</button>;
}

/** 資料つきの依頼の上の折りたたみ行。開くと資料を等幅で出す（Markdown 整形しない）。資料は開いた枠のコピーで別にコピーする。 */
export function EnvelopeFold({ env }: { env: SplitEnvelope }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  if (!env.envelope) return null;
  return (
    <div className="envfold">
      <button type="button" className="foldmsg" aria-expanded={open} aria-controls={id} onClick={() => setOpen((v) => !v)}>{envelopeLabel(env)} {open ? "‹" : "›"}</button>
      {open ? (
        <div id={id} className="envbody">
          <pre className="mono" data-find="body">{env.envelope}</pre>
          <div className="acts"><CopyBtn text={env.envelope} /></div>
        </div>
      ) : null}
    </div>
  );
}

function SegmentView({ history }: { history: AgentHistory }) {
  return (
    <>
      {history.turns.map((t) => (
        <div key={t.key.agent.id + t.key.turnId} className="prevturn">
          {t.entries.filter((e) => e.kind.kind === "userMessage" || e.kind.kind === "agentMessage").map((e) => {
            const text = e.text.kind === "value" ? e.text.value : "…";
            if (e.kind.kind === "userMessage") {
              const env = splitEnvelope(text);
              return (
                <div className="msg user" key={e.key.itemId}>
                  <div className="sr">あなた</div>
                  <EnvelopeFold env={env} />
                  {env.userText.trim() ? <div className="bubble" data-find="body">{env.userText}</div> : null}
                  <div className="acts">{env.userText.trim() ? <CopyBtn text={env.userText} /> : null}</div>
                </div>
              );
            }
            return (
              <div className="msg ai" key={e.key.itemId}>
                <div className="who">Codex</div>
                <div className="bubble" data-find="body"><Markdown text={text} streaming={false} /></div>
                <div className="acts"><CopyBtn text={text} /></div>
              </div>
            );
          })}
        </div>
      ))}
      {history.turns.length === 0 ? <div className="muted prevnote">この thread に表示できる発言はありません</div> : null}
    </>
  );
}

/** 先頭の会話の上端に置く。読み込み済みの thread を古い順に並べ、境目に薄い区切り線を入れる。 */
export function PreviousThreads({ threads, prev }: { threads: ThreadSegmentView[]; prev: PrevThreads }) {
  if (threads.length === 0) return null;
  const loadedN = prev.segs.filter((s) => s?.status === "ok").length;
  const nextIdx = threads.length - 1 - loadedN;
  const nextSeg = nextIdx >= 0 ? prev.segs[nextIdx] : null;
  return (
    <div className="prevthreads" aria-label="以前の会話（読取り専用）">
      {prev.unloaded ? (
        <div className="prevhead">
          <span className="muted">以前の会話（thread {threads.length}本{loadedN > 0 ? `、${loadedN}本を表示中` : ""}）</span>
          {nextSeg?.status === "loading"
            ? <span className="muted">読み込み中…</span>
            : <button type="button" className="small" onClick={prev.loadNext}>{loadedN > 0 ? "さらに前を表示" : "表示"}</button>}
          {nextSeg?.status === "error" ? <span className="prevnote bad" role="alert">この thread の履歴を読めませんでした（{nextSeg.message}）。もう一度押すと再試行します</span> : null}
        </div>
      ) : null}
      {threads.map((th, i) => {
        const s = prev.segs[i];
        if (!s || s.status !== "ok") return null;
        return (
          <div key={th.chat.id} className="prevseg readonly">
            <SegmentView history={s.history} />
            <div className="threadsep" role="separator"><span>ここから新しい thread（{stamp(th.handedOffAt)} に引き継ぎ）</span></div>
          </div>
        );
      })}
    </div>
  );
}
