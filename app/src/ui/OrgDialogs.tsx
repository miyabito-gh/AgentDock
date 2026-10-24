// 組織実行の開始ダイアログの本文と、設定の「組織実行」タブ（停滞時間・役割定義・グラフ定義）。P-ORG-7b、G-A5。
// 開始・有効化は、押したときだけホストの命令を呼ぶ。テンプレートを選ぶ・見積りを見る・定義ファイルを開くだけでは何も送らない。
// 有効化は権限の指紋を添えて確認後に送る（ホストが今の権限と照合する）。
import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import type { ChatKey, OrgDeleteOutcome, OrgDeletePreview, OrgGraphProblemView, OrgGraphsView, OrgRoleView, OrgRolesView } from "../ipc/types";
import * as host from "../ipc/client";
import { ConfirmBlock } from "./DialogParts";
import { showKnown } from "./format";
import { fmtBytes } from "./ManageDialogs";
import { elapsedLabel, isSlow, progressText, SLOW_NOTICE, type DeleteProgress } from "./orgDeleteUi";
import { intakeChoice, intakeOutcomeText } from "./intakeDelete";

/** 標準（従来の計画経路）。ホストの定義ファイルに依存せず、いつでも選べる。送るときは graphId を付けない（今までの開始と同じ）。 */
export const STANDARD_ID = "standard";

/** グラフ定義の読込み。reload=true は定義ファイルを読み直す（足りない組込みテンプレートも作る。何も送らない）。 */
export type LoadGraphs = (reload: boolean) => Promise<OrgGraphsView>;

/** 定義の誤りの一覧（ファイルごと。読み取りだけ）。 */
function GraphProblems({ problems, onOpen }: { problems: OrgGraphProblemView[]; onOpen?: (file: string) => void }) {
  return (
    <ul className="check-list">{problems.map((pr) => (
      <li key={pr.file}><span className="mono">{pr.file}</span>
        <ul>{pr.reasons.map((r, i) => <li key={i}>{r}</li>)}</ul>
        {onOpen && pr.file.endsWith(".json") ? <button type="button" className="btn sm" onClick={() => onOpen(pr.file)}>ファイルを開く</button> : null}</li>
    ))}</ul>
  );
}

const PERM_ROWS = (r: OrgRoleView): Array<[string, string]> => [
  ["sandbox", r.sandbox], ["承認の扱い", r.approval], ["作業場所", r.workplace], ["ネットワーク", r.allowNetwork ? "許可" : "なし"],
  ["追加の書込み先", r.extraWritableDirs.length ? r.extraWritableDirs.join("、") : "なし"],
  ["モデル", r.model ?? "既定"], ["推論の強さ", r.effort ?? "既定"], ["同時実行", r.maxConcurrent === null ? "既定" : String(r.maxConcurrent)],
];

export interface OrgSettingsProps {
  live: boolean;
  /** 停滞の通知までの時間（分。AppSettings.orgStallMinutes）。 */
  stallMinutes: number;
  /** 失敗の理由（成功は null）。通知はホストの設定更新イベントで反映される。 */
  setStall: (minutes: number) => Promise<string | null>;
}

/** 設定の「組織実行」タブ。役割・グラフは定義ファイルで編集する（この画面では編集しない）。 */
export function OrgSettingsBody({ live, stallMinutes, setStall }: OrgSettingsProps) {
  const [stall, setStallText] = useState(String(stallMinutes));
  useEffect(() => { setStallText(String(stallMinutes)); }, [stallMinutes]);
  const [stallMsg, setStallMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const stallN = /^\d+$/.test(stall.trim()) ? Number(stall.trim()) : NaN;
  const stallOk = Number.isInteger(stallN) && stallN >= 1 && stallN <= 1440;

  const [roles, setRoles] = useState<OrgRolesView | null>(null);
  const [rolesErr, setRolesErr] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [enabling, setEnabling] = useState<OrgRoleView | null>(null);
  const load = useCallback(async (reload: boolean) => {
    if (!live) return;
    setBusy(true); setRolesErr(null);
    try { setRoles(await (reload ? host.orgReloadRoles() : host.orgRolesView())); }
    catch (e) { setRolesErr(host.asIpcError(e).message); }
    finally { setBusy(false); }
  }, [live]);
  useEffect(() => { void load(false); }, [load]);
  // グラフ定義（読み取りだけ。再読み込みは定義ファイルを読み直すだけで、何も送らない）。
  const [graphs, setGraphs] = useState<OrgGraphsView | null>(null);
  const [graphsErr, setGraphsErr] = useState<string | null>(null);
  const [gMsg, setGMsg] = useState<string | null>(null);
  const [gBusy, setGBusy] = useState(false);
  const loadGraphs = useCallback(async (reload: boolean) => {
    if (!live) return;
    setGBusy(true); setGraphsErr(null);
    try {
      let v = await (reload ? host.orgReloadGraphs() : host.orgGraphsView());
      if (!reload && !v.loaded) v = await host.orgReloadGraphs();
      setGraphs(v);
    } catch (e) { setGraphsErr(host.asIpcError(e).message); }
    finally { setGBusy(false); }
  }, [live]);
  useEffect(() => { void loadGraphs(false); }, [loadGraphs]);
  // 有効化の確認が開いているあいだ、Esc は確認だけを閉じる（設定画面ごと閉じない。IME変換中は何もしない。実行中は閉じずに受け流す）。
  useEffect(() => {
    if (!enabling) return;
    const h = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.isComposing) return;
      e.preventDefault();
      e.stopPropagation();
      if (!busy) setEnabling(null);
    };
    document.addEventListener("keydown", h, true);
    return () => document.removeEventListener("keydown", h, true);
  }, [enabling, busy]);

  const open = async (file: string) => {
    try { await host.orgOpenRoleFile(file); setMsg(`「${file}」を関連アプリで開きました。編集後は「再読込み」を押してください。`); }
    catch (e) { setMsg(`開けませんでした: ${host.asIpcError(e).message}`); }
  };
  const openGraph = async (file: string) => {
    try { await host.orgOpenGraphFile(file); setGMsg(`「${file}」を関連アプリで開きました。編集後は「再読込み」を押してください。`); }
    catch (e) { setGMsg(`開けませんでした: ${host.asIpcError(e).message}`); }
  };
  const enable = async (r: OrgRoleView) => {
    setBusy(true);
    try { setRoles(await host.orgEnableRole(r.id, r.fingerprint)); setMsg(`役割「${r.displayName}」を有効にしました。グラフ定義の判定は「再読込み」で更新されます。`); setEnabling(null); }
    catch (e) { setMsg(`有効にできませんでした（確認後にファイルが変わった可能性があります。再読込みして、もう一度確認してください）: ${host.asIpcError(e).message}`); setEnabling(null); }
    finally { setBusy(false); }
  };

  return (
    <>
      <div className="field"><span>停滞の通知</span>
        <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
          <input type="text" inputMode="numeric" size={5} value={stall} onChange={(e) => { setStallText(e.target.value); setStallMsg(null); }} aria-label="停滞とみなす時間（分）" />分
          <button className="btn sm" disabled={!live || !stallOk || stallN === stallMinutes} onClick={() => void setStall(stallN).then((e) => setStallMsg(e ? { ok: false, text: e } : { ok: true, text: "保存しました。" }))}>保存</button>
        </div>
        <span className="note">実行中のタスクに新しい出力がないまま、この時間（1〜1440分）たつと通知します。通知だけで、自動では中断しません。</span>
        {!stallOk ? <span className="note why err">1〜1440 の整数で入れてください。</span> : null}
        {stallMsg ? <span className={`note${stallMsg.ok ? "" : " why err"}`} role="status">{stallMsg.text}</span> : null}
        {!live ? <span className="note">モックでは動きません。</span> : null}</div>
      <div className="field"><span>役割の定義</span>
        <div style={{ display: "flex", gap: 6 }}>
          <button className="btn-line" disabled={!live || busy} onClick={() => void load(true)} title="定義ファイルを読み直します（読み取りのみ）">再読込み</button>
        </div>
        <span className="note">役割は定義ファイルで編集します（この画面では編集しません）。権限を広げている定義は、理由を確認して有効にするまで使いません。</span></div>
      {msg ? <p className="small" role="status">{msg}</p> : null}
      {rolesErr ? <p className="why err" role="alert">役割の定義を取得できませんでした（{rolesErr}）。</p> : null}
      {!live ? null : !roles ? (rolesErr ? null : <p className="small muted">読み込んでいます…</p>) : !roles.loaded ? (
        <p className="small muted">役割の定義はまだ読み込まれていません（組織実行を開始するか「再読込み」で読み込みます）。</p>
      ) : (
        <>
          {roles.created.length > 0 ? <p className="small">標準の役割ファイルを作成しました: {roles.created.join("、")}</p> : null}
          <ul className="org-roles">
            {roles.roles.map((r) => (
              <li key={r.id}>
                <div className="org-role-h">
                  <b>{r.displayName}</b> <span className="mono muted">{r.id}</span>
                  <span className={`st ${r.enabled ? "done" : "wait"}`} style={{ marginLeft: 8 }}>{r.enabled ? "有効" : "無効"}</span>
                  <span className="grow" />
                  <button className="btn sm" onClick={() => void open(r.file)} title={r.file}>ファイルを開く</button>
                  {!r.enabled ? <button className="btn sm primary" disabled={busy} onClick={() => setEnabling(r)}>有効化…</button> : null}
                </div>
                {r.widened.length > 0
                  ? <div className="small"><b>権限を広げている理由:</b><ul className="check-list">{r.widened.map((x, i) => <li key={i}>{x}</li>)}</ul></div>
                  : <div className="small muted">基準の権限の範囲内です。</div>}
                {!r.enabled && r.widened.length > 0 ? <div className="small muted">{r.confirmed ? "この権限は確認済みですが、無効のままです。" : "この権限はまだ確認されていないため無効です。"}</div> : null}
                {enabling?.id === r.id ? (
                  <ConfirmBlock label={`役割「${r.displayName}」の有効化の確認`} wide actions={<>
                    <button className="btn" disabled={busy} onClick={() => setEnabling(null)}>やめる</button>
                    <button className="btn danger" disabled={busy} onClick={() => void enable(r)}>{busy ? "有効にしています…" : "この権限で有効にする"}</button></>}>
                    <p>この役割は次の権限で動きます。内容を確認して、問題なければ有効にしてください。</p>
                    <dl className="org-dl">{PERM_ROWS(r).map(([k, v]) => <Fragment key={k}><dt>{k}</dt><dd>{v}</dd></Fragment>)}</dl>
                    {r.widened.length > 0 ? <ul className="check-list">{r.widened.map((x, i) => <li key={i}>{x}</li>)}</ul> : null}
                  </ConfirmBlock>
                ) : null}
              </li>
            ))}
          </ul>
          {roles.problems.length > 0 ? (
            <div className="banner warn" role="status"><div>
              <b>読み込めなかった定義があります。</b>
              <ul className="check-list">{roles.problems.map((pr) => (
                <li key={pr.file}><span className="mono">{pr.file}</span>: {pr.reasons.join("、")}{pr.aside ? `（退避した先: ${pr.aside}）` : ""}
                  <button className="btn sm" style={{ marginLeft: 6 }} onClick={() => void open(pr.file)}>ファイルを開く</button></li>
              ))}</ul>
            </div></div>
          ) : null}
        </>
      )}

      <div className="field"><span>グラフ定義（進め方のテンプレート）</span>
        <div style={{ display: "flex", gap: 6 }}>
          <button className="btn-line" disabled={!live || gBusy} onClick={() => void loadGraphs(true)} title="定義ファイルを読み直します（読み取りのみ。組込みのテンプレートが無ければ作ります）">再読込み</button>
        </div>
        <span className="note">テンプレートは定義ファイルで編集します（この画面では編集しません）。誤りのある定義は、理由を示して選べなくします。動いているワークフローは、開始時の写しで続けます。組織実行が動いている間は読み直せません。</span></div>
      {gMsg ? <p className="small" role="status">{gMsg}</p> : null}
      {graphsErr ? <p className="why err" role="alert">グラフ定義を取得できませんでした（{graphsErr}）。</p> : null}
      {!live ? null : !graphs ? (graphsErr ? null : <p className="small muted">読み込んでいます…</p>) : !graphs.loaded ? (
        <p className="small muted">グラフ定義はまだ読み込まれていません（「再読込み」で読み込みます）。</p>
      ) : (
        <>
          {graphs.created.length > 0 ? <p className="small">標準のテンプレートを作成しました: {graphs.created.join("、")}</p> : null}
          <ul className="org-roles">
            {graphs.templates.map((x) => (
              <li key={x.id}>
                <div className="org-role-h">
                  <b>{x.name}</b> <span className="mono muted">{x.id}</span>
                  <span className={`st ${x.selectable ? "done" : "wait"}`} style={{ marginLeft: 8 }}>{x.selectable ? "有効" : "選べません"}</span>
                  <span className="grow" />
                  <button className="btn sm" onClick={() => void openGraph(x.file)} title={x.file}>ファイルを開く</button>
                </div>
                <div className="small">{x.description || "（説明なし）"}</div>
                <div className="small muted">置き場: {x.workspace === "git" ? "Git" : "フォルダ（Git なし）"}　使う役割: {x.roles.join("、") || "—"}　最大 {x.estimate.turnsMax} 回のやりとり（通常 {x.estimate.turnsTypical} 回）</div>
                {x.reasons.length > 0 ? <ul className="check-list">{x.reasons.map((r, i) => <li key={i}>{r}</li>)}</ul> : null}
              </li>
            ))}
          </ul>
          {graphs.templates.length === 0 && graphs.problems.length === 0 ? <p className="small muted">有効なテンプレートはありません。</p> : null}
          {graphs.problems.length > 0 ? (
            <div className="banner warn" role="status"><div>
              <b>誤りのある定義があります（使いません）。</b>
              <GraphProblems problems={graphs.problems} onOpen={(f) => void openGraph(f)} />
            </div></div>
          ) : null}
        </>
      )}
    </>
  );
}

/** ワークフローの削除（DESIGN_ORG_DELETE §6）。確認の内容は読取りで取得し、押したときだけ削除を実行する。worktree とブランチは削除しない。 */
export function OrgDeleteDialog({ workflowId, live, preview, remove, onDone, progress, onIssue, onClose }: {
  workflowId: string;
  live: boolean;
  preview: (id: string) => Promise<OrgDeletePreview>;
  remove: (id: string, deleteChats: boolean, expect: ChatKey[], intakeChat: ChatKey | null) => Promise<OrgDeleteOutcome>;
  /** 削除が済んだ。note＝依頼整理の会話についての一言（あれば）。 */
  onDone: (note: string | null) => void;
  /** ホストの削除の進行（この workflow の分だけ。表示だけ）。 */
  progress: DeleteProgress | null;
  /** 拒否・失敗・途中終了をトーストへも出す。 */
  onIssue: (message: string) => void;
  onClose: () => void;
}) {
  const [pv, setPv] = useState<OrgDeletePreview | null>(null);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [delChats, setDelChats] = useState(true);
  // 依頼整理の会話も削除する（役割チャットの「会話も削除」とは別。既定オン）。
  const [delIntake, setDelIntake] = useState(true);
  const [intakeNote, setIntakeNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [partial, setPartial] = useState<{ done: string[]; failed: string[] } | null>(null);
  const load = useCallback(async () => {
    setLoadErr(null);
    try { setPv(await preview(workflowId)); } catch (e) { setLoadErr(host.asIpcError(e).message); }
  }, [preview, workflowId]);
  useEffect(() => { void load(); }, [load]);
  const reasons = pv?.verdict.kind === "blocked" ? pv.verdict.reasons : [];
  // チャットの会話を残す選択なら、チャット単位の理由（と接続の理由）は削除を止めない（最終判定はホスト）。
  const chatSpecific = (r: string) => !!pv?.chats.some((c) => c.blocked === r) || r.startsWith("Codex に接続していません");
  const effective = delChats ? reasons : reasons.filter((r) => !chatSpecific(r));
  const resuming = !!pv?.deletion;
  const can = !!pv && live && !busy && effective.length === 0;
  // 経過時間（1秒ごとに更新）。90秒を超えたら案内し、閉じてよいと伝える（ホストの処理は続く）。
  const startedAt = useRef(0);
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    if (!busy) return;
    const t = setInterval(() => setElapsed(Date.now() - startedAt.current), 1000);
    return () => clearInterval(t);
  }, [busy]);
  const run = async () => {
    if (!pv || !can) return;
    startedAt.current = Date.now(); setElapsed(0);
    setBusy(true); setErr(null);
    try {
      const ic = intakeChoice(pv);
      const intakeChat = ic.kind === "offer" && !ic.blocked && delIntake && pv.intake ? pv.intake.chat : null;
      const o = await remove(workflowId, delChats, pv.chats.map((c) => c.chat), intakeChat);
      const note = intakeOutcomeText(o.intake);
      if (o.kind === "deleted") { onDone(note); return; }
      setIntakeNote(note);
      setPartial({ done: o.done, failed: o.failed });
      onIssue(`削除を最後まで終えられませんでした（失敗 ${o.failed.length} 件）。「削除を続ける…」で再開できます。`);
      await load();
    } catch (e) {
      const m = host.asIpcError(e).message;
      setErr(m);
      onIssue(`削除できませんでした: ${m.split("\n")[0]}`);
    } finally { setBusy(false); }
  };
  const pt = progressText(progress);
  const label = busy ? `削除しています…${pt ? ` ${pt}` : ""}（${elapsedLabel(elapsed)}）` : resuming || partial ? "削除を続ける" : "削除する";
  const nm = (c: OrgDeletePreview["chats"][number]) => c.name ?? c.chat.id;
  const ic = pv ? intakeChoice(pv) : null;
  return (
    <div className="content">
      {!pv && !loadErr ? <p className="small muted">確認の内容を読み込んでいます…</p> : null}
      {loadErr ? <p className="why err" role="alert">確認の内容を取得できませんでした（{loadErr}）。<button type="button" className="btn sm" onClick={() => void load()}>取り直す</button></p> : null}
      {pv ? (
        <ConfirmBlock label="ワークフローの削除の確認" wide actions={<button className="btn danger" disabled={!can} onClick={() => void run()}>{label}</button>}>
          <p>このワークフロー{pv.title ? `「${pv.title}」` : ""}の記録（計画・経過・使用量）を削除します。元に戻せません。</p>
          {(pv.deletion || partial) ? (
            <div className="banner warn" role="status"><div><b>削除の途中です。</b>
              <div>済: {(partial?.done ?? pv.deletion?.done ?? []).join("、") || "なし"}</div>
              <div>失敗: {(partial?.failed ?? pv.deletion?.failed ?? []).join("、") || "なし"}</div></div></div>
          ) : null}
          {pv.chats.length > 0 ? (
            <>
              <label><input type="checkbox" checked={delChats} disabled={busy || resuming} onChange={(e) => setDelChats(e.target.checked)} />役割チャット {pv.chats.length}件の会話も削除する（Codex の履歴を含む）</label>
              {!delChats ? <p className="small muted">オフにすると、手動に切り替えて通常のチャットとして一覧に残します。</p> : null}
              <ul className="check-list">{pv.chats.map((c) => <li key={c.chat.id}>{nm(c)}（{c.roleId || "—"}・{c.taskId || "—"}）{c.blocked && delChats ? <span className="why err"> {c.blocked}</span> : null}</li>)}</ul>
            </>
          ) : null}
          {ic ? (
            <div>
              {ic.kind !== "offer" ? <p className="small muted">{ic.note}</p> : pv.intake ? (
                <>
                  <label><input type="checkbox" checked={delIntake && !ic.blocked} disabled={busy || !!ic.blocked} onChange={(e) => setDelIntake(e.target.checked)} />このワークフローの依頼整理の会話も削除する（{pv.intake.name ?? "依頼整理"}・{pv.intake.state}）</label>
                  {ic.blocked ? <p className="small why err">削除しません: {ic.blocked}（ワークフローの削除は続けられます）</p> : <p className="small muted">オフのときは、依頼整理の会話をそのまま残します。</p>}
                </>
              ) : null}
              {intakeNote ? <p className="small why err" role="status">{intakeNote}</p> : null}
            </div>
          ) : null}
          <p className="small"><b>残るもの</b>: 手動に切り替え済みのチャット {pv.keptReleased.length}件／worktree {pv.worktrees.length}件</p>
          {pv.worktrees.length > 0 ? (
            <ul className="check-list">{pv.worktrees.map((w) => (
              <li key={w.path}><span className="mono">{w.path}</span>{w.branch ? <> ブランチ <span className="mono">{w.branch}</span></> : null}
                {!w.exists ? "（フォルダなし）" : <>　未コミット: {showKnown(w.dirty, (n) => `${n}件`)}</>}
                <span className={w.unmergedCommits.kind === "value" && w.unmergedCommits.value > 0 ? "why err" : undefined}>　未取込みコミット: {showKnown(w.unmergedCommits, (n) => `${n}件`)}</span>
                {w.unmergedCommits.kind === "value" && w.unmergedCommits.value > 0 ? <span className="small"> ブランチに残ります。取り込む場合は削除の前後どちらでも可能です。</span> : null}</li>
            ))}</ul>
          ) : null}
          <p className="small muted">worktree とブランチは削除しません。不要なら worktree の管理から個別に削除してください。記録の容量: {showKnown(pv.recordBytes, fmtBytes)}</p>
          {effective.length > 0 ? <div className="banner warn" role="status"><div><b>まだ削除できません。</b><ul>{effective.map((r, i) => <li key={i}>{r}</li>)}</ul></div></div> : null}
          {err ? <p className="why err" role="alert">{err}</p> : null}
          {busy && isSlow(elapsed) ? <div className="banner warn" role="status"><div>{SLOW_NOTICE}<div><button type="button" className="btn sm" onClick={onClose}>閉じる</button></div></div></div> : null}
        </ConfirmBlock>
      ) : null}
    </div>
  );
}
