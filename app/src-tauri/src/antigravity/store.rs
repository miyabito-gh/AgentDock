//! Antigravity のチャット記録の保存（`<app_data_dir>/antigravity/chats.json`）。
//!
//! 本文の正本は AgentDock のこの記録（agy 側の会話データは読まない・消さない）。
//! - 書込みは原子的（一時ファイル→rename、`store::atomic::write_atomic`）。
//! - 読めないファイルは上書きしない。内容が壊れていれば `chats.json.broken-<時刻>` へ退避して空で続行し、
//!   読込み自体に失敗した（権限等）・新しい形式のファイルは触らず、保存だけを止める。
//! - 会話・履歴は自動削除しない。起動時に実行中のまま残った turn は「結果不明」にする（成功・失敗にしない。再実行しない）。
//!   その turn の agy プロセスの印（[`StoredTurn::process`]）は残す。生存の確認と送信の拒否は backend が行う（停止はしない）。

use super::liveness::ProcessMark;
use crate::backend::backend::PermissionPreset;
use crate::backend::model::ChatKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "chats.json";
const FORMAT_VERSION: u32 = 1;

/// 起動時に実行中のまま残っていた turn に付ける説明。
pub const ORPHANED_TURN_NOTE: &str = "AgentDock が終了したなどで、この turn の結果を確認できませんでした（再実行・停止はしていません。agy のプロセスが残っている間は、このチャットへの送信を止めます）";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    Running,
    Completed,
    Failed,
    Interrupted,
    /// 結果を確認できなかった（プロセスの追跡を失った・出力を読めなかった）。完了・失敗とは断定しない。
    Unknown,
}

impl TurnStatus {
    pub fn is_running(self) -> bool {
        self == TurnStatus::Running
    }
}

/// 承認が必要で自動拒否された操作（agy の `result.denied_actions` の1件）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeniedAction {
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// トークン使用量（agy が示した値だけ。示されない項目は None で、0にしない）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub thinking_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredTurn {
    pub turn_id: String,
    pub client_message_id: String,
    pub user_text: String,
    #[serde(default)]
    pub response_text: String,
    pub status: TurnStatus,
    pub started_at: i64,
    #[serde(default)]
    pub ended_at: Option<i64>,
    #[serde(default)]
    pub denied_actions: Vec<DeniedAction>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
    /// この turn の agy プロセス。終了を確認するまで残す（残っている間は同じチャットへ送信しない）。
    /// 旧版の記録には無い（None）。追加の項目なので形式の版は上げない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessMark>,
}

fn development_kind() -> ChatKind {
    ChatKind::Development
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredChat {
    /// AgentDock が発行する ID（`agy-...`。ChatKey.id と同じ）。
    pub id: String,
    /// agy の会話ID。最初の turn の init で判明する。
    #[serde(default)]
    pub conversation_id: Option<String>,
    pub cwd: String,
    /// プライマリ `cwd` を除く追加の作業フォルダ（`--add-dir`）。
    #[serde(default)]
    pub extra_folders: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub permission: PermissionPreset,
    #[serde(default = "development_kind")]
    pub kind: ChatKind,
    #[serde(default)]
    pub name: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub last_used_at: Option<i64>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub turns: Vec<StoredTurn>,
    /// 保存しない一時の会話（メモリ上だけ。一覧に出さない）。
    #[serde(skip)]
    pub ephemeral: bool,
}

impl StoredChat {
    pub fn running_turn(&self) -> Option<&StoredTurn> {
        self.turns.iter().find(|t| t.status.is_running())
    }
}

#[derive(Deserialize)]
struct FileIn {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    chats: Vec<StoredChat>,
}

#[derive(Serialize)]
struct FileOut<'a> {
    version: u32,
    chats: Vec<&'a StoredChat>,
}

pub struct ChatStore {
    path: PathBuf,
    chats: Vec<StoredChat>,
    /// 保存を止めている理由（読めなかったファイルを上書きしないため）。
    blocked: Option<String>,
}

impl ChatStore {
    fn new(dir: &Path) -> Self {
        ChatStore { path: dir.join(FILE_NAME), chats: Vec::new(), blocked: None }
    }

    /// `dir`（`<app_data_dir>/antigravity`）の記録を読む。戻りの文字列は利用者へ知らせる警告。
    pub fn load(dir: &Path, now_ms: i64) -> (ChatStore, Vec<String>) {
        let mut store = ChatStore::new(dir);
        let mut warnings = Vec::new();
        let bytes = match std::fs::read(&store.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (store, warnings),
            Err(e) => {
                // 内容の問題ではないので触らない。保存だけ止めて、読めるようになるまで上書きしない。
                let msg = format!("{FILE_NAME} を読めませんでした（{e}）。上書きを避けるため、このチャット記録は保存しません");
                store.blocked = Some(msg.clone());
                warnings.push(msg);
                return (store, warnings);
            }
        };
        match serde_json::from_slice::<FileIn>(&bytes) {
            Ok(f) if f.version > FORMAT_VERSION => {
                let msg = format!("{FILE_NAME} は新しい形式（version {}）です。上書きを避けるため、このチャット記録は保存しません", f.version);
                store.blocked = Some(msg.clone());
                warnings.push(msg);
            }
            Ok(f) => {
                store.chats = f.chats;
                let orphaned = store.mark_orphaned_turns();
                if orphaned > 0 {
                    warnings.push(format!("実行中のまま残っていた {orphaned} 件の turn を「結果不明」にしました（再実行していません）"));
                    if let Err(e) = store.save() {
                        warnings.push(format!("結果不明への更新を保存できませんでした（{e}）"));
                    }
                }
            }
            Err(e) => match quarantine(&store.path, now_ms) {
                Ok(moved) => warnings.push(format!("{FILE_NAME} を読めなかったため {} へ退避し、空の記録で続けます（{e}）", moved.display())),
                Err(why) => {
                    let msg = format!("{FILE_NAME} が壊れていますが退避できませんでした（{why}）。上書きを避けるため、このチャット記録は保存しません");
                    store.blocked = Some(msg.clone());
                    warnings.push(msg);
                }
            },
        }
        (store, warnings)
    }

    /// 実行中のまま残った turn（プロセスを追跡していない）を「結果不明」へ。件数を返す。
    fn mark_orphaned_turns(&mut self) -> usize {
        let mut n = 0;
        for t in self.chats.iter_mut().flat_map(|c| c.turns.iter_mut()).filter(|t| t.status.is_running()) {
            t.status = TurnStatus::Unknown;
            t.ended_at = None;
            if t.error.is_none() {
                t.error = Some(ORPHANED_TURN_NOTE.to_string());
            }
            n += 1;
        }
        n
    }

    /// 原子的に保存する。一時（`ephemeral`）の会話は書かない。
    pub fn save(&self) -> Result<(), String> {
        if let Some(reason) = &self.blocked {
            return Err(reason.clone());
        }
        let out = FileOut { version: FORMAT_VERSION, chats: self.chats.iter().filter(|c| !c.ephemeral).collect() };
        let bytes = serde_json::to_vec(&out).map_err(|e| e.to_string())?;
        crate::store::atomic::write_atomic(&self.path, &bytes).map_err(|e| e.to_string())
    }

    pub fn chats(&self) -> &[StoredChat] {
        &self.chats
    }

    pub fn get(&self, id: &str) -> Option<&StoredChat> {
        self.chats.iter().find(|c| c.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut StoredChat> {
        self.chats.iter_mut().find(|c| c.id == id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    pub fn insert(&mut self, chat: StoredChat) {
        self.chats.push(chat);
    }

    pub fn remove(&mut self, id: &str) -> Option<StoredChat> {
        let i = self.chats.iter().position(|c| c.id == id)?;
        Some(self.chats.remove(i))
    }

    /// 終了を確認していない agy プロセスの印（チャットID・turn ID・印）。
    pub fn tracked_processes(&self) -> Vec<(String, String, ProcessMark)> {
        self.chats.iter().flat_map(|c| c.turns.iter().filter_map(move |t| t.process.map(|m| (c.id.clone(), t.turn_id.clone(), m)))).collect()
    }

    /// プロセスの終了を確認した turn の印を外す。外したら true（保存は呼び出し側）。
    pub fn clear_process(&mut self, chat_id: &str, turn_id: &str) -> bool {
        match self.get_mut(chat_id).and_then(|c| c.turns.iter_mut().find(|t| t.turn_id == turn_id)) {
            Some(t) if t.process.is_some() => {
                t.process = None;
                true
            }
            _ => false,
        }
    }
}

/// 壊れたファイルを `chats.json.broken-<時刻>` へ移す（移せなければ複製。どちらも失敗なら Err）。
fn quarantine(path: &Path, now_ms: i64) -> Result<PathBuf, String> {
    let dir = path.parent().ok_or_else(|| "no parent directory".to_string())?;
    let mut n = 0u32;
    let target = loop {
        let name = if n == 0 { format!("{FILE_NAME}.broken-{now_ms}") } else { format!("{FILE_NAME}.broken-{now_ms}-{n}") };
        let t = dir.join(name);
        if !t.exists() {
            break t;
        }
        n += 1;
    };
    match std::fs::rename(path, &target) {
        Ok(()) => Ok(target),
        Err(rename_err) => match std::fs::copy(path, &target) {
            Ok(_) => {
                // 複製できても元が残る。上書きされる前に消す（消せなければ、保存は元の上に書くので失敗扱いにする）。
                std::fs::remove_file(path).map(|_| target).map_err(|e| format!("rename: {rename_err}; remove after copy: {e}"))
            }
            Err(copy_err) => Err(format!("rename: {rename_err}; copy: {copy_err}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: &str) -> StoredChat {
        StoredChat {
            id: id.into(),
            conversation_id: None,
            cwd: "C:\\work".into(),
            extra_folders: vec!["C:\\extra".into()],
            model: Some("gemini-3.8-flash-medium".into()),
            permission: PermissionPreset::WorkspaceWriteOnRequest,
            kind: ChatKind::Development,
            name: Some("名前".into()),
            created_at: 10,
            last_used_at: Some(20),
            archived: false,
            turns: Vec::new(),
            ephemeral: false,
        }
    }

    fn turn(id: &str, status: TurnStatus) -> StoredTurn {
        StoredTurn {
            turn_id: id.into(),
            client_message_id: format!("cm-{id}"),
            user_text: "こんにちは".into(),
            response_text: "PONG\n".into(),
            status,
            started_at: 30,
            ended_at: Some(40),
            denied_actions: vec![DeniedAction { action: Some("command".into()), display_name: Some("Bash".into()) }],
            error: None,
            usage: Some(Usage { input_tokens: Some(10647), output_tokens: Some(2), thinking_tokens: Some(0), cache_read_tokens: None, total_tokens: Some(10649) }),
            process: None,
        }
    }

    #[test]
    fn missing_file_loads_empty_without_warning() {
        let dir = crate::test_support::unique_temp_dir("agentdock-agy-store");
        let (s, w) = ChatStore::load(&dir.join("antigravity"), 1);
        assert!(s.chats().is_empty() && w.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_and_load_round_trip_and_skip_ephemeral() {
        let dir = crate::test_support::unique_temp_dir("agentdock-agy-store");
        let sub = dir.join("antigravity");
        let (mut s, _) = ChatStore::load(&sub, 1);
        let mut a = chat("agy-a");
        a.conversation_id = Some("conv-1".into());
        a.turns.push(turn("agy-a-t1", TurnStatus::Completed));
        let mut tmp = chat("agy-tmp");
        tmp.ephemeral = true;
        s.insert(a.clone());
        s.insert(tmp);
        s.save().unwrap();
        let (r, w) = ChatStore::load(&sub, 2);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(r.chats(), [a]);
        assert!(!r.contains("agy-tmp"));
        // 一時ファイルが残らない。
        let names: Vec<_> = std::fs::read_dir(&sub).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, [FILE_NAME]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_file_is_moved_aside_and_not_overwritten() {
        let dir = crate::test_support::unique_temp_dir("agentdock-agy-store");
        let sub = dir.join("antigravity");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(FILE_NAME), b"{ this is not json").unwrap();
        let (mut s, w) = ChatStore::load(&sub, 12345);
        assert!(s.chats().is_empty());
        assert_eq!(w.len(), 1, "{w:?}");
        let broken = sub.join(format!("{FILE_NAME}.broken-12345"));
        assert_eq!(std::fs::read(&broken).unwrap(), b"{ this is not json");
        assert!(!sub.join(FILE_NAME).exists(), "the broken file is no longer in place");
        // 空で続行でき、保存すると新しいファイルになる（退避したものは残る）。
        s.insert(chat("agy-new"));
        s.save().unwrap();
        assert_eq!(std::fs::read(&broken).unwrap(), b"{ this is not json");
        assert!(ChatStore::load(&sub, 99).0.contains("agy-new"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_format_is_never_overwritten() {
        let dir = crate::test_support::unique_temp_dir("agentdock-agy-store");
        let sub = dir.join("antigravity");
        std::fs::create_dir_all(&sub).unwrap();
        let original = br#"{"version":99,"chats":[]}"#;
        std::fs::write(sub.join(FILE_NAME), original).unwrap();
        let (mut s, w) = ChatStore::load(&sub, 1);
        assert_eq!(w.len(), 1);
        s.insert(chat("agy-x"));
        assert!(s.save().is_err());
        assert_eq!(std::fs::read(sub.join(FILE_NAME)).unwrap(), original);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn running_turns_left_over_become_outcome_unknown() {
        let dir = crate::test_support::unique_temp_dir("agentdock-agy-store");
        let sub = dir.join("antigravity");
        let (mut s, _) = ChatStore::load(&sub, 1);
        let mut c = chat("agy-a");
        c.turns.push(turn("agy-a-t1", TurnStatus::Completed));
        let mut running = turn("agy-a-t2", TurnStatus::Running);
        running.ended_at = None;
        running.process = Some(ProcessMark { pid: 4242, created: Some(77) });
        c.turns.push(running);
        s.insert(c);
        s.save().unwrap();

        let (mut r, w) = ChatStore::load(&sub, 2);
        assert_eq!(w.len(), 1, "{w:?}");
        let c = r.get("agy-a").unwrap();
        assert_eq!(c.turns[0].status, TurnStatus::Completed);
        assert_eq!(c.turns[1].status, TurnStatus::Unknown, "neither completed nor failed");
        assert_eq!(c.turns[1].error.as_deref(), Some(ORPHANED_TURN_NOTE));
        assert!(c.turns[1].ended_at.is_none());
        assert!(c.running_turn().is_none(), "nothing is re-run");
        // プロセスの印は残る（終了を確認するまで送信を止めるため。ここでは止めも消しもしない）。
        assert_eq!(r.tracked_processes(), [("agy-a".to_string(), "agy-a-t2".to_string(), ProcessMark { pid: 4242, created: Some(77) })]);
        assert!(r.clear_process("agy-a", "agy-a-t2") && !r.clear_process("agy-a", "agy-a-t2"));
        assert!(r.tracked_processes().is_empty());
        // 印の無い turn は書き出しに項目を持たない（旧版の記録と同じ形）。
        r.save().unwrap();
        assert!(!std::fs::read_to_string(sub.join(FILE_NAME)).unwrap().contains("\"process\""));
        // 次の起動でも同じ（保存済み）。警告は繰り返さない。
        let (r2, w2) = ChatStore::load(&sub, 3);
        assert!(w2.is_empty());
        assert_eq!(r2.get("agy-a").unwrap().turns[1].status, TurnStatus::Unknown);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_deletes_only_the_record() {
        let dir = crate::test_support::unique_temp_dir("agentdock-agy-store");
        let sub = dir.join("antigravity");
        let (mut s, _) = ChatStore::load(&sub, 1);
        s.insert(chat("agy-a"));
        s.insert(chat("agy-b"));
        assert!(s.remove("agy-a").is_some() && s.remove("agy-a").is_none());
        s.save().unwrap();
        let (r, _) = ChatStore::load(&sub, 2);
        assert_eq!(r.chats().iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["agy-b"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
