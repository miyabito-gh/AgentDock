import { useState } from "react";
import type { ExternalToolsSettings } from "../ipc/types";

export interface ExternalToolsProps { value: ExternalToolsSettings; save: (value: ExternalToolsSettings) => Promise<boolean> }
export function ExternalToolsBody({ value, save }: ExternalToolsProps) {
  const [draft, setDraft] = useState(value);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const go = async () => { setBusy(true); try { setResult(await save(draft) ? "保存しました。" : "保存できませんでした。通知の理由を確認してください。"); } finally { setBusy(false); } };
  return <>
    <div className="field"><span>エディタ</span><select aria-label="エディタの種類" value={draft.editor} onChange={(e) => setDraft({ ...draft, editor: e.target.value as ExternalToolsSettings["editor"] })}><option value="vsCode">VS Code</option><option value="cursor">Cursor</option></select></div>
    <div className="field"><span>エディタの実行ファイル</span><input className="mono" aria-label="エディタの実行ファイル" value={draft.editorExecutable} placeholder="C:\Program Files\Microsoft VS Code\Code.exe" onChange={(e) => setDraft({ ...draft, editorExecutable: e.target.value })} /></div>
    <div className="field"><span>ターミナル</span><select aria-label="ターミナルの種類" value={draft.terminal} onChange={(e) => setDraft({ ...draft, terminal: e.target.value as ExternalToolsSettings["terminal"] })}><option value="windowsTerminal">Windows Terminal</option><option value="powerShell">PowerShell</option></select></div>
    <div className="field"><span>ターミナルの実行ファイル</span><input className="mono" aria-label="ターミナルの実行ファイル" value={draft.terminalExecutable} placeholder="C:\Program Files\PowerShell\7\pwsh.exe" onChange={(e) => setDraft({ ...draft, terminalExecutable: e.target.value })} /></div>
    <p className="small muted">ローカルの .exe の絶対パスを指定します。空欄は未設定です。会話内のファイルメニューからエディタでファイル・行を開き、チャット上部の「ターミナル」から現在の作業フォルダを開けます。</p>
    <p className="small muted">追加の引数やシェルコマンドは指定できません。保存時には起動せず、開く操作をしたときに実行ファイルの存在と対象パスを確認します。</p>
    <button className="btn-line" disabled={busy} onClick={() => void go()}>{busy ? "保存しています…" : "保存"}</button>
    {result ? <p role="status" className="small">{result}</p> : null}
  </>;
}
