// 設定「MCP・Plugins」タブの本体（段階③ P3-5）。
// 規則: 一覧は読取りだけ。一覧に出ることを既存の会話への反映と書かない（その項目は「未確認」のまま）。
//       導入・削除・サーバーの追加・削除は確認画面の後だけ。実行前後に設定を照合した結果を出し、自動では元に戻さない。
//       認可URLは「ブラウザで開く」を押したときだけ開き、画面にも出さない。完了通知が来ないときは「完了未確認」（失敗とは言わない）。
//       再読込みはボタンのときだけ。受付は既存の会話への反映の確認ではない。
import { useCallback, useEffect, useState } from "react";
import * as host from "../ipc/client";
import type { ConfigCompare, ExtensionOpRecord, ExtensionOpStatus, ExtensionView, OpCapability, ToolServerAuth, ToolServerConnection, ToolServerList, ToolServerLogin } from "../ipc/types";
import { UnverifiedTag } from "./PrefsDialogs";
import { showKnown } from "./format";
import { About } from "./DialogParts";

const errText = (e: unknown) => host.asIpcError(e).message;

const CONN_LABEL: Record<ToolServerConnection, string> = {
  notStarted: "未起動", starting: "起動中", connected: "接続中", authRequired: "認証が必要", failed: "失敗", cancelled: "取消", disabled: "無効", unknown: "不明（未取得）",
};
const AUTH_LABEL: Record<ToolServerAuth, string> = {
  notLoggedIn: "未ログイン", bearer: "トークン", oAuth: "OAuth", unsupported: "認証なし／非対応", unknown: "不明（未取得）",
};

function loginText(l: ToolServerLogin | undefined): string | null {
  if (!l) return null;
  switch (l.state.kind) {
    case "waiting": return "認可の完了を待っています（ブラウザでの操作後に更新されます）";
    case "completed": return l.state.success ? "認可の完了通知を受け取りました" : "認可は完了しませんでした（失敗の通知）";
    case "completionUnconfirmed": return "完了未確認（5分以内に完了の通知がありませんでした。失敗とは限りません）";
  }
}

function statusText(s: ExtensionOpStatus, code: string): string {
  switch (s.kind) {
    case "exitedZero": return `コマンドは正常に終了しました（終了コード ${code}）。結果は下の設定の照合と一覧で確認してください`;
    case "exitedNonZero": return `コマンドは失敗を返しました（終了コード ${code}）`;
    case "resultUnconfirmed": return `結果未確認: ${s.reason}。再実行せず、一覧と設定で状態を確認してください`;
    case "notRun": return `実行していません: ${s.reason}`;
  }
}

function compareText(c: ConfigCompare): string {
  switch (c.kind) {
    case "unchanged": return "設定に変化はありません";
    case "changedAsExpected": return "操作の対象の項目だけが変わりました";
    case "unexpected": return `想定外の項目が変わっています（自動では元に戻しません）: ${c.keys.join("、")}`;
    case "unverified": return `照合できませんでした: ${c.reason}`;
  }
}

const KIND_LABEL: Record<ExtensionOpRecord["kind"], string> = { install: "Plugin導入", remove: "Plugin削除", addToolServer: "サーバー追加", removeToolServer: "サーバー削除" };

/** 管理操作の記録1件（開始だけで終了の記録がなければ「結果未確認」）。 */
function OpRecordView({ r }: { r: ExtensionOpRecord }) {
  const res = r.result;
  const code = res ? showKnown(res.exitCode) : "";
  return (
    <li className="small" style={{ marginBottom: 6 }}>
      <b>{KIND_LABEL[r.kind]}</b> <span className="mono">{r.target}</span>　{new Date(r.at).toLocaleString()}
      {res ? (
        <div>
          <div className={res.status.kind === "exitedNonZero" || res.status.kind === "resultUnconfirmed" ? "why err" : undefined}>{statusText(res.status, code)}</div>
          <div className={res.configCompare.kind === "unexpected" || res.configCompare.kind === "unverified" ? "why err" : undefined}>設定の照合: {compareText(res.configCompare)}</div>
          <div className="muted">設定ファイルのハッシュ: 前 {showKnown(res.configHashBefore, (h) => h.slice(0, 8))} → 後 {showKnown(res.configHashAfter, (h) => h.slice(0, 8))}</div>
          {res.outputSummary ? <div className="muted">{res.summaryIsRaw ? "出力（想定外の形式のため、原文の先頭）" : "出力"}: <span className="mono" style={{ whiteSpace: "pre-wrap" }}>{res.outputSummary}</span></div> : null}
        </div>
      ) : <div className="why err">結果未確認（開始の記録だけが残っています。一覧と設定で状態を確認してください）</div>}
    </li>
  );
}

export function ExtensionsBody({ live, toolCap, extCap }: { live: boolean; toolCap: OpCapability | undefined; extCap: OpCapability | undefined }) {
  const [servers, setServers] = useState<ToolServerList | null>(null);
  const [serversErr, setServersErr] = useState<string | null>(null);
  const [exts, setExts] = useState<ExtensionView[] | null>(null);
  const [extsErr, setExtsErr] = useState<string | null>(null);
  const [ops, setOps] = useState<ExtensionOpRecord[]>([]);
  const [urls, setUrls] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [pluginId, setPluginId] = useState("");
  const [srvName, setSrvName] = useState("");
  const [srvCmd, setSrvCmd] = useState("");
  const [last, setLast] = useState<ExtensionOpRecord | null>(null);

  const loadServers = useCallback(() => {
    host.listToolServers().then((l) => { setServers(l); setServersErr(null); }).catch((e) => setServersErr(errText(e)));
  }, []);
  const loadExts = useCallback(() => {
    host.listExtensions().then((l) => { setExts(l); setExtsErr(null); }).catch((e) => setExtsErr(errText(e)));
  }, []);
  const loadOps = useCallback(() => { host.listExtensionOps().then(setOps).catch(() => undefined); }, []);

  useEffect(() => { if (live) { loadServers(); loadExts(); loadOps(); } }, [live, loadServers, loadExts, loadOps]);
  // 通知は取り直しの合図にだけ使う（取り逃しても、再読み込みで取り直せる）。
  useEffect(() => {
    if (!live) return;
    const un = host.subscribeHostEvents((e) => {
      const k = e.event.kind;
      if (k === "toolServersChanged" || k === "toolServerLoginUpdated") loadServers();
      else if (k === "extensionOpUpdated") loadOps();
    });
    return () => { void un.then((f) => f()); };
  }, [live, loadServers, loadOps]);

  if (!live) return <p className="small muted">モックでは動きません（実接続で使えます）。</p>;

  const startLogin = async (name: string) => {
    setBusy(true); setMsg(null);
    try { const r = await host.loginToolServer(name); setUrls((u) => ({ ...u, [name]: r.authorizationUrl })); setMsg(`「${name}」の認可を始めました。「ブラウザで開く」を押すと認可の画面が開きます。`); loadServers(); }
    catch (e) { setMsg(`認可を始められませんでした: ${errText(e)}`); } finally { setBusy(false); }
  };
  const openUrl = async (name: string) => {
    const url = urls[name];
    if (!url) return;
    try { await host.openAuthorizationUrl(url); } catch (e) { setMsg(`ブラウザで開けませんでした: ${errText(e)}`); }
  };
  const reload = async () => {
    setBusy(true); setMsg(null);
    try {
      const a = await host.reloadToolServers();
      setMsg(a.kind === "accepted" ? "再読込みの要求は受け付けられました。既存の会話に反映されたかは未確認です（新しい会話で使えるかは別に確認してください）。" : a.kind === "rejected" ? `再読込みは拒否されました: ${a.message}` : `再読込みの結果は不明です: ${a.message}`);
      loadServers();
    } catch (e) { setMsg(`再読込みを要求できませんでした: ${errText(e)}`); } finally { setBusy(false); }
  };
  const run = async (op: Parameters<typeof host.manageExtension>[0], what: string, detail: string) => {
    if (!window.confirm(`${what}\n\ncodex.exe のコマンドを実行します（${detail}）。~/.codex の設定が変わる可能性があります。\n実行の前後に設定を読み直して照合し、結果を表示します。自動では元に戻しません。\n\n実行しますか？`)) return;
    setBusy(true); setMsg(null); setLast(null);
    try { setLast(await host.manageExtension(op)); } catch (e) { setMsg(`実行できませんでした: ${errText(e)}`); } finally { setBusy(false); loadServers(); loadExts(); loadOps(); }
  };

  const loginOf = (name: string) => servers?.logins.find((l) => l.name === name);
  return (
    <>
      <h4 style={{ margin: "0 0 4px" }}>MCPサーバー <UnverifiedTag cap={toolCap} /></h4>
      <p className="small muted">いまの接続状態です。一覧に出ていることは、既存の会話にそのサーバーが反映されていることを意味しません（会話への反映: 未確認）。</p>
      <div style={{ marginBottom: 6 }}>
        <button className="btn-line" disabled={busy} onClick={loadServers}>一覧を読み込み直す</button>{" "}
        <button className="btn-line" disabled={busy} onClick={() => void reload()}>MCP設定の再読込みを要求</button>
      </div>
      {serversErr ? <div className="why err">一覧を取得できませんでした（未取得）: {serversErr}</div> : null}
      {servers && servers.servers.length === 0 ? <p className="small">設定されたサーバーはありません。</p> : null}
      {servers && servers.servers.length > 0 ? (
        <table className="parity-table">
          <thead><tr><th>名前</th><th>接続</th><th>認証</th><th>ツール数</th><th>操作</th></tr></thead>
          <tbody>
            {servers.servers.map((s) => (
              <tr key={s.name}>
                <td className="mono">{s.name}</td>
                <td>{CONN_LABEL[s.connection]}</td>
                <td>{AUTH_LABEL[s.auth]}{loginText(loginOf(s.name)) ? <div className="small muted">{loginText(loginOf(s.name))}</div> : null}</td>
                <td>{showKnown(s.toolCount)}{s.toolsError ? <div className="small why err">取得失敗: {s.toolsError}</div> : null}</td>
                <td>
                  {s.auth === "oAuth" || s.auth === "notLoggedIn" || s.connection === "authRequired" ? <button className="btn-line" disabled={busy} onClick={() => void startLogin(s.name)}>認可を始める</button> : null}
                  {urls[s.name] ? <button className="btn-line" onClick={() => void openUrl(s.name)}>ブラウザで開く</button> : null}
                  <button className="btn-line" disabled={busy} onClick={() => void run({ kind: "removeToolServer", name: s.name }, `MCPサーバー「${s.name}」を削除します。`, `codex mcp remove ${s.name}`)}>削除…</button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
      <div className="field" style={{ marginTop: 8 }}><span>サーバーを追加（stdio）</span>
        <div style={{ display: "flex", gap: 6 }}>
          <input type="text" className="mono" style={{ width: 120 }} placeholder="名前" value={srvName} onChange={(e) => setSrvName(e.target.value)} aria-label="サーバー名" />
          <input type="text" className="mono" style={{ flex: 1 }} placeholder="起動コマンド（例: npx -y パッケージ名）" value={srvCmd} onChange={(e) => setSrvCmd(e.target.value)} aria-label="起動コマンド" />
          <button className="btn-line" disabled={busy || !srvName.trim() || !srvCmd.trim()} onClick={() => void run({ kind: "addToolServer", name: srvName.trim(), command: srvCmd.trim().split(/\s+/) }, `MCPサーバー「${srvName.trim()}」を追加します。`, `codex mcp add ${srvName.trim()} -- …`)}>追加…</button>
        </div>
        <span className="note">起動コマンドは空白で区切ります（空白を含む引数は指定できません）。環境変数・HTTPサーバーの追加は codex のコマンドで行ってください。起動コマンドは操作の記録に残しません。</span>
      </div>
      <hr />
      <h4 style={{ margin: "0 0 4px" }}>Plugins <UnverifiedTag cap={extCap} /></h4>
      <p className="small muted">一覧は読取りです。導入・削除は codex のコマンドを実行します。キャッシュの有無と会話への提示は取得できないため「未確認」と表示します。</p>
      {extsErr ? <div className="why err">一覧を取得できませんでした（未取得）: {extsErr}</div> : null}
      {exts && exts.length === 0 ? <p className="small">表示できるPluginはありません。</p> : null}
      {exts && exts.length > 0 ? (
        <table className="parity-table">
          <thead><tr><th>Plugin</th><th>導入</th><th>有効（設定）</th><th>キャッシュ</th><th>会話への提示</th><th>操作</th></tr></thead>
          <tbody>
            {exts.map((x) => (
              <tr key={x.id}>
                <td><span className="mono">{x.id}</span></td>
                <td>{showKnown(x.installed, (b) => (b ? "導入済み" : "未導入"))}</td>
                <td>{showKnown(x.enabledInConfig, (b) => (b ? "有効" : "無効"))}</td>
                <td>{showKnown(x.cachePresent, (b) => (b ? "あり" : "なし"))}</td>
                <td>{showKnown(x.advertisedInChat, (b) => (b ? "提示されている" : "提示されていない"))}</td>
                <td>{x.installed.kind === "value" && x.installed.value
                  ? <button className="btn-line" disabled={busy} onClick={() => void run({ kind: "remove", id: x.id }, `Plugin「${x.id}」を削除します（ローカルのキャッシュも消えます）。`, `codex plugin remove ${x.id}`)}>削除…</button>
                  : <button className="btn-line" disabled={busy} onClick={() => void run({ kind: "install", id: x.id }, `Plugin「${x.id}」を導入します。`, `codex plugin add ${x.id}`)}>導入…</button>}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
      <div className="field" style={{ marginTop: 8 }}><span>Pluginを導入</span>
        <div style={{ display: "flex", gap: 6 }}>
          <input type="text" className="mono" style={{ flex: 1 }} placeholder="名前@マーケットプレイス" value={pluginId} onChange={(e) => setPluginId(e.target.value)} aria-label="Pluginの識別子" />
          <button className="btn-line" disabled={busy || !pluginId.trim()} onClick={() => void run({ kind: "install", id: pluginId.trim() }, `Plugin「${pluginId.trim()}」を導入します。`, `codex plugin add ${pluginId.trim()}`)}>導入…</button>
        </div>
      </div>
      {msg ? <p className="small" style={{ marginTop: 6 }}>{msg}</p> : null}
      {last ? <><h4 style={{ margin: "10px 0 4px" }}>実行結果</h4><ul style={{ margin: 0, paddingLeft: 18 }}><OpRecordView r={last} /></ul></> : null}
      <hr />
      <h4 style={{ margin: "0 0 4px" }}>管理操作の記録</h4>
      <About><p>操作の種類・対象の名前・終了コード・設定の照合だけを記録します（起動コマンド・認証情報は記録しません）。</p></About>
      {ops.length === 0 ? <p className="small">記録はありません。</p> : <ul style={{ margin: 0, paddingLeft: 18 }}>{ops.map((r) => <OpRecordView key={r.id} r={r} />)}</ul>}
    </>
  );
}
