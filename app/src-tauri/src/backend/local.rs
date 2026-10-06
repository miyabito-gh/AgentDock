//! 段階②（P1設計）: アプリ側で保持・判断するデータとIPC契約（要件§3.4・§3.6〜3.8・§3.11・§3.12）。
//!
//! - Codex固有の型・メソッド名を含めない。チャットは [`ChatKey`]（バックエンド種別＋内部ID）で識別する（§2.3）。
//! - Codexの保存履歴が会話の正本。ここにあるのは補足情報（ピン・下書き・添付・キュー・設定・監視活動の取得記録）だけ（§3.6）。
//! - 不明値は [`Known`] で表す。保存状態は [`SaveState`]（保存済み／未保存／保存失敗）で表す（§4.3）。
//! - TS側は `app/src/ipc/local.ts`。P2〜P7で `ipc.rs` の `HostEvent`・`command_names` と `types.ts` に統合する
//!   （統合時に [`LocalHostEvent`] の各variantを `HostEvent` へ移し、`live.ts` の switch に分岐を足す）。
//! - 設計メモは `app/DESIGN_P2.md`。

use serde::{Deserialize, Serialize};

use super::backend::PermissionPreset;
use super::model::*;

// ───────────────────────────── 保存状態（§3.12、D06） ─────────────────────────────

/// 保存の単位。1単位＝1ファイル（原子的書込みの単位）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SaveScope {
    /// アプリ設定（通知・窓・自動起動・codex.exeパス等）。
    AppSettings,
    /// 窓の位置・サイズ。
    WindowBounds,
    /// チャット別の補足情報（ピン・下書き・モデル/権限・アーカイブ・削除保留・添付台帳）。
    ChatLocal { chat: ChatKey },
    /// チャット別のキューと受理不明の送信記録。
    Queue { chat: ChatKey },
    /// チャット別の監視活動履歴（追記）。
    Activity { chat: ChatKey },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SaveStatus {
    pub scope: SaveScope,
    pub state: SaveState,
    /// 最後に書込みを試みた時刻。未試行は None。
    pub last_attempt_at: Option<UnixMillis>,
    /// 自動再試行の回数（ユーザーの「再試行」で0に戻す）。
    pub retries: u32,
}

// ───────────────────────────── チャット別の補足情報 ─────────────────────────────

/// 入力途中の文章と送信前の添付（M40）。復元だけで送信しない。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub text: String,
    /// 送信前の添付（[`AttachmentEntry::id`]）。
    pub attachments: Vec<LocalId>,
    pub updated_at: Option<UnixMillis>,
}

/// アプリの一覧での見え方（§3.6、M43）。Codex側のarchive状態とは別に持つ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ListVisibility {
    Visible,
    /// アプリ上はアーカイブ一覧へ移した。作業・通知・キューは継続する。
    Archived { at: UnixMillis, sync: ArchiveSync },
    /// 外部作成の会話の「削除」。元の履歴は参照扱いで消さず、アプリの一覧から外しただけ（M36、§3.6）。
    RemovedFromList { at: UnixMillis },
}

/// Codex側への `archive` 反映の進み具合。作業完了・停止確認の後に送る（合意 2026-10-06）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ArchiveSync {
    /// 作業中・停止未確認・キュー残りのため待っている。
    WaitingForWorkEnd,
    Sending,
    Synced { at: UnixMillis },
    /// 失敗・結果不明。アプリ上はアーカイブのまま、再試行できる。
    Failed { message: String, at: UnixMillis },
    /// バックエンドがarchive非対応。アプリ側の非表示だけ。
    Unsupported,
}

/// 削除保留（M46）。停止未確認・所有不明・部分失敗の間は会話・添付・成果物を保持し、送信を止める。
/// 停止確認後も自動では削除しない。ユーザーが再度「削除」を選んだときだけ削除する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct DeletePending {
    pub requested_at: UnixMillis,
    pub reason: DeletePendingReason,
    /// 対応する停止記録（あれば）。
    pub stop_record: Option<LocalId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DeletePendingReason {
    StopUnconfirmed,
    OwnershipUnknown,
    /// 一部の削除に失敗した。完了と偽らない（§3.6）。
    PartialFailure { done: Vec<String>, failed: Vec<String> },
    /// 停止は確認できた。ユーザーの再操作を待っている（自動削除しない）。
    ReadyForUserRetry,
}

/// 左の一覧の印（§3.11）。通知設定に関係なく付ける。永続化しない（状態から導出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ChatMarks {
    pub awaiting_answer: bool,
    /// 「確認済み」にしていない失敗がある。
    pub unacknowledged_failure: bool,
}

/// チャット別の補足情報（UIへ出す形）。`Chat` と並べて表示する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ChatLocalView {
    pub chat: ChatKey,
    pub pinned: bool,
    /// ユーザーの利用（開く・送信）でだけ更新する。背景活動では更新しない（§3.6）。
    pub last_used_at: Option<UnixMillis>,
    /// チャット別のモデル選択（既定値の変更は波及させない）。
    pub model: Option<ModelChoice>,
    /// チャット別の権限。None＝初期値（WorkspaceWriteOnRequest）。
    pub permission: Option<PermissionPreset>,
    /// 次のturnから使う作業フォルダ（M44）。None＝会話の現在の作業フォルダ。
    pub next_cwd: Option<String>,
    pub draft: Draft,
    pub visibility: ListVisibility,
    pub delete_pending: Option<DeletePending>,
    pub marks: ChatMarks,
    /// 「確認済み」にした失敗（agent＋turn）。UIが現在の失敗との一致で確認済みを判定する（M42）。
    pub acknowledged_failures: Vec<TurnKey>,
    pub save: SaveState,
}

// ───────────────────────────── 添付・成果物（§3.8） ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AttachmentSource {
    /// ファイル選択・ドラッグ＆ドロップ。元パスは参照のみ（変更・削除しない、M32）。
    File { original_path: String },
    /// クリップボードの画像（元ファイルなし）。
    ClipboardImage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CopyFailure {
    InsufficientSpace { #[cfg_attr(test, ts(type = "number"))] required: u64, #[cfg_attr(test, ts(type = "number"))] available: u64 },
    SourceUnreadable,
    WriteFailed,
    /// コピー中にアプリが終了した（起動時に検出）。
    Interrupted,
    SizeMismatch,
}

/// 添付の状態。`Ready` だけが送信に使える（不完全なコピーを利用可能にしない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AttachmentState {
    Copying,
    Ready,
    CopyFailed { reason: CopyFailure, message: String },
    /// 台帳にあるがコピーの実体がない（再開時の欠損表示）。
    Missing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct AttachmentEntry {
    pub id: LocalId,
    pub chat: ChatKey,
    pub kind: AttachmentKind,
    pub display_name: String,
    pub source: AttachmentSource,
    /// チャット領域内のコピーの絶対パス。`Ready` 以外では使わない。
    pub copy_path: Option<String>,
    #[cfg_attr(test, ts(as = "Known<u32>"))]
    pub size: Known<u64>,
    /// 同じファイルの再添付を区別する（別コピー・別添付）。
    pub attached_at: UnixMillis,
    pub state: AttachmentState,
    /// この添付を使った送信試行・キュー項目（エクスポートと削除確認の表示用）。
    pub used_by: Vec<LocalId>,
}

/// 成果物（会話で作られたファイル）。実在を確認したものだけ会話内のファイル項目として示す（§3.8）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ArtifactEntry {
    pub id: LocalId,
    pub chat: ChatKey,
    pub agent: AgentKey,
    pub item: Option<ItemKey>,
    pub path: String,
    pub observed_at: UnixMillis,
    /// 最後の実在確認の結果。`Value{false}` は欠損表示。
    pub exists: Known<bool>,
    pub checked_at: Option<UnixMillis>,
    /// チャット専用領域内のファイルか（削除対象の判定。領域外は削除しない、§3.6）。
    pub in_chat_area: bool,
}

/// 開く・名前を付けて保存の対象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum FileRef {
    Attachment { chat: ChatKey, id: LocalId },
    Artifact { chat: ChatKey, id: LocalId },
}

// ───────────────────────────── キュー（§3.7） ─────────────────────────────

/// 送信時点で適用した設定（§3.7「送信時点で有効なチャットの設定」）。登録時には決めない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct AppliedSettings {
    pub model: Option<ModelChoice>,
    pub permission: PermissionPreset,
    pub cwd: Known<String>,
    pub decided_at: UnixMillis,
}

/// キュー全体の進行。項目の状態とは別に持つ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum QueueRun {
    /// 条件がそろえば先頭を1件送る。
    Active,
    /// 失敗・中断・送信拒否で止めた。明示的な「キューを再開」だけで Active に戻る（M42）。
    Stopped { cause: QueueStopCause, at: UnixMillis },
    /// 起動直後（再起動後に自動送信しない、§3.11）。明示的な「キューを再開」で Active。
    PausedAfterRestart,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum QueueStopCause {
    ParentFailed { turn: TurnKey },
    ParentInterrupted { turn: TurnKey },
    DescendantFailed { agent: AgentKey },
    DescendantInterrupted { agent: AgentKey },
    /// 先頭項目の送信が明示的に拒否された。
    SendRejected { entry: LocalId },
    /// 受理不明を照合した結果、受理の痕跡がなかった。
    NotAcceptedAfterReconcile { entry: LocalId },
    /// 先頭項目の添付（コピー中・失敗・欠損など）を送信時に使えず、送らずに止めた。送信は試みていない。
    AttachmentUnavailable { entry: LocalId, name: String },
    /// 強制終了でApp Serverのプロセスが消えたため、送信済みのturnの終端を確認できないまま止めた（完了扱いにしない）。
    AppServerKilled { turn: TurnKey },
}

/// 保留の対象（表示用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct HoldTarget {
    pub agent: AgentKey,
    pub state: AgentState,
    pub freshness: Freshness,
}

/// 自動送信を待っている理由。対象と理由を表示する（§3.7）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum QueueHold {
    /// 親が作業中（通常の待ち）。
    ParentWorking,
    /// 子・孫に作業中・待機中がある。
    DescendantsActive { targets: Vec<HoldTarget> },
    /// 状態不明（親の終端が未確認・子孫の走査が未完了を含む）。
    StateUnknown { targets: Vec<HoldTarget>, descendant_scan_incomplete: bool },
    /// 鮮度が live でない（wake後・切断・履歴のみ）。
    NotLive { targets: Vec<HoldTarget> },
    StopUnconfirmed { record: LocalId },
    AcceptanceUnknown { attempt: LocalId },
    DeletePending,
    Disconnected,
    /// 完全終了の手順中のため、新しい送信を止めている。
    Quitting,
    /// 送信前の保存に失敗したため、送っていない。保存が成功するまで保留する。
    SaveFailed,
}

/// キュー項目の状態。保留（Hold）は項目ではなくキュー全体に付く。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum QueueEntryState {
    /// 送信待ち。編集・取消できる。
    Waiting,
    Sending { attempt: LocalId },
    /// 受理不明。照合で確定するまで次へ進まず、再送しない。
    AcceptanceUnknown { attempt: LocalId },
    Sent { turn: TurnKey },
    /// 受理なしが確定（拒否・照合で痕跡なし）。自動再送しない。ユーザーは再送（`retry_send`）か取消を選ぶ。
    NotAccepted { attempt: LocalId, message: String },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct QueueEntry {
    pub id: LocalId,
    pub chat: ChatKey,
    pub text: String,
    pub attachments: Vec<LocalId>,
    /// 登録順（チャット内で単調増加）。
    #[cfg_attr(test, ts(type = "number"))]
    pub order: u64,
    pub registered_at: UnixMillis,
    pub state: QueueEntryState,
    pub attempts: Vec<SendAttempt>,
    /// 送信時点で決めた設定。未送信なら None。
    pub applied: Option<AppliedSettings>,
}

/// チャット1件分のキュー（UIへ出す形・保存する形の共通部分）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ChatQueue {
    pub chat: ChatKey,
    pub run: QueueRun,
    /// 現在の保留理由（ホストが評価のたびに更新。保存しない）。
    pub hold: Option<QueueHold>,
    /// 失敗・中断を監視する基準時刻（待っている親turnの開始観測時刻）。これより前の終端では止めない。
    pub baseline_at: Option<UnixMillis>,
    /// 自動送信して終端をまだ確認していないturn。確認できるまで次の依頼を送らない。
    /// 失敗・中断で終わればキューを止める。明示的な「キューを再開」で外す。
    #[serde(default)]
    pub awaiting: Option<TurnKey>,
    pub entries: Vec<QueueEntry>,
    #[cfg_attr(test, ts(type = "number"))]
    pub next_order: u64,
}

// ───────────────────────────── 通知・窓・常駐（§3.11） ─────────────────────────────

/// 通知設定（M38）。初期値はすべてオン、チャット名を表示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct NotificationSettings {
    pub enabled: bool,
    pub approval_and_question: bool,
    pub completed: bool,
    pub failed: bool,
    pub sound: bool,
    pub show_chat_name: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        NotificationSettings { enabled: true, approval_and_question: true, completed: true, failed: true, sound: true, show_chat_name: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum WindowKind {
    Main,
    /// コンパクト監視窓（別Tauri窓）。
    Monitor,
}

/// コンパクト監視窓の表示範囲（初期値は選択中チャット）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum MonitorWindowScope {
    #[default]
    SelectedChat,
    AllChats,
}

/// 物理ピクセルの窓位置・サイズ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct WindowBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct WindowPrefs {
    /// 最前面（窓ごとに独立、初期値オフ、M37）。
    pub always_on_top: bool,
    pub bounds: Option<WindowBounds>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum SendKey {
    CtrlEnter,
    Enter,
}

/// アプリ設定。`%LOCALAPPDATA%` 配下の専用領域に保存する。`~/.codex` には書かない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// codex.exe のパス。None＝PATH（§11の推奨）。
    pub codex_executable: Option<String>,
    /// Windowsログイン時に起動（初期値オフ）。オンならトレイ格納で起動する。
    pub autostart: bool,
    pub notifications: NotificationSettings,
    pub main_window: WindowPrefs,
    pub monitor_window: WindowPrefs,
    pub monitor_scope: MonitorWindowScope,
    /// 新しいチャットの既定モデル（既存チャットに波及させない）。
    pub default_model: Option<ModelChoice>,
    pub send_key: SendKey,
}

impl Default for AppSettings {
    /// 初期値（通知はすべてオン・自動起動オフ・最前面オフ・Ctrl+Enterで送信）。
    fn default() -> Self {
        AppSettings {
            codex_executable: None,
            autostart: false,
            notifications: NotificationSettings::default(),
            main_window: WindowPrefs::default(),
            monitor_window: WindowPrefs::default(),
            monitor_scope: MonitorWindowScope::default(),
            default_model: None,
            send_key: SendKey::CtrlEnter,
        }
    }
}

// ───────────────────────────── 終了・強制終了（§3.4・§3.11） ─────────────────────────────

/// 完全終了の進行。閉じる操作（トレイ格納）では使わない（M26）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum QuitPhase {
    Idle,
    /// 作業中のチャットを示して「終了を取り消す／作業を中断して終了」を選ばせる。
    Confirming { busy: Vec<ChatKey> },
    /// 中断要求を出し、停止と保存を照合している。
    Stopping { records: Vec<LocalId>, started_at: UnixMillis },
    /// 10秒で停止を確認できない対象がある。「待つ／中断を再試行／終了を取り消す」と強制終了を出す（D04）。
    StopUnconfirmed { records: Vec<LocalId> },
    /// 保存の書き出し中。
    Flushing,
    /// 保存失敗。正常終了と表示しない（D06）。
    SaveFailed { failed: Vec<SaveScope> },
}

/// 強制終了の確認内容。App Server（Job Object）単位で終了するため、同じ監視元の全チャットが影響を受ける。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ForceKillPreview {
    pub source: SourceId,
    /// 影響を受けるチャット（作業中・停止未確認）。
    pub affected: Vec<ChatKey>,
    /// Job内で稼働中のプロセス数（取得できなければ NotFetched）。
    pub active_processes: Known<u32>,
}

// ───────────────────────────── 使用量（§3.12、D02） ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct UsageBreakdown {
    #[cfg_attr(test, ts(type = "number"))]
    pub attachments: u64,
    #[cfg_attr(test, ts(type = "number"))]
    pub artifacts: u64,
    /// 一般チャットの作業領域。
    #[cfg_attr(test, ts(type = "number"))]
    pub workspace: u64,
    #[cfg_attr(test, ts(type = "number"))]
    pub activity: u64,
    /// 設定・台帳などのJSON。
    #[cfg_attr(test, ts(type = "number"))]
    pub metadata: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ChatUsage {
    pub chat: ChatKey,
    pub breakdown: UsageBreakdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub total: UsageBreakdown,
    pub chats: Vec<ChatUsage>,
    /// 段階①で `%APPDATA%` 側に作った一般チャットの作業領域（旧領域）。移動していないので別に数える（合計には含めない）。
    #[cfg_attr(test, ts(type = "number"))]
    pub legacy_area: u64,
    /// 専用領域のあるドライブの空き。
    #[cfg_attr(test, ts(as = "Known<u32>"))]
    pub free_space: Known<u64>,
    pub measured_at: UnixMillis,
    /// 読めなかったパス（合計に含まれていない。0で代用しない）。
    pub unreadable: Vec<String>,
}

// ───────────────────────────── 削除・エクスポート（§3.6・§3.12） ─────────────────────────────

/// 削除確認に表示する内容（§3.6、§27）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct DeletePreview {
    pub chat: ChatKey,
    /// true＝Codexの履歴を削除する。外部作成の会話は false（一覧から外すだけ）。
    pub deletes_backend_history: bool,
    pub descendants: Known<u32>,
    pub attachments: u32,
    pub artifacts_in_chat_area: u32,
    #[cfg_attr(test, ts(as = "Known<u32>"))]
    pub chat_area_bytes: Known<u64>,
    /// 作業中なら、中断して停止を確認してから削除する（停止未確認なら保留）。
    pub requires_stop: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DeleteOutcome {
    Deleted,
    /// 停止未確認などで保留した。
    Pending { pending: DeletePending },
    Partial { done: Vec<String>, failed: Vec<String> },
}

// ───────────────────────────── IPC（P2〜P7で追加するコマンド） ─────────────────────────────

/// 追加コマンド名。統合時に `ipc::command_names` へ移す（P2分・P3分・P6分は移動済み）。
pub mod local_command_names {
    pub const ACKNOWLEDGE_FAILURE: &str = "acknowledge_failure";
    pub const SET_SELECTED_CHAT: &str = "set_selected_chat";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ChatArgs {
    pub chat: ChatKey,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SetAppSettingsArgs {
    pub settings: AppSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct RetrySaveArgs {
    pub scope: SaveScope,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SetDraftArgs {
    pub chat: ChatKey,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct AddAttachmentFileArgs {
    pub chat: ChatKey,
    /// ファイル選択・ドロップで得たパス。フォルダなら `Blocked{FolderIsWorkspace}`（コピーしない）。
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct AttachmentArgs {
    pub chat: ChatKey,
    pub attachment: LocalId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct OpenFileArgs {
    pub target: FileRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SaveFileAsArgs {
    pub target: FileRef,
    /// 保存ダイアログで選んだパス。
    pub dest: String,
    /// 同名ファイルがあるとき、UIの確認で上書きを選んだか。false なら `Blocked{TargetExists}`。
    pub overwrite_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct EnqueueArgs {
    pub chat: ChatKey,
    pub text: String,
    pub attachments: Vec<LocalId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct EditQueueEntryArgs {
    pub chat: ChatKey,
    pub entry: LocalId,
    pub text: String,
    pub attachments: Vec<LocalId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct QueueEntryArgs {
    pub chat: ChatKey,
    pub entry: LocalId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ReconcileSendArgs {
    pub chat: ChatKey,
    pub attempt: LocalId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SetChatPermissionArgs {
    pub chat: ChatKey,
    pub permission: PermissionPreset,
}

/// 設定変更が送信待ちの依頼に及ぶ範囲（§3.7「設定変更時に待機依頼への影響を表示」）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SettingsImpact {
    pub local: ChatLocalView,
    pub affected_entries: Vec<LocalId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SetChatCwdArgs {
    pub chat: ChatKey,
    pub cwd: String,
    /// 送信待ちがあるとき、新しいフォルダが対象になることをユーザーが確認したか（M44）。
    pub queue_retarget_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct AcknowledgeFailureArgs {
    pub chat: ChatKey,
    /// None＝チャット内の未確認の失敗すべて。
    pub agent: Option<AgentKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SetSelectedChatArgs {
    pub chat: Option<ChatKey>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ExportMarkdownArgs {
    pub chat: ChatKey,
    /// 「監視活動履歴も含める」（子・孫を含む取得済み活動、M41）。
    pub include_monitor_activity: bool,
    pub dest: String,
    pub overwrite_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct GetUsageArgs {
    /// None＝全体と全チャット。
    pub chat: Option<ChatKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub enum QuitDecision {
    /// 終了を取り消す。
    Cancel,
    /// 作業を中断して終了（停止と保存を照合してから終了）。
    StopAndQuit,
    /// 停止未確認のまま待つ。
    Wait,
    /// 停止未確認の対象へ中断を再試行する。
    RetryInterrupt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct QuitDecisionArgs {
    pub decision: QuitDecision,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ForceKillArgs {
    pub source: SourceId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SetAlwaysOnTopArgs {
    pub window: WindowKind,
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct SetMonitorWindowScopeArgs {
    pub scope: MonitorWindowScope,
}

/// 通常画面を前面に出す（監視窓の「通常画面で開く」。承認・質問への回答は通常画面で行う）。`chat` があればそのチャットを開く（表示だけ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct ShowMainWindowArgs {
    pub chat: Option<ChatKey>,
}

/// `IpcError.blocked` に追加する理由。統合時に `ipc::BlockedReason` へ移す（P2分・P3分は移動済み）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LocalBlockedReason {
    DeletePending,
}

/// 追加のホスト→UIイベント。統合時に `ipc::HostEvent` の variant へ移す（seqは共通。P2分・P3分は移動済み）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LocalHostEvent {
    /// トレイ・通知のクリックで、通常画面にこのチャットを表示する（回答・再実行はしない）。
    NavigateToChat { chat: ChatKey },
    UsageUpdated { report: UsageReport },
}
