// 応答本文の `::name{…}` 形式の指示（assistant directive）の解析（DESIGN_IMPROVE2 §C）。純粋部分。React・DOMに依存しない。
// 読んで表示を変えるだけ。書込み・送信・実行はしない。壊れた・未知のものは null（原文のまま）。
// 記法は Codex CLI（tui/assistant_directives.rs）の規則に合わせる: コロン1〜3個、属性 key=値（"…"・'…'・引用符なし）、キー重複は無効。

export const MAX_VALUE_CHARS = 4_000;
export const MAX_DIRECTIVE_CHARS = 8_000;
const MAX_LABEL_CHARS = 1_000;

export interface RawDirective {
  start: number;
  end: number;
  raw: string;
  name: string;
  label: string | null;
  attrs: Record<string, string>;
}

const NAME_START = /[A-Za-z]/;
const NAME_CHAR = /[A-Za-z0-9_-]/;
const KEY = /^[A-Za-z_][A-Za-z0-9_-]*$/;
/** 引用符内の `\"` を引用符として扱う指示（それ以外は文字どおり）。 */
const ESCAPING = new Set(["code-comment"]);
/** `[label]` を持てる指示。 */
const LABELED = new Set(["codex-followup"]);

/** `{` の直後から属性を読む。成功なら属性と `}` の次の位置。 */
function parseAttrs(text: string, from: number, limit: number, escaping: boolean): { attrs: Record<string, string>; next: number } | null {
  const attrs: Record<string, string> = {};
  let i = from;
  const ws = () => { while (i < limit && /[ \t]/.test(text[i])) i++; };
  for (;;) {
    ws();
    if (i >= limit) return null;
    if (text[i] === "}") return { attrs, next: i + 1 };
    let k = i;
    while (k < limit && /[A-Za-z0-9_-]/.test(text[k])) k++;
    const key = text.slice(i, k);
    if (!KEY.test(key) || Object.prototype.hasOwnProperty.call(attrs, key)) return null;
    i = k;
    ws();
    if (text[i] !== "=") return null;
    i++;
    ws();
    if (i >= limit) return null;
    let value = "";
    const q = text[i];
    if (q === '"' || q === "'") {
      i++;
      for (;;) {
        if (i >= limit) return null;
        const c = text[i];
        if (c === "\n" || c === "\r") return null;
        if (escaping && c === "\\" && text[i + 1] === q) { value += q; i += 2; }
        else if (c === q) { i++; break; }
        else { value += c; i++; }
        if (value.length > MAX_VALUE_CHARS) return null;
      }
    } else {
      let e = i;
      while (e < limit && !/[\s}]/.test(text[e])) e++;
      value = text.slice(i, e);
      if (value === "" || value.length > MAX_VALUE_CHARS) return null;
      i = e;
    }
    attrs[key] = value;
  }
}

/**
 * 本文から指示の候補を前から順に取り出す。重なりなし。走査量は入力長の定数倍までで、超えたら残りは探さない。
 * 前が英数字・コロンの位置（`http://…`、`a::b`）は候補にしない。
 */
export function scanDirectives(text: string): RawDirective[] {
  const out: RawDirective[] = [];
  let budget = text.length * 4 + 64;
  for (let i = 0; i < text.length; i++) {
    if (text[i] !== ":") continue;
    if (i > 0 && /[A-Za-z0-9_:-]/.test(text[i - 1])) continue;
    let j = i;
    while (j < text.length && text[j] === ":" && j - i < 4) j++;
    if (j - i > 3) { i = j - 1; continue; }
    if (j >= text.length || !NAME_START.test(text[j])) { i = j - 1; continue; }
    let n = j;
    while (n < text.length && NAME_CHAR.test(text[n])) n++;
    const name = text.slice(j, n);
    const limit = Math.min(text.length, i + MAX_DIRECTIVE_CHARS);
    budget -= Math.min(limit - i, 256);
    if (budget < 0) break;
    let p = n;
    let label: string | null = null;
    if (LABELED.has(name) && text[p] === "[") {
      const close = text.indexOf("]", p + 1);
      if (close < 0 || close >= limit || close - p - 1 > MAX_LABEL_CHARS || /[\r\n]/.test(text.slice(p + 1, close))) { i = n - 1; continue; }
      label = text.slice(p + 1, close);
      p = close + 1;
    }
    if (text[p] !== "{") { i = n - 1; continue; }
    const r = parseAttrs(text, p + 1, limit, ESCAPING.has(name));
    if (!r) { i = n - 1; continue; }
    out.push({ start: i, end: r.next, raw: text.slice(i, r.next), name, label, attrs: r.attrs });
    i = r.next - 1;
  }
  return out;
}

export type GitOp = "stage" | "commit" | "create-branch" | "push" | "create-pr";

/** 解釈した指示。種類ごとの必須項目が揃ったものだけ。`created-thread`・`automation-update`・未知の名前は対象外（null）。 */
export type Directive =
  | { kind: "git"; op: GitOp; cwd: string; branch: string | null; url: string | null; draft: boolean }
  | { kind: "codeComment"; title: string; body: string; file: string; start: number; end: number; priority: number | null }
  | { kind: "followup"; label: string; prompt: string }
  | { kind: "citation"; path: string; label: string | null };

const GIT_OPS: Record<string, GitOp> = {
  "git-stage": "stage", "git-commit": "commit", "git-create-branch": "create-branch", "git-push": "push", "git-create-pr": "create-pr",
};

const present = (v: string | undefined): v is string => v !== undefined && v.trim() !== "";

function lineNo(v: string | undefined): number | null {
  if (v === undefined || !/^\d{1,9}$/.test(v.trim())) return null;
  const n = Number(v.trim());
  return n >= 1 ? n : null;
}

/** 名前ごとの必須属性・既定値を確かめる。足りなければ null（原文のまま）。 */
export function interpretDirective(d: RawDirective): Directive | null {
  const a = d.attrs;
  const op = GIT_OPS[d.name];
  if (op) {
    if (!present(a.cwd)) return null;
    if ((op === "create-branch" || op === "push" || op === "create-pr") && !present(a.branch)) return null;
    return { kind: "git", op, cwd: a.cwd, branch: present(a.branch) ? a.branch : null, url: op === "create-pr" && present(a.url) ? a.url : null, draft: op === "create-pr" && a.isDraft === "true" };
  }
  if (d.name === "code-comment") {
    if (!present(a.title) || !present(a.body) || !present(a.file)) return null;
    const start = lineNo(a.start) ?? 1;
    const end = Math.max(lineNo(a.end) ?? start, start);
    const pm = a.priority === undefined ? null : /^P?([0-3])$/i.exec(a.priority.trim());
    return { kind: "codeComment", title: a.title, body: a.body, file: a.file, start, end, priority: pm ? Number(pm[1]) : null };
  }
  if (d.name === "codex-followup") {
    if (d.label === null || d.label.trim() === "" || !present(a.prompt)) return null;
    return { kind: "followup", label: d.label, prompt: a.prompt };
  }
  if (d.name === "codex-file-citation") {
    if (!present(a.path)) return null;
    return { kind: "citation", path: a.path, label: present(a.label) ? a.label : null };
  }
  return null;
}

/** 行頭（字下げのみ可）にあるか。code-comment はこの時だけ変換する。 */
export function atLineStart(text: string, index: number): boolean {
  const nl = text.lastIndexOf("\n", index - 1);
  return /^[ \t]*$/.test(text.slice(nl + 1, index));
}

export type DirectivePiece = { t: "text"; v: string } | { t: "directive"; raw: string; d: Directive };

/**
 * 文字列を、通常の文字と解釈できた指示へ分ける。`lineStartAtZero` は、この文字列の先頭が行頭か
 * （段落の最初の子なら true）。解釈できない候補は文字のまま。
 */
export function splitDirectives(text: string, lineStartAtZero = true): DirectivePiece[] {
  const out: DirectivePiece[] = [];
  let last = 0;
  for (const raw of scanDirectives(text)) {
    const d = interpretDirective(raw);
    if (!d) continue;
    if (d.kind === "codeComment") {
      const ls = raw.start === 0 ? lineStartAtZero : atLineStart(text, raw.start);
      if (!ls) continue;
    }
    if (raw.start > last) out.push({ t: "text", v: text.slice(last, raw.start) });
    out.push({ t: "directive", raw: raw.raw, d });
    last = raw.end;
  }
  if (!out.length) return [{ t: "text", v: text }];
  if (last < text.length) out.push({ t: "text", v: text.slice(last) });
  return out;
}
