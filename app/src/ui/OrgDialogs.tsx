// 組織実行の開始ダイアログの本文と、設定の「組織実行」タブ（停滞時間・役割定義）。P-ORG-7b。
// 開始・有効化は、押したときだけホストの命令を呼ぶ。有効化は権限の指紋を添えて確認後に送る（ホストが今の権限と照合する）。
import { Fragment, useCallback, useEffect, useState } from "react";
import type { OrgRoleView, OrgRolesView, StartOrgArgs } from "../ipc/types";
import * as host from "../ipc/client";
import { FootActions, ConfirmBlock } from "./DialogParts";

/** 開始。`start` は失敗の理由（成功は null。成功時に呼び出し側がダイアログを閉じる）。 */
export function OrgStartBody({ start, browse }: {
  start: (a: StartOrgArgs) => Promise<string | null>;
  browse: () => Promise<string | null>;
}) {
  const [request, setRequest] = useState("");
  const [repo, setRepo] = useState("");
  const [writers, setWriters] = useState("");
  const [total, setTotal] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const num = (s: string): number | null | undefined => (s.trim() === "" ? null : /^[1-4]$/.test(s.trim()) ? Number(s.trim()) : undefined);
  const w = num(writers), t = num(total);
  const bad = w === undefined || t === undefined ? "並列数は1〜4の整数で入れてください（空欄なら既定）" : null;
  const can = request.trim() !== "" && repo.trim() !== "" && !bad && !busy;
  const go = async () => {
    if (!can) return;
    setBusy(true); setErr(null);
    const e = await start({ request: request.trim(), repo: repo.trim(), maxWriters: w ?? null, maxTotal: t ?? null });
    setBusy(false);
    if (e) setErr(e);
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
      <div className="field"><span>並列数（任意）</span>
        <div style={{ display: "flex", gap: 10, alignItems: "center" }}>
          <label>Worker 同時 <input type="text" inputMode="numeric" size={3} value={writers} placeholder="2" onChange={(e) => setWriters(e.target.value)} aria-label="Worker の同時実行数（1〜4）" /></label>
          <label>全体 <input type="text" inputMode="numeric" size={3} value={total} placeholder="3" onChange={(e) => setTotal(e.target.value)} aria-label="全体の同時実行数（1〜4）" /></label>
        </div>
        <span className="note">1〜4。空欄なら既定（Worker 同時2・全体3）です。</span></div>
      <div className="banner info" role="note"><div>
        開始しても、すぐタスクが実行されるとは限りません。Planner が計画を作り、確認が必要な内容（あなたの作業ツリーへの書込み・push・ネットワーク取得・削除など）が含まれる計画は、<b>計画承認カードで承認するまで</b>タスクを送りません。確認が不要な計画だけが自動で承認されます。AgentDock は自動でマージしません。
      </div></div>
      {bad ? <p className="small why err" role="alert">{bad}</p> : null}
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

/** 設定の「組織実行」タブ。役割は定義ファイルで編集する（この画面では編集しない）。 */
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
  const enable = async (r: OrgRoleView) => {
    setBusy(true);
    try { setRoles(await host.orgEnableRole(r.id, r.fingerprint)); setMsg(`役割「${r.displayName}」を有効にしました。`); setEnabling(null); }
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
    </>
  );
}
