// 組織実行の開始ダイアログの本文と、設定の「組織実行」タブ（停滞時間・役割定義・グラフ定義）。P-ORG-7b、G-A5。
// 開始・有効化は、押したときだけホストの命令を呼ぶ。テンプレートを選ぶ・見積りを見る・定義ファイルを開くだけでは何も送らない。
// 有効化は権限の指紋を添えて確認後に送る（ホストが今の権限と照合する）。
import { Fragment, useCallback, useEffect, useState } from "react";
import type { ChatKey, OrgDeleteOutcome, OrgDeletePreview, OrgGraphProblemView, OrgGraphTemplateView, OrgGraphsView, OrgRoleView, OrgRolesView, StartOrgArgs } from "../ipc/types";
import * as host from "../ipc/client";
import { FootActions, ConfirmBlock } from "./DialogParts";
import { estimateLines } from "./org";
import { showKnown } from "./format";
import { fmtBytes } from "./ManageDialogs";

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

/** 開始。`start` は失敗の理由（成功は null。成功時に呼び出し側がダイアログを閉じる）。テンプレートを選ぶ・見積りを見るだけでは何も送らない。 */
export function OrgStartBody({ start, browse, live, loadGraphs }: {
  start: (a: StartOrgArgs) => Promise<string | null>;
  browse: () => Promise<string | null>;
  live: boolean;
  loadGraphs: LoadGraphs;
}) {
  const [request, setRequest] = useState("");
  const [repo, setRepo] = useState("");
  const [writers, setWriters] = useState("");
  const [total, setTotal] = useState("");
  const [turns, setTurns] = useState("");
  const [sel, setSel] = useState(STANDARD_ID);
  const [graphs, setGraphs] = useState<OrgGraphsView | null>(null);
  const [gErr, setGErr] = useState<string | null>(null);
  const [gMsg, setGMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const loadG = useCallback(async (reload: boolean) => {
    setGErr(null);
    try {
      let v = await loadGraphs(reload);
      if (!reload && !v.loaded) v = await loadGraphs(true); // 初回は読み込み（足りない組込みテンプレートを作る）
      setGraphs(v);
    } catch (e) { setGErr(host.asIpcError(e).message); }
  }, [loadGraphs]);
  useEffect(() => { void loadG(false); }, [loadG]);
  const num = (s: string): number | null | undefined => (s.trim() === "" ? null : /^[1-4]$/.test(s.trim()) ? Number(s.trim()) : undefined);
  const w = num(writers), t = num(total);
  const bad = w === undefined || t === undefined ? "並列数は1〜4の整数で入れてください（空欄なら既定）" : null;
  const turnLimit = graphs?.maxTurnsLimit ?? 200;
  const turnDefault = graphs?.defaultMaxTurns ?? 30;
  const mt = turns.trim() === "" ? null : /^\d+$/.test(turns.trim()) && Number(turns.trim()) >= 1 && Number(turns.trim()) <= turnLimit ? Number(turns.trim()) : undefined;
  const badTurns = mt === undefined ? `turn 上限は 1〜${turnLimit} の整数で入れてください（空欄なら既定 ${turnDefault}）` : null;
  const templates = graphs?.templates ?? [];
  const std = templates.find((x) => x.id === STANDARD_ID) ?? null;
  const others = templates.filter((x) => x.id !== STANDARD_ID);
  const stdProblem = graphs?.problems.find((x) => x.file === `${STANDARD_ID}.json`) ?? null;
  const otherProblems = (graphs?.problems ?? []).filter((x) => x.file !== `${STANDARD_ID}.json`);
  const chosen: OrgGraphTemplateView | null = sel === STANDARD_ID ? std : others.find((x) => x.id === sel) ?? null;
  // 選んでいたテンプレートが読み直しで選べなくなったら、標準に戻す（誤りのある定義では始めない）。
  useEffect(() => { if (graphs && sel !== STANDARD_ID && !others.some((x) => x.id === sel && x.selectable)) setSel(STANDARD_ID); }, [graphs, sel, others]);
  const effTurns = mt ?? turnDefault;
  const est = graphs ? estimateLines(chosen?.estimate ?? null, graphs.tokensPerTurnHint, effTurns) : null;
  const can = request.trim() !== "" && repo.trim() !== "" && !bad && !badTurns && !busy && (sel === STANDARD_ID || !!chosen?.selectable);
  const go = async () => {
    if (!can) return;
    setBusy(true); setErr(null);
    const e = await start({ request: request.trim(), repo: repo.trim(), maxWriters: w ?? null, maxTotal: t ?? null, graphId: sel === STANDARD_ID ? null : sel, maxTurns: mt ?? null });
    setBusy(false);
    if (e) setErr(e);
  };
  const openFile = async (file: string) => {
    try { await host.orgOpenGraphFile(file); setGMsg(`「${file}」を関連アプリで開きました。編集後は「再読み込み」を押してください。`); }
    catch (e) { setGMsg(`開けませんでした: ${host.asIpcError(e).message}`); }
  };
  return (
    <div className="content">
      <div className="field"><span>依頼</span><textarea rows={5} style={{ width: "100%" }} value={request} onChange={(e) => setRequest(e.target.value)} aria-label="組織実行の依頼" placeholder="例: ○○機能を実装し、テストを足す" autoFocus /></div>
      <div className="field"><span>作業フォルダ</span>
        <div style={{ display: "flex", gap: 4 }}>
          <input type="text" className="mono grow" value={repo} onChange={(e) => setRepo(e.target.value)} placeholder="git リポジトリのフルパス" aria-label="作業フォルダのパス" />
          <button type="button" className="btn sm" onClick={() => void browse().then((p) => { if (p) setRepo(p); })}>参照…</button>
        </div>
        <span className="note">git リポジトリのフォルダです。Worker はこの中の専用 worktree で作業し、あなたのブランチ・作業ツリーには書き込みません。</span></div>
      <fieldset className="field org-tpls"><legend>テンプレート（進め方）</legend>
        <label className="org-tpl">
          <input type="radio" name="org-tpl" checked={sel === STANDARD_ID} onChange={() => setSel(STANDARD_ID)} />
          <span><b>{std?.name ?? "計画して実行（標準）"}</b> <span className="mono muted">{STANDARD_ID}</span>
            <span className="note">{std?.description || "Planner が計画を作り、Worker と Reviewer が実行・検証します（従来の開始と同じ動きです）。"}</span></span>
        </label>
        {stdProblem ? <p className="small muted">standard.json に誤りがありますが、標準は定義ファイルを使わず従来の計画経路で始めるため、影響しません（誤り: {stdProblem.reasons.join("、")}）。</p> : null}
        {others.map((x) => (
          <label key={x.id} className={`org-tpl${x.selectable ? "" : " off"}`}>
            <input type="radio" name="org-tpl" disabled={!x.selectable} checked={sel === x.id} onChange={() => setSel(x.id)} />
            <span><b>{x.name}</b> <span className="mono muted">{x.id}</span>
              <span className="note">{x.description || "（説明なし）"}</span>
              <span className="note">置き場: {x.workspace === "git" ? "Git" : "フォルダ（Git なし）"}　使う役割: {x.roles.join("、") || "—"}{x.suggestedMaxTurns !== null ? `　提案の turn 上限: ${x.suggestedMaxTurns}` : ""}</span>
              {!x.selectable ? <span className="note why err" role="status">選べません: {x.reasons.join("、")}</span> : null}</span>
          </label>
        ))}
        {otherProblems.length > 0 ? (
          <div className="banner warn" role="status"><div><b>使えない定義があります（選べません）。</b>
            <GraphProblems problems={otherProblems} onOpen={live ? (f) => void openFile(f) : undefined} /></div></div>
        ) : null}
        <div style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
          <button type="button" className="btn sm" disabled={!live || busy} onClick={() => void loadG(true)} title="定義ファイルを読み直します（何も送りません）">再読み込み</button>
          {chosen && sel !== STANDARD_ID && live ? <button type="button" className="btn sm" onClick={() => void openFile(chosen.file)}>定義ファイルを開く</button> : null}
          {!graphs && !gErr ? <span className="small muted">テンプレートを読み込んでいます…</span> : null}
        </div>
        {gErr ? <p className="small why err" role="alert">テンプレートを取得できませんでした（{gErr}）。標準（従来の計画経路）では始められます。</p> : null}
        {gMsg ? <p className="small" role="status">{gMsg}</p> : null}
      </fieldset>
      <div className="field"><span>並列数（任意）</span>
        <div style={{ display: "flex", gap: 10, alignItems: "center" }}>
          <label>Worker 同時 <input type="text" inputMode="numeric" size={3} value={writers} placeholder="2" onChange={(e) => setWriters(e.target.value)} aria-label="Worker の同時実行数（1〜4）" /></label>
          <label>全体 <input type="text" inputMode="numeric" size={3} value={total} placeholder="3" onChange={(e) => setTotal(e.target.value)} aria-label="全体の同時実行数（1〜4）" /></label>
        </div>
        <span className="note">1〜4。空欄なら既定（Worker 同時2・全体3）です。</span></div>
      <div className="field"><span>turn 上限（任意）</span>
        <div><input type="text" inputMode="numeric" size={5} value={turns} placeholder={String(turnDefault)} onChange={(e) => setTurns(e.target.value)} aria-label="turn 上限" /></div>
        <span className="note">ワークフロー全体で送れる turn の数です。上限に達すると、その場で止まります（あとから上げて続けられます）。空欄なら既定（{turnDefault}）です。</span></div>
      <div className="banner info" role="note"><div>
        <b>見積り（{chosen?.name ?? (sel === STANDARD_ID ? "標準" : sel)}）</b>
        {est ? est.lines.map((l, i) => <div key={i} className={i > 0 && est.over ? "why err" : undefined}>{l}</div>) : <div>読み込み中…</div>}
        <div className="small muted">定義から計算した上限の目安です。実際の使用量ではありません。</div>
      </div></div>
      <div className="banner info" role="note"><div>
        開始しても、すぐタスクが実行されるとは限りません。{sel === STANDARD_ID
          ? <>Planner が計画を作り、確認が必要な内容（あなたの作業ツリーへの書込み・push・ネットワーク取得・削除など）が含まれる計画は、<b>計画承認カードで承認するまで</b>タスクを送りません。確認が不要な計画だけが自動で承認されます。</>
          : <>テンプレートの流れに沿って、工程ごとに役割のチャットへ送ります。判断を求める工程（gate）は、あなたが選ぶまで進みません。周回が上限に達したときも、あなたの判断を待ちます。</>}
        AgentDock は自動でマージしません。
      </div></div>
      {bad ? <p className="small why err" role="alert">{bad}</p> : null}
      {badTurns ? <p className="small why err" role="alert">{badTurns}</p> : null}
      {err ? <div className="why err" role="alert">{err}</div> : null}
      <FootActions main={<button className="btn primary" disabled={!can} onClick={() => void go()}>{busy ? "開始しています…" : "開始"}</button>} />
    </div>
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
                <div className="small muted">置き場: {x.workspace === "git" ? "Git" : "フォルダ（Git なし）"}　使う役割: {x.roles.join("、") || "—"}　最大 {x.estimate.turnsMax} turn（通常 {x.estimate.turnsTypical}）</div>
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
export function OrgDeleteDialog({ workflowId, live, preview, remove, onDone }: {
  workflowId: string;
  live: boolean;
  preview: (id: string) => Promise<OrgDeletePreview>;
  remove: (id: string, deleteChats: boolean, expect: ChatKey[]) => Promise<OrgDeleteOutcome>;
  onDone: () => void;
}) {
  const [pv, setPv] = useState<OrgDeletePreview | null>(null);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [delChats, setDelChats] = useState(true);
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
  const run = async () => {
    if (!pv || !can) return;
    setBusy(true); setErr(null);
    try {
      const o = await remove(workflowId, delChats, pv.chats.map((c) => c.chat));
      if (o.kind === "deleted") { onDone(); return; }
      setPartial({ done: o.done, failed: o.failed });
      await load();
    } catch (e) { setErr(host.asIpcError(e).message); }
    setBusy(false);
  };
  const label = busy ? "削除しています…" : resuming || partial ? "削除を続ける" : "削除する";
  const nm = (c: OrgDeletePreview["chats"][number]) => c.name ?? c.chat.id;
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
        </ConfirmBlock>
      ) : null}
    </div>
  );
}
