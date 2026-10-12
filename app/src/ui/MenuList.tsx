// メニューの中身（DESIGN_MENUS §3.1・§3.4）。見出し・区切り・✓・ショートカット・未確認・印・無効理由・危険色を描く。
// 上部メニューバーが使う（ヘッダー「…」は M4 で同じ部品へ）。↑↓ で有効な項目を移動する（変換中は無視）。
import { type KeyboardEvent } from "react";
import type { ResolvedEntry } from "./menuModel";

/** メニュー内の押せる項目へ ↑↓ で移動する。移動したら true。 */
export function moveInMenu(e: KeyboardEvent, root: HTMLElement | null): boolean {
  if (e.nativeEvent.isComposing || (e.key !== "ArrowDown" && e.key !== "ArrowUp" && e.key !== "Home" && e.key !== "End")) return false;
  const items = Array.from(root?.querySelectorAll<HTMLElement>("button:not(:disabled)") ?? []);
  if (!items.length) return false;
  e.preventDefault();
  const i = items.indexOf(document.activeElement as HTMLElement);
  const next = e.key === "Home" ? 0 : e.key === "End" ? items.length - 1 : (i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
  items[next].focus();
  return true;
}

export function MenuList({ entries, onPick }: { entries: ResolvedEntry[]; onPick: (act: string) => void }) {
  return (
    <>
      {entries.map((en, i) => {
        if (en.kind === "sep") return <hr key={i} />;
        if (en.kind === "heading") return <div key={i} className="menu-h" role="presentation">{en.text}</div>;
        const v = en.view;
        const st = v.state;
        const off = st.kind === "disabled";
        const check = v.checked !== undefined;
        return (
          <button key={v.cmd.id} type="button" role={check ? "menuitemcheckbox" : "menuitem"} aria-checked={check ? v.checked : undefined}
            className={[off ? "has-why" : "", v.cmd.danger ? "danger" : ""].filter(Boolean).join(" ") || undefined}
            disabled={off} onClick={(e) => { e.stopPropagation(); onPick(v.act); }}>
            <span>{v.checked ? "✓ " : ""}{v.label}</span>
            {v.mark ? <span className="tag" title="押すと対処の案内を表示します">{v.mark}</span> : null}
            {st.kind === "enabled" && st.unverified ? <span className="tag unv">未確認</span> : v.cmd.kbd ? <span className="kbd">{v.cmd.kbd}</span> : null}
            {off ? <span className="why">{st.reason}</span> : null}
          </button>
        );
      })}
    </>
  );
}
