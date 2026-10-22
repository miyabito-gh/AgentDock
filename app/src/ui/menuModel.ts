// メニュー定義のデータ（DESIGN_MENUS §3.2・§3.3）。項目は台帳（commands.ts）の id で参照する。
// 規則: 区切り線は見出しの前（と、危険操作の前）だけ。危険操作（danger）は各メニューの最下段。
import type { CommandView, MenuGroup } from "./commands.ts";

export type MenuEntry = { id: string } | { heading: string } | "-";
export interface MenuSpec { title: MenuGroup; entries: MenuEntry[] }

const id = (x: string): MenuEntry => ({ id: x });
const h = (heading: string): MenuEntry => ({ heading });

export const MENUBAR: MenuSpec[] = [
  {
    title: "チャット",
    entries: [
      h("新規"), id("newChatQuick"), id("newChat"), id("worktree"),
      "-", h("選択中のチャット"), id("renameChat"), id("togglePin"), id("archive"), id("exportMd"), id("newChatSameFolder"),
      "-", h("作業フォルダ"), id("openCwd"), id("cwdChange"),
      "-", h("一覧"), id("refreshList"),
      "-", id("deleteChat"), id("quit"),
    ],
  },
  {
    title: "作業",
    entries: [
      h("変更"), id("changeList"), id("revertChanges"), id("codeReview"), id("reviewToNewChat"),
      "-", h("会話"), id("fork"), id("compact"), id("purposeNote"), id("purposeDraft"), id("handoffThread"), id("orgIntakeFromChat"), id("reloadHistory"),
      "-", h("依頼の補助"), id("goal"), id("workMode"), id("referenceChat"), id("skills"), id("instructionFiles"), id("sideChat"),
      "-", h("実行"), id("interrupt"),
    ],
  },
  {
    title: "表示",
    entries: [
      h("画面"), id("toggleLeft"), id("toggleRight"), id("toggleMonitor"),
      "-", h("最前面"), id("toggleMainTop"), id("toggleMiniTop"),
      "-", h("ドック"), id("toggleDone"),
      "-", h("検索"), id("find"), id("palette"),
    ],
  },
  {
    title: "ツール",
    entries: [
      h("連携"), id("toolServers"), id("skillsHub"), id("worktreeManage"), id("cloudDelegation"),
      "-", h("状態"), id("backendStatus"), id("usage"), id("modeMenu"),
      "-", h("設定"), id("settings"),
    ],
  },
  {
    title: "ヘルプ",
    entries: [id("shortcuts"), id("howto"), id("parity"), id("about")],
  },
];

/** ヘッダー「…」（このチャットでよく使う操作の近道。上部メニューの部分集合）。 */
export const HEADER_MORE: MenuEntry[] = [
  h("変更"), id("changeList"), id("revertChanges"), id("codeReview"),
  "-", h("会話"), id("fork"), id("purposeNote"), id("purposeDraft"), id("handoffThread"), id("goal"), id("reloadHistory"),
  "-", h("このチャット"), id("cwdChange"), id("newChatSameFolder"), id("renameChat"), id("togglePin"),
  "-", id("palette"),
];

/** 描画用に解決した1行。 */
export type ResolvedEntry =
  | { kind: "heading"; text: string }
  | { kind: "sep" }
  | { kind: "item"; view: CommandView };

/** `id` を台帳の view に解決する。台帳にない id は捨てる。先頭・末尾・連続の区切りは詰める。 */
export function resolveEntries(entries: MenuEntry[], views: CommandView[]): ResolvedEntry[] {
  const byId = new Map(views.map((v) => [v.cmd.id, v]));
  const out: ResolvedEntry[] = [];
  for (const e of entries) {
    if (e === "-") out.push({ kind: "sep" });
    else if ("heading" in e) out.push({ kind: "heading", text: e.heading });
    else { const v = byId.get(e.id); if (v) out.push({ kind: "item", view: v }); }
  }
  // 見出しだけで中身のないものと、先頭・末尾・連続の区切りを取り除く。
  const kept: ResolvedEntry[] = [];
  for (let i = 0; i < out.length; i++) {
    const cur = out[i];
    if (cur.kind === "heading" && (i + 1 >= out.length || out[i + 1].kind !== "item")) continue;
    if (cur.kind === "sep" && (kept.length === 0 || kept[kept.length - 1].kind === "sep")) continue;
    kept.push(cur);
  }
  while (kept.length && kept[kept.length - 1].kind === "sep") kept.pop();
  return kept;
}

/** 上部メニューの1つを解決する。 */
export const resolveMenu = (title: MenuGroup, views: CommandView[]): ResolvedEntry[] => {
  const m = MENUBAR.find((x) => x.title === title);
  return m ? resolveEntries(m.entries, views) : [];
};

/** メニュー定義が参照する id の一覧。 */
export const entryIds = (entries: MenuEntry[]): string[] => entries.flatMap((e) => (e !== "-" && "id" in e ? [e.id] : []));
