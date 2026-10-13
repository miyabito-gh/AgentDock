import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Chat, ChatListSettings, ChatListView, HostSnapshot, ProjectsOutcome, UiTextSize } from "../ipc/types";
import { Icon } from "./Icon";
import { Popover } from "./Popover";
import { TENTATIVE_HINT, TENTATIVE_STYLE, chatAgents, chatName, chatRequests, chatStatusText, chatTitle, isRunning, sortChats } from "./derive";
import { STATE, keyStr, knownValue } from "./format";
import { PENDING_TEXT, archiveTag } from "./ManageDialogs";
import { GROUP_HEADER_HEIGHT, WINDOW_THRESHOLD, cumulativeTops, rowHeight, scrollTopToRevealVar, windowRangeVar } from "./listWindow";
import { BASIS_TEXT, groupByProject, type MatchBasis } from "./projectGroups";
import { projectUpdateMessage } from "./listFetch";
import { PROJECTS_ENABLED } from "./features";
import { UNKNOWN_KEY, collapseAll, flattenGroups, groupChats, shortNames, worktreeOf, type FlatItem, type FolderGroup } from "./folderGroups";

/** 行の「…」メニューで選べる操作（App.tsx の onChatAct が対象チャットIDつきで受ける）。 */
export type ChatOp = "rename" | "pin" | "archive" | "unarchive" | "export" | "delete" | "sameFolderNew";

/** 一覧の状態文字（chatStatusText）から状態チップの色クラスを引く。対応が無い文言（履歴なし等）は色なしの控えめな文字。 */
function statusClass(text: string): string {
  const head = text.split("（")[0];
  for (const s of Object.values(STATE)) if (s.t === head) return s.c;
  if (text.startsWith("対応待ち")) return "wait";
  if (text.startsWith("作業中")) return text.includes("失敗") ? "fail" : "run";
  if (text.includes("状態不明")) return "unk";
  return "";
}

const NL = "\n";
interface MenuItem { act: string; label: string; danger?: boolean; sep?: boolean; title?: string; /** 押せない項目（理由は sub に出す） */ disabled?: boolean; /** 2行目の小さな補足（長いときは省略表示。全文は title） */ sub?: string }
/** 開いているメニュー（行の「…」・見出しの「…」で共通）。位置は「…」ボタンに寄せる。 */
type OpenMenu = { kind: "chat"; id: string } | { kind: "group"; key: string };

export function LeftPane({ snap, sel, onSelect, onAct, onAcknowledge, onChatAct, chatList, onChatList, projects = null, onNotice, listStatus, uiSize = "Normal", onNewChatIn, onReclassify, onFetchProjectChats, onLoadMore }: {
  /** プロジェクト見出しの「このプロジェクトのチャットを取得」。roots を cwd 条件に読む（読み取りのみ）。結果の通知は呼び出し側が出す。 */
  onFetchProjectChats?: (label: string, roots: string[]) => Promise<void>;
  /** 一覧末尾の「続きを読み込む」。続きがあるときだけ渡される（読み取りのみ）。 */
  onLoadMore?: () => Promise<void>;
  /** 見出しの「このフォルダで新しいチャット」。第1引数が初期の作業フォルダ、第2引数は候補（プロジェクトの全フォルダ）。作成はダイアログで押したときだけ。 */
  onNewChatIn?: (cwd: string, choices: string[]) => void;
  /** プロジェクトとチャット一覧を取り直す（読み取りのみ）。失敗の理由を返す（成功は null）。 */
  onReclassify?: () => Promise<string | null>;
  snap: HostSnapshot; sel: string | null; onSelect: (id: string) => void; onAct: (a: string) => void; onAcknowledge: (c: Chat) => void;
  /** 行の「…」メニュー・右クリック・Shift+F10 の操作。対象は選択中ではなくその行のチャット。 */
  onChatAct: (op: ChatOp, chatId: string) => void;
  /** 一覧の表示設定（AppSettings.chatList）と、その更新（直列化された updateSettings 経由）。 */
  chatList: ChatListSettings; onChatList: (f: (c: ChatListSettings) => ChatListSettings) => void;
  /** プロジェクト一覧の取得結果（null＝未取得）。取得できなかったことを「プロジェクトなし」にしない。 */
  projects?: ProjectsOutcome | null;
  /** 短い通知（パスのコピーなど）。 */
  onNotice?: (text: string) => void;
  /** 直近の「更新」の結果（成功・失敗とも次の更新まで残す）。 */
  listStatus?: { ok: boolean; text: string } | null;
  /** 画面の文字サイズ（一覧の行の高さが変わる）。 */
  uiSize?: UiTextSize;
}) {
  const rowH = rowHeight(uiSize);
  const [query, setQuery] = useState("");
  const [inclArch, setInclArch] = useState(false);
  const [showArch, setShowArch] = useState(false);
  const [menu, setMenu] = useState<OpenMenu | null>(null);
  const q = query.trim();
  const localOf = (c: Chat) => snap.chatLocals.find((l) => l.chat.id === c.key.id);
  // アプリ側のアーカイブ（Codex へ未反映でも一覧から隠す、M43）と Codex 側のアーカイブのどちらかなら、アーカイブ扱い。
  const archived = (c: Chat) => localOf(c)?.visibility.kind === "archived" || knownValue(c.archived) === true;
  // 外部作成の会話を「一覧から外した」ものは、元の履歴を消さずにアプリの一覧からだけ隠す。
  const removed = (c: Chat) => localOf(c)?.visibility.kind === "removedFromList";
  const visible = sortChats(snap.chats).filter((c) => !removed(c)).filter((c) =>
    (showArch ? archived(c) : !archived(c) || (q !== "" && inclArch)) && (!q || chatName(c).includes(q)));
  const loadedCount = snap.chats.filter((c) => !removed(c) && !archived(c)).length;
  const pinned = visible.filter((c) => c.pinned && !showArch);
  const rest = visible.filter((c) => !c.pinned || showArch);

  // 「プロジェクトごと」は、プロジェクト一覧を読めたときだけ。読めない間は作業フォルダごとで表示する（保存した設定は変えない）。
  // PROJECTS_ENABLED=false の間は常に読めていない扱い（保存値の ByProject は表示時に作業フォルダごとへ読み替える）。
  const loadedProjects = PROJECTS_ENABLED && projects?.kind === "loaded" ? projects.projects : null;
  const projectsOff = projects?.kind === "unsupported" ? projects.reason : null;
  const view: ChatListView = chatList.view === "ByProject" && !loadedProjects ? "ByFolder" : chatList.view;
  const byFolder = view === "ByFolder" && !showArch;
  const byProject = view === "ByProject" && !showArch;
  const grouped = byFolder || byProject;
  const projectGrouping = byProject && loadedProjects ? groupByProject(rest, loadedProjects, chatList.pinnedFolders, (c) => localOf(c)?.acceptedFolders?.folders.folders ?? localOf(c)?.nextFolders?.folders ?? [], q === "") : null;
  // 「振り分け直す」の結果通知。取り直しの完了後、最新の振り分けから件数を数えて1回だけ通知する。
  const [reclass, setReclass] = useState<"idle" | "busy" | "done">("idle");
  const doReclassify = async () => {
    if (!onReclassify || reclass === "busy") return;
    setReclass("busy");
    const err = await onReclassify();
    if (err) { onNotice?.(err); setReclass("idle"); } else setReclass("done");
  };
  const groups = projectGrouping ? projectGrouping.groups : byFolder ? groupChats(rest, snap.worktrees, chatList.pinnedFolders) : [];
  useEffect(() => {
    if (reclass !== "done" || !projectGrouping) return;
    let conf = 0, guess = 0, none = 0, unk = 0;
    for (const g of projectGrouping.groups) {
      if (g.kind === "project") { const n = g.guessedCount ?? 0; guess += n; conf += g.chats.length - n; }
      else if (g.kind === "noProject") none += g.chats.length;
      else if (g.kind === "unknown") unk += g.chats.length;
    }
    onNotice?.(projectUpdateMessage({ projects: loadedProjects?.length ?? 0, confirmed: conf, guessed: guess, none, unknown: unk }));
    setReclass("idle");
  }, [reclass, projectGrouping]); // eslint-disable-line react-hooks/exhaustive-deps
  // 取得系の操作（見出しごと・続き）の実行中。二重実行しない。
  const [fetching, setFetching] = useState(false);
  const runFetch = async (f?: () => Promise<void>) => {
    if (!f || fetching) return;
    setFetching(true);
    try { await f(); } finally { setFetching(false); }
  };
  const multi = projectGrouping?.multi ?? new Map<string, string[]>();
  const basisOf = projectGrouping?.basis ?? new Map<string, MatchBasis>();
  const shorts = shortNames(groups.filter((g) => g.kind === "folder").map((g) => ({ key: g.key, path: g.path })));
  const labelOf = (g: FolderGroup) => (g.kind === "folder" ? shorts.get(g.key) ?? g.path : g.kind === "project" ? g.label ?? g.path : g.path);
  /** 一覧で「プロジェクトごと」を選んでいるのに読めていないときの案内（読めない理由が「非対応」のときはメニューの項目に出す）。 */
  const projectsNote = PROJECTS_ENABLED && chatList.view === "ByProject" && !showArch && !loadedProjects
    ? projects === null ? { text: "プロジェクト情報を取得中です（作業フォルダごとで表示中）", bad: false }
      : projects.kind === "failed" ? { text: "プロジェクト情報を取得できませんでした（作業フォルダごとで表示中）", bad: true } : null
    : null;
  // 検索中は一致のあるグループを折りたたみ無視で開く（保存値は変えない）。
  const isCollapsed = (key: string) => q === "" && chatList.collapsedFolders.includes(key);
  const items: FlatItem[] = grouped
    ? flattenGroups(groups, isCollapsed)
    : rest.map((c) => ({ type: "chat" as const, chat: c, group: null }));
  const heights = items.map((it) => (it.type === "group" ? GROUP_HEADER_HEIGHT : rowH));

  // 200件を超えたら固定行高の窓表示（見える範囲＋少しだけ描く）。スクロール位置は一覧の先頭からの相対で持つ。見出し行は別の固定高さ。
  const scrollerRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [metrics, setMetrics] = useState({ rel: 0, view: 600 });
  const windowed = items.length > WINDOW_THRESHOLD;
  const measure = useCallback(() => {
    const sc = scrollerRef.current, ls = listRef.current;
    if (!sc || !ls) return;
    const offset = ls.getBoundingClientRect().top - sc.getBoundingClientRect().top + sc.scrollTop;
    const next = { rel: sc.scrollTop - offset, view: sc.clientHeight };
    setMetrics((m) => (m.rel === next.rel && m.view === next.view ? m : next));
  }, []);
  useLayoutEffect(() => { if (windowed) measure(); }, [windowed, items.length, showArch, pinned.length, measure]);
  useEffect(() => {
    const sc = scrollerRef.current;
    if (!windowed || !sc || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => measure());
    ro.observe(sc);
    return () => ro.disconnect();
  }, [windowed, measure]);
  const range = windowed ? windowRangeVar(heights, metrics.rel, metrics.view) : { start: 0, end: items.length };
  // 矢印キーの移動で窓の外へ出る行は、スクロールしてから描画後にフォーカスする。
  const focusIdx = useRef<number | null>(null);
  useEffect(() => {
    if (focusIdx.current === null) return;
    const el = listRef.current?.querySelector<HTMLElement>(`[data-idx="${focusIdx.current}"]`);
    if (el) { el.focus(); focusIdx.current = null; }
  });
  const onListKey = (e: React.KeyboardEvent) => {
    if (!windowed || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
    const cur = (e.target as HTMLElement).closest<HTMLElement>("[data-idx]");
    if (!cur) return;
    e.preventDefault();
    const idx = Math.min(items.length - 1, Math.max(0, Number(cur.dataset.idx) + (e.key === "ArrowDown" ? 1 : -1)));
    const sc = scrollerRef.current, ls = listRef.current;
    if (sc && ls) {
      const offset = ls.getBoundingClientRect().top - sc.getBoundingClientRect().top + sc.scrollTop;
      sc.scrollTop = offset + scrollTopToRevealVar(heights, idx, sc.scrollTop - offset, sc.clientHeight);
    }
    focusIdx.current = idx;
    measure();
    const el = ls?.querySelector<HTMLElement>(`[data-idx="${idx}"]`);
    if (el) { el.focus(); focusIdx.current = null; }
  };
  const listRows = () => {
    if (!windowed) return <div ref={listRef}>{items.map((it, i) => renderItem(it, i))}</div>;
    const tops = cumulativeTops(heights);
    return (
      <div ref={listRef} onKeyDown={onListKey} style={{ height: tops[items.length], position: "relative" }}>
        <div style={{ position: "absolute", top: tops[range.start], left: 0, right: 0 }}>
          {items.slice(range.start, range.end).map((it, i) => renderItem(it, range.start + i))}
        </div>
      </div>
    );
  };

  // ── 「…」メニュー（行・見出し共通）。マウス位置ではなく「…」ボタンに寄せて開く。選択・最近利用は変えない。 ──
  const menuRef = useRef<HTMLDivElement>(null);
  const anchorRef = useRef<HTMLElement | null>(null);
  const openMenu = (m: OpenMenu, anchor: HTMLElement | null) => { anchorRef.current = anchor; setMenu(m); };
  const closeMenu = useCallback((refocus: boolean) => {
    setMenu(null);
    if (refocus) anchorRef.current?.focus();
  }, []);
  useLayoutEffect(() => {
    const el = menuRef.current, a = anchorRef.current;
    if (!menu || !el || !a) return;
    const r = a.getBoundingClientRect(), w = el.offsetWidth, h = el.offsetHeight;
    const left = Math.max(4, Math.min(r.right - w, window.innerWidth - w - 4));
    let top = r.bottom + 2;
    if (top + h > window.innerHeight - 4) top = Math.max(4, r.top - h - 2);
    el.style.left = `${left}px`; el.style.top = `${top}px`; el.style.visibility = "visible";
    el.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
  }, [menu]);
  useEffect(() => {
    if (!menu) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation(); e.preventDefault();
      closeMenu(true);
    };
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (menuRef.current?.contains(t) || anchorRef.current?.contains(t)) return;
      closeMenu(false);
    };
    document.addEventListener("keydown", onKey, true);
    document.addEventListener("pointerdown", onDown, true);
    return () => { document.removeEventListener("keydown", onKey, true); document.removeEventListener("pointerdown", onDown, true); };
  }, [menu, closeMenu]);
  const onMenuKey = (e: React.KeyboardEvent) => {
    if (e.key === "Tab") { closeMenu(false); return; }
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const its = Array.from(menuRef.current?.querySelectorAll<HTMLElement>("button:not(:disabled)") ?? []);
    if (!its.length) return;
    e.preventDefault();
    const i = its.indexOf(document.activeElement as HTMLElement);
    its[(i + (e.key === "ArrowDown" ? 1 : -1) + its.length) % its.length].focus();
  };

  const copyPath = (text: string) => {
    const done = () => onNotice?.("パスをコピーしました。");
    const fail = () => onNotice?.("パスをコピーできませんでした。");
    if (navigator.clipboard?.writeText) navigator.clipboard.writeText(text).then(done, fail); else fail();
  };
  const toggleFolderPin = (key: string) => onChatList((c) => ({ ...c, pinnedFolders: c.pinnedFolders.includes(key) ? c.pinnedFolders.filter((k) => k !== key) : [...c.pinnedFolders, key] }));
  const toggleCollapsed = (key: string) => onChatList((c) => ({ ...c, collapsedFolders: c.collapsedFolders.includes(key) ? c.collapsedFolders.filter((k) => k !== key) : [...c.collapsedFolders, key] }));

  /** 見出しから新しいチャットを作る作業フォルダ（先頭が初期値）。フォルダが無い見出しは null。実在確認はしない（ダイアログ側の検証に任せる）。 */
  const newChatFolders = (g: FolderGroup): string[] | null => {
    if (!onNewChatIn) return null;
    if (g.kind === "folder") return g.path.trim() ? [g.path] : null;
    if (g.kind === "project") { const r = (g.roots ?? []).filter((x) => x.trim() !== ""); return r.length ? r : null; }
    return null;
  };

  /** メニューの項目（チャットの項目は上部メニュー「チャット」と同じ順・同じ文言）。 */
  const menuItems = (): { label: string; items: MenuItem[] } | null => {
    if (!menu) return null;
    if (menu.kind === "chat") {
      const c = snap.chats.find((x) => x.key.id === menu.id);
      if (!c) return null;
      return {
        label: `「${chatName(c)}」の操作`,
        items: [
          { act: "rename", label: "名前を変更…" },
          { act: "pin", label: c.pinned ? "ピン留めを外す" : "ピン留め" },
          { act: archived(c) ? "unarchive" : "archive", label: archived(c) ? "アーカイブを解除" : "アーカイブ" },
          { act: "export", label: "Markdown にエクスポート…" },
          c.kind === "general"
            ? { act: "sameFolderNew", label: "同じ作業フォルダで新しいチャット…", disabled: true, sub: "一般チャット（作業フォルダなし）です", title: "一般チャット（作業フォルダなし）です" }
            : { act: "sameFolderNew", label: "同じ作業フォルダで新しいチャット…" },
          { act: "-", label: "", sep: true },
          { act: "delete", label: "削除…", danger: true },
        ],
      };
    }
    const g = groups.find((x) => x.key === menu.key);
    if (!g) return null;
    const newIn = newChatFolders(g);
    return {
      label: `「${labelOf(g)}」の操作`,
      items: [
        ...(newIn ? [{
          act: "gnew",
          label: g.kind === "project" ? "このプロジェクトで新しいチャット…" : "このフォルダで新しいチャット…",
          sub: g.kind === "project" ? `先頭フォルダ: ${newIn[0]}` : newIn[0],
          title: g.kind === "project" ? `先頭フォルダ（プライマリと推定）: ${newIn[0]}` : newIn[0],
        }] : []),
        ...(g.kind === "project" && onFetchProjectChats && (g.roots ?? []).some((x) => x.trim() !== "") ? [{
          act: "gfetch",
          label: fetching ? "取得中…" : "このプロジェクトのチャットを取得",
          title: "このプロジェクトのフォルダを作業フォルダに持つチャットを Codex から取得します（読み取りのみ。押したときだけ）。フォルダの完全一致のみで、下位フォルダのチャットは取得できません",
        }] : []),
        ...(g.kind !== "unknown" ? [{ act: "gpin", label: g.kind === "project" ? (g.pinned ? "プロジェクトのピン留めを外す" : "プロジェクトをピン留め") : (g.pinned ? "フォルダのピン留めを外す" : "フォルダをピン留め") }] : []),
        ...(g.kind === "folder" ? [{ act: "gcopy", label: "パスをコピー" }] : []),
      ],
    };
  };
  const pick = (act: string) => {
    if (!menu) return;
    const m = menu;
    closeMenu(act === "pin" || act === "archive" || act === "unarchive" || act === "gpin" || act === "gcopy");
    // gnew はダイアログが開くので、フォーカスは戻さない。
    if (m.kind === "chat") onChatAct(act as ChatOp, m.id);
    else if (act === "gnew") { const g = groups.find((x) => x.key === m.key); const f = g ? newChatFolders(g) : null; if (f && onNewChatIn) onNewChatIn(f[0], f); }
    else if (act === "gfetch") {
      const g = groups.find((x) => x.key === m.key);
      const roots = (g?.roots ?? []).filter((x) => x.trim() !== "");
      if (g && roots.length && onFetchProjectChats) void runFetch(() => onFetchProjectChats(labelOf(g), roots));
    }
    else if (act === "gpin") toggleFolderPin(m.key);
    else if (act === "gcopy") { const g = groups.find((x) => x.key === m.key); if (g) copyPath(g.path); }
  };

  // 印はホストが状態から導く（通知設定に関係なく付く）。ホストの値がなければ（モック）画面側の状態から出す。
  const flagsOf = (c: Chat) => {
    const marks = localOf(c)?.marks;
    return {
      hasReq: !!(marks?.awaitingAnswer) || chatRequests(snap, c).length > 0,
      unconfirmedFail: marks ? marks.unacknowledgedFailure : chatAgents(snap, c).some((a) => a.status.state === "failed"),
    };
  };

  const renderItem = (it: FlatItem, idx: number) => (it.type === "group" ? groupHeader(it.group, idx) : row(it.chat, idx, it.group?.key));

  const moreBtn = (kind: "chat" | "group", id: string, label: string, open: boolean, onOpen: (el: HTMLElement) => void) => (
    <button type="button" className="row-more" data-more={`${kind}:${id}`} aria-label={label} aria-haspopup="menu" aria-expanded={open}
      onClick={(e) => { e.stopPropagation(); if (open) closeMenu(false); else onOpen(e.currentTarget); }}>
      <span aria-hidden="true">…</span>
    </button>
  );

  const groupHeader = (g: FolderGroup, idx: number) => {
    const collapsed = isCollapsed(g.key);
    const counts = { wait: 0, fail: 0, run: 0 };
    for (const c of g.chats) {
      const f = flagsOf(c);
      if (f.hasReq) counts.wait++;
      if (f.unconfirmedFail) counts.fail++;
      if (isRunning(snap, c)) counts.run++;
    }
    const label = labelOf(g);
    const open = menu?.kind === "group" && menu.key === g.key;
    const guessN = g.kind === "project" ? g.guessedCount ?? 0 : 0;
    const title = g.kind === "folder" ? g.path
      : g.kind === "project"
        ? [`プロジェクト「${label}」`, g.roots && g.roots.length ? `フォルダ（先頭をプライマリと推定）:${NL}${g.roots.join(NL)}` : "フォルダなし",
          guessN ? `フォルダ一致による推測で入ったチャット: ${guessN}件（すべてのフォルダを含む／先頭フォルダに一致／一部のフォルダに一致のいずれか。確定ではありません）` : ""].filter((x) => x).join(NL)
      : g.kind === "noProject" ? "どのプロジェクトのフォルダにも一致しないチャットです（一般チャットを含む）"
      : g.kind === "unknown" ? "作業フォルダを取得できていないチャットです" : "作業フォルダを持たない一般チャットです";
    return (
      <div key={`g:${g.key}`} className={`list-grp${open ? " menu-open" : ""}`} style={{ height: GROUP_HEADER_HEIGHT }}
        onContextMenu={(e) => { e.preventDefault(); const b = e.currentTarget.querySelector<HTMLElement>(".row-more"); if (b && g.key !== UNKNOWN_KEY) openMenu({ kind: "group", key: g.key }, b); }}>
        <button type="button" className="grp-toggle" data-idx={idx} aria-expanded={!collapsed} title={title} onClick={() => { if (q === "") toggleCollapsed(g.key); }}
          aria-disabled={q !== "" || undefined} aria-label={`${label}（${g.chats.length}件）${guessN ? "。フォルダ一致による推測を含む" : ""}${g.pinned ? "。ピン留め" : ""}${collapsed && q === "" ? "。折りたたみ中" : ""}`}>
          <span className="grp-caret" aria-hidden="true">{collapsed && q === "" ? "▸" : "▾"}</span>
          <span className="grp-ico" aria-hidden="true"><Icon name={g.kind === "general" || g.kind === "noProject" ? "chat" : g.kind === "unknown" ? "folderUnknown" : collapsed && q === "" ? "folder" : "folderOpen"} /></span>
          <span className="grp-nm">{label}</span>
          {guessN ? <span className="grp-guess" title="フォルダ一致による推測を含みます" aria-hidden="true">推定</span> : null}
          {g.chats.length === 0 ? <span className="grp-empty" style={{ fontSize: "0.85em", opacity: 0.6, marginLeft: "auto", marginRight: 6 }}>チャットなし</span> : null}
          <span className="grp-n">{g.chats.length}</span>
        </button>
        {g.pinned ? <span className="grp-pin" title="ピン留めしたフォルダ" aria-label="ピン留めしたフォルダ" role="img"><Icon name="pin" /></span> : null}
        <span className="grp-marks">
          {counts.wait ? <span className="mark wait" title={`承認・質問待ち ${counts.wait}件`}>待 {counts.wait}</span> : null}
          {counts.fail ? <span className="mark fail" title={`未確認の失敗 ${counts.fail}件`}>失 {counts.fail}</span> : null}
          {counts.run ? <span className="mark pend" title={`作業中 ${counts.run}件`}>作業中 {counts.run}</span> : null}
        </span>
        {g.kind !== "unknown" ? moreBtn("group", g.key, `フォルダ「${label}」の操作`, open, (el) => openMenu({ kind: "group", key: g.key }, el)) : null}
      </div>
    );
  };

  /** `idx` は一覧本体（窓表示の対象）の中の位置。ピン留め欄の行には付けない。 */
  const row = (c: Chat, idx?: number, gkey?: string) => {
    const k = keyStr(c.key);
    const { hasReq, unconfirmedFail } = flagsOf(c);
    const open = menu?.kind === "chat" && menu.id === c.key.id;
    const fixed = windowed && idx !== undefined;
    const wt = byFolder && c.kind !== "general" && c.cwd.kind === "value" ? worktreeOf(c.cwd.value, snap.worktrees) : null;
    const openFromRow = (el: Element) => {
      const b = el.closest(".row-chat")?.querySelector<HTMLElement>(".row-more");
      if (b) openMenu({ kind: "chat", id: c.key.id }, b);
    };
    const statusText = chatStatusText(snap, c);
    const statusCls = statusClass(statusText);
    const arc = archiveTag(localOf(c));
    const cwdText = c.kind !== "general" && c.cwd.kind === "value" ? c.cwd.value : null;
    const kindTitle = `${c.kind === "general" ? "一般チャット" : "作業フォルダのチャット"}${cwdText ? `（${cwdText}）` : ""}\n状態: ${statusText}`;
    return (
      <div key={gkey ? `${gkey}|${k}` : k} className={`row-chat${grouped ? " in-grp" : ""}${k === sel ? " sel" : ""}${fixed ? " fixed" : ""}${open ? " menu-open" : ""}`}
        style={fixed ? { height: rowH, minHeight: rowH } : undefined}
        onContextMenu={(e) => { e.preventDefault(); openFromRow(e.currentTarget); }}>
        <button type="button" className="row-main" onClick={() => onSelect(c.key.id)} aria-current={k === sel} data-idx={idx}
          {...(fixed && !grouped ? { "aria-posinset": idx! + 1, "aria-setsize": items.length } : {})}
          onKeyDown={(e) => { if ((e.shiftKey && e.key === "F10") || e.key === "ContextMenu") { e.preventDefault(); e.stopPropagation(); openFromRow(e.currentTarget); } }}>
          <span className={`st st-mark ${statusCls || "none"}`} role="img" aria-label={`状態: ${statusText}`} title={statusText}><i aria-hidden="true" /></span>
          <span className="nm" style={chatTitle(c).confirmed ? undefined : TENTATIVE_STYLE} title={chatTitle(c).confirmed ? kindTitle : TENTATIVE_HINT}>{chatName(c)}</span>
          {c.origin === "external" ? <span className="tag" title="Codex 側で作られた会話（AgentDock が起動したものではない）">外部</span> : null}
          {c.noHistory ? <span className="tag" title="Codex に記録がありません">履歴なし</span> : null}
          {multi.has(c.key.id) ? <span className="tag" title={`他に一致: ${multi.get(c.key.id)!.join("、")}
（フォルダ一致による推測${basisOf.has(c.key.id) ? `: ${BASIS_TEXT[basisOf.get(c.key.id)!]}` : ""}。表示は先頭のプロジェクトにだけ置いています）`}>複数</span> : null}
          {wt ? <span className="tag" title={`AgentDock が作った worktree（ブランチ ${wt.branch}）\n${wt.path}`}>worktree</span> : null}
          {archived(c) && (!showArch || arc.text !== "アーカイブ") ? <span className="tag" title={arc.title || undefined}>{arc.text}</span> : null}
          {localOf(c)?.deletePending ? <span className="tag unv" title={PENDING_TEXT(localOf(c)!.deletePending!.reason)}>削除保留</span> : null}
          <span className="marks">
            {hasReq ? <span className="mark wait" title="承認・質問待ち">待</span> : null}
            {unconfirmedFail ? (
              <span className="mark fail" role="button" tabIndex={0} title="未確認の失敗があります。クリックで「確認済み」にします（再実行ではありません）"
                onClick={(e) => { e.stopPropagation(); onAcknowledge(c); }}
                onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); e.stopPropagation(); onAcknowledge(c); } }}>失</span>
            ) : null}
            {isRunning(snap, c) ? <span className="mark pend" title="作業中">作業中</span> : null}
          </span>
        </button>
        {moreBtn("chat", c.key.id, `「${chatName(c)}」の操作`, open, (el) => openMenu({ kind: "chat", id: c.key.id }, el))}
      </div>
    );
  };

  const VIEW_LABEL: Record<ChatListView, string> = { Recent: "最近使った順", ByFolder: "作業フォルダごと", ByProject: "プロジェクトごと" };
  const viewLabel = VIEW_LABEL[view];
  const setView = (v: ChatListView) => onChatList((c) => ({ ...c, view: v }));
  const mi = menuItems();
  // 開いたまま対象（チャット・フォルダ）が一覧から無くなったら閉じる。
  useEffect(() => { if (menu && !mi) setMenu(null); });

  return (
    <>
      <div className="left-top">
        <div className="lt-row">
          <button className="btn-line grow" onClick={() => onAct("newChat")}><Icon name="plus" />新しいチャット</button>
          <Popover role="menu" align="right" triggerClassName="btn lt-view" label={`一覧の並べ方（現在: ${viewLabel}）`} title={`一覧の並べ方: ${viewLabel}`} trigger={<>並べ方 <span aria-hidden="true">▾</span></>}>
            {(close) => (
              <>
                {((PROJECTS_ENABLED ? ["Recent", "ByFolder", "ByProject"] : ["Recent", "ByFolder"]) as ChatListView[]).map((v) => (
                  <button key={v} type="button" role="menuitemradio" aria-checked={view === v} disabled={v === "ByProject" && projectsOff !== null}
                    title={v === "ByProject" ? projectsOff ?? "プロジェクト（名前とフォルダの集合）ごとにまとめます。所属はフォルダ一致による推測を含みます" : undefined}
                    onClick={() => { setView(v); close(); }}>
                    <span>{VIEW_LABEL[v]}</span><span aria-hidden="true">{view === v ? "✓" : ""}</span>
                  </button>
                ))}
                {PROJECTS_ENABLED && chatList.view === "ByProject" ? (
                  <>
                    <hr />
                    <button type="button" role="menuitem" disabled={!onReclassify || reclass === "busy" || !loadedProjects}
                      title="プロジェクト一覧だけを取り直し（読み取りのみ）、所属を判定し直します。チャット一覧は取得しません（チャットの更新は更新ボタン）。押したときだけ実行します"
                      onClick={() => { close(); void doReclassify(); }}>
                      <span>{reclass === "busy" ? "更新中…" : "プロジェクト一覧を更新"}</span>
                    </button>
                  </>
                ) : null}
                {view !== "Recent" ? (
                  <>
                    <hr />
                    <button type="button" role="menuitem" disabled={groups.length === 0}
                      onClick={() => { const keys = groups.map((g) => g.key); onChatList((c) => ({ ...c, collapsedFolders: collapseAll(c.collapsedFolders, keys) })); close(); }}>
                      <span>すべて折りたたむ</span>
                    </button>
                    <button type="button" role="menuitem" disabled={chatList.collapsedFolders.length === 0}
                      onClick={() => { onChatList((c) => ({ ...c, collapsedFolders: [] })); close(); }}>
                      <span>すべて展開</span>
                    </button>
                  </>
                ) : null}
              </>
            )}
          </Popover>
          <button className="btn icon" title={listStatus?.ok ? `チャット一覧を Codex から取り直します（読み取りのみ）。直近の結果: ${listStatus.text}` : "チャット一覧を Codex から取り直します（読み取りのみ）"} aria-label="一覧を更新" onClick={() => onAct("refreshList")}><Icon name="refresh" /></button>
        </div>
        <input type="search" placeholder="名前で検索" aria-label="チャットを名前で検索" value={query} onChange={(e) => setQuery(e.target.value)} />
        {q ? <label className="small"><input type="checkbox" checked={inclArch} onChange={(e) => setInclArch(e.target.checked)} />アーカイブも含める</label> : null}
        {projectsNote ? <div className={`small${projectsNote.bad ? " lt-fail" : ""}`} role="status">{projectsNote.text}</div> : null}
        {listStatus && !listStatus.ok ? <div className="small lt-fail" role="status">{listStatus.text}</div> : null}
      </div>
      <div className="left-scroll" ref={scrollerRef} onScroll={() => { if (menu) closeMenu(false); if (windowed) measure(); }}>
        {showArch ? (
          <>
            <div className="list-h"><span>アーカイブ</span><button className="small" style={{ height: 20 }} onClick={() => setShowArch(false)}>戻る</button></div>
            {rest.length ? listRows() : <div className="empty-note">アーカイブしたチャットはありません</div>}
          </>
        ) : (
          <>
            {pinned.length ? <><div className="list-h"><span>ピン留め</span></div>{pinned.map((c) => row(c))}</> : null}
            <div className="list-h"><span>{viewLabel}</span></div>
            {rest.length ? listRows() : <div className="empty-note">{q ? "一致するチャットはありません" : "まだチャットがありません"}</div>}
            {onLoadMore && q === "" ? (
              <div className="list-more" style={{ padding: "6px 8px", display: "flex", alignItems: "center", gap: 8 }}>
                <button type="button" className="small" disabled={fetching} onClick={() => void runFetch(onLoadMore)}
                  title="Codex の一覧の続きを次の1ページ取得します（読み取りのみ）">{fetching ? "読み込み中…" : "続きを読み込む"}</button>
                <span className="small" style={{ opacity: 0.7 }}>取得済み{loadedCount}件</span>
              </div>
            ) : null}
          </>
        )}
      </div>
      {mi ? (
        <div ref={menuRef} className="pop row-menu" role="menu" aria-label={mi.label} tabIndex={-1} style={{ visibility: "hidden" }} onKeyDown={onMenuKey}>
          {mi.items.map((it, i) => it.sep ? <hr key={i} /> : (
            <button key={it.act} type="button" role="menuitem" className={it.danger ? "danger" : it.sub ? "has-sub" : undefined} title={it.title} disabled={it.disabled} onClick={() => pick(it.act)}>{it.sub ? <><span>{it.label}</span><span className="sub">{it.sub}</span></> : it.label}</button>
          ))}
        </div>
      ) : null}
      <div className="left-foot">
        <button className="small" style={{ height: 22 }} onClick={() => setShowArch(true)}><Icon name="archive" />アーカイブ（{snap.chats.filter((c) => !removed(c) && archived(c)).length}）</button>
        <span className="only-narrow-l"><button className="small" style={{ height: 22 }} onClick={() => onAct("toggleLeft")}>閉じる</button></span>
      </div>
    </>
  );
}
