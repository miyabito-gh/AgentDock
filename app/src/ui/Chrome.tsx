import { useEffect } from "react";
import { Icon } from "./Icon";
import type { CommandView } from "./commands";
import type { SaveScope, SaveStatus, SourceInfo, StartupWarning } from "../ipc/types";

export function TitleBar({ title, tag, mini, top, onAct }: { title: string; tag?: string; mini?: boolean; top?: boolean; onAct: (a: string) => void }) {
  return (
    <div className="titlebar">
      <span className="app-mark" aria-hidden="true" />
      <span className="title">{title}</span>
      {mini || !tag ? null : <button className="mock-tag" title="接続先（実接続／モック）を切り替える" aria-haspopup="true" onClick={(e) => { e.stopPropagation(); onAct("modeMenu"); }}>{tag}</button>}
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
  "チャット": [["新しいチャット", "newChat", "Ctrl+N"], ["名前を変更…", "renameChat"], ["ピン留めを切替", "togglePin"], ["一覧を更新", "refreshList"], ["アーカイブ", "archiveChat"], ["アーカイブを解除", "unarchiveChat"], ["Markdown にエクスポート…", "exportMd"], "-", ["削除…", "deleteChat"], "-", ["AgentDock を終了…", "quit"]],
  "表示": [["チャット一覧", "toggleLeft"], ["エージェントのドック", "toggleRight"], ["コンパクト監視窓", "toggleMini"], ["コマンドメニュー…", "palette", "Ctrl+K"], "-", ["通常画面を最前面に", "toggleMainTop"], ["監視窓を最前面に", "toggleMiniTop"], "-", ["終了済みも表示（全チャット）", "toggleDone"]],
  "設定": [["設定…", "settings:general"], ["MCP・Plugins…", "settings:mcp"], ["Skills・AGENTS.md…", "settings:skills"]],
  "ヘルプ": [["状態の見方", "stub"], ["Codex の状態", "status"], ["同等性の確認状況…", "parity"], ["バージョン情報", "stub"]],
};

/** 上部メニューの並び。「作業」は操作台帳（commands.ts）から作る。 */
const ORDER = ["チャット", "作業", "表示", "設定", "ヘルプ"];

export function MenuBar({ open, setOpen, checked, views, onAct, tag }: {
  open: string | null; setOpen: (m: string | null) => void; checked: Record<string, boolean>; views: CommandView[]; onAct: (a: string) => void;
  /** 接続先の版タグ（右端。押すと接続先の切替）。 */
  tag?: string;
}) {
  // Escで閉じる。外側クリックはウィンドウ側（App）で閉じる。
  useEffect(() => {
    if (!open) return;
    const h = (e: KeyboardEvent) => { if (e.key === "Escape") setOpen(null); };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [open, setOpen]);
  return (
    <nav className="menubar" aria-label="メニュー">
      {ORDER.map((m) => (
        <div className="menu" key={m}>
          <button aria-haspopup="true" aria-expanded={open === m} onClick={(e) => { e.stopPropagation(); setOpen(open === m ? null : m); }}>{m}</button>
          {open === m ? (
            <div className="dropdown" role="menu">
              {m === "作業" ? <WorkItems views={views} onPick={(a) => { setOpen(null); onAct(a); }} /> : MENUS[m].map((it, i) => it === "-" ? <hr key={i} /> : (
                <button key={i} role="menuitem" onClick={(e) => { e.stopPropagation(); setOpen(null); onAct(it[1]); }}>
                  <span>{checked[it[1]] ? "✓ " : ""}{it[0]}</span>
                  {it[1].startsWith("unv:") ? <span className="tag unv">未確認</span> : it[2] ? <span className="kbd">{it[2]}</span> : null}
                </button>
              ))}
            </div>
          ) : null}
        </div>
      ))}
      <span className="grow" />
      {tag ? <button className="mock-tag" title="接続先（実接続／モック）を切り替える" aria-haspopup="true" onClick={(e) => { e.stopPropagation(); onAct("modeMenu"); }}>{tag}</button> : null}
    </nav>
  );
}

/** 「作業」メニューの中身。台帳の順に、無効なものは理由を添えて表示する。文字のみ（アイコンなし）。 */
function WorkItems({ views, onPick }: { views: CommandView[]; onPick: (act: string) => void }) {
  const items = views.filter((v) => v.cmd.menu);
  return (
    <>
      {items.map((v) => {
        const st = v.state;
        return (
          <div key={v.cmd.id}>
            {v.cmd.id === "interrupt" ? <hr /> : null}
            <button role="menuitem" className={st.kind === "disabled" ? "has-why" : undefined} disabled={st.kind === "disabled"} title={st.kind === "disabled" ? st.reason : undefined} onClick={(e) => { e.stopPropagation(); onPick(v.cmd.act); }}>
              <span>{v.cmd.label}</span>
              {st.kind === "enabled" && st.unverified ? <span className="tag unv">未確認</span> : v.cmd.kbd ? <span className="kbd">{v.cmd.kbd}</span> : null}
              {st.kind === "disabled" ? <span className="why">{st.reason}</span> : null}
            </button>
          </div>
        );
      })}
    </>
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

export const SCOPE_TEXT: Record<SaveScope["kind"], string> = {
  appSettings: "アプリ設定", windowBounds: "窓の位置・サイズ", chatLocal: "チャットのピン・下書き・モデル選択", queue: "送信待ち", activity: "監視活動の履歴",
};

/** 保存できていない内容と、起動時に読めなかった保存ファイルの警告（目立ちすぎない帯）。保存失敗を保存済みと表示しない。 */
export function SaveBanner({ failed, warnings, onDismissWarnings, onAct }: {
  failed: SaveStatus[]; warnings: StartupWarning[]; onDismissWarnings: () => void; onAct: (a: string) => void;
}) {
  const first = failed[0]?.state;
  return (
    <>
      {failed.length > 0 && first?.kind === "saveFailed" ? (
        <div className="gbanner err" role="alert">
          <Icon name="alert" />
          <span className="grow">
            保存できていない内容があります（{[...new Set(failed.map((f) => SCOPE_TEXT[f.scope.kind]))].join("、")}）。{first.message}
          </span>
          <button className="btn-line" onClick={() => onAct("retrySave")}>再試行</button>
        </div>
      ) : null}
      {warnings.length > 0 ? (
        <div className="gbanner warn" role="status">
          <Icon name="alert" /><span className="grow">保存データの警告: {warnings.map((w) => w.message).join(" / ")}</span>
          <button className="btn-line" title={warnings.some((w) => !w.acknowledgeable) ? "保存できない状態の警告は確認済みにせず、この起動中だけ隠します" : "確認済みにして、以後は出しません"} onClick={onDismissWarnings}>閉じる</button>
        </div>
      ) : null}
    </>
  );
}
