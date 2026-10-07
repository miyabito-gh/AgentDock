// Ctrl+K のコマンドメニュー（検索つき一覧、P3-8）。項目は操作台帳（commands.ts）から作る。文字のみ。
// 利用者が明示的に開いたときだけ検索欄へフォーカスを移し、閉じたら元の場所へ戻す。通知や新しい活動では開かない。
import { useEffect, useMemo, useRef, useState } from "react";
import { filterCommands, type CommandView } from "./commands";

export function CommandPalette({ views, onRun, onClose }: { views: CommandView[]; onRun: (act: string) => void; onClose: () => void }) {
  const [q, setQ] = useState("");
  const [pos, setPos] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const items = useMemo(() => filterCommands(views, q), [views, q]);
  const cur = Math.min(pos, Math.max(0, items.length - 1));

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    input.current?.focus();
    return () => { if (prev && document.contains(prev)) prev.focus(); };
  }, []);

  const run = (v: CommandView | undefined) => {
    if (!v || v.state.kind === "disabled") return;
    onClose();
    onRun(v.cmd.act);
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (e.nativeEvent.isComposing) return;
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); }
    else if (e.key === "ArrowDown") { e.preventDefault(); setPos(Math.min(cur + 1, items.length - 1)); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setPos(Math.max(cur - 1, 0)); }
    else if (e.key === "Enter") { e.preventDefault(); run(items[cur]); }
  };

  return (
    <div className="scrim" onClick={onClose}>
      <div className="dialog palette" role="dialog" aria-modal="true" aria-label="コマンドメニュー" onClick={(e) => e.stopPropagation()} onKeyDown={onKey}>
        <header>
          <input ref={input} type="text" className="palette-q" value={q} placeholder="操作を検索" aria-label="操作を検索" onChange={(e) => { setQ(e.target.value); setPos(0); }} />
        </header>
        <ul className="palette-list" role="listbox" aria-label="操作の一覧">
          {items.length === 0 ? <li className="small muted palette-none">一致する操作はありません</li> : null}
          {items.map((v, i) => {
            const st = v.state;
            const off = st.kind === "disabled";
            return (
              <li key={v.cmd.id} role="option" aria-selected={i === cur} aria-disabled={off} className={`${i === cur ? "cur" : ""} ${off ? "off" : ""}`}
                title={off ? st.reason : undefined} onMouseEnter={() => setPos(i)} onClick={() => run(v)}>
                <span className="grow">{v.cmd.label}</span>
                {st.kind === "enabled" && st.unverified ? <span className="tag unv">未確認</span> : null}
                {v.cmd.kbd ? <span className="kbd">{v.cmd.kbd}</span> : null}
                {off ? <span className="why">{st.reason}</span> : null}
              </li>
            );
          })}
        </ul>
        <footer><span className="small muted grow">↑↓ で選択、Enter で実行、Esc で閉じる</span></footer>
      </div>
    </div>
  );
}
