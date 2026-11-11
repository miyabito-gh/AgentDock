//! 直接編集方式のターン記録。元のファイルを直接編集させ、依頼の直前のスナップショットからターンごとに戻す。
//! 保存先 `<store>/turns.json`（原子的に更新）と `<store>/t-NNNN/{before,after}/<rel>`。ホストの状態は持たない
//! （会話が実行中かどうかは、呼び出し側が `running` 述語で渡す）。
use super::{
    load_snapshot, open_checked, pin_directories, root_contains, save_snapshot,
    FileAccess, FileScope, ProjectRoot, RelativePath, SelectedFile, Snapshot, VisualError, MAX_FILE_BYTES,
};
use crate::backend::model::ChatKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// 保持するターン数（超過分は古い順に削除）。
pub const KEEP_TURNS: usize = 30;
/// 保存したスナップショットの合計がこれを超えたら、古いターンから削除する（最新の1ターンは残す）。
pub const MAX_STORE_BYTES: u64 = 400 * 1024 * 1024;

#[derive(Debug)]
pub enum DirectError {
    /// 現在のファイルが期待する内容と違う（パス一覧）。
    Conflict(Vec<String>),
    /// 戻す・やり直す対象が無い。
    Nothing(String),
    /// 会話が実行中（またはまだ終わっていない編集）のため拒否。
    Running(String),
    /// 保存したスナップショットが壊れている・失われている。
    Damaged(String),
    Other(String),
}
impl std::fmt::Display for DirectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict(paths) => write!(f, "conflict:\n{}", paths.join("\n")),
            Self::Nothing(m) => write!(f, "nothing: {m}"),
            Self::Running(m) => write!(f, "running: {m}"),
            Self::Damaged(m) | Self::Other(m) => write!(f, "{m}"),
        }
    }
}
/// 利用者向けの日本語（理由と次の行動）。
pub fn jp_io(e: &std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match e.kind() {
        PermissionDenied => "ファイルに書き込めません。読み取り専用になっているか、他のアプリが使用中の可能性があります。属性を確認するか、使用中のアプリを閉じてからやり直してください。".into(),
        NotFound => "ファイルまたはフォルダが見つかりません。移動・削除されていないか確認してください。".into(),
        _ => format!("ファイル操作に失敗しました（{e}）。他のアプリがファイルを開いていないか確認して、もう一度お試しください。"),
    }
}
pub fn jp_visual(e: &VisualError) -> String {
    match e {
        VisualError::Limit("discovered file count") => format!("編集対象のファイルが多すぎます（最大{}件）。もっと小さいフォルダを選んでください。", super::MAX_FILES),
        VisualError::Limit("single file bytes") => "8MBを超えるファイルがあるため扱えません。大きなファイルを編集対象のフォルダの外へ移してください。".into(),
        VisualError::Limit("total bytes") => "編集対象のファイルの合計が大きすぎます（64MB超）。もっと小さいフォルダを選んでください。".into(),
        VisualError::Limit(_) => "編集対象の大きさが上限を超えています。もっと小さいフォルダを選んでください。".into(),
        VisualError::Conflict("link in discovered scope") => "フォルダの中にリンク（ショートカット）があるため扱えません。リンクを取り除くか、リンクのないフォルダを選んでください。".into(),
        VisualError::Conflict("scope path escaped root") => "選んだフォルダの外を指すファイルがあるため扱えません。フォルダの構成を確認してください。".into(),
        VisualError::Conflict("selected HTML is not an editable file") => "選んだ HTML ファイルが編集対象として見つかりません。ファイルの場所と名前を確認してください。".into(),
        VisualError::Conflict("changed during read") | VisualError::Conflict("file appeared/disappeared during capture") => "読み取り中にファイルが変更されました。少し待ってから、もう一度お試しください。".into(),
        VisualError::Conflict("snapshot bytes do not match manifest") => damaged_text(),
        VisualError::Conflict(_) => "ファイルの状態が想定と違うため続けられません。画面を更新して、もう一度お試しください。".into(),
        VisualError::InvalidPath(p) => format!("編集対象に使えないファイル名があります: {p}"),
        VisualError::UnsafePath(p) => format!("リンクまたはフォルダの外を指すパスのため扱えません: {}", p.display()),
        VisualError::Io(io) => jp_io(io),
        VisualError::Json(_) => "保存された記録の形式が壊れています。".into(),
    }
}
fn damaged_text() -> String {
    "この編集の保存内容が壊れているか失われているため、戻す・差分表示ができません。".into()
}
impl From<VisualError> for DirectError {
    fn from(e: VisualError) -> Self {
        Self::Other(jp_visual(&e))
    }
}
impl From<std::io::Error> for DirectError {
    fn from(e: std::io::Error) -> Self {
        Self::Other(jp_io(&e))
    }
}
impl From<serde_json::Error> for DirectError {
    fn from(_: serde_json::Error) -> Self {
        Self::Other("保存された記録の形式が壊れています。".into())
    }
}
pub type DResult<T> = Result<T, DirectError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnState {
    Open,
    Done,
    Undone,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Change {
    Add,
    Change,
    Delete,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectFile {
    pub path: String,
    pub change: Change,
}
/// turns.json の1件。`scope` は開始時の対象、`after_scope` は終了時の対象（開始時＋その後に増えたファイル。終了前は None）。
/// `chat` は begin 時の会話（無ければ旧ターン）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnRecord {
    pub id: String,
    pub seq: u32,
    pub at_ms: u64,
    pub state: TurnState,
    pub files: Vec<DirectFile>,
    pub scope: Vec<SelectedFile>,
    #[serde(default)]
    pub after_scope: Option<Vec<SelectedFile>>,
    #[serde(default)]
    pub chat: Option<ChatKey>,
}
/// turns.json 本体。`next_seq` は単調増加（削除されたターンの番号を再利用しない）。旧形式（配列）も読める。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnsFile {
    #[serde(default)]
    next_seq: u32,
    turns: Vec<TurnRecord>,
}
/// 画面へ返すターン（保存用の対象一覧は含めない）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnView {
    pub id: String,
    pub seq: u32,
    pub at_ms: u64,
    pub state: TurnState,
    pub files: Vec<DirectFile>,
    pub can_undo: bool,
    pub can_redo: bool,
}
/// 差分の両側（バイト列。無いファイルは None）。
pub struct DiffSides {
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}

/// ルートと HTML の相対パスから保存先の名前を作る。
pub fn store_key(root: &Path, html: &RelativePath) -> String {
    let text = format!("{}\n{}", root.to_string_lossy().to_uppercase(), html.key());
    format!("{:x}", Sha256::digest(text.as_bytes()))[..32].to_string()
}

pub fn views(records: &[TurnRecord]) -> Vec<TurnView> {
    let latest = records.iter().filter(|r| r.state != TurnState::Undone).map(|r| r.seq).max();
    let redo = records.iter().filter(|r| r.state == TurnState::Undone).map(|r| r.seq).min();
    records
        .iter()
        .map(|r| TurnView {
            id: r.id.clone(),
            seq: r.seq,
            at_ms: r.at_ms,
            state: r.state,
            files: r.files.clone(),
            can_undo: r.state != TurnState::Undone && Some(r.seq) == latest,
            can_redo: r.state == TurnState::Undone && Some(r.seq) == redo,
        })
        .collect()
}

fn same_key(a: &str, b: &str) -> bool {
    a.to_uppercase() == b.to_uppercase()
}
/// 開始時の対象に、いまの探索結果で増えたファイル（AI が作った関連ファイル）を足す。作れなければ開始時の対象のまま。
fn union_scope(before: &[SelectedFile], fresh: Option<&FileScope>) -> DResult<FileScope> {
    let mut all = before.to_vec();
    if let Some(fresh) = fresh {
        for f in fresh.selected() {
            if !all.iter().any(|b| same_key(&b.path, &f.path)) {
                all.push(f);
            }
        }
    }
    match FileScope::new(all) {
        Ok(s) => Ok(s),
        Err(_) => Ok(FileScope::new(before.to_vec())?),
    }
}
fn dir_bytes(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

pub struct DirectStore {
    dir: PathBuf,
    max_bytes: u64,
}
impl DirectStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, max_bytes: MAX_STORE_BYTES }
    }
    /// フォルダに残っている t-NNNN の最大+1（記録が失われたときも番号を再利用しない）。
    fn dir_next_seq(&self) -> u32 {
        let max = fs::read_dir(&self.dir)
            .map(|rd| rd.flatten().filter_map(|e| e.file_name().to_string_lossy().strip_prefix("t-").and_then(|n| n.parse::<u32>().ok())).max().unwrap_or(0))
            .unwrap_or(0);
        max + 1
    }
    /// turns.json を読む。壊れていれば `.bad` へ改名退避して新規に始める（利用者を止めない。記録は diag に残す）。
    fn load_file(&self) -> DResult<TurnsFile> {
        let path = self.dir.join("turns.json");
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(TurnsFile { next_seq: self.dir_next_seq(), turns: Vec::new() }),
            Err(e) => return Err(e.into()),
        };
        let parsed = serde_json::from_slice::<TurnsFile>(&bytes).or_else(|_| serde_json::from_slice::<Vec<TurnRecord>>(&bytes).map(|turns| TurnsFile { next_seq: 0, turns }));
        match parsed {
            Ok(mut file) => {
                file.turns.sort_by_key(|r| r.seq);
                file.next_seq = file.next_seq.max(file.turns.iter().map(|r| r.seq).max().unwrap_or(0) + 1).max(1);
                Ok(file)
            }
            Err(e) => {
                let bad = self.dir.join("turns.json.bad");
                let _ = fs::remove_file(&bad);
                match fs::rename(&path, &bad) {
                    Ok(()) => crate::diag::log("visual-direct", &format!("turns.json was unreadable ({e}); moved it to turns.json.bad and started a new record")),
                    Err(re) => return Err(DirectError::Other(format!("編集の記録（turns.json）が壊れていて、退避もできませんでした。保存フォルダ {} の turns.json を手動で削除してください。（{re}）", self.dir.display()))),
                }
                Ok(TurnsFile { next_seq: self.dir_next_seq(), turns: Vec::new() })
            }
        }
    }
    pub fn load(&self) -> DResult<Vec<TurnRecord>> {
        Ok(self.load_file()?.turns)
    }
    fn save_with(&self, records: &[TurnRecord], next_seq: u32) -> DResult<()> {
        fs::create_dir_all(&self.dir)?;
        let next_seq = next_seq.max(records.iter().map(|r| r.seq).max().unwrap_or(0) + 1);
        let file = TurnsFile { next_seq, turns: records.to_vec() };
        crate::store::atomic::write_atomic(&self.dir.join("turns.json"), &serde_json::to_vec_pretty(&file)?)?;
        Ok(())
    }
    /// 番号の払い出しは据え置く（現在の next_seq 以上を保つ）。
    fn save(&self, records: &[TurnRecord]) -> DResult<()> {
        let next = self.load_file().map(|f| f.next_seq).unwrap_or(1);
        self.save_with(records, next)
    }
    fn turn_dir(&self, id: &str) -> DResult<PathBuf> {
        let ok = id.len() > 2 && id.starts_with("t-") && id[2..].chars().all(|c| c.is_ascii_digit());
        if !ok {
            return Err(DirectError::Other(format!("編集の識別子が正しくありません: {id}")));
        }
        Ok(self.dir.join(id))
    }
    /// 保存先の中の、リンクでないターンのフォルダだけを削除する。無ければ何もしない。
    fn remove_turn_dir(&self, id: &str) -> DResult<()> {
        let dir = self.turn_dir(id)?;
        match fs::symlink_metadata(&dir) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => Ok(fs::remove_dir_all(&dir)?),
            Ok(_) => Err(DirectError::Other("保存フォルダの中身が想定と違います（編集の記録フォルダがリンクまたはファイルです）。保存フォルダを確認してください。".into())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    fn view_of(records: &[TurnRecord], id: &str) -> Option<TurnView> {
        views(records).into_iter().find(|v| v.id == id)
    }
    /// 確定が必要になりうる会話（open のターンの会話と、指定ターンの会話）。呼び出し側が実行中かを調べて `running` に渡す。
    pub fn chats_to_check(&self, turn_id: Option<&str>) -> DResult<Vec<ChatKey>> {
        let mut out: Vec<ChatKey> = Vec::new();
        for r in self.load()? {
            if let Some(chat) = &r.chat {
                if (r.state == TurnState::Open || Some(r.id.as_str()) == turn_id) && !out.contains(chat) {
                    out.push(chat.clone());
                }
            }
        }
        Ok(out)
    }

    /// 記録とフォルダを捨てる（確定できない・壊れたターン）。
    fn drop_turn(&self, records: &mut Vec<TurnRecord>, id: &str) -> DResult<()> {
        records.retain(|r| r.id != id);
        self.save(records)?;
        self.remove_turn_dir(id)
    }
    /// 保存量が上限を超えたら、古いターンから削除する。最新の1ターンと open のターンは残す。外した id を返す。
    fn enforce_size_cap(&self, records: &mut Vec<TurnRecord>) -> Vec<String> {
        let mut sizes: Vec<u64> = records.iter().map(|r| dir_bytes(&self.dir.join(&r.id))).collect();
        let mut total: u64 = sizes.iter().sum();
        let newest = records.iter().map(|r| r.seq).max();
        let mut removed = Vec::new();
        while total > self.max_bytes {
            let Some(i) = records.iter().position(|r| Some(r.seq) != newest && r.state != TurnState::Open) else { break };
            total -= sizes.remove(i);
            removed.push(records.remove(i).id);
        }
        removed
    }

    /// 終了時の内容を after として確定する。変更が無ければ false（呼び出し側が記録を消す）。開始前の保存が壊れていれば Damaged。
    fn finalize(&self, root: &ProjectRoot, rec: &mut TurnRecord, fresh: Option<&FileScope>) -> DResult<bool> {
        let turn = self.turn_dir(&rec.id)?;
        let before = load_snapshot(&turn.join("before"), &FileScope::new(rec.scope.clone())?).map_err(|_| DirectError::Damaged(format!("{}（編集前の保存内容が壊れているか失われています）", damaged_text())))?;
        let union = union_scope(&rec.scope, fresh)?;
        let after = Snapshot::capture(root, &union)?;
        let mut files = Vec::new();
        for (p, _) in union.files() {
            let change = match (before.bytes(p.as_str()), after.bytes(p.as_str())) {
                (None, None) => continue,
                (None, Some(_)) => Change::Add,
                (Some(_), None) => Change::Delete,
                (Some(b), Some(a)) if b == a => continue,
                (Some(_), Some(_)) => Change::Change,
            };
            files.push(DirectFile { path: p.as_str().into(), change });
        }
        if files.is_empty() {
            return Ok(false);
        }
        let after_dir = turn.join("after");
        if fs::symlink_metadata(&after_dir).is_ok() {
            fs::remove_dir_all(&after_dir)?;
        }
        save_snapshot(&after_dir, &after)?;
        rec.files = files;
        rec.after_scope = Some(union.selected());
        rec.state = TurnState::Done;
        Ok(true)
    }

    /// `open` のまま残ったターンを、いまの内容で確定する（変更が無ければ削除）。会話が実行中のターンは確定しない。
    /// 会話の記録があるターンは、その会話が実行中でないときだけ確定する。記録の無い旧ターンは最新の1件だけ。
    /// 確定に失敗しても全体は止めない: 保存内容が壊れたターンは破棄し、それ以外の失敗は open のまま残して記録する。
    pub fn recover_open(&self, root: &ProjectRoot, fresh: Option<&FileScope>, running: &dyn Fn(&ChatKey) -> bool) -> DResult<Vec<TurnRecord>> {
        let mut records = self.load()?;
        let latest_unbound = records.iter().filter(|r| r.state == TurnState::Open && r.chat.is_none()).map(|r| r.seq).max();
        let targets: Vec<String> = records
            .iter()
            .filter(|r| r.state == TurnState::Open)
            .filter(|r| match &r.chat {
                Some(chat) => !running(chat),
                None => Some(r.seq) == latest_unbound,
            })
            .map(|r| r.id.clone())
            .collect();
        for id in targets {
            let Some(i) = records.iter().position(|r| r.id == id) else { continue };
            let mut rec = records[i].clone();
            match self.finalize(root, &mut rec, fresh) {
                Ok(true) => {
                    records[i] = rec;
                    self.save(&records)?;
                }
                Ok(false) => self.drop_turn(&mut records, &id)?,
                Err(DirectError::Damaged(m)) => {
                    crate::diag::log("visual-direct", &format!("discarded {id}: {m}"));
                    self.drop_turn(&mut records, &id)?;
                }
                Err(e) => crate::diag::log("visual-direct", &format!("could not settle {id}; it stays open: {e}")),
            }
        }
        Ok(records)
    }

    /// 依頼の直前に対象ファイルを保存して新しいターンを始める。失敗したらターンは残さない（呼び出し側は送らない）。
    pub fn begin(&self, root: &ProjectRoot, scope: &FileScope, now_ms: u64, chat: Option<&ChatKey>, running: &dyn Fn(&ChatKey) -> bool) -> DResult<TurnView> {
        fs::create_dir_all(&self.dir)?;
        let mut records = self.recover_open(root, Some(scope), running)?;
        let seq = self.load_file()?.next_seq;
        let id = format!("t-{seq:04}");
        self.remove_turn_dir(&id)?;
        let turn = self.turn_dir(&id)?;
        let made = (|| -> DResult<()> {
            fs::create_dir_all(&turn)?;
            let snapshot = Snapshot::capture(root, scope)?;
            save_snapshot(&turn.join("before"), &snapshot)?;
            Ok(())
        })();
        if let Err(e) = made {
            let _ = self.remove_turn_dir(&id);
            return Err(e);
        }
        // undone は新しい編集で捨てる。
        let mut dropped: Vec<String> = records.iter().filter(|r| r.state == TurnState::Undone).map(|r| r.id.clone()).collect();
        records.retain(|r| r.state != TurnState::Undone);
        records.push(TurnRecord { id: id.clone(), seq, at_ms: now_ms, state: TurnState::Open, files: Vec::new(), scope: scope.selected(), after_scope: None, chat: chat.cloned() });
        while records.len() > KEEP_TURNS {
            dropped.push(records.remove(0).id);
        }
        dropped.extend(self.enforce_size_cap(&mut records));
        if let Err(e) = self.save_with(&records, seq + 1) {
            let _ = self.remove_turn_dir(&id);
            return Err(e);
        }
        for old in dropped {
            // 記録からは外れているので、片付けの失敗は無視してよい（次回以降も参照されない）。
            let _ = self.remove_turn_dir(&old);
        }
        Ok(Self::view_of(&records, &id).expect("new turn is recorded"))
    }

    /// ターンを終える。変更が無ければ削除して None。すでに終えている・無いターンはそのまま返す（冪等。番号は再利用しない）。
    pub fn end(&self, root: &ProjectRoot, turn_id: &str, fresh: Option<&FileScope>) -> DResult<Option<TurnView>> {
        let mut records = self.load()?;
        let Some(i) = records.iter().position(|r| r.id == turn_id) else { return Ok(None) };
        if records[i].state != TurnState::Open {
            return Ok(Self::view_of(&records, turn_id));
        }
        let mut rec = records[i].clone();
        match self.finalize(root, &mut rec, fresh) {
            Ok(true) => {
                records[i] = rec;
                let removed = self.enforce_size_cap(&mut records);
                self.save(&records)?;
                for old in removed {
                    let _ = self.remove_turn_dir(&old);
                }
                Ok(Self::view_of(&records, turn_id))
            }
            Ok(false) => {
                self.drop_turn(&mut records, turn_id)?;
                Ok(None)
            }
            Err(DirectError::Damaged(m)) => {
                self.drop_turn(&mut records, turn_id)?;
                Err(DirectError::Damaged(m))
            }
            Err(e) => Err(e),
        }
    }

    pub fn turns(&self) -> DResult<Vec<TurnView>> {
        Ok(views(&self.load()?))
    }

    /// 対象ターンの会話（記録が無い旧ターンは渡された会話）が実行中なら拒否する。会話が分からない open のターンも、止まっていると言えないので拒否する。
    /// 「停止を確認できない」だけなら実行中と断定できないので拒否しない（呼び出し側の述語は、実行中と分かるときだけ true）。
    fn check_not_running(rec: &TurnRecord, fallback: Option<&ChatKey>, running: &dyn Fn(&ChatKey) -> bool) -> DResult<()> {
        match rec.chat.as_ref().or(fallback) {
            Some(chat) if running(chat) => Err(DirectError::Running("この編集をした会話が実行中です。停止してから、もう一度お試しください。".into())),
            None if rec.state == TurnState::Open => Err(DirectError::Running("この編集はまだ終わっておらず、会話が止まっているか確認できません。会話が止まってから、もう一度お試しください。".into())),
            _ => Ok(()),
        }
    }

    /// 最新の done（または open）のターンを開始前の内容へ戻す。
    pub fn undo(&self, root: &ProjectRoot, turn_id: &str, force: bool, fresh: Option<&FileScope>, chat: Option<&ChatKey>, running: &dyn Fn(&ChatKey) -> bool) -> DResult<Vec<TurnView>> {
        let mut records = self.load()?;
        let latest = records.iter().enumerate().filter(|(_, r)| r.state != TurnState::Undone).max_by_key(|(_, r)| r.seq).map(|(i, _)| i);
        let Some(i) = latest else { return Err(DirectError::Nothing("戻せる変更がありません。".into())) };
        if records[i].id != turn_id {
            return Err(DirectError::Nothing("戻せるのは最新の編集だけです。画面を更新して、もう一度お試しください。".into()));
        }
        Self::check_not_running(&records[i], chat, running)?;
        if records[i].state == TurnState::Open {
            let mut rec = records[i].clone();
            match self.finalize(root, &mut rec, fresh) {
                Ok(true) => {
                    records[i] = rec;
                    self.save(&records)?;
                }
                Ok(false) => {
                    self.drop_turn(&mut records, turn_id)?;
                    return Err(DirectError::Nothing("戻せる変更がありません。".into()));
                }
                Err(DirectError::Damaged(m)) => {
                    self.drop_turn(&mut records, turn_id)?;
                    return Err(DirectError::Damaged(m));
                }
                Err(e) => return Err(e),
            }
        }
        self.restore(root, &records[i], false, force)?;
        records[i].state = TurnState::Undone;
        self.save(&records)?;
        Ok(views(&records))
    }

    /// 最も seq が小さい undone のターンを、終了時の内容へやり直す。
    pub fn redo(&self, root: &ProjectRoot, turn_id: &str, force: bool, chat: Option<&ChatKey>, running: &dyn Fn(&ChatKey) -> bool) -> DResult<Vec<TurnView>> {
        let mut records = self.load()?;
        let next = records.iter().enumerate().filter(|(_, r)| r.state == TurnState::Undone).min_by_key(|(_, r)| r.seq).map(|(i, _)| i);
        let Some(i) = next else { return Err(DirectError::Nothing("やり直せる変更がありません。".into())) };
        if records[i].id != turn_id {
            return Err(DirectError::Nothing("やり直せるのは、最も古い取り消し済みの編集だけです。画面を更新して、もう一度お試しください。".into()));
        }
        Self::check_not_running(&records[i], chat, running)?;
        self.restore(root, &records[i], true, force)?;
        records[i].state = TurnState::Done;
        self.save(&records)?;
        Ok(views(&records))
    }

    /// `to_after` が真なら終了時の内容、偽なら開始前の内容へ書く。現在の内容が「元」か「行き先」のどちらかと一致しないファイルは conflict
    /// （行き先と一致は、前回の途中失敗の再試行を通すため）。複数ファイルの書き込みは原子的ではない。
    fn restore(&self, root: &ProjectRoot, rec: &TurnRecord, to_after: bool, force: bool) -> DResult<()> {
        let turn = self.turn_dir(&rec.id)?;
        let Some(after_scope) = rec.after_scope.clone() else {
            return Err(DirectError::Nothing("この編集には保存された結果がありません。".into()));
        };
        let after_scope = FileScope::new(after_scope)?;
        let damaged = |_| DirectError::Damaged(damaged_text());
        let before = load_snapshot(&turn.join("before"), &FileScope::new(rec.scope.clone())?).map_err(damaged)?;
        let after = load_snapshot(&turn.join("after"), &after_scope).map_err(damaged)?;
        let (from, to) = if to_after { (&before, &after) } else { (&after, &before) };
        if !force {
            let current = Snapshot::capture(root, &after_scope)?;
            let conflicts: Vec<String> = rec
                .files
                .iter()
                .filter(|f| {
                    let now = current.bytes(&f.path);
                    now != from.bytes(&f.path) && now != to.bytes(&f.path)
                })
                .map(|f| f.path.clone())
                .collect();
            if !conflicts.is_empty() {
                return Err(DirectError::Conflict(conflicts));
            }
        }
        for f in &rec.files {
            safe_write(root, &RelativePath::parse(&f.path)?, to.bytes(&f.path))?;
        }
        Ok(())
    }

    /// そのターンの before と after（after が無ければ、または `side == "current"` なら現在）。対象外のパスは拒否。
    pub fn diff(&self, root: &ProjectRoot, turn_id: &str, path: &str, side: Option<&str>) -> DResult<DiffSides> {
        let records = self.load()?;
        let rec = records.iter().find(|r| r.id == turn_id).ok_or_else(|| DirectError::Nothing("この編集はもう保存されていません（最新30件だけ保持します）。".into()))?;
        let rel = RelativePath::parse(path)?;
        let listed = |scope: &[SelectedFile]| scope.iter().any(|f| f.path == rel.as_str());
        if !listed(&rec.scope) && !rec.after_scope.as_deref().is_some_and(listed) {
            return Err(DirectError::Other("指定されたファイルは、この編集の対象外です。".into()));
        }
        let turn = self.turn_dir(&rec.id)?;
        let damaged = |_| DirectError::Damaged(damaged_text());
        let before = load_snapshot(&turn.join("before"), &FileScope::new(rec.scope.clone())?).map_err(damaged)?.bytes(rel.as_str()).map(<[u8]>::to_vec);
        let after = match (&rec.after_scope, side) {
            (Some(scope), side) if side != Some("current") => load_snapshot(&turn.join("after"), &FileScope::new(scope.clone())?).map_err(damaged)?.bytes(rel.as_str()).map(<[u8]>::to_vec),
            _ => read_current(root, &rel)?,
        };
        Ok(DiffSides { before, after })
    }
}

pub(crate) fn read_current(root: &ProjectRoot, rel: &RelativePath) -> DResult<Option<Vec<u8>>> {
    let Some(held) = root.open_relative(rel)? else { return Ok(None) };
    let mut bytes = Vec::new();
    (&held.file).take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(VisualError::Limit("single file bytes").into());
    }
    Ok(Some(bytes))
}

/// ルートの中へ1ファイルを書く（None は削除）。パスの途中・対象がリンクなら拒否し、ルートの外へは出ない。書き込みは一時ファイル＋置換。
pub(crate) fn safe_write(root: &ProjectRoot, rel: &RelativePath, bytes: Option<&[u8]>) -> DResult<()> {
    let path = rel.join(root.path());
    let parent = path.parent().ok_or_else(|| VisualError::UnsafePath(path.clone()))?;
    let inside = |dir: &Path| -> DResult<()> {
        if root_contains(root.path(), &fs::canonicalize(dir)?) {
            Ok(())
        } else {
            Err(VisualError::UnsafePath(dir.into()).into())
        }
    };
    let Some(bytes) = bytes else {
        if let Err(e) = fs::symlink_metadata(&path) {
            return if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e.into()) };
        }
        {
            let _pins = pin_directories(parent)?;
            inside(parent)?;
            drop(open_checked(&path, false)?);
        }
        return Ok(fs::remove_file(&path)?);
    };
    // 存在する最も深い祖先を検証してから、足りないフォルダを作る。
    let mut existing = parent;
    while fs::symlink_metadata(existing).is_err() {
        existing = existing.parent().ok_or_else(|| VisualError::UnsafePath(path.clone()))?;
    }
    {
        let _pins = pin_directories(existing)?;
        inside(existing)?;
        fs::create_dir_all(parent)?;
    }
    let _pins = pin_directories(parent)?;
    inside(parent)?;
    if fs::symlink_metadata(&path).is_ok() {
        drop(open_checked(&path, false)?);
    }
    crate::store::atomic::write_atomic(&path, bytes)?;
    Ok(())
}

/// スコープの編集対象ファイルの path・size・mtime を連結した更新印（読み取りのみ）。
pub fn stamp(root: &ProjectRoot, scope: &FileScope) -> String {
    let mut parts = Vec::new();
    for (path, access) in scope.files() {
        if access != FileAccess::Edit {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(path.join(root.path())) else { continue };
        if !meta.is_file() {
            continue;
        }
        let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
        parts.push(format!("{}:{}:{modified}", path.as_str(), meta.len()));
    }
    parts.join("|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        base: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let base = std::env::current_dir().unwrap().join("target").join(format!("visual-direct-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
            fs::create_dir_all(base.join("source")).unwrap();
            let f = Self { base };
            f.write("index.html", "<p>one</p>");
            f.write("style.css", "p{}");
            f
        }
        fn root(&self) -> ProjectRoot {
            ProjectRoot::open(&self.base.join("source")).unwrap()
        }
        fn store(&self) -> DirectStore {
            DirectStore::new(self.base.join("store"))
        }
        fn write(&self, rel: &str, text: &str) {
            let p = self.base.join("source").join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        fn read(&self, rel: &str) -> Option<String> {
            fs::read_to_string(self.base.join("source").join(rel)).ok()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }
    fn idle(_: &ChatKey) -> bool {
        false
    }
    fn chat(id: &str) -> ChatKey {
        ChatKey { backend: crate::backend::model::BackendKind::Codex, id: crate::backend::model::ExternalId(id.into()) }
    }
    fn scope(paths: &[&str]) -> FileScope {
        FileScope::new(paths.iter().map(|p| SelectedFile { path: (*p).into(), access: FileAccess::Edit }).collect()).unwrap()
    }
    fn base_scope() -> FileScope {
        scope(&["index.html", "style.css"])
    }

    #[test]
    fn begin_edit_end_detects_changes() {
        let f = Fixture::new();
        let (store, root) = (f.store(), f.root());
        let turn = store.begin(&root, &base_scope(), 1, None, &idle).unwrap();
        assert_eq!(turn.state, TurnState::Open);
        f.write("index.html", "<p>two</p>");
        f.write("extra.css", "a{}"); // 開始後に AI が作った関連ファイル
        let done = store.end(&root, &turn.id, Some(&scope(&["index.html", "style.css", "extra.css"]))).unwrap().unwrap();
        assert_eq!(done.state, TurnState::Done);
        assert_eq!(done.files, vec![DirectFile { path: "extra.css".into(), change: Change::Add }, DirectFile { path: "index.html".into(), change: Change::Change }]);
        assert!(done.can_undo && !done.can_redo);
        // 冪等
        assert_eq!(store.end(&root, &turn.id, None).unwrap().unwrap().files.len(), 2);
    }

    #[test]
    fn undo_then_redo_restores_contents_and_removes_added_files() {
        let f = Fixture::new();
        let (store, root) = (f.store(), f.root());
        let turn = store.begin(&root, &base_scope(), 1, None, &idle).unwrap();
        f.write("index.html", "<p>two</p>");
        f.write("extra.css", "a{}");
        store.end(&root, &turn.id, Some(&scope(&["index.html", "style.css", "extra.css"]))).unwrap().unwrap();
        let list = store.undo(&root, &turn.id, false, None, None, &idle).unwrap();
        assert_eq!(list[0].state, TurnState::Undone);
        assert!(list[0].can_redo && !list[0].can_undo);
        assert_eq!(f.read("index.html").unwrap(), "<p>one</p>");
        assert!(f.read("extra.css").is_none());
        assert!(matches!(store.undo(&root, &turn.id, false, None, None, &idle), Err(DirectError::Nothing(_))));
        let list = store.redo(&root, &turn.id, false, None, &idle).unwrap();
        assert_eq!(list[0].state, TurnState::Done);
        assert_eq!(f.read("index.html").unwrap(), "<p>two</p>");
        assert_eq!(f.read("extra.css").unwrap(), "a{}");
    }

    #[test]
    fn external_change_is_a_conflict_until_forced() {
        let f = Fixture::new();
        let (store, root) = (f.store(), f.root());
        let turn = store.begin(&root, &base_scope(), 1, None, &idle).unwrap();
        f.write("index.html", "<p>two</p>");
        store.end(&root, &turn.id, None).unwrap().unwrap();
        f.write("index.html", "<p>external</p>");
        match store.undo(&root, &turn.id, false, None, None, &idle) {
            Err(DirectError::Conflict(paths)) => assert_eq!(paths, vec!["index.html".to_string()]),
            other => panic!("expected conflict, got {other:?}"),
        }
        assert_eq!(f.read("index.html").unwrap(), "<p>external</p>");
        assert!(DirectError::Conflict(vec!["a".into(), "b".into()]).to_string().starts_with("conflict:\na\nb"));
        store.undo(&root, &turn.id, true, None, None, &idle).unwrap();
        assert_eq!(f.read("index.html").unwrap(), "<p>one</p>");
    }

    #[test]
    fn open_turn_is_finalized_on_recovery_and_unchanged_one_is_dropped() {
        let f = Fixture::new();
        let (store, root) = (f.store(), f.root());
        let t1 = store.begin(&root, &base_scope(), 1, None, &idle).unwrap();
        f.write("style.css", "p{color:red}");
        // アプリ終了で open のまま残った想定。次の begin が確定する。
        let t2 = store.begin(&root, &base_scope(), 2, None, &idle).unwrap();
        let list = store.turns().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, t1.id);
        assert_eq!(list[0].state, TurnState::Done);
        assert_eq!(list[0].files, vec![DirectFile { path: "style.css".into(), change: Change::Change }]);
        assert_eq!(list[1].id, t2.id);
        // t2 は変更なしで open のまま残った → 復旧で削除される。
        let kept = store.recover_open(&root, None, &idle).unwrap();
        assert_eq!(kept.len(), 1);
        assert!(!store.dir.join(&t2.id).exists());
    }

    #[test]
    fn begin_settles_only_the_latest_unbound_open_turn() {
        let f = Fixture::new();
        let (store, root) = (f.store(), f.root());
        let t1 = store.begin(&root, &base_scope(), 1, None, &idle).unwrap();
        f.write("index.html", "<p>two</p>");
        store.begin(&root, &base_scope(), 2, None, &idle).unwrap(); // t1 を確定
        let mut records = store.load().unwrap();
        records[0].state = TurnState::Open; // 別会話の実行中ターンに見立てる
        store.save(&records).unwrap();
        f.write("style.css", "p{color:red}");
        store.begin(&root, &base_scope(), 3, None, &idle).unwrap(); // 確定するのは t2 だけ
        let list = store.turns().unwrap();
        assert_eq!((list[0].id.as_str(), list[0].state), (t1.id.as_str(), TurnState::Open));
        assert_eq!(list[1].state, TurnState::Done);
        assert_eq!(list[2].state, TurnState::Open);
    }

    #[test]
    fn failed_settlement_keeps_the_turn_open_and_does_not_stop_recovery() {
        let f = Fixture::new();
        let (store, root) = (f.store(), f.root());
        let t1 = store.begin(&root, &base_scope(), 1, None, &idle).unwrap();
        f.write("index.html", &"x".repeat(9 * 1024 * 1024)); // 上限超過で確定できない（壊れてはいない）
        let kept = store.recover_open(&root, None, &idle).unwrap();
        assert_eq!((kept.len(), kept[0].state, kept[0].id.as_str()), (1, TurnState::Open, t1.id.as_str()));
    }

    #[test]
    fn open_turn_of_a_running_chat_is_not_settled_or_undone_but_a_stopped_or_unknown_one_is() {
        let f = Fixture::new();
        let (store, root) = (f.store(), f.root());
        let (a, b) = (chat("a"), chat("b"));
        let running_a = |c: &ChatKey| *c == chat("a");
        let ta = store.begin(&root, &base_scope(), 1, Some(&a), &idle).unwrap();
        f.write("index.html", "<p>two</p>");
        // 別会話 b の begin は、実行中の a のターンを確定しない。
        store.begin(&root, &base_scope(), 2, Some(&b), &running_a).unwrap();
        let list = store.turns().unwrap();
        assert_eq!((list[0].state, list[1].state), (TurnState::Open, TurnState::Open));
        store.begin(&root, &base_scope(), 3, Some(&b), &running_a).unwrap();
        assert_eq!(store.turns().unwrap()[0].state, TurnState::Open);
        // 実行中でなければ（停止済み・不明・削除済み）次の begin で確定する。
        let t4 = store.begin(&root, &base_scope(), 4, Some(&b), &idle).unwrap();
        let list = store.turns().unwrap();
        assert_eq!(list[0].id, ta.id);
        assert_eq!(list[0].state, TurnState::Done);
        assert_eq!(list.last().unwrap().id, t4.id);
    }

    #[test]
    fn undo_is_refused_while_the_recorded_chat_runs_and_allowed_when_unknown() {
        let f = Fixture::new();
        let (store, root) = (f.store(), f.root());
        let a = chat("a");
        let running_a = |c: &ChatKey| *c == chat("a");
        let t = store.begin(&root, &base_scope(), 1, Some(&a), &idle).unwrap();
        f.write("index.html", "<p>two</p>");
        store.end(&root, &t.id, None).unwrap().unwrap();
        // 渡された会話ではなく、記録された会話で判定する。
        match store.undo(&root, &t.id, false, None, Some(&chat("other")), &running_a) {
            Err(e @ DirectError::Running(_)) => assert!(e.to_string().starts_with("running:")),
            other => panic!("expected running, got {other:?}"),
        }
        assert_eq!(f.read("index.html").unwrap(), "<p>two</p>");
        store.undo(&root, &t.id, false, None, None, &idle).unwrap();
        assert_eq!(f.read("index.html").unwrap(), "<p>one</p>");
        assert!(matches!(store.redo(&root, &t.id, false, None, &running_a), Err(DirectError::Running(_))));
        store.redo(&root, &t.id, false, None, &idle).unwrap();
    }

    #[test]
    fn nothing_messages_differ_for_undo_and_redo() {
        let f = Fixture::new();
        let (store, root) = (f.store(), f.root());
        let u = store.undo(&root, "t-0001", false, None, None, &idle).unwrap_err().to_string();
        let r = store.redo(&root, "t-0001", false, None, &idle).unwrap_err().to_string();
        assert_eq!((u.as_str(), r.as_str()), ("nothing: 戻せる変更がありません。", "nothing: やり直せる変更がありません。"));
    }

    #[test]
    fn broken_turns_json_is_set_aside_and_numbers_are_not_reused() {
        let f = Fixture::new();
        let (store, root) = (f.store(), f.root());
        let t1 = store.begin(&root, &base_scope(), 1, None, &idle).unwrap();
        assert_eq!(t1.id, "t-0001");
        assert!(store.end(&root, &t1.id, None).unwrap().is_none()); // 変更なしで削除
        let t2 = store.begin(&root, &base_scope(), 2, None, &idle).unwrap();
        assert_eq!(t2.id, "t-0002"); // 削除された番号を再利用しない
        assert!(store.end(&root, "t-0001", None).unwrap().is_none()); // 重複した end は別ターンを確定しない
        assert_eq!(store.turns().unwrap()[0].state, TurnState::Open);
        fs::write(store.dir.join("turns.json"), b"{broken").unwrap();
        assert!(store.turns().unwrap().is_empty());
        assert!(store.dir.join("turns.json.bad").exists());
        assert_eq!(store.begin(&root, &base_scope(), 3, None, &idle).unwrap().id, "t-0003");
    }

    #[test]
    fn old_array_format_is_read_and_damaged_open_turn_is_discarded() {
        let f = Fixture::new();
        let (store, root) = (f.store(), f.root());
        let t1 = store.begin(&root, &base_scope(), 1, None, &idle).unwrap();
        let records = store.load().unwrap();
        fs::write(store.dir.join("turns.json"), serde_json::to_vec(&records).unwrap()).unwrap(); // 旧形式（配列）
        assert_eq!(store.turns().unwrap().len(), 1);
        fs::remove_dir_all(store.dir.join(&t1.id).join("before")).unwrap();
        let t2 = store.begin(&root, &base_scope(), 2, None, &idle).unwrap();
        assert_eq!(t2.id, "t-0002");
        assert_eq!(store.turns().unwrap().len(), 1); // 壊れた t-0001 は閉じて残さない
    }

    #[test]
    fn oversized_store_drops_old_turns_but_keeps_the_newest() {
        let f = Fixture::new();
        let mut store = f.store();
        store.max_bytes = 100;
        let root = f.root();
        for n in 0..3 {
            let t = store.begin(&root, &base_scope(), n, None, &idle).unwrap();
            f.write("index.html", &format!("<p>{n}{}</p>", "x".repeat(60)));
            store.end(&root, &t.id, None).unwrap().unwrap();
        }
        let list = store.turns().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "t-0003");
    }

    #[test]
    fn unchanged_turn_is_deleted_at_end() {
        let f = Fixture::new();
        let (store, root) = (f.store(), f.root());
        let turn = store.begin(&root, &base_scope(), 1, None, &idle).unwrap();
        assert!(store.end(&root, &turn.id, None).unwrap().is_none());
        assert!(store.turns().unwrap().is_empty());
        assert!(!store.dir.join(&turn.id).exists());
    }

    #[test]
    fn keeps_only_the_latest_thirty_turns_and_drops_undone_on_new_turn() {
        let f = Fixture::new();
        let (store, root) = (f.store(), f.root());
        for n in 0..(KEEP_TURNS + 3) {
            let t = store.begin(&root, &base_scope(), n as u64, None, &idle).unwrap();
            f.write("index.html", &format!("<p>{n}</p>"));
            store.end(&root, &t.id, None).unwrap().unwrap();
        }
        let list = store.turns().unwrap();
        assert_eq!(list.len(), KEEP_TURNS);
        assert_eq!(list[0].seq, 4);
        assert!(!store.dir.join("t-0001").exists() && store.dir.join("t-0004").exists());
        let last = list.last().unwrap().id.clone();
        store.undo(&root, &last, false, None, None, &idle).unwrap();
        let t = store.begin(&root, &base_scope(), 99, None, &idle).unwrap();
        let list = store.turns().unwrap();
        assert!(list.iter().all(|v| v.state != TurnState::Undone));
        assert!(!store.dir.join(&last).exists());
        assert_eq!(list.last().unwrap().id, t.id);
    }

    #[test]
    fn paths_outside_the_scope_or_root_are_rejected() {
        let f = Fixture::new();
        f.write("other.css", "x{}");
        let (store, root) = (f.store(), f.root());
        let turn = store.begin(&root, &base_scope(), 1, None, &idle).unwrap();
        f.write("index.html", "<p>two</p>");
        store.end(&root, &turn.id, None).unwrap().unwrap();
        assert!(store.diff(&root, &turn.id, "other.css", None).is_err());
        assert!(RelativePath::parse("../outside.html").is_err());
        let d = store.diff(&root, &turn.id, "index.html", None).unwrap();
        assert_eq!((d.before.as_deref(), d.after.as_deref()), (Some(&b"<p>one</p>"[..]), Some(&b"<p>two</p>"[..])));
        // 範囲外のファイルは undo しても触れない。
        f.write("other.css", "y{}");
        store.undo(&root, &turn.id, false, None, None, &idle).unwrap();
        assert_eq!(f.read("other.css").unwrap(), "y{}");
    }

    #[test]
    fn linked_directory_in_the_write_path_is_rejected() {
        let f = Fixture::new();
        let outside = f.base.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let link = f.base.join("source").join("sub");
        #[cfg(windows)]
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&outside).output().map(|o| o.status.success()).unwrap_or(false);
        #[cfg(not(windows))]
        let made = std::os::unix::fs::symlink(&outside, &link).is_ok();
        if !made {
            return;
        }
        let root = f.root();
        let err = safe_write(&root, &RelativePath::parse("sub/x.css").unwrap(), Some(b"x"));
        assert!(err.is_err());
        assert!(!outside.join("x.css").exists());
    }
}
