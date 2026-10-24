// 『完了したものをまとめてアーカイブ』の確認（appConfirm と同じ作り。取り込み待ちを含めるかのチェックつき）。押したときだけ開く。
import { useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { useDialogFocus } from "./useDialogFocus";
import { clipList } from "./orgFold";

export interface BulkItem { id: string; name: string; state: string; pending: boolean }

function View({ items, done }: { items: BulkItem[]; done: (r: { ids: string[] } | null) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useDialogFocus(ref);
  const [incl, setIncl] = useState(false);
  const targets = items.filter((i) => incl || !i.pending);
  const nPending = items.filter((i) => i.pending).length;
  const { head, more } = clipList(targets, 5);
  return (
    <div className="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) done(null); }}
      onKeyDown={(e) => { if (e.key === "Escape" && !e.nativeEvent.isComposing) { e.preventDefault(); done(null); } }}>
      <div ref={ref} tabIndex={-1} className="dialog" role="alertdialog" aria-modal="true" aria-label="完了したワークフローをまとめてアーカイブ">
        <header><h3>まとめてアーカイブしますか</h3></header>
        <div className="content">
          <p>完了・取り消し済みの {targets.length} 件のワークフローを一覧から隠します（アーカイブ）。</p>
          <p>アーカイブしたものは、一覧の下の「アーカイブ」から戻せます。記録は消えません。</p>
          {head.length ? <ul>{head.map((i) => <li key={i.id}>{i.name}（{i.state}{i.pending ? "・取り込み待ちあり" : ""}）</li>)}{more > 0 ? <li>ほか {more} 件</li> : null}</ul> : <p className="muted">対象がありません。</p>}
          {nPending > 0 ? (
            <label className="small"><input type="checkbox" checked={incl} onChange={(e) => setIncl(e.target.checked)} />取り込み待ちのものも含める（{nPending}件。含めない場合はそのまま残ります）</label>
          ) : null}
        </div>
        <footer>
          <div className="foot-l" />
          <div className="foot-r">
            <button className="btn" onClick={() => done(null)}>やめる</button>
            <button className="btn primary" disabled={targets.length === 0} onClick={() => done({ ids: targets.map((i) => i.id) })}>{targets.length}件をアーカイブ</button>
          </div>
        </footer>
      </div>
    </div>
  );
}

/** 確認して、選ばれた対象のIDを返す（やめたら null）。 */
export function askBulkArchive(items: BulkItem[]): Promise<string[] | null> {
  return new Promise((resolve) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    const done = (r: { ids: string[] } | null) => { resolve(r ? r.ids : null); queueMicrotask(() => { root.unmount(); host.remove(); }); };
    root.render(<View items={items} done={done} />);
  });
}
