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
