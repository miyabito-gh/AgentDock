// 作業フォルダのセット（名前つきの組）の選択・保存・管理の画面部品。判定は ui/workFolderSets.ts（純粋）。
// 選んだセットは、呼び出し側が検査（check_work_folder）を通してから並びへ入れる。ここでは検査せず、自動で送信もしない。
import { useState } from "react";
import type { WorkFolderSet } from "../ipc/types";
import { moveSet, removeSet, renameSet, saveSet, type SetEdit } from "./workFolderSets";

/** 作業フォルダまわりの画面から使う道具（App が渡す）。 */
export interface FolderTools {
  sets: WorkFolderSet[];
  /** 設定へ保存する（直列化）。`f` は最新の一覧に適用され、null を返すと変更なし（false）。保存できたときだけ true。失敗の通知は呼び出し側。 */
  update: (f: (sets: WorkFolderSet[]) => WorkFolderSet[] | null) => Promise<boolean>;
  /** フォルダの「参照…」。選ぶだけで検査はしない。キャンセル・失敗は null。 */
  browse: () => Promise<string | null>;
}

const newSetId = (): string =>
  typeof crypto !== "undefined" && typeof crypto.randomUUID === "function" ? crypto.randomUUID() : `s${Date.now().toString(36)}${Math.random().toString(36).slice(2, 10)}`;

const okSets = (r: SetEdit): WorkFolderSet[] | null => (r.kind === "ok" ? r.sets : null);

export function FolderSetBar({ tools, current, saveBlock, onPick, disabledReason }: {
  tools: FolderTools;
  /** 「セットとして保存」する現在の並び（先頭＝プライマリ）。 */
  current: string[];
  /** 保存できない理由（あれば保存ボタンを止めて理由を出す）。 */
  saveBlock: string | null;
  onPick: (set: WorkFolderSet) => void;
  disabledReason?: string | null;
}) {
  const [mode, setMode] = useState<"none" | "save" | "manage">("none");
  const [name, setName] = useState("");
  const [overwrite, setOverwrite] = useState<WorkFolderSet | null>(null);
  const [msg, setMsg] = useState<{ text: string; err: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);
  const sets = tools.sets;

  /** 保存を依頼して、保存できたと分かったときだけ true。 */
  const commit = async (f: (prev: WorkFolderSet[]) => WorkFolderSet[] | null, failText: string): Promise<boolean> => {
    setBusy(true);
    try {
      const ok = await tools.update(f);
      if (!ok) setMsg({ text: failText, err: true });
      return ok;
    } catch { setMsg({ text: failText, err: true }); return false; } finally { setBusy(false); }
  };

  const doSave = async (confirmOverwrite: boolean) => {
    setMsg(null);
    const make = (prev: WorkFolderSet[]) => saveSet(prev, name, current, { overwrite: confirmOverwrite, now: Date.now(), newId: newSetId });
    const r = make(sets);
    if (r.kind === "error") { setMsg({ text: r.message, err: true }); return; }
    if (r.kind === "needsOverwrite") { setOverwrite(r.existing); return; }
    const saved = r.saved.name;
    if (await commit((prev) => okSets(make(prev)), "セットを保存できませんでした")) { setMsg({ text: `セット「${saved}」を保存しました`, err: false }); setMode("none"); setName(""); setOverwrite(null); }
  };

  const doRename = async () => {
    if (!renaming) return;
    setMsg(null);
    const r = renameSet(sets, renaming.id, renaming.name, Date.now());
    if (r.kind === "error") { setMsg({ text: r.message, err: true }); return; }
    if (await commit((prev) => okSets(renameSet(prev, renaming.id, renaming.name, Date.now())), "名前を変更できませんでした")) { setMsg({ text: "名前を変更しました", err: false }); setRenaming(null); }
  };

  const doRemove = async (id: string) => {
    setMsg(null);
    if (await commit((prev) => (prev.some((s) => s.id === id) ? removeSet(prev, id) : null), "セットを削除できませんでした")) { setMsg({ text: "セットを削除しました（フォルダは削除していません）", err: false }); setRemoving(null); }
  };

  const doMove = async (id: string, dir: -1 | 1) => {
    setMsg(null);
    await commit((prev) => (prev.some((s) => s.id === id) ? moveSet(prev, id, dir) : null), "並び順を保存できませんでした");
  };

  const toggle = (m: "save" | "manage") => { setMode((cur) => (cur === m ? "none" : m)); setMsg(null); setOverwrite(null); setRenaming(null); setRemoving(null); };

  return (
    <div className="small" style={{ marginBottom: 4 }}>
      <div style={{ display: "flex", gap: 4, alignItems: "center", flexWrap: "wrap" }}>
        <span className="muted">セット</span>
        <select aria-label="作業フォルダのセット" value="" disabled={!!disabledReason || sets.length === 0 || busy} title={disabledReason ?? (sets.length === 0 ? "保存したセットはありません" : undefined)}
          onChange={(e) => { const s = sets.find((x) => x.id === e.target.value); if (s) { setMsg(null); onPick(s); } }}>
          <option value="">{sets.length === 0 ? "セットなし" : "セットを選ぶ…"}</option>
          {sets.map((s) => <option key={s.id} value={s.id}>{s.name}（{s.folders.length}件）</option>)}
        </select>
        <button type="button" className="btn sm" aria-expanded={mode === "save"} disabled={busy} onClick={() => toggle("save")}>セットとして保存…</button>
        <button type="button" className="btn subtle sm" aria-expanded={mode === "manage"} disabled={busy} onClick={() => toggle("manage")}>管理</button>
      </div>
      {mode === "save" ? (
        <div style={{ marginTop: 4 }}>
          {saveBlock ? <div className="why" role="status">{saveBlock}</div> : null}
          <div style={{ display: "flex", gap: 4, alignItems: "center" }}>
            <input className="grow" aria-label="セットの名前" placeholder="セットの名前" value={name} disabled={busy || !!saveBlock}
              onChange={(e) => { setName(e.target.value); setOverwrite(null); setMsg(null); }} onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) { e.preventDefault(); void doSave(false); } }} />
            <button type="button" className="btn sm" disabled={busy || !!saveBlock || !name.trim() || overwrite !== null} onClick={() => void doSave(false)}>保存</button>
          </div>
          {overwrite ? (
            <div role="alertdialog" aria-label="同名のセットの上書きの確認" className="why" style={{ marginTop: 4 }}>
              <div>同じ名前のセット「{overwrite.name}」があります（{overwrite.folders.length}件）。上書きすると元の内容は戻せません。</div>
              <div style={{ marginTop: 2 }}>
                <button type="button" className="btn sm" disabled={busy} onClick={() => void doSave(true)}>上書きする</button>{" "}
                <button type="button" className="btn subtle sm" disabled={busy} onClick={() => setOverwrite(null)}>やめる</button>
              </div>
            </div>
          ) : null}
        </div>
      ) : null}
      {mode === "manage" ? (
        <ol aria-label="保存したセット" style={{ listStyle: "none", margin: "4px 0 0", padding: 0 }}>
          {sets.length === 0 ? <li className="muted">保存したセットはありません</li> : null}
          {sets.map((s, i) => (
            <li key={s.id} style={{ padding: "3px 0", borderBottom: "1px dashed var(--line)" }}>
              {renaming?.id === s.id ? (
                <div style={{ display: "flex", gap: 4 }}>
                  <input className="grow" aria-label="新しい名前" value={renaming.name} disabled={busy} onChange={(e) => setRenaming({ id: s.id, name: e.target.value })}
                    onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) { e.preventDefault(); void doRename(); } }} />
                  <button type="button" className="btn sm" disabled={busy || !renaming.name.trim()} onClick={() => void doRename()}>変更</button>
                  <button type="button" className="btn subtle sm" disabled={busy} onClick={() => { setRenaming(null); setMsg(null); }}>やめる</button>
                </div>
              ) : (
                <div style={{ display: "flex", gap: 4, alignItems: "center" }}>
                  <span className="grow" title={s.folders.join("\n")} style={{ wordBreak: "break-all" }}>{s.name} <span className="muted">（{s.folders.length}件）</span></span>
                  <button type="button" className="btn subtle icon" aria-label={`${s.name} を上へ`} title="上へ" disabled={busy || i === 0} onClick={() => void doMove(s.id, -1)}>↑</button>
                  <button type="button" className="btn subtle icon" aria-label={`${s.name} を下へ`} title="下へ" disabled={busy || i === sets.length - 1} onClick={() => void doMove(s.id, 1)}>↓</button>
                  <button type="button" className="btn subtle sm" aria-label={`${s.name} の名前を変更`} disabled={busy} onClick={() => { setMsg(null); setRemoving(null); setRenaming({ id: s.id, name: s.name }); }}>名前を変更</button>
                  <button type="button" className="btn subtle sm" aria-label={`${s.name} を削除`} disabled={busy} onClick={() => { setMsg(null); setRenaming(null); setRemoving(s.id); }}>削除</button>
                </div>
              )}
              {removing === s.id ? (
                <div role="alertdialog" aria-label="セットの削除の確認" className="why" style={{ marginTop: 2 }}>
                  <div>セット「{s.name}」を削除しますか？（フォルダ自体は削除されません）</div>
                  <button type="button" className="btn sm" disabled={busy} onClick={() => void doRemove(s.id)}>削除する</button>{" "}
                  <button type="button" className="btn subtle sm" disabled={busy} onClick={() => setRemoving(null)}>やめる</button>
                </div>
              ) : null}
            </li>
          ))}
        </ol>
      ) : null}
      {msg ? <div className={msg.err ? "why err" : "muted"} role={msg.err ? "alert" : "status"} style={{ marginTop: 2 }}>{msg.text}</div> : null}
    </div>
  );
}
