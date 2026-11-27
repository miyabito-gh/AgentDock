import { useEffect, useRef, useState, type FormEvent } from "react";
import MeetingFiles from "./MeetingFiles";
import { parseCalendar } from "./meetingIcs";
import { invoke } from "@tauri-apps/api/core";

type Agenda = { title: string; minutes: number | null };
type Plan = { title: string; joinUrl: string | null; location: string | null; description: string | null; start: number; end: number; timeZone: string; agenda: Agenda[] };
type Meeting = {
  id: string; revision: number; plan: Plan;
  phase: "prepared" | "running" | "break" | "ended" | "cancelled";
  run: { start: number; end: number | null; breakStart: number | null;
    agendaRuns: { index: number; start: number; end: number | null; pausedMs: number; completed: boolean }[];
    deadline: number; agendaDeadline: number | null; agendaIndex: number | null; pausedRemaining: number | null } | null;
  consumed: string[];
};
type Snapshot = { items: Meeting[]; damaged: number; unreadable: boolean; shared: boolean; at: number; clock_changed: boolean; focus: string | null };
const labels = { prepared: "準備済み", running: "進行中", break: "休憩中", ended: "終了", cancelled: "キャンセル済み" };
const active = (meeting: Meeting) => meeting.phase === "running" || meeting.phase === "break";

function localInput(ms: number) {
  return new Date(ms - new Date(ms).getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
}

function clock(ms: number) {
  const sec = Math.ceil(Math.abs(ms) / 1000);
  return (ms < 0 ? "超過 " : "") + Math.floor(sec / 60) + ":" + String(sec % 60).padStart(2, "0");
}

function MeetingForm({ meeting, submit, close, busy }: {
  meeting: Meeting | null;
  submit: (plan: Plan) => void;
  close: () => void;
  busy: boolean;
}) {
  const plan = meeting?.plan;
  const [agenda, setAgenda] = useState<Agenda[]>(plan?.agenda ?? []);
  const [start, setStart] = useState(() => localInput(plan?.start ?? Date.now()));
  const [end, setEnd] = useState(() => localInput(plan?.end ?? Date.now() + 1_800_000));
  const totalMinutes = agenda.reduce((sum, item) => sum + (item.minutes ?? 0), 0);
  const duration = new Date(end).getTime() - new Date(start).getTime();
  function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    submit({
      title: String(form.get("title")).trim(), joinUrl: String(form.get("url")).trim() || null,
      location: String(form.get("location")).trim() || null, description: String(form.get("description")).trim() || null,
      start: new Date(String(form.get("start"))).getTime(), end: new Date(String(form.get("end"))).getTime(),
      timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone, agenda,
    });
  }
  return <form onSubmit={save} className="meeting-form">
    <label>件名 <input name="title" defaultValue={plan?.title} required /></label>
    <label>参加 URL <input name="url" type="url" defaultValue={plan?.joinUrl ?? ""} /></label>
    <label>場所 <input name="location" defaultValue={plan?.location ?? ""} /></label>
    <label>説明 <textarea name="description" defaultValue={plan?.description ?? ""} /></label>
    <label>予定開始 <input name="start" type="datetime-local" value={start} onChange={e => setStart(e.target.value)} required /></label>
    <label>予定終了 <input name="end" type="datetime-local" value={end} onChange={e => setEnd(e.target.value)} required /></label>
    {agenda.map((item, index) => <div className="meeting-actions" key={index}>
      <input aria-label="議題名" value={item.title} required onChange={e => setAgenda(agenda.map((a, i) => i === index ? { ...a, title: e.target.value } : a))} />
      <input aria-label="持ち時間（分）" placeholder="分" type="number" min={1} max={1440} value={item.minutes ?? ""} onChange={e => setAgenda(agenda.map((a, i) => i === index ? { ...a, minutes: e.target.value ? e.target.valueAsNumber : null } : a))} />
      <button type="button" className="btn" onClick={() => setAgenda(agenda.filter((_, i) => i !== index))}>外す</button>
    </div>)}
    {totalMinutes * 60_000 > duration && duration > 0 && <p role="status">議題の持ち時間の合計が予定枠を超えています。</p>}
    <div className="meeting-actions">
      <button type="button" className="btn" onClick={() => setAgenda([...agenda, { title: "", minutes: null }])}>議題を追加</button>
      <button className="btn" disabled={busy}>保存</button>
      <button type="button" className="btn" onClick={close}>戻る</button>
    </div>
  </form>;
}

export default function MeetingApp() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [selected, setSelected] = useState<string>();
  const [editor, setEditor] = useState<Meeting | "new" | null>(null);
  const [error, setError] = useState("");
  const [importStatus, setImportStatus] = useState("");
  const [busy, setBusy] = useState(false);
  const [late, setLate] = useState<{ id: string; end: string } | null>(null);
  const pending = useRef(false);
  const generation = useRef(0);
  const [cover, setCover] = useState(false);
  const [alwaysOnTop, setAlwaysOnTop] = useState(false);

  useEffect(() => {
    let disposed = false;
    let inflight = false;
    async function refresh() {
      if (inflight || pending.current) return;
      inflight = true;
      const version = generation.current;
      try {
        const next = await invoke<Snapshot>("meeting_list");
        if (!disposed && version === generation.current) {
          setSnapshot(next);
          if (next.focus) {
            setSelected(next.focus);
            setEditor(null);
            setLate(null);
          }
        }
      } catch {
        if (!disposed && version === generation.current) setSnapshot(null);
      } finally { inflight = false; }
    }
    void refresh();
    const timer = window.setInterval(refresh, 500);
    window.addEventListener("focus", refresh);
    return () => {
      disposed = true;
      clearInterval(timer);
      window.removeEventListener("focus", refresh);
    };
  }, []);

  async function command(name: string, args: Record<string, unknown>, done?: (result: unknown) => void) {
    if (pending.current) return;
    pending.current = true;
    generation.current += 1;
    setBusy(true);
    setError("");
    try {
      const result = await invoke(name, args);
      done?.(result);
    } catch (e) { setError(String(e)); }
    finally {
      try { setSnapshot(await invoke<Snapshot>("meeting_list")); }
      catch { setSnapshot(null); }
      pending.current = false;
      setCover(false);
      setBusy(false);
    }
  }

  async function importCalendar(file: File) {
    setImportStatus("");
    try {
      const { entries, skipped } = parseCalendar(await file.text());
      await command("meeting_import", { entries }, raw => {
        const result = raw as { added: number; duplicates: number; errors: string[] };
        setImportStatus([`取込 ${result.added} 件 · 重複 ${result.duplicates} 件`, ...skipped, ...result.errors].join("\n"));
        if (result.added === 1 && entries.length === 1) setSelected(entries[0].id);
      });
    } catch (error) { setError(String(error)); }
  }

  const at = snapshot?.at ?? Date.now();
  const current = (cover ? undefined : snapshot)?.items.find(m => m.id === selected) ?? (cover ? undefined : snapshot)?.items.find(active);
  const showFiles = !!snapshot && !snapshot.shared && !cover && !editor;
  const [filesMeeting, setFilesMeeting] = useState<Meeting>();
  useEffect(() => {
    if (showFiles && current) setFilesMeeting(current);
  }, [showFiles, current]);
  const filesTarget = showFiles && current ? current : filesMeeting;
  const run = current?.run;
  const agendaRun = run?.agendaRuns[run.agendaRuns.length - 1];
  const apply = (kind: string, fields?: Record<string, unknown>, done?: () => void) => {
    if (current) void command("meeting_update", { id: current.id, revision: current.revision, action: { kind, ...fields } }, done);
  };
  function start(meeting: Meeting) {
    if (Date.now() >= meeting.plan.end) {
      setLate({ id: meeting.id, end: localInput(Date.now() + meeting.plan.end - meeting.plan.start) });
    } else apply("start");
  }

  return <main className="meeting-page">
    <header className="meeting-actions"><h1>会議</h1>
      <button className="btn" aria-pressed={alwaysOnTop} onClick={() => {
        void command("meeting_top", { enabled: !alwaysOnTop }, () => setAlwaysOnTop(!alwaysOnTop));
      }}>最前面</button>
      {snapshot && !snapshot.shared && !cover && <button className="btn" onClick={() => setEditor("new")}>会議を登録</button>}
    </header>
    {snapshot && !snapshot.shared && !cover && <div>
      <label>ICS 取り込み <input type="file" accept=".ics,text/calendar" disabled={busy} onChange={event => {
        const file = event.target.files?.[0];
        event.target.value = "";
        if (file) void importCalendar(file);
      }} /></label>
      {importStatus && <p role="status" style={{ whiteSpace: "pre-wrap" }}>{importStatus}</p>}
    </div>}
    {!snapshot && <p role="status">会議を読み込み中です。</p>}
    {snapshot?.shared && <p role="status">共有モード中。件名と議題を隠し、通知を停止しています。</p>}
    {snapshot?.unreadable && <p role="alert">会議の保存領域を読み込めません。</p>}
    {!!snapshot?.damaged && <p role="alert">{snapshot.damaged} 件の会議を読み込めません。保存ファイルは保持されています。</p>}
    {snapshot?.clock_changed && <p role="alert">PC の時刻が変わりました。会議の残り時間と予定時刻を確認してください。</p>}
    {error && !snapshot?.shared && !cover && <p role="alert">{error}</p>}
    {snapshot && !snapshot.shared && !cover && editor && <MeetingForm
      key={editor === "new" ? "new" : editor.id}
      meeting={editor === "new" ? null : editor} busy={busy} close={() => setEditor(null)}
      submit={plan => {
        if (editor === "new") {
          const id = crypto.randomUUID();
          void command("meeting_create", { id, plan }, () => { setEditor(null); setSelected(id); });
        } else {
          void command("meeting_update", { id: editor.id, revision: editor.revision, action: { kind: "edit", plan } }, () => setEditor(null));
        }
      }} />}
    {current && !editor && <section className="meeting-card">
      <h2>{snapshot?.shared ? "進行中の会議" : current.plan.title}</h2>
      <p>{labels[current.phase]}</p>
      {!snapshot?.shared && current.plan.location && <p>{current.plan.location}</p>}
      {!snapshot?.shared && current.plan.description && <p style={{ whiteSpace: "pre-wrap" }}>{current.plan.description}</p>}
      {!snapshot?.shared && current.plan.joinUrl && <button className="btn" disabled={busy} onClick={() => void command("meeting_join", { id: current.id })}>会議に参加</button>}
      {current.phase === "prepared" && <div className="meeting-actions">
        <button className="btn" disabled={busy} onClick={() => start(current)}>開始</button>
        <button className="btn" disabled={busy} onClick={() => setEditor(current)}>編集</button>
        <button className="btn" disabled={busy} onClick={() => { if (window.confirm("予定をキャンセルしますか？")) apply("cancel"); }}>キャンセル</button>
      </div>}
      {current.phase === "prepared" && late?.id === current.id && <form className="meeting-form" onSubmit={e => {
        e.preventDefault();
        apply("start", { lateEnd: new Date(late.end).getTime() }, () => setLate(null));
      }}>
        <p>予定の終了時刻を過ぎています。</p>
        <label>終了時刻 <input type="datetime-local" required value={late.end} onChange={e => setLate({ ...late, end: e.target.value })} /></label>
        <div className="meeting-actions">
          <button className="btn" disabled={busy}>この時刻まで開始</button>
          <button type="button" className="btn" disabled={busy} onClick={() => apply("start", { lateEnd: Date.now() + current.plan.end - current.plan.start }, () => setLate(null))}>予定と同じ長さで開始</button>
        </div>
      </form>}
      {run?.end !== null && run?.end !== undefined && !snapshot?.shared && <>
        <p>会議の実績　{clock(Math.max(0, run.end - run.start))}</p>
        <ul>{run.agendaRuns.map(item => <li key={item.index}>
          {current.plan.agenda[item.index]?.title}　{item.completed ? "完了" : "途中終了"}　{clock(Math.max(0, (item.end ?? run.end!) - item.start - item.pausedMs))}
        </li>)}</ul>
      </>}
      {run && active(current) && <>
        <div className="meeting-clocks">
          <div>会議 <strong>{clock(run.deadline - at)}</strong></div>
          {run.agendaIndex !== null && <div>
            {snapshot?.shared ? "議題" : current.plan.agenda[run.agendaIndex]?.title}
            {run.agendaDeadline !== null || run.pausedRemaining !== null ? <strong>{clock(run.pausedRemaining ?? (run.agendaDeadline! - at))}</strong> : agendaRun && <strong>経過 {clock(Math.max(0, (run.breakStart ?? at) - agendaRun.start - agendaRun.pausedMs))}</strong>}
          </div>}
        </div>
        {!snapshot?.shared && run.agendaIndex !== null && current.plan.agenda[run.agendaIndex + 1] && <p>次の議題　{current.plan.agenda[run.agendaIndex + 1].title}</p>}
        {(run.agendaDeadline ?? (run.pausedRemaining === null ? 0 : at + run.pausedRemaining)) > run.deadline && <p role="status">議題が会議の終了予定を超えています。</p>}
        {snapshot?.shared && current.consumed.length > 0 && <details>
          <summary>通知の処理履歴</summary>
          <p>{current.consumed.length} 件の通知期限を処理済みです。</p>
        </details>}
        <div className="meeting-actions">
          <button className="btn" disabled={busy || current.phase !== "running" || run.agendaIndex === null} onClick={() => apply("next")}>{run.agendaIndex !== null && run.agendaIndex + 1 === current.plan.agenda.length ? "議題を完了" : "次の議題"}</button>
          <button className="btn" disabled={busy} onClick={() => {
            if (current.phase === "break") apply("resume");
            else if (window.confirm("休憩に入りますか？")) apply("pause");
          }}>{current.phase === "break" ? "再開" : "休憩"}</button>
          <button className="btn" disabled={busy || (run.agendaDeadline === null && run.pausedRemaining === null)} onClick={() => apply("extendAgenda")}>議題 +1分</button>
          <button className="btn" disabled={busy} onClick={() => apply("extendMeeting")}>会議 +5分</button>
          <button className="btn" disabled={busy} onClick={() => {
            if (snapshot?.shared || window.confirm("共有対象を選ぶときは、この会議窓を含めないでください。共有モードを開始しますか？")) {
              if (!snapshot?.shared) {
                setCover(true);
                setEditor(null);
                setLate(null);
              }
              apply("share", { enabled: !snapshot?.shared });
            }
          }}>{snapshot?.shared ? "共有モードを解除" : "共有モードを開始"}</button>
          <button className="btn" disabled={busy} onClick={() => { if (window.confirm("会議を終了しますか？")) apply("end"); }}>終了</button>
        </div>
      </>}
    </section>}
    {filesTarget && <MeetingFiles key={filesTarget.id} id={filesTarget.id} title={filesTarget.plan.title}
      agenda={filesTarget.plan.agenda.map(item => item.title)} hidden={!showFiles || !current} />}
    {snapshot && !snapshot.shared && !cover && <section>
      <h2>会議一覧</h2>
      {snapshot.items.length === 0 && <p>登録された会議はありません。</p>}
      <ul className="meeting-list">{snapshot.items.map(meeting => <li key={meeting.id}>
        <button className="btn" aria-pressed={current?.id === meeting.id} onClick={() => { setSelected(meeting.id); setEditor(null); setLate(null); }}>{meeting.plan.title}</button>
        <span>{new Date(meeting.plan.start).toLocaleString()} · {labels[meeting.phase]}</span>
      </li>)}</ul>
    </section>}
  </main>;
}
