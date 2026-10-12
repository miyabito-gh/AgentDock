// 作業フォルダ（複数）の画面部品（DESIGN_WORKFOLDERS.md）。判定は ui/workFolders.ts（純粋）、検査と保存はホスト。
// 追加不可は理由つきで足せず、強い確認は確認チェックのあとにだけ confirmedStrong へ渡る（ホストも再検査する）。
import { useEffect, useRef, useState } from "react";
import type { FolderCheck, Support, WorkFoldersView } from "../ipc/types";
import {
  addBlockReason, applyCheck, REJECT_TEXT, confirmedStrongPaths, droppedExtrasNote, folderKey, makePrimary, moveItem, needsAddConfirm,
  projectMatchText, saveBlockReason, strongText, toItems, unconfirmedStrong, viewRows, type FolderItem,
} from "./workFolders";

export type SaveResult = { kind: "ok" } | { kind: "needsQueueConfirm"; waiting: number } | { kind: "error"; message: string };

/** 画面から使うホスト操作（実接続のときだけ渡る）。 */
export interface WorkFoldersApi {
  load: () => Promise<WorkFoldersView>;
  check: (path: string, current: string[]) => Promise<FolderCheck>;
  save: (folders: string[], confirmedStrong: string[], queueConfirmed: boolean) => Promise<SaveResult>;
}

/** フォルダの並び。先頭＝プライマリ。追加は検査してから（拒否は理由つき、強い確認は確認チェックが済むまで保存不可）。 */
export function FolderListEditor({ items, setItems, support, check, disabledReason, minItems = 1, primaryFixed = false }: {
  items: FolderItem[]; setItems: (v: FolderItem[]) => void; support: Support; check: (path: string, current: string[]) => Promise<FolderCheck>;
  /** 編集できない理由（作業中など）。あれば全操作を止めて理由を出す。 */
  disabledReason?: string | null;
  /** 残すべき最小件数（変更では1、新しいチャットの追加分では0）。 */
  minItems?: number;
  /** 先頭はこの一覧では扱わない（新しいチャットの追加分）。ラベルは付けず、プライマリへの昇格もしない。 */
  primaryFixed?: boolean;
}) {
  const [path, setPath] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const blocked = addBlockReason(support);
  // 追加できない接続では「置き換え」（プライマリだけの変更）にする。
  const replaceMode = blocked !== null && !primaryFixed;
  const off = !!disabledReason || busy;
  // 追加できない接続では、追加専用の一覧（新しいチャットの追加分）は追加を止める。
  const addOff = off || (blocked !== null && primaryFixed);
  const add = async () => {
    const p = path.trim();
    if (!p || addOff) return;
    setBusy(true); setErr(null);
    try {
      const c = await check(p, replaceMode ? [] : items.map((i) => i.path));
      const r = applyCheck(replaceMode ? [] : items, c, primaryFixed ? 1 : 0);
      if (r.error) setErr(r.error); else { setItems(r.items); setPath(""); }
    } catch (e) { setErr(e instanceof Error ? e.message : "検査できませんでした"); } finally { setBusy(false); }
  };
  return (
    <div>
      {disabledReason ? <div className="why" style={{ marginBottom: 4 }}>{disabledReason}</div> : null}
      <ol style={{ listStyle: "none", margin: 0, padding: 0 }} aria-label="作業フォルダの並び">
        {items.map((it, i) => (
          <li key={it.path} style={{ padding: "3px 0", borderBottom: "1px dashed var(--line)" }}>
            <div style={{ display: "flex", gap: 4, alignItems: "center" }}>
              {primaryFixed ? null : <span className={`tag ${i === 0 ? "ai" : ""}`}>{i === 0 ? "プライマリ" : "追加"}</span>}
              <span className="mono grow" style={{ wordBreak: "break-all" }}>{it.path}</span>
              <button className="btn subtle icon" aria-label={`${it.path} を上へ`} disabled={off || blocked !== null || i === 0} title={blocked ?? "上へ"} onClick={() => setItems(moveItem(items, i, -1))}>↑</button>
              <button className="btn subtle icon" aria-label={`${it.path} を下へ`} disabled={off || blocked !== null || i === items.length - 1} title={blocked ?? "下へ"} onClick={() => setItems(moveItem(items, i, 1))}>↓</button>
              {!primaryFixed && i > 0 ? <button className="btn subtle sm" disabled={off || blocked !== null} title={blocked ?? "このフォルダをプライマリ（先頭）にします"} onClick={() => setItems(makePrimary(items, i))}>プライマリにする</button> : null}
              <button className="btn subtle sm" disabled={off || items.length <= minItems} title={items.length <= minItems ? "プライマリは外せません（別のフォルダに置き換えてください）" : "並びから外します（フォルダ自体は削除されません）"} onClick={() => setItems(items.filter((_, j) => j !== i))}>外す</button>
            </div>
            {it.strong.length > 0 ? (
              <label className="small" style={{ display: "block", marginTop: 2 }}>
                <input type="checkbox" checked={it.confirmed} disabled={off} onChange={(e) => setItems(items.map((x, j) => (j === i ? { ...x, confirmed: e.target.checked } : x)))} />{" "}
                このフォルダ全体を許可することを理解しました（{strongText(it.strong)}）
              </label>
            ) : null}
          </li>
        ))}
      </ol>
      <div style={{ display: "flex", gap: 4, marginTop: 4 }}>
        <input className="mono grow" aria-label={replaceMode ? "新しいプライマリのパス" : "追加するフォルダのパス"} placeholder={replaceMode ? "置き換えるフォルダのフルパス" : "追加するフォルダのフルパス"} value={path} disabled={off}
          onChange={(e) => { setPath(e.target.value); setErr(null); }} onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) { e.preventDefault(); void add(); } }} />
        <button className="btn sm" disabled={addOff || !path.trim()} title={blocked !== null && primaryFixed ? blocked : undefined} onClick={() => void add()}>{replaceMode ? "置き換え" : "フォルダを追加"}</button>
      </div>
      {blocked ? <div className="small muted" style={{ marginTop: 2 }}>{blocked}{primaryFixed ? "" : "（プライマリだけ置き換えできます）"}</div> : null}
      {err ? <div className="why err" role="alert" style={{ marginTop: 4 }}>{err}</div> : null}
    </div>
  );
}

/** 作業フォルダの確認と変更（ヘッダーから開く帯。押したときだけ出る）。 */
export function WorkFoldersPanel({ api, lockReason, permissionText, fallback, onClose }: {
  api: WorkFoldersApi | null; lockReason: string | null; permissionText: string; fallback: string[]; onClose: () => void;
}) {
  const [view, setView] = useState<WorkFoldersView | null>(null);
  const [at, setAt] = useState<number | null>(null);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [items, setItems] = useState<FolderItem[] | null>(null);
  const [before, setBefore] = useState<string[]>([]);
  const [confirmStep, setConfirmStep] = useState(false);
  const [queueNeed, setQueueNeed] = useState<number | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  // api は毎レンダーで作り直されるので、取得はパネルを開いたとき（と接続の有無が変わったとき）だけ。ref で最新を使う。
  const apiRef = useRef(api);
  apiRef.current = api;
  const hasApi = !!api;
  useEffect(() => {
    const cur = apiRef.current;
    if (!cur) return;
    let alive = true;
    cur.load().then((v) => { if (alive) { setView(v); setAt(Date.now()); } }).catch((e) => { if (alive) setLoadErr(e?.message ?? "取得できませんでした"); });
    return () => { alive = false; };
  }, [hasApi]);
  useEffect(() => { if (confirmStep) cancelRef.current?.focus(); }, [confirmStep]);
  // 強い確認が未チェックの件が増えたら、そのチェックへフォーカスを移す（保存は確認が済むまで押せない）。
  const strongPending = items ? unconfirmedStrong(items).length : 0;
  const prevPending = useRef(0);
  useEffect(() => {
    if (strongPending > prevPending.current) rootRef.current?.querySelector<HTMLInputElement>("input[type=checkbox]:not(:checked)")?.focus();
    prevPending.current = strongPending;
  }, [strongPending]);
  if (!api) return <div className="cbanner info wfpanel"><h4>作業フォルダ</h4><div>モックでは動きません。</div><div className="acts"><button className="btn-line" onClick={onClose}>閉じる</button></div></div>;
  const support: Support = view?.support ?? "unknown";
  const cancelEdit = () => { setItems(null); setConfirmStep(false); setQueueNeed(null); setMsg(null); };
  const startEdit = async () => {
    const base = view?.next?.folders ?? (view?.current.kind === "value" ? view.current.value.folders : null) ?? view?.lastAccepted?.folders.folders ?? fallback;
    setBefore(base); setItems(toItems(base)); setConfirmStep(false); setQueueNeed(null); setMsg(null);
    // 既存の並びも検査する（ホーム直下・ドライブ直下などは強い確認が要る。ホストも保存時に再検査する）。
    const checked: FolderItem[] = [];
    const notes: string[] = [];
    for (let i = 0; i < base.length; i++) {
      try {
        const c = await api.check(base[i], base.filter((_, j) => j !== i));
        if (c.kind === "needsStrongConfirm") checked.push({ path: base[i], strong: c.reasons, confirmed: false });
        else { checked.push({ path: base[i], strong: [], confirmed: false }); if (c.kind === "rejected") notes.push(`${base[i]}: ${REJECT_TEXT[c.reason]}`); }
      } catch { checked.push({ path: base[i], strong: [], confirmed: false }); }
    }
    setItems((cur) => (cur ? checked : cur));
    if (notes.length) setMsg(`現在の並びに、いまは使えないフォルダがあります（外すと保存できます）: ${notes.join("、")}`);
  };
  const paths = items?.map((i) => i.path) ?? [];
  const block = items ? saveBlockReason(items, support) : null;
  const save = async () => {
    if (!items) return;
    setBusy(true); setMsg(null);
    try {
      const r = await api.save(paths, confirmedStrongPaths(items), queueNeed !== null);
      if (r.kind === "ok") onClose();
      else if (r.kind === "needsQueueConfirm") { setQueueNeed(r.waiting); setConfirmStep(false); }
      else setMsg(r.message);
    } finally { setBusy(false); }
  };
  const onSave = () => { if (needsAddConfirm(before, paths) && !confirmStep) setConfirmStep(true); else void save(); };
  const rows = view ? viewRows(view, at) : [];
  const dropped = view ? droppedExtrasNote(view.next, support) : null;
  const added = items ? items.filter((i) => !before.some((b) => folderKey(b) === folderKey(i.path))) : [];
  return (
    <div className="cbanner info wfpanel" role="region" aria-label="作業フォルダ" ref={rootRef}
      onKeyDown={(e) => { if (items !== null && e.key === "Escape" && !e.nativeEvent.isComposing) { e.preventDefault(); e.stopPropagation(); cancelEdit(); } }}>
      <h4>作業フォルダ</h4>
      {loadErr ? <div className="why err">{loadErr}</div> : null}
      {!view && !loadErr ? <div className="muted">確認しています…</div> : null}
      {items === null ? (
        <>
          {rows.map((r) => (
            <div key={r.key} style={{ marginTop: 4 }}>
              <div><b>{r.title}</b> <span className="muted small">（根拠: {r.how}）</span></div>
              {r.folders ? <ul style={{ margin: "2px 0 0", paddingLeft: 18 }}>{r.folders.map((f, i) => <li key={f} className="mono">{r.key === "next" || r.key === "current" || r.key === "accepted" ? <span className="muted">{i === 0 ? "プライマリ " : "追加 "}</span> : null}{f}</li>)}</ul> : null}
              {r.note ? <div className="small muted">{r.note}</div> : null}
            </div>
          ))}
          {view?.projectMatch ? <div className="small" style={{ marginTop: 4 }}>{projectMatchText(view.projectMatch)}</div> : null}
          {dropped ? <div className="why err" style={{ marginTop: 4 }}>{dropped}</div> : null}
          <div className="small muted" style={{ marginTop: 4 }}>AGENTS.md はプライマリのフォルダ基準です。「変更を戻す」の対象もプライマリだけです。</div>
          {lockReason ? <div className="why" style={{ marginTop: 4 }}>{lockReason}</div> : null}
          <div className="acts">
            <button className="btn-main" disabled={!view || !!lockReason} title={lockReason ?? "次の送信から使う作業フォルダを変更します"} onClick={() => void startEdit()}>変更…</button>
            <button className="btn-line" onClick={onClose}>閉じる</button>
          </div>
        </>
      ) : (
        <>
          <div className="small muted">次の送信から使う作業フォルダ（上から順。先頭がプライマリ）。送信するまで何も送りません。</div>
          <FolderListEditor items={items} setItems={(v) => { setItems(v); setConfirmStep(false); setQueueNeed(null); }} support={support} check={api.check} disabledReason={lockReason} />
          {confirmStep ? (
            <div role="alertdialog" aria-label="作業フォルダの追加の確認" className="why" style={{ marginTop: 6 }}>
              <div><b>Codex の読取り・編集の範囲が広がります。</b></div>
              {added.length ? <ul style={{ margin: "2px 0", paddingLeft: 18 }}>{added.map((a) => <li key={a.path} className="mono" style={{ wordBreak: "break-all" }}>{a.path}</li>)}</ul> : <div>プライマリを <span className="mono">{paths[0]}</span> に変更します。</div>}
              <div>このチャットの次の送信から、Codex がこのフォルダを検索・読取り・編集できるようになります。</div>
              <div>権限: {permissionText}</div>
              <div>「変更を戻す」の対象はプライマリだけです（追加フォルダの変更は戻せません）。</div>
            </div>
          ) : null}
          {queueNeed !== null ? <div style={{ marginTop: 4 }}><b>送信待ちの依頼 {queueNeed}件も、新しい作業フォルダが対象になります。</b></div> : null}
          {unconfirmedStrong(items).length ? <div className="small muted" style={{ marginTop: 4 }}>確認が必要なフォルダがあります。内容を確認してチェックを入れてください。</div> : null}
          {msg ? <div className="why err" role="alert" style={{ marginTop: 4 }}>{msg}</div> : null}
          <div className="acts">
            <button className="btn-main" disabled={busy || !!block || !!lockReason} title={block ?? lockReason ?? undefined} onClick={onSave}>
              {confirmStep ? "確認して次の送信から使う" : queueNeed !== null ? "確認して変更" : "次の送信から使う"}
            </button>
            <button className="btn-line" ref={cancelRef} onClick={cancelEdit}>やめる</button>
          </div>
        </>
      )}
    </div>
  );
}
