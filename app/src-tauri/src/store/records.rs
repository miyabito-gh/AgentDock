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

/// worktree台帳（ルートの `worktrees.json`、P3-7）。AgentDockが作った（作ろうとした）ものだけ。外部のworktreeは持たない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreesFile {
    pub schema_version: u32,
    pub worktrees: Vec<WorktreeRecord>,
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
    /// このアプリが thread/start で開始した会話（アプリ管理）。再起動後も「アプリ管理」として扱う。
    /// 外部で作られた会話をユーザー確認のうえ再開した印は含めない（再起動後は外部扱いに戻り、再開確認が要る）。
    #[serde(default)]
    pub hosted: bool,
    // ── 段階③の追加（すべて既定値あり。旧ファイルも読める。SCHEMA_VERSION は上げない） ──
    /// 計画／実行の選択値（None＝未選択。受理値は別。P3-3）。
    #[serde(default)]
    pub work_mode: Option<WorkMode>,
    /// memoriesのチャット別設定（バックエンドが示す不透明な値。P3-3）。
    #[serde(default)]
    pub memory_mode: Option<String>,
    /// このチャットがレビュー用に作られたときの、レビュー元（P3-2）。
    #[serde(default)]
    pub review_of: Option<ChatKey>,
    /// このチャットが分岐で作られたときの、分岐元（P3-2）。
    #[serde(default)]
    pub fork_of: Option<ForkOrigin>,
    /// side相談の記録（P3-4）。
    #[serde(default)]
    pub side_sessions: Vec<SideSessionMeta>,
    /// このチャットの作業場所として作ったworktree（`worktrees.json` の項目、P3-7）。
    #[serde(default)]
    pub worktree: Option<LocalId>,
    /// 送る前に保存した、turnを開始する操作（レビュー・圧縮）の未解決の記録。照合（読取り）だけが消す。
    #[serde(default)]
    pub pending_ops: Vec<PendingOp>,
}

/// 文脈の圧縮前の控え（`chats\<dirId>\compactions\<ms>.json`）。AgentDock保存・表示専用で、バックエンドへは戻さない。
/// `turns` は圧縮を送る前に読んだ履歴（読めた範囲。部分取得は各turnの `complete` で分かる）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionFile {
    pub schema_version: u32,
    pub chat: ChatKey,
    pub created_at: UnixMillis,
    pub turns: Vec<TurnRecord>,
}

/// side相談の確定した発言の記録（`chats\<dirId>\side\<id>.json`）。AgentDock保存・表示専用で、バックエンドへは戻さない。
/// 閉じた後・再起動後は「終了（再開不可）」の読取り専用の記録として開く。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideFile {
    pub schema_version: u32,
    pub id: LocalId,
    pub main: ChatKey,
    pub thread: ChatKey,
    pub opened_at: UnixMillis,
    pub entries: Vec<SideEntry>,
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
            hosted: false,
            work_mode: None,
            memory_mode: None,
            review_of: None,
            fork_of: None,
            side_sessions: Vec::new(),
            worktree: None,
            pending_ops: Vec::new(),
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
        f.model = Some(ModelChoice { model: "m".into(), effort: Some("high".into()), speed_tier: None });
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

    /// 段階③の追加フィールドを持たない旧ファイルが読め、追加分は既定値になる。
    #[test]
    fn chat_local_without_stage3_fields_still_reads() {
        let mut v: serde_json::Value = serde_json::from_slice(&to_bytes(&sample_local()).unwrap()).unwrap();
        let o = v.as_object_mut().unwrap();
        for k in ["workMode", "memoryMode", "reviewOf", "forkOf", "sideSessions", "worktree", "pendingOps"] {
            assert!(o.remove(k).is_some(), "{k}");
        }
        let back: ChatLocalFile = parse_versioned(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(back, sample_local());
        assert!(back.pending_ops.is_empty() && back.side_sessions.is_empty() && back.work_mode.is_none());
    }

    #[test]
    fn stage3_fields_round_trip() {
        let mut f = sample_local();
        f.work_mode = Some(WorkMode::Plan);
        f.memory_mode = Some("raw-mode".into());
        f.review_of = Some(key());
        f.fork_of = Some(ForkOrigin { chat: key(), through_turn: Some(ExternalId("turn-2".into())) });
        f.side_sessions.push(SideSessionMeta { id: LocalId("s1".into()), main: key(), thread: key(), state: SideState::Ended { reason: "closed".into() }, opened_at: Some(UnixMillis(5)) });
        f.worktree = Some(LocalId("w1".into()));
        f.pending_ops.push(PendingOp { id: LocalId("op-1".into()), op: ParityOp::Compact, since: UnixMillis(7) });
        f.model = Some(ModelChoice { model: "m".into(), effort: None, speed_tier: Some("fast".into()) });
        let back: ChatLocalFile = parse_versioned(&to_bytes(&f).unwrap()).unwrap();
        assert_eq!(back, f);
        let v: serde_json::Value = serde_json::from_slice(&to_bytes(&f).unwrap()).unwrap();
        assert_eq!(v["pendingOps"][0]["op"], "compact");
        assert_eq!(v["schemaVersion"], 1);
    }

    /// `speedTier` を持たない旧い `model` も読める。
    #[test]
    fn model_choice_without_speed_tier_reads() {
        let m: ModelChoice = serde_json::from_str(r#"{"model":"m","effort":"low"}"#).unwrap();
        assert_eq!(m.speed_tier, None);
    }

    #[test]
    fn settings_and_queue_round_trip() {
        let s = AppSettingsFile::new(AppSettings::default());
        let back: AppSettingsFile = parse_versioned(&to_bytes(&s).unwrap()).unwrap();
        assert_eq!(back, s);

        let q = QueueFile {
            schema_version: SCHEMA_VERSION,
            queue: ChatQueue { chat: key(), run: QueueRun::PausedAfterRestart, hold: None, baseline_at: None, awaiting: None, entries: vec![], next_order: 3 },
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
