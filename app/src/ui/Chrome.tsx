import { useEffect, useRef } from "react";
import { Icon } from "./Icon";
import type { CommandView } from "./commands";
import { Popover } from "./Popover";
import { MenuList, moveInMenu } from "./MenuList";
import { MENUBAR, resolveMenu } from "./menuModel";
import { UsagePanel } from "./UsagePanel";
import { globalBannerView, isConnectionProblem, showVersionNotice, versionNoticeKey } from "./versionNotice";
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

export function MenuBar({ open, setOpen, views, onAct, tag, live }: {
  open: string | null; setOpen: (m: string | null) => void; views: CommandView[]; onAct: (a: string) => void;
  /** 接続先の版タグ（右端。押すと接続先の切替）。 */
  tag?: string;
  /** 実接続か。実接続では版タグが利用状況のポップオーバーを開く（接続先の切替はその中）。 */
  live?: boolean;
}) {
  const bar = useRef<HTMLElement>(null);
  const drop = useRef<HTMLDivElement>(null);
  const titles = MENUBAR.map((m) => m.title as string);
  const focusTrigger = (t: string) => bar.current?.querySelector<HTMLElement>(`[data-menu="${t}"]`)?.focus();
  // Esc で閉じてメニュー名へフォーカスを戻す（変換中は閉じない）。外側クリックはウィンドウ側（App）で閉じる。
  useEffect(() => {
    if (!open) return;
    const h = (e: KeyboardEvent) => { if (e.key === "Escape" && !e.isComposing) { setOpen(null); focusTrigger(open); } };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [open, setOpen]);
  // 開いたら先頭の有効な項目へフォーカス。
  useEffect(() => {
    if (open) drop.current?.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
  }, [open]);
  const onKey = (e: React.KeyboardEvent) => {
    if (!open || e.nativeEvent.isComposing) return;
    // Tab: メニュー名へフォーカスを置いてから閉じる（既定の Tab 移動で、メニュー名の次／前へ進む）。
    if (e.key === "Tab") { focusTrigger(open); setOpen(null); return; }
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      const i = titles.indexOf(open);
      const t = titles[(i + (e.key === "ArrowRight" ? 1 : -1) + titles.length) % titles.length];
      setOpen(t);
      return;
    }
    moveInMenu(e, drop.current);
  };
  return (
    <nav className="menubar" aria-label="メニュー" ref={bar} onKeyDown={onKey}>
      {MENUBAR.map(({ title: m }) => (
        <div className="menu" key={m}>
          <button data-menu={m} aria-haspopup="true" aria-expanded={open === m}
            onKeyDown={(e) => { if (e.key === "ArrowDown" && open !== m && !e.nativeEvent.isComposing) { e.preventDefault(); e.stopPropagation(); setOpen(m); } }}
            onClick={(e) => { e.stopPropagation(); setOpen(open === m ? null : m); }}>{m}</button>
          {open === m ? (
            <div className="dropdown" role="menu" aria-label={m} ref={drop}>
              <MenuList entries={resolveMenu(m, views)} onPick={(a) => { setOpen(null); onAct(a); }} />
            </div>
          ) : null}
        </div>
      ))}
      <span className="grow" />
      {tag && live ? (
        <Popover trigger={tag} label="利用状況" title="利用状況を見る（読取りのみ）" align="right" triggerClassName="mock-tag">
          {(close) => (
            <div className="usage-pop" onClick={(e) => e.stopPropagation()}>
              <UsagePanel live />
              <div className="pop-acts"><button type="button" className="btn-line" onClick={() => { close(); onAct("modeMenu"); }}>接続先の切替…</button></div>
            </div>
          )}
        </Popover>
      ) : tag ? <button className="mock-tag" title="接続先（実接続／モック）を切り替える" aria-haspopup="true" onClick={(e) => { e.stopPropagation(); onAct("modeMenu"); }}>{tag}</button> : null}
    </nav>
  );
}

/** 全体バナー。接続状態と版の不一致をSourceInfoから作る。 */
export function GlobalBanner({ source, dismissed, onDismiss, onAct }: {
  source: SourceInfo | undefined;
  /** 閉じた版の識別子（AppSettings.dismissedVersionNotice）。 */
  dismissed: string | null | undefined;
  /** 版の注意を閉じる（接続断・起動失敗のバナーには付けない）。 */
  onDismiss: (key: string) => void;
  onAct: (a: string) => void;
}) {
  if (!source) return null;
  const c = source.connection;
  const view = globalBannerView(c as { kind: string; message?: string | null }, source.version);
  if (!view) return null;
  if (!isConnectionProblem(c) && !showVersionNotice(source.version, dismissed)) return null;
  const dismissKey = isConnectionProblem(c) ? null : versionNoticeKey(source.version);
  return (
    <div className={`gbanner ${view.cls}`} role={view.role}>
      <Icon name="alert" /><span className="grow">{view.text}</span>
      {c.kind === "launchFailed" ? <button className="btn-line" onClick={() => onAct("settings:codex")}>codex.exe の場所を確認</button> : null}
      {dismissKey ? <button className="btn-line" title="使用中の版と検出結果を設定で確認します" onClick={() => onAct("settings:codex")}>詳しく</button> : null}
      {isConnectionProblem(c) ? <button className="btn-line" onClick={() => onAct("retryLaunch")}>再試行</button> : null}
      {dismissKey ? <button className="btn-line" title="同じ版のあいだは出しません。Codex の版が変わるとまた出します（接続先の表示・設定・ヘルプには残ります）" onClick={() => onDismiss(dismissKey)}>この版では表示しない</button> : null}
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
