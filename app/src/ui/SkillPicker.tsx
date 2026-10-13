// Skill選択の部品（一覧の行・SKILL.md冒頭のプレビュー・入力欄の候補）。段階③ P3-4 の改善。
// 規則: Skillの定義は読むだけ（変更・有効無効の切替はしない）。プレビューは文字として表示し、Markdown・HTMLとして解釈しない。
//       指定は入力欄への追加だけで、送信しない。
import { useEffect, useRef, useState } from "react";
import * as host from "../ipc/client";
import type { ChatKey, RecentSkill, SkillInfo } from "../ipc/types";
import { showKnown } from "./format";
import { descText, highlightSegments, isDisabled, originLabel, sameNameNote, shortPath } from "./skillPick";

/** 入力欄が使うSkill候補の窓口（モック・未接続は null）。 */
export interface SkillPickApi {
  /** このチャットで取得済みの一覧。undefined＝まだ取れていない。 */
  skills: SkillInfo[] | undefined;
  /** 未取得なら読取りで取得する（resumeしない。重複・再試行はホストの外で抑える）。 */
  load: () => void;
  recent: RecentSkill[];
  pick: (s: SkillInfo) => void;
}

export function Hl({ text, terms }: { text: string; terms: string[] }) {
  return <>{highlightSegments(text, terms).map((g, i) => (g.hit ? <mark key={i} className="skhl">{g.text}</mark> : <span key={i}>{g.text}</span>))}</>;
}

/** 一覧の1行。1行目に名前と出所、2行目に説明（1行）、3行目に場所の末尾（薄く）。 */
export function SkillRow({ s, all, terms, active, id, onSelect, onHover, trailing }: {
  s: SkillInfo; all: SkillInfo[]; terms: string[]; active: boolean; id: string;
  onSelect: () => void; onHover?: () => void; trailing?: React.ReactNode;
}) {
  const off = isDisabled(s);
  const dup = sameNameNote(s, all);
  const desc = descText(s);
  return (
    <div id={id} role="option" aria-selected={active} className={`skrow${active ? " active" : ""}${off ? " off" : ""}`} onMouseDown={(e) => e.preventDefault()} onClick={onSelect} onMouseMove={onHover}>
      <div className="grow">
        <div className="skname">
          <b><Hl text={s.name} terms={terms} /></b>
          <span className="tag" title={`出所: ${originLabel(s)}`}>{originLabel(s)}</span>
          {dup ? <span className="tag need" title={dup}>同名あり</span> : null}
          {off ? <span className="tag">無効</span> : s.enabled.kind !== "value" ? <span className="tag" title="有効かどうかを取得できていません">有効かは未確認</span> : null}
        </div>
        <div className="skdesc small" title={desc || undefined}>{desc ? <Hl text={desc} terms={terms} /> : <span className="muted">説明: {showKnown(s.description)}</span>}</div>
        <div className="skpath mono small muted" title={s.path}><Hl text={shortPath(s.path)} terms={terms} /></div>
      </div>
      {trailing}
    </div>
  );
}

type PreviewState = { kind: "idle" } | { kind: "loading" } | { kind: "ok"; text: string; truncated: boolean } | { kind: "err"; message: string };

const PREVIEW_DELAY_MS = 150;

/** 選択中の1件のSKILL.md冒頭。選択が変わってから短い遅延で1件だけ読む（連打で読み続けない）。キャッシュは画面内だけ。 */
export function useSkillPreview(chat: ChatKey | undefined, path: string | null): PreviewState {
  const cache = useRef(new Map<string, { text: string; truncated: boolean }>());
  const [state, setState] = useState<PreviewState>({ kind: "idle" });
  const chatId = chat?.id;
  useEffect(() => { cache.current.clear(); }, [chatId]);
  useEffect(() => {
    if (!chat || !path) { setState({ kind: "idle" }); return; }
    const hit = cache.current.get(path);
    if (hit) { setState({ kind: "ok", ...hit }); return; }
    setState({ kind: "loading" });
    let alive = true;
    const t = window.setTimeout(() => {
      host.readSkillPreview(chat, path).then((r) => {
        if (!alive) return;
        cache.current.set(path, { text: r.text, truncated: r.truncated });
        setState({ kind: "ok", text: r.text, truncated: r.truncated });
      }).catch((e) => { if (alive) setState({ kind: "err", message: host.asIpcError(e).message }); });
    }, PREVIEW_DELAY_MS);
    return () => { alive = false; window.clearTimeout(t); };
  }, [chatId, path]); // eslint-disable-line react-hooks/exhaustive-deps
  return state;
}

export function SkillPreviewPane({ state, skill }: { state: PreviewState; skill: SkillInfo | undefined }) {
  return (
    <aside className="skprev" aria-label="Skillの内容（冒頭）" aria-live="polite">
      {!skill ? <p className="small muted">行を選ぶと、SKILL.md の冒頭を表示します（読取りのみ）。</p> : (
        <>
          <div className="small muted">{skill.name} の冒頭（読取りのみ）</div>
          {state.kind === "loading" ? <p className="small muted">読んでいます…</p> : null}
          {state.kind === "err" ? <p className="small why err">{state.message}</p> : null}
          {state.kind === "ok" ? (
            <>
              <pre className="skprev-text">{state.text || "（空のファイルです）"}</pre>
              {state.truncated ? <div className="small muted">…（続きは省略しています）</div> : null}
            </>
          ) : null}
        </>
      )}
    </aside>
  );
}

/** 入力欄の上に出す候補（`$`・`/skill ` で開く）。キー操作は入力欄側が持つ。 */
export function SkillSuggest({ items, all, terms, active, listId, onHover, onPick }: {
  items: SkillInfo[]; all: SkillInfo[]; terms: string[]; active: number; listId: string; onHover: (i: number) => void; onPick: (s: SkillInfo) => void;
}) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => { box.current?.querySelector<HTMLElement>(".skrow.active")?.scrollIntoView({ block: "nearest" }); }, [active]);
  return (
    <div className="pop skpop" ref={box} role="listbox" id={listId} aria-label="Skillの候補（↑↓で選び、Enter か Tab で指定、Esc で閉じる）">
      {items.map((s, i) => (
        <SkillRow key={s.path} s={s} all={all} terms={terms} active={i === active} id={`${listId}-${i}`} onSelect={() => onPick(s)} onHover={() => { if (i !== active) onHover(i); }} />
      ))}
    </div>
  );
}
