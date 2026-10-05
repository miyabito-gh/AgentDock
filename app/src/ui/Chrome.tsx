import { Icon } from "./Icon";
import type { SourceInfo } from "../ipc/types";

export function TitleBar({ title, mini, top, onAct }: { title: string; mini?: boolean; top?: boolean; onAct: (a: string) => void }) {
  return (
    <div className="titlebar">
      <span className="app-mark" aria-hidden="true" />
      <span className="title">{title}</span>
      {mini ? null : <span className="mock-tag">モック</span>}
      <span className="grow" />
      {mini ? (
        <label className="small"><input type="checkbox" checked={!!top} onChange={() => onAct("miniTop")} />最前面</label>
      ) : null}
      <button className="winbtn" aria-label="最小化" onClick={() => onAct("noop")}><Icon name="min" /></button>
      {mini ? null : <button className="winbtn" aria-label="最大化" onClick={() => onAct("noop")}><Icon name="max" /></button>}
      <button className="winbtn close" aria-label={mini ? "監視窓を閉じる" : "閉じる（トレイに格納）"} onClick={() => onAct(mini ? "closeMini" : "closeToTray")}><Icon name="x" /></button>
    </div>
  );
}

type MenuItem = [label: string, action: string, kbd?: string] | "-";
const MENUS: Record<string, MenuItem[]> = {
  "チャット": [["新しいチャット", "newChat", "Ctrl+N"], ["名前を変更", "stub"], ["ピン留めを切替", "stub"], ["アーカイブ", "stub"], ["Markdown にエクスポート…", "stub"], "-", ["削除…", "stub"], "-", ["AgentDock を終了…", "quit"]],
  "作業": [["差分を確認", "stub"], ["変更を戻す…", "stub"], ["コードレビュー…", "stub"], ["計画／実行の切替", "unv:Plan（collaborationMode）は experimental"], ["会話を分岐", "stub"], ["文脈を圧縮", "stub"], ["Goal を設定…", "stub"], ["worktree で開始…", "unv:worktree の公開経路は未確認"], ["クラウドに委任…", "unv:クラウド委任の公開経路は未確認"], "-", ["中断", "interrupt", "Esc"]],
  "表示": [["チャット一覧", "toggleLeft"], ["エージェントのドック", "toggleRight"], ["コンパクト監視窓", "toggleMini"], "-", ["終了済みも表示（全チャット）", "toggleDone"]],
  "設定": [["設定…", "settings:general"], ["MCP・Plugins…", "settings:mcp"], ["Skills・AGENTS.md…", "settings:skills"]],
  "ヘルプ": [["状態の見方", "stub"], ["Codex の状態", "stub"], ["バージョン情報", "stub"]],
};

export function MenuBar({ open, setOpen, checked, onAct }: {
  open: string | null; setOpen: (m: string | null) => void; checked: Record<string, boolean>; onAct: (a: string) => void;
}) {
  return (
    <nav className="menubar" aria-label="メニュー">
      {Object.keys(MENUS).map((m) => (
        <div className="menu" key={m}>
          <button aria-haspopup="true" aria-expanded={open === m} onClick={() => setOpen(open === m ? null : m)}>{m}</button>
          {open === m ? (
            <div className="dropdown" role="menu">
              {MENUS[m].map((it, i) => it === "-" ? <hr key={i} /> : (
                <button key={i} role="menuitem" onClick={() => { setOpen(null); onAct(it[1]); }}>
                  <span>{checked[it[1]] ? "✓ " : ""}{it[0]}</span>
                  {it[1].startsWith("unv:") ? <span className="tag unv">未確認</span> : it[2] ? <span className="kbd">{it[2]}</span> : null}
                </button>
              ))}
            </div>
          ) : null}
        </div>
      ))}
    </nav>
  );
}

/** 全体バナー。接続状態と版の不一致をSourceInfoから作る。 */
export function GlobalBanner({ source, onAct }: { source: SourceInfo | undefined; onAct: (a: string) => void }) {
  if (!source) return null;
  const c = source.connection;
  let text: string | null = null;
  let cls = "warn";
  if (c.kind === "launchFailed") { text = `Codex を起動できませんでした。${c.message}`; cls = "err"; }
  else if (c.kind === "disconnected") { text = `Codex との接続が切れています${c.message ? `（${c.message}）` : ""}。状態は最後に受信した内容のままで、完了・失敗を意味しません。`; cls = "err"; }
  else if (source.version.kind === "mismatch") text = `Codex は ${source.version.actual} です。対象版は ${source.version.expected} のため、一部の機能が動かない可能性があります。`;
  else if (source.version.kind === "unknown") text = `Codex の版を確認できません（${source.version.message}）。`;
  if (!text) return null;
  return (
    <div className={`gbanner ${cls}`} role="alert">
      <Icon name="alert" /><span className="grow">{text}</span>
      <button className="btn-line" onClick={() => onAct("settings:codex")}>Codex の設定を開く</button>
      {c.kind === "launchFailed" || c.kind === "disconnected" ? <button className="btn-line" onClick={() => onAct("retryLaunch")}>再試行</button> : null}
    </div>
  );
}
