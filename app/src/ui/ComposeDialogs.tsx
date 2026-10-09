// 過去会話の参考指定（#8）・Skillの指定と指示ファイル（#11）の本体（段階③ P3-4）。外枠（Shell）は Dialogs.tsx 側。
// 規則: 参考・一覧・確認は読取りだけ（会話を再開しない・参照先を変更しない）。入力欄へ入れるだけで自動送信しない（要約もしない）。
//       取得できなかったものは「未取得」と書き、空・0で代用しない。雛形の作成は確認の後だけで、既存のファイルは上書きしない。
import { useEffect, useState } from "react";
import * as host from "../ipc/client";
import type { Chat, ChatKey, ExternalId, InstructionFiles, Known, OpCapability, ReferenceBlock, ReferenceTurns, SkillInfo, SkillList } from "../ipc/types";
import { UnverifiedTag } from "./PrefsDialogs";
import { chatName } from "./derive";
import { showKnown } from "./format";
import { About, FootActions } from "./DialogParts";

const errText = (e: unknown) => host.asIpcError(e).message;

export interface ComposeProps {
  chat: Chat | undefined;
  live: boolean;
  /** 入力欄のカーソル位置へ文章を入れる（送信はしない）。 */
  insertText: (text: string) => void;
  /** Skillを入力欄の指定（チップ）にする。戻り値は表示する文。 */
  pickSkill: (chat: ChatKey, name: string, path: string) => Promise<string>;
}

// ───────────────────────────── 過去の会話を参考に ─────────────────────────────

const DEFAULT_PICK = 3;

/** 参考にする過去の会話。選べるのは、いま選んでいるチャットの履歴（アーカイブ・外部の会話も、一覧から選んで開けば同じ）。 */
export function ReferenceBody({ chat, chats, live, cap, insertText, onDone }: { chat: Chat | undefined; chats: Chat[]; live: boolean; cap: OpCapability | undefined; insertText: (t: string) => void; onDone: () => void }) {
  // 参考にする会話（初期値は選んでいるチャット。アーカイブ・外部の会話も選べる）。
  const [srcId, setSrcId] = useState(chat?.key.id ?? "");
  const src = chats.find((c) => c.key.id === srcId) ?? chat;
  const [list, setList] = useState<ReferenceTurns | null>(null);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [picked, setPicked] = useState<ExternalId[]>([]);
  const [block, setBlock] = useState<ReferenceBlock | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const key = src?.key;
  const chatId = key?.id;

  useEffect(() => {
    if (!live || !key) return;
    let alive = true;
    setList(null); setLoadErr(null); setBlock(null); setPicked([]);
    host.listReferenceTurns(key).then((l) => {
      if (!alive) return;
      setList(l);
      setPicked(l.turns.slice(-DEFAULT_PICK).map((t) => t.turn));
    }).catch((e) => { if (alive) setLoadErr(errText(e)); });
    return () => { alive = false; };
  }, [live, chatId]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!src) return <div className="content"><p>チャットを選んでください。</p></div>;
  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;

  const toggle = (id: ExternalId) => { setBlock(null); setPicked((p) => (p.includes(id) ? p.filter((x) => x !== id) : [...p, id])); };
  const make = async () => {
    setBusy(true); setErr(null);
    try { setBlock(await host.buildReference(src.key, list ? list.turns.map((t) => t.turn).filter((t) => picked.includes(t)) : picked)); } catch (e) { setErr(errText(e)); } finally { setBusy(false); }
  };
  const insert = () => { if (block) { insertText(block.text); onDone(); } };
  return (
    <>
    <div className="content">
      <p>過去のやり取りの本文を、そのまま入力欄へ入れます。要約はしません。送信前に確認・編集できます。 <UnverifiedTag cap={cap} /></p>
      <About><p>履歴を読むだけで、参照先の会話の再開や変更はしません。アーカイブ・外部の会話も選べます。</p></About>
      <div className="field"><span>参考にする会話</span>
        <select value={src.key.id} onChange={(e) => setSrcId(e.target.value)} aria-label="参考にする会話">
          {chats.map((c) => <option key={c.key.id} value={c.key.id}>{chatName(c)}{c.key.id === chat?.key.id ? "（このチャット）" : ""}{c.noHistory ? "（履歴なし）" : ""}</option>)}
        </select></div>
      {loadErr ? <div className="why err">履歴を取得できませんでした（未取得）: {loadErr}</div> : null}
      {!list && !loadErr ? <p className="small muted">履歴を取得しています…</p> : null}
      {list && list.turns.length === 0 ? <p className="small muted">参考にできるやり取りがありません（会話本文のあるturnがありません）。</p> : null}
      {list && list.turns.length > 0 ? (
        <div className="check-list" role="group" aria-label="参考にするturn">
          {list.turns.map((t, i) => (
            <label key={t.turn} style={{ display: "block" }}>
              <input type="checkbox" checked={picked.includes(t.turn)} onChange={() => toggle(t.turn)} />
              {" "}#{i + 1}　{t.userPreview || "（依頼の本文なし）"}
              {!t.hasAgentText ? <span className="small muted">　応答の本文なし</span> : null}
              {!t.complete ? <span className="small muted">　一部のみ取得</span> : null}
            </label>
          ))}
        </div>
      ) : null}
      <div style={{ marginTop: 8 }}><button className="btn-line" disabled={busy || picked.length === 0} onClick={() => void make()}>{busy ? "作成しています…" : "抜粋を作る"}</button></div>
      {err ? <div className="why err" style={{ marginTop: 4 }}>{err}</div> : null}
      {block ? (
        <>
          <p className="small" style={{ marginTop: 8 }}>{block.chars.toLocaleString("ja-JP")} 文字{block.overLimit ? "（長いため、モデルの入力を圧迫するおそれがあります。必要なturnだけに絞ることをおすすめします。入れること自体は止めません）" : ""}</p>
          {block.missing.length > 0 ? <div className="why err">取得できなかったturnがあります（本文に「未取得」と書いてあります）: {block.missing.join("、")}</div> : null}
          <textarea readOnly value={block.text} rows={10} style={{ width: "100%" }} className="mono" aria-label="入力欄へ入れる抜粋" />
        </>
      ) : null}
    </div>
    <FootActions main={<button className="btn primary" disabled={!block} onClick={insert}>入力欄へ入れる（送信はしません）</button>} />
    </>
  );
}

// ───────────────────────────── Skill・指示ファイル ─────────────────────────────

const scopeText = (k: Known<string>): string => {
  if (k.kind !== "value") return showKnown(k);
  return ({ user: "ユーザー", repo: "このリポジトリ", system: "システム", admin: "管理者" } as Record<string, string>)[k.value] ?? k.value;
};

function SkillRows({ skills, pick }: { skills: SkillInfo[]; pick?: (s: SkillInfo) => void }) {
  if (skills.length === 0) return <p className="small muted">この作業フォルダで使えるSkillはありません。</p>;
  return (
    <div>
      {skills.map((s) => {
        const off = s.enabled.kind === "value" && !s.enabled.value;
        return (
          <div className="frow" key={s.path}>
            <div className="grow">
              <b>{s.name}</b>　<span className="small muted">{scopeText(s.scope)}{off ? "・無効" : s.enabled.kind !== "value" ? "・有効かは未確認" : ""}</span>
              <div className="small">{s.description.kind === "value" ? s.description.value : `説明: ${showKnown(s.description)}`}</div>
              <div className="mono small muted">{s.path}</div>
            </div>
            {pick ? <button className="small" disabled={off} title={off ? "無効のSkillは指定できません" : "入力欄の指定（チップ）にします。送信時に明示的に呼び出します"} onClick={() => pick(s)}>指定</button> : null}
          </div>
        );
      })}
    </div>
  );
}

/** 作業フォルダのSkillの一覧。`pick` があれば「指定」（入力欄のチップ）を出す。確認だけなら省略。有効・無効の切替は提供しない。 */
export function SkillsBody({ chat, live, cap, pickSkill, onDone }: { chat: Chat | undefined; live: boolean; cap: OpCapability | undefined; pickSkill?: ComposeProps["pickSkill"]; onDone?: () => void }) {
  const [list, setList] = useState<SkillList | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const key = chat?.key;
  const chatId = key?.id;
  const load = (force: boolean) => {
    if (!key) return;
    setBusy(true); setErr(null);
    host.listSkills(key, force).then(setList).catch((e) => setErr(errText(e))).finally(() => setBusy(false));
  };
  useEffect(() => { if (live && key) load(false); }, [live, chatId]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!chat) return <div className="content"><p>チャットを選んでください（そのチャットの作業フォルダのSkillを表示します）。</p></div>;
  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;
  const pick = pickSkill ? async (s: SkillInfo) => { setMsg(await pickSkill(chat.key, s.name, s.path)); onDone?.(); } : undefined;
  return (
    <div className="content">
      <p>作業フォルダのSkillです。{pickSkill ? "指定すると、入力欄のチップになり、送信時にそのSkillを明示的に呼び出します。" : ""} <UnverifiedTag cap={cap} /></p>
      <About><p>Skillの定義は読むだけで、変更や有効・無効の切替はしません（~/.codex 配下も同じです）。</p></About>
      <div style={{ marginBottom: 6 }}><button className="btn-line" disabled={busy} onClick={() => load(true)}>{busy ? "読み込んでいます…" : "再読み込み（ディスクを再走査）"}</button></div>
      {err ? <div className="why err">Skillを取得できませんでした（未取得）: {err}</div> : null}
      {list ? <SkillRows skills={list.skills} pick={pick ? (s) => void pick(s) : undefined} /> : null}
      {list && list.errors.length > 0 ? <div className="why err" style={{ marginTop: 6 }}>読み込めなかったSkillの定義があります:<ul>{list.errors.map((e, i) => <li key={i} className="mono small">{e}</li>)}</ul></div> : null}
      {msg ? <p className="small" style={{ marginTop: 6 }}>{msg}</p> : null}
    </div>
  );
}

/** 依頼文（Codexに AGENTS.md の作成を頼む定型文）。入力欄へ入れるだけで、送信はユーザー。 */
export const AGENTS_REQUEST_TEXT = "このフォルダに AGENTS.md を作成してください。まずフォルダの内容を確認し、プロジェクトの概要・作業のルール・ビルドやテストの方法を簡潔にまとめてください。既に AGENTS.md がある場合は上書きせず、変更案だけ示してください。";

/** 指示ファイル（AGENTS.md等）の確認と、無いときの初期化（雛形の作成／Codexへの依頼文）。 */
export function InstructionBody({ chat, live, cap, insertText, openFile }: { chat: Chat | undefined; live: boolean; cap: OpCapability | undefined; insertText: (t: string) => void; openFile: (path: string) => void }) {
  const [info, setInfo] = useState<InstructionFiles | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const key = chat?.key;
  const chatId = key?.id;
  const load = () => {
    if (!key) return;
    host.getInstructionFiles(key).then((i) => { setInfo(i); setErr(null); }).catch((e) => setErr(errText(e)));
  };
  useEffect(() => { if (live && key) load(); }, [live, chatId]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!chat) return <div className="content"><p>チャットを選んでください。</p></div>;
  if (!live) return <div className="content"><p>モックでは動きません（実接続で使えます）。</p></div>;
  const cwd = info && info.cwd.kind === "value" ? info.cwd.value : null;
  const create = async () => {
    if (!window.confirm(`作業フォルダに AGENTS.md の雛形を作成します。\n${cwd ?? ""}\n\n同じ名前のファイルがすでにある場合は何も書きません。作成しますか？`)) return;
    setBusy(true); setMsg(null);
    try {
      const r = await host.createInstructionTemplate(chat.key);
      setMsg(`雛形を作成しました: ${r.path}（内容は編集してください）`);
      load();
      openFile(r.path);
    } catch (e) {
      const er = host.asIpcError(e);
      setMsg(er.blocked?.kind === "alreadyExists" ? `${er.message}` : `作成できませんでした: ${er.message}`);
      load();
    } finally { setBusy(false); }
  };
  const missing = info?.agentsMdExists.kind === "value" && !info.agentsMdExists.value;
  return (
    <div className="content">
      <p>Codex が会話に読み込んでいる指示ファイル（AGENTS.md など）です。 <UnverifiedTag cap={cap} /></p>
      <About><p>一覧は、この接続で会話を開始・再開・分岐したときの応答に示されたものだけを表示します（表示のために会話を再開しません）。~/.codex 配下のファイルは読むだけです。</p></About>
      {err ? <div className="why err">取得できませんでした（未取得）: {err}</div> : null}
      {info ? (
        <>
          <div className="field"><span>作業フォルダ</span><span className="mono">{showKnown(info.cwd)}</span></div>
          <div className="field"><span>読み込み済み</span>
            {info.sources.kind === "value" ? (
              info.sources.value.length === 0 ? <span>なし（Codex は指示ファイルを読み込んでいません）</span> : (
                <div>
                  {info.sources.value.map((s) => (
                    <div className="frow" key={s.path}>
                      <div className="grow mono small">{s.path}　<span className="muted">{s.exists.kind === "value" ? (s.exists.value ? "実在を確認" : "見つかりません") : "実在は未確認"}</span></div>
                      <button className="small" disabled={s.exists.kind !== "value" || !s.exists.value} onClick={() => openFile(s.path)}>開く</button>
                    </div>
                  ))}
                </div>
              )
            ) : <span>{showKnown(info.sources)}（この接続ではまだ示されていません。会話を再開または新しく開始すると表示されます）</span>}
          </div>
          <div className="field"><span>この作業フォルダの AGENTS.md</span>
            <span>{info.agentsMdExists.kind === "value" ? (info.agentsMdExists.value ? "あります" : "ありません") : showKnown(info.agentsMdExists)}</span></div>
          {missing ? (
            <div className="field"><span>初期化</span>
              <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
                <button className="btn-line" disabled={busy} onClick={() => void create()}>雛形を作成…</button>
                <button className="btn-line" onClick={() => { insertText(AGENTS_REQUEST_TEXT); setMsg("依頼文を入力欄へ入れました。内容を確認して、送信するかは決めてください。"); }}>Codex に作成を依頼する文を入力欄へ</button>
              </div>
              <span className="note">雛形は新規作成のみで、既存のファイルは上書きしません。依頼文は入力欄へ入れるだけで、送信しません。</span></div>
          ) : null}
        </>
      ) : null}
      {msg ? <p className="small" style={{ marginTop: 6 }}>{msg}</p> : null}
    </div>
  );
}
