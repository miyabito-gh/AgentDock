#[path = "meeting_files.rs"]
pub mod files;

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::Mutex,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{Emitter, Manager, State};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Agenda {
    title: String,
    minutes: Option<u32>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    title: String,
    join_url: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    description: Option<String>,
    start: i64,
    end: i64,
    time_zone: String,
    agenda: Vec<Agenda>,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
enum Phase {
    Prepared,
    Running,
    Break,
    Ended,
    Cancelled,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Source {
    Manual,
    Ics,
    Graph,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgendaRun {
    index: usize,
    start: i64,
    end: Option<i64>,
    paused_ms: i64,
    extended_ms: i64,
    completed: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Run {
    start: i64,
    end: Option<i64>,
    deadline: i64,
    agenda_deadline: Option<i64>,
    paused_remaining: Option<i64>,
    break_start: Option<i64>,
    agenda_runs: Vec<AgendaRun>,
    agenda_index: Option<usize>,
    meeting_revision: u64,
    agenda_revision: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meeting {
    schema_version: u32,
    id: String,
    revision: u64,
    plan: Plan,
    phase: Phase,
    source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ics_uid: Option<String>,
    run: Option<Run>,
    #[serde(default)]
    shared: bool,
    #[serde(default)]
    consumed: BTreeSet<String>,
}

impl Run {
    fn valid(&self) -> bool {
        let end = 253_402_300_799_999;
        let valid = |v| (0..=end).contains(&v);
        valid(self.start)
            && valid(self.deadline)
            && self.agenda_deadline.is_none_or(valid)
            && self.break_start.is_none_or(valid)
            && self
                .paused_remaining
                .is_none_or(|v| (-end..=end).contains(&v))
            && self.agenda_runs.iter().all(|r| {
                valid(r.start)
                    && r.end.is_none_or(valid)
                    && valid(r.paused_ms)
                    && valid(r.extended_ms)
            })
    }
}

impl Meeting {
    fn active(&self) -> bool {
        matches!(self.phase, Phase::Running | Phase::Break)
    }

    fn select_agenda(&mut self, index: usize, at: i64) {
        let run = self.run.as_mut().unwrap();
        let agenda = self.plan.agenda.get(index);
        run.agenda_index = agenda.map(|_| index);
        run.agenda_deadline = agenda
            .and_then(|a| a.minutes)
            .map(|m| at + i64::from(m) * 60_000);
        run.agenda_revision += 1;
        if agenda.is_some() {
            run.agenda_runs.push(AgendaRun {
                index,
                start: at,
                end: None,
                paused_ms: 0,
                extended_ms: 0,
                completed: false,
            });
        }
    }

    fn advance(&mut self, action: Action, at: i64) -> Result<(), String> {
        match action {
            Action::Edit { plan } if self.phase == Phase::Prepared => {
                validate(&plan)?;
                self.plan = plan;
                self.consumed.clear();
            }
            Action::Start { late_end } if self.phase == Phase::Prepared => {
                let deadline = if at < self.plan.end {
                    self.plan.end
                } else {
                    late_end
                        .filter(|v| *v > at && *v <= at + MAX_SPAN)
                        .ok_or("終了時刻を指定してください")?
                };
                self.phase = Phase::Running;
                self.run = Some(Run {
                    start: at,
                    end: None,
                    deadline,
                    agenda_deadline: None,
                    paused_remaining: None,
                    break_start: None,
                    agenda_runs: vec![],
                    agenda_index: None,
                    meeting_revision: 0,
                    agenda_revision: 0,
                });
                self.select_agenda(0, at);
            }
            Action::Cancel if self.phase == Phase::Prepared => self.phase = Phase::Cancelled,
            Action::Share { enabled } if self.active() => self.shared = enabled,
            Action::Next if self.phase == Phase::Running => {
                let run = self.run.as_mut().unwrap();
                let index = run.agenda_index.ok_or("進行中の議題がありません")?;
                let current = run.agenda_runs.last_mut().unwrap();
                current.end = Some(at);
                current.completed = true;
                self.select_agenda(index + 1, at);
            }
            Action::Pause if self.phase == Phase::Running => {
                let run = self.run.as_mut().unwrap();
                run.break_start = Some(at);
                run.paused_remaining = run.agenda_deadline.take().map(|deadline| deadline - at);
                self.phase = Phase::Break;
            }
            Action::Resume if self.phase == Phase::Break => {
                let run = self.run.as_mut().unwrap();
                close_break(run, at);
                run.agenda_deadline = run.paused_remaining.take().map(|remaining| at + remaining);
                self.phase = Phase::Running;
            }
            Action::ExtendMeeting if self.active() => {
                let run = self.run.as_mut().unwrap();
                let previous = run.deadline;
                run.deadline += 300_000;
                rearm(
                    &mut self.consumed,
                    "meeting",
                    &mut run.meeting_revision,
                    previous,
                    run.deadline,
                    at,
                    &[300, 60, 0],
                );
            }
            Action::ExtendAgenda if self.active() => {
                let run = self.run.as_mut().unwrap();
                let paused = self.phase == Phase::Break;
                let clock = if paused {
                    &mut run.paused_remaining
                } else {
                    &mut run.agenda_deadline
                };
                let deadline = clock.as_mut().ok_or("時間を設定した議題がありません")?;
                let previous = *deadline;
                *deadline += 60_000;
                rearm(
                    &mut self.consumed,
                    "agenda",
                    &mut run.agenda_revision,
                    previous,
                    *deadline,
                    if paused { 0 } else { at },
                    &[60, 0],
                );
                run.agenda_runs.last_mut().unwrap().extended_ms += 60_000;
            }
            Action::End if self.active() => {
                let run = self.run.as_mut().unwrap();
                close_break(run, at);
                if run.agenda_index.is_some() {
                    run.agenda_runs.last_mut().unwrap().end = Some(at);
                }
                run.end = Some(at);
                self.shared = false;
                self.phase = Phase::Ended;
            }
            _ => return Err("現在の状態ではこの操作を実行できません".into()),
        }
        self.revision += 1;
        Ok(())
    }
}

fn close_break(run: &mut Run, at: i64) {
    if let Some(start) = run.break_start.take() {
        if run.agenda_index.is_some() {
            run.agenda_runs.last_mut().unwrap().paused_ms += (at - start).max(0);
        }
    }
}

fn rearm(
    consumed: &mut BTreeSet<String>,
    scope: &str,
    revision: &mut u64,
    old: i64,
    new: i64,
    at: i64,
    thresholds: &[i64],
) {
    let previous = *revision;
    *revision += 1;
    for threshold in thresholds {
        let offset = threshold * 1000;
        if new - offset <= at
            && (old - offset <= at || consumed.contains(&format!("{scope}:{previous}:{threshold}")))
        {
            consumed.insert(format!("{scope}:{revision}:{threshold}"));
        }
    }
}

const MAX_SPAN: i64 = 366 * 86_400_000;

fn validate(plan: &Plan) -> Result<(), String> {
    if plan.title.trim().is_empty()
        || plan.start < 0
        || plan.end <= plan.start
        || plan.end.saturating_sub(plan.start) > MAX_SPAN
        || plan.end > 253_402_300_799_999
        || plan
            .agenda
            .iter()
            .any(|a| a.title.trim().is_empty() || a.minutes.is_some_and(|m| m == 0 || m > 1440))
    {
        return Err("件名・予定時刻・議題を確認してください".into());
    }
    if let Some(url) = &plan.join_url {
        let url = tauri::Url::parse(url).map_err(|_| "参加 URL を確認してください")?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("参加 URL は HTTP または HTTPS を指定してください".into());
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    Edit { plan: Plan },
    Start { late_end: Option<i64> },
    Cancel,
    Share { enabled: bool },
    Pause,
    Resume,
    ExtendMeeting,
    ExtendAgenda,
    Next,
    End,
}

pub struct Meetings {
    root: PathBuf,
    data: Mutex<Data>,
    wake: tokio::sync::Notify,
    clock: Mutex<(Instant, i64)>,
    focus: Mutex<Option<String>>,
}
#[derive(Default)]
struct Data {
    items: Vec<Meeting>,
    damaged: usize,
    unreadable: bool,
}

impl Meetings {
    pub fn load(root: PathBuf) -> Self {
        let mut data = Data::default();
        match std::fs::read_dir(&root) {
            Ok(entries) => {
                for entry in entries {
                    let Ok(entry) = entry else {
                        data.damaged += 1;
                        continue;
                    };
                    if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                        continue;
                    }
                    if let Some(meeting) = std::fs::read(entry.path().join("meeting.json"))
                        .ok()
                        .and_then(|b| serde_json::from_slice::<Meeting>(&b).ok())
                        .filter(|m| {
                            m.schema_version == 1
                                && valid_id(&m.id)
                                && entry.file_name() == std::ffi::OsStr::new(&m.id)
                                && validate(&m.plan).is_ok()
                                && (!m.active()
                                    || m.run.as_ref().is_some_and(|run| {
                                        run.valid()
                                            && run.agenda_index.is_none_or(|i| {
                                                i < m.plan.agenda.len()
                                                    && !run.agenda_runs.is_empty()
                                            })
                                    }))
                        })
                    {
                        data.items.push(meeting);
                    } else {
                        data.damaged += 1;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => data.unreadable = true,
        }
        Self {
            root,
            data: Mutex::new(data),
            wake: tokio::sync::Notify::new(),
            clock: Mutex::new((Instant::now(), now())),
            focus: Mutex::new(None),
        }
    }

    fn save(&self, meeting: &Meeting) -> Result<(), String> {
        let dir = self.root.join(&meeting.id);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_vec_pretty(meeting).map_err(|e| e.to_string())?;
        crate::store::atomic::write_atomic(&dir.join("meeting.json"), &json)
            .map_err(|e| e.to_string())
    }

    fn running_now(&self) -> i64 {
        let (anchor, utc) = *self.clock.lock().unwrap();
        utc + anchor.elapsed().as_millis() as i64
    }

    pub fn system_resumed(&self) {
        *self.clock.lock().unwrap() = (Instant::now(), now());
        self.resume();
    }

    pub fn resume(&self) {
        self.wake.notify_one();
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

#[derive(Serialize)]
pub struct Snapshot {
    items: Vec<Meeting>,
    damaged: usize,
    unreadable: bool,
    shared: bool,
    at: i64,
    clock_changed: bool,
    focus: Option<String>,
}

#[tauri::command]
pub fn meeting_list(window: tauri::WebviewWindow, store: State<'_, Meetings>) -> Snapshot {
    let data = store.data.lock().unwrap();
    let mut items = data.items.clone();
    items.sort_by_key(|m| (m.plan.start, m.id.clone()));
    let shared = !may_disclose(&items);
    if shared {
        items.retain(Meeting::active);
        for item in &mut items {
            item.plan.title.clear();
            item.plan.join_url = None;
            item.plan.location = None;
            item.plan.description = None;
            item.ics_uid = None;
            for agenda in &mut item.plan.agenda {
                agenda.title.clear();
            }
        }
    }
    let at = store.running_now();
    let clock_changed = items.iter().any(Meeting::active) && (now() - at).abs() > 5_000;
    Snapshot {
        at,
        clock_changed,
        focus: if shared || window.label() != "meeting" {
            None
        } else {
            store.focus.lock().unwrap().take()
        },
        shared,
        items,
        damaged: data.damaged,
        unreadable: data.unreadable,
    }
}

#[tauri::command]
pub fn meeting_create(store: State<'_, Meetings>, id: String, plan: Plan) -> Result<(), String> {
    validate(&plan)?;
    let mut data = store.data.lock().unwrap();
    if data.unreadable || !valid_id(&id) || store.root.join(&id).exists() {
        return Err("会議の保存先を確保できません".into());
    }
    let meeting = Meeting {
        schema_version: 1,
        id,
        revision: 0,
        plan,
        phase: Phase::Prepared,
        source: Source::Manual,
        ics_uid: None,
        run: None,
        shared: false,
        consumed: BTreeSet::new(),
    };
    store.save(&meeting)?;
    data.items.push(meeting);
    store.resume();
    Ok(())
}

#[derive(Deserialize)]
pub struct IcsEntry {
    id: String,
    uid: String,
    plan: Plan,
}

#[derive(Default, Serialize)]
pub struct ImportResult {
    added: usize,
    duplicates: usize,
    errors: Vec<String>,
}

impl Meetings {
    fn import(&self, entries: Vec<IcsEntry>) -> Result<ImportResult, String> {
        let mut data = self.data.lock().unwrap();
        if !may_disclose(&data.items) || data.unreadable {
            return Err("現在は取り込めません。共有モードと保存領域を確認してください。".into());
        }
        let mut result = ImportResult::default();
        for (index, entry) in entries.into_iter().enumerate() {
            if data
                .items
                .iter()
                .any(|m| m.ics_uid.as_ref() == Some(&entry.uid))
            {
                result.duplicates += 1;
                continue;
            }
            if entry.uid.trim().is_empty()
                || !valid_id(&entry.id)
                || self.root.join(&entry.id).exists()
            {
                result
                    .errors
                    .push(format!("予定 {} の識別子を確認できません。", index + 1));
                continue;
            }
            if let Err(error) = validate(&entry.plan) {
                result.errors.push(format!("予定 {} · {error}", index + 1));
                continue;
            }
            let meeting = Meeting {
                schema_version: 1,
                id: entry.id,
                revision: 0,
                plan: entry.plan,
                phase: Phase::Prepared,
                source: Source::Ics,
                ics_uid: Some(entry.uid),
                run: None,
                shared: false,
                consumed: BTreeSet::new(),
            };
            match self.save(&meeting) {
                Ok(()) => {
                    data.items.push(meeting);
                    result.added += 1;
                }
                Err(error) => result.errors.push(format!("予定 {} · {error}", index + 1)),
            }
        }
        self.resume();
        Ok(result)
    }
}

#[tauri::command]
pub fn meeting_import(
    store: State<'_, Meetings>,
    entries: Vec<IcsEntry>,
) -> Result<ImportResult, String> {
    store.import(entries)
}

#[tauri::command]
pub fn meeting_update(
    app: tauri::AppHandle,
    store: State<'_, Meetings>,
    id: String,
    revision: u64,
    action: Action,
) -> Result<(), String> {
    let mut data = store.data.lock().unwrap();
    let at = if matches!(&action, Action::Start { .. }) {
        if !data.items.iter().any(Meeting::active) {
            *store.clock.lock().unwrap() = (Instant::now(), now());
        }
        now()
    } else {
        store.running_now()
    };
    if matches!(&action, Action::Start { .. }) && data.items.iter().any(Meeting::active) {
        return Err("進行中の会議を終了してから開始してください".into());
    }
    let index = data
        .items
        .iter()
        .position(|m| m.id == id)
        .ok_or("会議が見つかりません")?;
    let meeting = &data.items[index];
    if meeting.revision != revision {
        return Err("会議が更新されました。表示を確認して操作し直してください".into());
    }
    let mut next = meeting.clone();
    let hide = matches!(&action, Action::Share { enabled: true });
    if matches!(&action, Action::Share { enabled: false }) && meeting.shared {
        // Suppressed reminders for all prepared meetings also expire before disclosure resumes.
        let wall = now();
        for other in data.items.iter_mut().filter(|m| m.phase == Phase::Prepared) {
            let mut updated = other.clone();
            updated.consumed.extend(
                other
                    .thresholds()
                    .into_iter()
                    .filter(|(deadline, _)| *deadline <= wall)
                    .map(|(_, key)| key),
            );
            if updated.consumed != other.consumed {
                store.save(&updated)?;
                *other = updated;
            }
        }
        next.consumed.extend(
            next.thresholds()
                .into_iter()
                .filter(|(deadline, _)| *deadline <= at)
                .map(|(_, key)| key),
        );
    }
    if matches!(&action, Action::Pause) {
        next.consumed.extend(
            next.thresholds()
                .into_iter()
                .filter(|(deadline, key)| *deadline <= at && key.starts_with("agenda:"))
                .map(|(_, key)| key),
        );
    }
    next.advance(action, at)?;
    store.save(&next)?;
    data.items[index] = next;
    drop(data);
    store.resume();
    let _ = app.emit("meeting-chat-privacy", files::chat_protected(&store, None));
    if hide {
        if let Some(window) = app.get_webview_window("meeting") {
            window.hide().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn meeting_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("meeting") {
        window
            .show()
            .and_then(|_| window.set_focus())
            .map_err(|e| e.to_string())?;
    } else {
        tauri::WebviewWindowBuilder::new(
            &app,
            "meeting",
            tauri::WebviewUrl::App("index.html?view=meeting".into()),
        )
        .title("AgentDock 会議")
        .inner_size(640.0, 720.0)
        .build()
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl Meeting {
    fn thresholds(&self) -> Vec<(i64, String)> {
        let mut due = Vec::new();
        let mut add = |scope: &str, deadline: i64, revision: u64, offsets: &[i64]| {
            due.extend(offsets.iter().map(|offset| {
                (
                    deadline - offset * 1000,
                    format!("{scope}:{revision}:{offset}"),
                )
            }));
        };
        match (self.phase, &self.run) {
            (Phase::Prepared, _) => add("start", self.plan.start, self.revision, &[300, 0]),
            (Phase::Running | Phase::Break, Some(run)) => {
                add("meeting", run.deadline, run.meeting_revision, &[300, 60, 0]);
                if let Some(deadline) = run.agenda_deadline.filter(|_| self.phase == Phase::Running)
                {
                    add("agenda", deadline, run.agenda_revision, &[60, 0]);
                }
            }
            _ => {}
        }
        due
    }
}

// The window and the external delivery use the same disclosure decision.
fn may_disclose(items: &[Meeting]) -> bool {
    !items
        .iter()
        .any(|meeting| meeting.shared && meeting.active())
}

impl Meetings {
    fn reconcile(&self, at: i64, deliver: impl FnOnce(Option<String>)) -> std::time::Duration {
        let mut data = self.data.lock().unwrap();
        let visible = may_disclose(&data.items);
        let mut targets = Vec::new();
        let mut delay = 30_000;
        for meeting in &mut data.items {
            let mut next = meeting.clone();
            let at = if meeting.active() {
                self.running_now()
            } else {
                at
            };
            let mut practical = false;
            for (deadline, key) in meeting.thresholds() {
                if next.consumed.contains(&key) {
                    continue;
                }
                if deadline > at {
                    delay = delay.min(deadline - at);
                    continue;
                }
                next.consumed.insert(key);
                practical |= if meeting.active() {
                    // A newly started meeting discards warnings predating its start.
                    let run = meeting.run.as_ref().unwrap();
                    deadline >= run.start
                        || deadline == run.deadline
                        || Some(deadline) == run.agenda_deadline
                } else {
                    at <= meeting.plan.start + 300_000
                        && (deadline == meeting.plan.start || at < meeting.plan.start)
                };
            }
            if next.consumed != meeting.consumed {
                match self.save(&next) {
                    Ok(()) => {
                        *meeting = next;
                        if practical {
                            targets.push(meeting.id.clone());
                        }
                    }
                    Err(_) => {
                        crate::diag::log("meeting", "notification keys could not be saved");
                        delay = delay.min(5_000);
                    }
                }
            }
        }
        if !targets.is_empty() && visible {
            deliver(if targets.len() == 1 {
                targets.pop()
            } else {
                None
            });
        }
        std::time::Duration::from_millis(delay as u64)
    }
}

pub fn start_scheduler(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let store = app.state::<Meetings>();
        loop {
            let handle = app.clone();
            let delay = store.reconcile(now(), move |target| {
                let notice = crate::rules::notify::OsNotification {
                    kind: crate::rules::notify::NotificationKind::Completed,
                    title: "AgentDock 会議".into(),
                    body: "会議の時刻を確認してください。".into(),
                    chat: None,
                    sound: false,
                };
                crate::win::toast::show(&notice, move |_| {
                    let app = handle.clone();
                    let target = target.clone();
                    tauri::async_runtime::spawn(async move {
                        let store = app.state::<Meetings>();
                        if may_disclose(&store.data.lock().unwrap().items) {
                            *store.focus.lock().unwrap() = target;
                            let _ = meeting_window(app.clone()).await;
                        }
                    });
                });
            });
            tokio::select! {
                _ = store.wake.notified() => {},
                _ = tokio::time::sleep(delay) => {},
            }
        }
    });
}

#[tauri::command]
pub fn meeting_top(window: tauri::WebviewWindow, enabled: bool) -> Result<(), String> {
    if window.label() != "meeting" {
        return Err("会議窓から操作してください".into());
    }
    window.set_always_on_top(enabled).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn meeting_join(
    app: tauri::AppHandle,
    store: State<'_, Meetings>,
    id: String,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let data = store.data.lock().unwrap();
    if !may_disclose(&data.items) {
        return Err("共有モードを解除してください".into());
    }
    let url = data
        .items
        .iter()
        .find(|m| m.id == id)
        .and_then(|m| m.plan.join_url.as_ref())
        .ok_or("参加 URL がありません")?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|_| "参加 URL を開けませんでした".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(at: i64) -> Meeting {
        Meeting {
            schema_version: 1,
            id: "1253d97a-fd99-4426-9f9c-b38d201b56be".into(),
            revision: 0,
            plan: Plan {
                title: "Sample".into(),
                join_url: None,
                location: None,
                description: None,
                start: at,
                end: at + 600_000,
                time_zone: "UTC".into(),
                agenda: vec![Agenda {
                    title: "First".into(),
                    minutes: Some(2),
                }],
            },
            phase: Phase::Prepared,
            source: Source::Manual,
            ics_uid: None,
            run: None,
            shared: false,
            consumed: BTreeSet::new(),
        }
    }

    #[test]
    fn ics_uid_is_deduplicated_in_batch_and_after_reload() {
        let root =
            std::env::temp_dir().join(format!("agentdock-ics-{}-{}", std::process::id(), now()));
        let entry = |id: &str| IcsEntry {
            id: id.into(),
            uid: "calendar-event".into(),
            plan: prepared(now()).plan,
        };
        let one = "1253d97a-fd99-4426-9f9c-b38d201b56be";
        let two = "1253d97a-fd99-4426-9f9c-b38d201b56bf";
        let store = Meetings::load(root.clone());
        let result = store.import(vec![entry(one), entry(two)]).unwrap();
        assert_eq!((result.added, result.duplicates), (1, 1));
        let reloaded = Meetings::load(root.clone());
        let result = reloaded.import(vec![entry(two)]).unwrap();
        assert_eq!((result.added, result.duplicates), (0, 1));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn break_stops_agenda_only_and_final_agenda_keeps_meeting_running() {
        let at = 1_000_000;
        let mut meeting = prepared(at);
        meeting
            .advance(Action::Start { late_end: None }, at)
            .unwrap();
        meeting.advance(Action::Pause, at + 60_000).unwrap();
        assert_eq!(meeting.run.as_ref().unwrap().paused_remaining, Some(60_000));
        assert_eq!(meeting.thresholds().len(), 3);
        meeting.advance(Action::ExtendAgenda, at + 100_000).unwrap();
        meeting.advance(Action::Resume, at + 180_000).unwrap();
        let run = meeting.run.as_ref().unwrap();
        assert_eq!(run.deadline, at + 600_000);
        assert_eq!(run.agenda_deadline, Some(at + 300_000));
        assert_eq!(run.agenda_runs[0].paused_ms, 120_000);
        meeting.advance(Action::Next, at + 200_000).unwrap();
        assert_eq!(meeting.phase, Phase::Running);
        assert!(meeting.run.as_ref().unwrap().agenda_runs[0].completed);
        assert!(meeting.run.as_ref().unwrap().agenda_index.is_none());
    }

    #[test]
    fn notification_consumption_survives_reload_and_shared_mode() {
        let root = crate::test_support::unique_temp_dir("meeting-notify");
        let at = now();
        let store = Meetings::load(root.clone());
        let mut meeting = prepared(at);
        store.save(&meeting).unwrap();
        store.data.lock().unwrap().items.push(meeting.clone());
        let mut deliveries = 0;
        store.reconcile(at, |_| deliveries += 1);
        let restored = Meetings::load(root.clone());
        restored.reconcile(at, |_| deliveries += 1);
        assert_eq!(deliveries, 1);

        meeting
            .advance(Action::Start { late_end: None }, at)
            .unwrap();
        meeting
            .advance(Action::Share { enabled: true }, at)
            .unwrap();
        store.save(&meeting).unwrap();
        let restored = Meetings::load(root.clone());
        *restored.clock.lock().unwrap() = (Instant::now(), at + 600_000);
        restored.reconcile(at + 600_000, |_| deliveries += 1);
        assert_eq!(deliveries, 1);
        let data = restored.data.lock().unwrap();
        assert!(data.items[0].consumed.contains("meeting:0:0"));
        assert!(!may_disclose(&data.items));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extending_past_deadline_does_not_replay_elapsed_thresholds() {
        let at = 1_000_000;
        let mut consumed = BTreeSet::new();
        let mut revision = 0;
        rearm(
            &mut consumed,
            "meeting",
            &mut revision,
            at - 120_000,
            at + 180_000,
            at,
            &[300, 60, 0],
        );
        assert!(consumed.contains("meeting:1:300"));
        assert!(!consumed.contains("meeting:1:60"));
        assert!(!consumed.contains("meeting:1:0"));
    }
}
