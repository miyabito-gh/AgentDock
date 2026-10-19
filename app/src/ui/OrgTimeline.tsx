// 組織実行の進行タイムライン（DESIGN_ORG_R2 1.4）。ホストが整えた行を新しい順に出す。
// 差分は orgTimelineUpdated の合図で取り直す（1秒に1回まで）。上へスクロールしていない間は新しい行を差し込まず、「新しい記録 N件」を出す。
import { useCallback, useEffect, useRef, useState } from "react";
import * as host from "../ipc/client";
import type { OrgTimelineEntry } from "../ipc/types";
import { hms } from "./format";

export type TimelineFilter = "all" | "mine" | "task";

/** 絞り込み: あなたの操作と止まった所（利用者の行・警告・失敗・待ち）／指定のタスク。 */
export function filterEntries(rows: OrgTimelineEntry[], filter: TimelineFilter, task: string | null): OrgTimelineEntry[] {
  if (filter === "mine") return rows.filter((e) => e.actor.kind === "user" || ["warn", "bad", "wait"].includes(e.tone));
  if (filter === "task" && task) return rows.filter((e) => e.task === task);
  return rows;
}

const PAGE = 200;

export function OrgTimeline({ workflowId, tick, live, task, onClearTask }: {
  workflowId: string;
  /** orgTimelineUpdated の合図の回数（変わったら差分を取り直す）。 */
  tick: number;
  live: boolean;
  /** 「このタスクの経過」で絞るタスク。 */
  task: string | null;
  onClearTask: () => void;
}) {
  const [rows, setRows] = useState<Map<number, OrgTimelineEntry>>(new Map());
  const [err, setErr] = useState<string | null>(null);
  const [filter, setFilter] = useState<TimelineFilter>(task ? "task" : "all");
  const [limit, setLimit] = useState(PAGE);
  const [cut, setCut] = useState(false);
  /** スクロール中に届いた行（新しい記録 N件）。 */
  const [held, setHeld] = useState<OrgTimelineEntry[]>([]);
  const last = useRef<number | null>(null);
  const box = useRef<HTMLDivElement | null>(null);
  const seq = useRef(0);

  useEffect(() => { if (task) setFilter("task"); else setFilter((f) => (f === "task" ? "all" : f)); }, [task]);
  // 別のワークフローに変わったら最初から読む。
  useEffect(() => { setRows(new Map()); setHeld([]); setErr(null); setCut(false); setLimit(PAGE); last.current = null; }, [workflowId]);

  const merge = useCallback((entries: OrgTimelineEntry[]) => {
    setRows((prev) => { const m = new Map(prev); for (const e of entries) m.set(e.seq, e); return m; });
  }, []);

  useEffect(() => {
    if (!live) { setErr("モックでは経過を出せません"); return; }
    const n = ++seq.current;
    void (async () => {
      try {
        const v = await host.orgTimeline(workflowId, last.current, 500);
        if (n !== seq.current) return;
        setErr(null);
        const first = last.current === null;
        last.current = v.lastSeq;
        if (v.truncated && first) setCut(true);
        if (v.entries.length === 0) return;
        const scrolled = !first && (box.current?.scrollTop ?? 0) > 8;
        if (scrolled) setHeld((h) => [...h, ...v.entries]); else merge(v.entries);
      } catch (e) { if (n === seq.current) setErr(host.asIpcError(e).message); }
    })();
  }, [workflowId, tick, live, merge]);

  const showHeld = () => { merge(held); setHeld([]); box.current?.scrollTo({ top: 0 }); };
  const sorted = Array.from(rows.values()).sort((a, b) => b.seq - a.seq);
  const shown = filterEntries(sorted, filter, task);
  const page = shown.slice(0, limit);
  return (
    <section aria-label="経過" className="org-tl">
      <div className="acts">
        <label className="small">絞り込み
          <select value={filter} aria-label="経過の絞り込み" onChange={(e) => { setFilter(e.target.value as TimelineFilter); setLimit(PAGE); }}>
            <option value="all">すべて</option>
            <option value="mine">あなたの操作と止まった所</option>
            <option value="task" disabled={!task}>このタスク{task ? `（${task}）` : ""}</option>
          </select>
        </label>
        {task && filter === "task" ? <button className="btn sm" onClick={onClearTask}>絞り込みを外す</button> : null}
        <span className="grow" />
        {held.length > 0 ? <button className="btn sm attn" onClick={showHeld}>新しい記録 {held.length}件</button> : null}
      </div>
      {err ? <div className="banner warn" role="status">経過を取得できませんでした（{err}）</div> : null}
      <div className="org-tl-box" ref={box} role="log" aria-label="進行の記録（新しい順）">
        {page.length === 0 ? <div className="empty-note">{err ? "" : "記録はまだありません"}</div> : (
          <ul className="org-tl-list">
            {page.map((e) => (
              <li key={e.seq} className={`org-tl-row tone-${e.tone}`}>
                <span className="mono small org-tl-at" title={new Date(e.at).toLocaleString()}>{hms(e.at)}</span>
                <span className={`org-tl-actor ${e.actor.kind}`}>{e.actor.label}</span>
                {e.task ? <span className="mono small">{e.task}</span> : null}
                <span className="org-tl-text">{e.text}</span>
              </li>
            ))}
          </ul>
        )}
        {shown.length > page.length ? <button className="btn sm" onClick={() => setLimit((l) => l + PAGE)}>さらに表示（あと {shown.length - page.length} 件）</button> : null}
        {cut ? <div className="small muted">古い記録は保持の上限で省いています（最新の500行まで）。</div> : null}
      </div>
    </section>
  );
}
