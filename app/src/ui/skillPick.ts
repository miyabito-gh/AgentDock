// Skill選択の純粋ロジック（絞り込み・並び・最近使った・同名の見分け・入力欄の `$` 呼出しの判定）。DOMは扱わない。
// 規則: Skillの定義は読むだけ。ここは表示用の整形だけで、送信・変更はしない。
import type { RecentSkill, SkillInfo } from "../ipc/types";

export const MAX_RECENT_SKILLS = 5;

/** 一文字の正規化: 全角半角（NFKC）・大文字小文字・カタカナ→ひらがな。 */
const normChar = (c: string): string => {
  const n = c.normalize("NFKC").toLowerCase();
  let out = "";
  for (const ch of n) {
    const code = ch.codePointAt(0)!;
    out += code >= 0x30a1 && code <= 0x30f6 ? String.fromCodePoint(code - 0x60) : ch;
  }
  return out;
};

/** 正規化した文字列と、正規化後の各UTF-16位置に対応する元の位置（元の文字列のUTF-16位置）。 */
function normalizeWithMap(s: string): { text: string; map: number[] } {
  let text = "";
  const map: number[] = [];
  let i = 0;
  for (const ch of s) {
    const n = normChar(ch);
    for (let k = 0; k < n.length; k++) map.push(i);
    text += n;
    i += ch.length;
  }
  return { text, map };
}

export const normalizeText = (s: string): string => normalizeWithMap(s).text;

/** 検索語を空白（全角含む）で分け、正規化した語の並びにする。 */
export const queryTerms = (q: string): string[] => normalizeText(q).split(/\s+/).filter(Boolean);

export const descText = (s: SkillInfo): string => (s.description.kind === "value" ? s.description.value : "");
export const isDisabled = (s: SkillInfo): boolean => s.enabled.kind === "value" && !s.enabled.value;

/** パスの区切りを / にそろえる。 */
const slashPath = (p: string) => p.replace(/\\/g, "/");

/** 出所の表示名（色だけに頼らず文字で出す）。プラグイン配下はパスから判定する（SkillInfo に pluginId が無いため）。 */
export function originLabel(s: Pick<SkillInfo, "scope" | "path">): string {
  if (/\/plugins\//i.test(slashPath(s.path))) return "プラグイン";
  const v = s.scope.kind === "value" ? s.scope.value : "";
  const t = ({ repo: "プロジェクト", user: "個人", system: "システム", admin: "管理者" } as Record<string, string>)[v];
  return t ?? (v || "出所不明");
}

/** パス末尾の短縮表示（末尾3区間）。 */
export function shortPath(path: string, keep = 3): string {
  const parts = slashPath(path).split("/").filter(Boolean);
  return parts.length <= keep ? path : `…/${parts.slice(-keep).join("/")}`;
}

/** すべての語が、名前・説明・場所のどれかに含まれる（AND）。語が空なら全部。 */
export function matchesSkill(s: SkillInfo, terms: string[]): boolean {
  if (terms.length === 0) return true;
  const hay = normalizeText(`${s.name}\n${descText(s)}\n${s.path}`);
  return terms.every((t) => hay.includes(t));
}

const collator = new Intl.Collator("ja", { numeric: true, sensitivity: "base" });

/** 名前の五十音／アルファベット順。無効は最下段。 */
export function sortSkills(skills: SkillInfo[]): SkillInfo[] {
  return [...skills].sort((a, b) => Number(isDisabled(a)) - Number(isDisabled(b)) || collator.compare(a.name, b.name) || collator.compare(a.path, b.path));
}

/** 記録のうち、いまの一覧に実在するもの（新しい順）。存在しなくなったものと、無効のものは出さない。 */
export function resolveRecent(recent: RecentSkill[], skills: SkillInfo[]): SkillInfo[] {
  const out: SkillInfo[] = [];
  for (const r of recent) {
    const s = skills.find((x) => x.path === r.path);
    if (s && !isDisabled(s) && !out.includes(s)) out.push(s);
    if (out.length >= MAX_RECENT_SKILLS) break;
  }
  return out;
}

export interface SkillView {
  items: SkillInfo[];
  /** 先頭の何件が「最近」の見出しの下か（絞り込み中は 0 で、見出しを出さない）。 */
  recentCount: number;
}

/** 表示する並び。語が空: 最近 → その他（名前順）。語あり: 一致したものの中で最近を先頭へ寄せ、残りは名前順。 */
export function filterSkills(skills: SkillInfo[], query: string, recent: RecentSkill[]): SkillView {
  const terms = queryTerms(query);
  const hit = skills.filter((s) => matchesSkill(s, terms));
  const rec = resolveRecent(recent, hit);
  const rest = sortSkills(hit.filter((s) => !rec.includes(s)));
  return { items: [...rec, ...rest], recentCount: terms.length === 0 ? rec.length : 0 };
}

/** 最近使った一覧の更新（先頭へ・同じ path は1件・最大5件）。 */
export function pushRecent(recent: RecentSkill[], picked: RecentSkill): RecentSkill[] {
  return [picked, ...recent.filter((r) => r.path !== picked.path)].slice(0, MAX_RECENT_SKILLS);
}

/** 同名の他のSkill（自分以外）。名前は正規化して比べる。 */
export function sameNameOthers(s: SkillInfo, all: SkillInfo[]): SkillInfo[] {
  const n = normalizeText(s.name);
  return all.filter((x) => x.path !== s.path && normalizeText(x.name) === n);
}

/** 同名ありの説明（ツールチップ用）。同名がなければ null。 */
export function sameNameNote(s: SkillInfo, all: SkillInfo[]): string | null {
  const o = sameNameOthers(s, all);
  if (o.length === 0) return null;
  return `同名のSkillが他に${o.length}件あります: ${o.map((x) => `${originLabel(x)}（${shortPath(x.path)}）`).join("、")}`;
}

export interface Seg { text: string; hit: boolean }

/** 検索語に一致した部分の強調用の分割（正規化して探し、元の文字列の位置へ戻す）。 */
export function highlightSegments(text: string, terms: string[]): Seg[] {
  if (!text) return [];
  if (terms.length === 0) return [{ text, hit: false }];
  const { text: n, map } = normalizeWithMap(text);
  const mark = new Array<boolean>(text.length).fill(false);
  for (const t of terms) {
    for (let from = n.indexOf(t); from >= 0 && t; from = n.indexOf(t, from + t.length)) {
      for (let k = from; k < from + t.length; k++) mark[map[k]] = true;
    }
  }
  const segs: Seg[] = [];
  let i = 0;
  for (const ch of text) { // コードポイント単位（サロゲートペアを割らない）
    const last = segs[segs.length - 1];
    if (last && last.hit === mark[i]) last.text += ch;
    else segs.push({ text: ch, hit: mark[i] });
    i += ch.length;
  }
  return segs;
}

export interface SkillTrigger {
  /** 消す範囲（トリガー文字の位置〜キャレット）。 */
  start: number;
  end: number;
  query: string;
}

/** 入力欄のキャレット直前が `$語`（行頭または空白の直後）か `/skill 語` なら、その範囲と語。そうでなければ null。 */
export function skillTrigger(draft: string, caret: number): SkillTrigger | null {
  const before = draft.slice(0, caret);
  const m1 = /(^|\s)(\$)([^\s$]*)$/.exec(before);
  if (m1) return { start: m1.index + m1[1].length, end: caret, query: m1[3] };
  const m2 = /(^|\s)(\/skill[ 　]+)([^\s]*)$/.exec(before);
  if (m2) return { start: m2.index + m2[1].length, end: caret, query: m2[3] };
  return null;
}

/** トリガーの範囲を下書きから消す（前後の空白は重ねない）。戻り値の cursor は消した位置。 */
export function removeTrigger(draft: string, t: SkillTrigger): { text: string; cursor: number } {
  let head = draft.slice(0, t.start);
  const tail = draft.slice(t.end);
  if (head.endsWith(" ") && tail.startsWith(" ")) head = head.slice(0, -1);
  return { text: head + tail, cursor: head.length };
}
