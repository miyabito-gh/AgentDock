//! 保存ファイルの形（P2）。すべて `schemaVersion` を持つ。新しい版のファイルは上書きしない。
//!
//! UIへ出す型（`backend::local`）とは分ける。保存形式にCodex固有の型を入れない（ChatKey・LocalIdだけで参照する）。

use serde::{Deserialize, Serialize};

use crate::backend::backend::PermissionPreset;
use crate::backend::local::*;
use crate::backend::model::*;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettingsFile {
    pub schema_version: u32,
    pub settings: AppSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsFile {
    pub schema_version: u32,
    pub main: Option<WindowBounds>,
    pub monitor: Option<WindowBounds>,
}

/// チャット別の補足情報（`chats\<dirId>\chat.json`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatLocalFile {
    pub schema_version: u32,
    pub dir_id: LocalId,
    /// thread開始前（一般チャットの作業領域だけ作った段階）は None。
    pub chat: Option<ChatKey>,
    pub pinned: bool,
    pub last_used_at: Option<UnixMillis>,
    pub model: Option<ModelChoice>,
    pub permission: Option<PermissionPreset>,
    pub next_cwd: Option<String>,
    pub draft: Draft,
    pub visibility: ListVisibility,
    pub delete_pending: Option<DeletePending>,
    pub attachments: Vec<AttachmentEntry>,
    pub artifacts: Vec<ArtifactEntry>,
    /// 「確認済み」にした失敗（agent＋turn）。確認済みは再実行・成功化・キュー再開を意味しない（M42）。
    pub acknowledged_failures: Vec<TurnKey>,
    /// 起動直後に一覧へ出すための表示用メタデータ（正本はCodex。接続後に上書きされる）。
    pub cached_meta: Option<CachedChatMeta>,
}

/// 接続前の一覧表示用。鮮度は常に「切断」として扱い、状態の根拠にしない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedChatMeta {
    pub name: Known<String>,
    pub cwd: Known<String>,
    pub kind: ChatKind,
    pub origin: ChatOrigin,
    pub saved_at: UnixMillis,
}

/// 受理不明の送信の記録（再起動後も照合を続けるため。照合以外で解消しない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedSendRecord {
    pub attempt: LocalId,
    pub chat: ChatKey,
    pub client_message_id: String,
    pub since: UnixMillis,
    /// キュー項目から送ったものなら、その項目。
    pub entry: Option<LocalId>,
    pub text: String,
    pub attachments: Vec<LocalId>,
    pub applied: Option<AppliedSettings>,
}

/// キュー（`chats\<dirId>\queue.json`）。`run` は保存するが、読込み時に `Active` は `PausedAfterRestart` に変える。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueFile {
    pub schema_version: u32,
    pub queue: ChatQueue,
    pub unresolved_sends: Vec<UnresolvedSendRecord>,
}

/// 監視活動履歴の1行（`activity.jsonl`）。逐次本文（delta）は保存しない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ActivityLine {
    AgentSeen { at: UnixMillis, agent: Agent },
    StatusChanged { at: UnixMillis, agent: AgentKey, state: AgentState, raw: String, turn: Option<ExternalId>, source: EvidenceSource },
    TurnEnded { at: UnixMillis, turn: TurnKey, end: TurnEnd },
    /// 完了・未確認で確定したitemの短い表示本文。
    Activity { at: UnixMillis, activity: Activity },
    /// 鮮度の変化（切断・wake・live復旧）。
    Freshness { at: UnixMillis, agent: Option<AgentKey>, freshness: Freshness },
}

// ───────────────────────────── 読み書きの補助 ─────────────────────────────

/// 保存ファイルを読んだときの失敗。どちらも元のファイルを上書きしない（呼出し側で退避・保護する）。
#[derive(Debug, PartialEq)]
pub enum ParseError {
    /// JSONとして、または形として読めない。
    Corrupt(String),
    /// このアプリより新しい版で書かれている。
    Newer(u32),
}

/// `schemaVersion` を先に確認してから読む。版が無い・数でないものは壊れているとみなす。
pub fn parse_versioned<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ParseError> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| ParseError::Corrupt(e.to_string()))?;
    let version = value.get("schemaVersion").and_then(|v| v.as_u64()).ok_or_else(|| ParseError::Corrupt("schemaVersion is missing".into()))?;
    if version > SCHEMA_VERSION as u64 {
        return Err(ParseError::Newer(version.min(u32::MAX as u64) as u32));
    }
    serde_json::from_value(value).map_err(|e| ParseError::Corrupt(e.to_string()))
}

pub fn to_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec_pretty(value)
}

impl ChatLocalFile {
    /// 何も保存していないチャットの初期値（ピンなし・下書きなし・既定の見え方）。
    pub fn new(dir_id: LocalId, chat: Option<ChatKey>) -> Self {
        ChatLocalFile {
            schema_version: SCHEMA_VERSION,
            dir_id,
            chat,
            pinned: false,
            last_used_at: None,
            model: None,
            permission: None,
            next_cwd: None,
            draft: Draft::default(),
            visibility: ListVisibility::Visible,
            delete_pending: None,
            attachments: Vec::new(),
            artifacts: Vec::new(),
            acknowledged_failures: Vec::new(),
            cached_meta: None,
        }
    }
}

impl AppSettingsFile {
    pub fn new(settings: AppSettings) -> Self {
        AppSettingsFile { schema_version: SCHEMA_VERSION, settings }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId("thread-1".into()) }
    }

    fn sample_local() -> ChatLocalFile {
        let mut f = ChatLocalFile::new(LocalId("dir-1".into()), Some(key()));
        f.pinned = true;
        f.last_used_at = Some(UnixMillis(1_700_000_000_000));
        f.model = Some(ModelChoice { model: "m".into(), effort: Some("high".into()) });
        f.permission = Some(PermissionPreset::ReadOnly);
        f.next_cwd = Some(r"C:\work".into());
        f.draft = Draft { text: "下書き\n2行目".into(), attachments: vec![LocalId("att-1".into())], updated_at: Some(UnixMillis(5)) };
        f.visibility = ListVisibility::Archived { at: UnixMillis(9), sync: ArchiveSync::WaitingForWorkEnd };
        f.attachments.push(AttachmentEntry {
            id: LocalId("att-1".into()),
            chat: key(),
            kind: AttachmentKind::File,
            display_name: "a.txt".into(),
            source: AttachmentSource::File { original_path: r"C:\src\a.txt".into() },
            copy_path: None,
            size: Known::NotFetched,
            attached_at: UnixMillis(3),
            state: AttachmentState::CopyFailed { reason: CopyFailure::Interrupted, message: "m".into() },
            used_by: vec![],
        });
        f
    }

    #[test]
    fn chat_local_round_trips() {
        let f = sample_local();
        let bytes = to_bytes(&f).unwrap();
        let back: ChatLocalFile = parse_versioned(&bytes).unwrap();
        assert_eq!(back, f);
        // 保存形式は camelCase で、schemaVersion を持つ。
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["schemaVersion"], 1);
        assert_eq!(v["dirId"], "dir-1");
        assert_eq!(v["pinned"], true);
    }

    #[test]
    fn settings_and_queue_round_trip() {
        let s = AppSettingsFile::new(AppSettings::default());
        let back: AppSettingsFile = parse_versioned(&to_bytes(&s).unwrap()).unwrap();
        assert_eq!(back, s);

        let q = QueueFile {
            schema_version: SCHEMA_VERSION,
            queue: ChatQueue { chat: key(), run: QueueRun::PausedAfterRestart, hold: None, baseline_at: None, entries: vec![], next_order: 3 },
            unresolved_sends: vec![UnresolvedSendRecord {
                attempt: LocalId("att".into()),
                chat: key(),
                client_message_id: "cm-1".into(),
                since: UnixMillis(1),
                entry: None,
                text: "t".into(),
                attachments: vec![],
                applied: None,
            }],
        };
        let back: QueueFile = parse_versioned(&to_bytes(&q).unwrap()).unwrap();
        assert_eq!(back, q);
    }

    #[test]
    fn newer_schema_version_is_rejected_not_read() {
        let mut v: serde_json::Value = serde_json::from_slice(&to_bytes(&sample_local()).unwrap()).unwrap();
        v["schemaVersion"] = serde_json::json!(SCHEMA_VERSION + 1);
        let r: Result<ChatLocalFile, _> = parse_versioned(&serde_json::to_vec(&v).unwrap());
        assert_eq!(r.unwrap_err(), ParseError::Newer(SCHEMA_VERSION + 1));
    }

    #[test]
    fn broken_or_unversioned_files_are_corrupt() {
        assert!(matches!(parse_versioned::<ChatLocalFile>(b"{not json"), Err(ParseError::Corrupt(_))));
        assert!(matches!(parse_versioned::<ChatLocalFile>(b"{\"pinned\":true}"), Err(ParseError::Corrupt(_))));
        assert!(matches!(parse_versioned::<ChatLocalFile>(b"{\"schemaVersion\":1}"), Err(ParseError::Corrupt(_))));
        assert!(matches!(parse_versioned::<ChatLocalFile>(b""), Err(ParseError::Corrupt(_))));
    }
}
