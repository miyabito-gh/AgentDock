// Codex app-server のスキーマを、版どうしで比べる（読み取りだけ。ネットワーク・Codexの設定には触れない）。
// 使い方: node probes/app-server/schema-diff.mjs <旧版> <新版> [--write-methods]
//   例: node probes/app-server/schema-diff.mjs 0.163.0 0.166.0
// 入力: probes/app-server/schema/<版>/json-stable と json-experimental（`codex app-server generate-json-schema [--experimental] --out <dir>` の出力）。
// 出力: メソッドの追加・削除、型（v2/*.json）の項目の追加・削除・必須の変更、AgentDockのソースが使っているメソッドへの影響。
// --write-methods: 新版のディレクトリに methods-<stable|experimental>-<種別>.txt を書く（既存の形式。並べ替え済み）。
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const [oldV, newV, ...flags] = process.argv.slice(2);
if (!oldV || !newV) { console.error("使い方: node schema-diff.mjs <旧版> <新版> [--write-methods]"); process.exit(2); }
const dir = (v) => path.join(here, "schema", v);
const readJson = (f) => JSON.parse(fs.readFileSync(f, "utf8"));
const KINDS = ["ClientRequest", "ServerNotification", "ServerRequest"];

function methods(v, channel, kind) {
  const f = path.join(dir(v), `json-${channel}`, `${kind}.json`);
  if (!fs.existsSync(f)) return null;
  const j = readJson(f);
  return new Set((j.oneOf ?? []).map((o) => o?.properties?.method?.enum?.[0]).filter(Boolean));
}
const diffSet = (a, b) => ({ added: [...b].filter((x) => !a.has(x)).sort(), removed: [...a].filter((x) => !b.has(x)).sort() });

// 型の項目（トップレベルの properties と required）の違い。
function shape(f) {
  const j = readJson(f);
  return { props: Object.keys(j.properties ?? {}).sort(), required: [...(j.required ?? [])].sort(), text: JSON.stringify(j) };
}
function typeDiff(v1, v2, channel) {
  const d1 = path.join(dir(v1), `json-${channel}`, "v2"), d2 = path.join(dir(v2), `json-${channel}`, "v2");
  if (!fs.existsSync(d1) || !fs.existsSync(d2)) return { added: [], removed: [], changed: [] };
  const f1 = new Set(fs.readdirSync(d1)), f2 = new Set(fs.readdirSync(d2));
  const changed = [];
  for (const f of f2) {
    if (!f1.has(f)) continue;
    const a = shape(path.join(d1, f)), b = shape(path.join(d2, f));
    if (a.text === b.text) continue;
    changed.push({
      type: f.replace(/\.json$/, ""),
      addedProps: b.props.filter((x) => !a.props.includes(x)),
      removedProps: a.props.filter((x) => !b.props.includes(x)),
      requiredAdded: b.required.filter((x) => !a.required.includes(x)),
      requiredRemoved: a.required.filter((x) => !b.required.includes(x)),
    });
  }
  return { added: [...f2].filter((x) => !f1.has(x)).sort(), removed: [...f1].filter((x) => !f2.has(x)).sort(), changed };
}

// AgentDockのRustソースに、引用符つきで現れるメソッド名（＝使っている可能性が高いもの）。
function usedMethods(all) {
  const root = path.join(here, "..", "..", "app", "src-tauri", "src");
  const text = [];
  (function walk(d) {
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p); else if (e.name.endsWith(".rs")) text.push(fs.readFileSync(p, "utf8"));
    }
  })(root);
  const src = text.join("\n");
  return new Set([...all].filter((m) => src.includes(`"${m}"`)));
}

// 型の中身を再帰的に比べ、後方互換を壊しうる変更（項目の削除・必須化・列挙値の削除）だけを拾う。追加は互換なので拾わない。
function deepRisks(v1, v2, channel) {
  const d1 = path.join(dir(v1), `json-${channel}`, "v2"), d2 = path.join(dir(v2), `json-${channel}`, "v2");
  const out = [];
  if (!fs.existsSync(d1) || !fs.existsSync(d2)) return out;
  const walk = (x, y, at, file) => {
    if (x && typeof x === "object" && y && typeof y === "object" && !Array.isArray(x) && !Array.isArray(y)) {
      if (x.properties) for (const k of Object.keys(x.properties)) if (!(y.properties && k in y.properties)) out.push(`${file} ${at}: 項目削除 ${k}`);
      if (Array.isArray(x.required)) for (const k of (y.required ?? [])) if (!x.required.includes(k)) out.push(`${file} ${at}: 必須化 ${k}`);
      if (Array.isArray(x.enum)) for (const k of x.enum) if (!(y.enum ?? []).includes(k)) out.push(`${file} ${at}: 列挙値の削除 ${k}`);
      for (const k of Object.keys(x)) if (k in y) walk(x[k], y[k], `${at}/${k}`, file);
    } else if (Array.isArray(x) && Array.isArray(y) && x.length === y.length) x.forEach((v, i) => walk(v, y[i], `${at}[${i}]`, file));
  };
  for (const f of fs.readdirSync(d1)) if (fs.existsSync(path.join(d2, f))) walk(readJson(path.join(d1, f)), readJson(path.join(d2, f)), "", f);
  return out;
}

const report = [];
const allOld = new Set(), removedAny = new Set(), changedTypes = new Set();
for (const channel of ["stable", "experimental"]) {
  report.push(`\n## ${channel}`);
  for (const kind of KINDS) {
    const a = methods(oldV, channel, kind), b = methods(newV, channel, kind);
    if (!a || !b) { report.push(`- ${kind}: スキーマがありません（${!a ? oldV : newV}）`); continue; }
    a.forEach((m) => allOld.add(m));
    const { added, removed } = diffSet(a, b);
    removed.forEach((m) => removedAny.add(m));
    report.push(`- ${kind}: +${added.length} / -${removed.length}`);
    added.forEach((m) => report.push(`    + ${m}`));
    removed.forEach((m) => report.push(`    - ${m}`));
    if (flags.includes("--write-methods")) {
      fs.writeFileSync(path.join(dir(newV), `methods-${channel}-${kind}.txt`), [...b].sort().join("\n") + "\n");
    }
  }
  const t = typeDiff(oldV, newV, channel);
  report.push(`- 型(v2): 追加 ${t.added.length} / 削除 ${t.removed.length} / 変更 ${t.changed.length}`);
  t.added.forEach((f) => report.push(`    + ${f}`));
  t.removed.forEach((f) => report.push(`    - ${f}`));
  for (const c of t.changed) {
    const parts = [];
    if (c.addedProps.length) parts.push(`項目追加: ${c.addedProps.join(", ")}`);
    if (c.removedProps.length) parts.push(`項目削除: ${c.removedProps.join(", ")}`);
    if (c.requiredAdded.length) parts.push(`必須になった: ${c.requiredAdded.join(", ")}`);
    if (c.requiredRemoved.length) parts.push(`必須でなくなった: ${c.requiredRemoved.join(", ")}`);
    report.push(`    ~ ${c.type}${parts.length ? "（" + parts.join(" / ") + "）" : "（中身の変更）"}`);
    if (c.removedProps.length || c.requiredAdded.length) changedTypes.add(c.type);
  }
}
const deep = [...new Set([...deepRisks(oldV, newV, "stable"), ...deepRisks(oldV, newV, "experimental")])];
// 同じ変更が多数の型に現れるので、型の名前を除いた内容でまとめて数える。
const deepGroups = new Map();
for (const line of deep) { const [file, ...rest] = line.split(" "); const key = rest.join(" "); (deepGroups.get(key) ?? deepGroups.set(key, []).get(key)).push(file.replace(/.json$/, "")); }
report.push("\n## 互換を壊しうる変更（深い比較。項目の削除・必須化・列挙値の削除）");
if (!deepGroups.size) report.push("- なし");
for (const [key, files] of deepGroups) report.push(`- ${key}（${files.length}か所: ${files.slice(0, 3).join(", ")}${files.length > 3 ? " ほか" : ""}）`);
const used = usedMethods(allOld);
const usedRemoved = [...removedAny].filter((m) => used.has(m));
report.push("\n## 判定");
report.push(`- AgentDockが使っているメソッドで、新版から無くなったもの: ${usedRemoved.length ? usedRemoved.join(", ") : "なし"}`);
report.push(`- 項目の削除・必須化があった型（使用箇所は要確認）: ${changedTypes.size ? [...changedTypes].sort().join(", ") : "なし"}`);
report.push(usedRemoved.length || changedTypes.size || deepGroups.size ? "- 結論: 互換を壊しうる変更あり。AgentDockが使っているかを、一覧で確認してから、確認済みにする。" : "- 結論: 追加のみ（使っているメソッドの削除なし、型の項目削除・必須化なし）。実Codexのスモーク確認後に、確認済みにできる。");
console.log(`# スキーマ差分 ${oldV} → ${newV}` + report.join("\n"));
