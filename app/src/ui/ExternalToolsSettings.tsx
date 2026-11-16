import { useState } from "react";
import type { ExternalToolsSettings } from "../ipc/types";

/** `\\server\…`・`//server/…` の形か（割り当てたネットワークドライブは画面側では見分けない）。 */
export function isNetworkExe(p: string): boolean {
  return /^[\\/]{2}/.test(p.trim());
}

export interface ExternalToolsProps { value: ExternalToolsSettings; save: (value: ExternalToolsSettings) => Promise<boolean> }
export function ExternalToolsBody({ value, save }: ExternalToolsProps) {
  const [draft, setDraft] = useState(value);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  // ネットワーク上（\\server\…）の実行ファイルは、保存の前に一度確認する（確認した内容から変えたら、もう一度確認する）。
  const [confirmed, setConfirmed] = useState<string | null>(null);
  const network = [draft.editorExecutable, draft.terminalExecutable].filter(isNetworkExe);
  const key = network.join("\n");
  const asking = network.length > 0 && confirmed === key;
  const go = async () => {
    if (network.length > 0 && !asking) { setConfirmed(key); setResult(null); return; }
    setBusy(true); try { setResult(await save(draft) ? "保存しました。" : "保存できませんでした。通知の理由を確認してください。"); } finally { setBusy(false); }
  };
  return <>
    <div className="field"><span>エディタ</span><select aria-label="エディタの種類" value={draft.editor} onChange={(e) => setDraft({ ...draft, editor: e.target.value as ExternalToolsSettings["editor"] })}><option value="vsCode">VS Code</option><option value="cursor">Cursor</option></select></div>
    <div className="field"><span>エディタの実行ファイル</span><input className="mono" aria-label="エディタの実行ファイル" value={draft.editorExecutable} placeholder="C:\Program Files\Microsoft VS Code\Code.exe" onChange={(e) => setDraft({ ...draft, editorExecutable: e.target.value })} /></div>
    <div className="field"><span>ターミナル</span><select aria-label="ターミナルの種類" value={draft.terminal} onChange={(e) => setDraft({ ...draft, terminal: e.target.value as ExternalToolsSettings["terminal"] })}><option value="windowsTerminal">Windows Terminal</option><option value="powerShell">PowerShell</option></select></div>
    <div className="field"><span>ターミナルの実行ファイル</span><input className="mono" aria-label="ターミナルの実行ファイル" value={draft.terminalExecutable} placeholder="C:\Program Files\PowerShell\7\pwsh.exe" onChange={(e) => setDraft({ ...draft, terminalExecutable: e.target.value })} /></div>
    <p className="small muted">.exe の絶対パスを指定します（ネットワーク上の場所も可）。空欄は未設定です。会話内のファイルメニューからエディタでファイル・行を開き、チャット上部の「ターミナル」から現在の作業フォルダを開けます。</p>
    <p className="small muted">追加の引数やシェルコマンドは指定できません。保存時には起動せず、開く操作をしたときに実行ファイルの存在と対象パスを確認します。</p>
    {asking ? <div className="cbanner warn" role="alert">
      <p className="small">ネットワーク上の実行ファイルを指定しています: <span className="mono">{network.join("、")}</span></p>
      <p className="small">開く操作のたびに、その共有フォルダへ接続して起動します。接続時にWindowsの認証情報が相手先へ送られることがあり、共有フォルダの中身はほかの人が差し替えられます。信頼できる共有フォルダの場合だけ保存してください。</p>
    </div> : null}
    <button className="btn-line" disabled={busy} onClick={() => void go()}>{busy ? "保存しています…" : asking ? "ネットワーク上の実行ファイルとして保存" : "保存"}</button>
    {result ? <p role="status" className="small">{result}</p> : null}
  </>;
}
