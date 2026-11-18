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
    /// ペット窓の位置。古い `windows.json` にはない。
    #[serde(default)]
    pub pet: Option<WindowBounds>,
}

/// worktree台帳（ルートの `worktrees.json`、P3-7）。AgentDockが作った（作ろうとした）ものだけ。外部のworktreeは持たない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreesFile {
    pub schema_version: u32,
    pub worktrees: Vec<WorktreeRecord>,
}

/// クラウド委任の記録（ルートの `cloud-tasks.json`、P3-6）。チャットに属さない操作なので、起点チャットは参照として持つ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudTasksFile {
    pub schema_version: u32,
    pub tasks: Vec<CloudTaskRecord>,
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
    // ── 作業フォルダ（複数、`app/DESIGN_WORKFOLDERS.md`。すべて既定値あり。SCHEMA_VERSION は上げない） ──
    /// 次の送信から使う作業フォルダの並び（先頭＝プライマリ）。None＝変更なし。`next_cwd` と併存し、あればこちらを優先する。
    #[serde(default)]
    pub next_folders: Option<Vec<String>>,
    /// AgentDockが送って受理を確認した作業フォルダの記録（バックエンドの現在値とは断定しない）。
    #[serde(default)]
    pub accepted_folders: Option<AcceptedFolders>,
    /// 受理不明の送信に含まれた作業フォルダ（照合で受理と確定したら `accepted_folders` へ、受理なしなら捨てる）。
    #[serde(default)]
    pub unconfirmed_folders: Option<Vec<String>>,
    // ── 組織実行（DESIGN_ORG_P4 §3.5。既定値あり。SCHEMA_VERSION は上げない） ──
    /// 組織実行が作ったチャットの所有記録。所有はこの記録だけで判定する（推定しない）。
    #[serde(default)]
    pub org: Option<OrgBinding>,
    // ── 継続チャット（DESIGN_CONT §3。既定値あり。SCHEMA_VERSION は上げない） ──
    /// このチャット（thread）が会話の先頭として持つ会話の記録。None＝目的メモも引き継ぎもない普通のチャット。
    #[serde(default)]
    pub conversation: Option<ConversationRecord>,
    /// この thread が引き継がれた先。Some で、かつ引き継ぎ先のチャット領域が保存先にあるとき、左一覧から隠す。
    #[serde(default)]
    pub superseded_by: Option<Superseded>,
    /// 直近に観測した文脈の使用量（ルートのエージェントの値だけ）。表示用で、いつの値かと一緒に出す。
    #[serde(default)]
    pub context_usage: Option<ContextUsageSample>,
}

/// 会話の記録。将来 AgentDockプロジェクト（G-C）ができたら `purpose` をプロジェクトのゴール・メモへ移す（DESIGN_ORG_GRAPH §5.3）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRecord {
    /// 会話のID（最初に記録を作ったときに採番。引き継いでも変えない）。
    pub id: LocalId,
    /// 目的メモ。None＝未設定。
    #[serde(default)]
    pub purpose: Option<PurposeNote>,
    /// 以前の thread の列（古い順）。先頭（このチャット自身）は含めない。
    #[serde(default)]
    pub threads: Vec<ThreadSegment>,
    /// この thread へ引き継いだときの資料。最初の thread では None。
    #[serde(default)]
    pub seed: Option<HandoffSeed>,
    /// 目的メモをこの thread へ届けた版（受理を確認した送信に含めた版）。thread が替われば新しい先頭では None から始まる。
    #[serde(default)]
    pub purpose_delivered_rev: Option<u32>,
    /// 届けた後に、この thread で文脈の圧縮を観測したか（真なら次の手動送信で目的メモを再び添える）。
    #[serde(default)]
    pub compacted_since_delivery: bool,
    /// 空の先頭のやり直しで置き換えた、空の先頭（N）。起動時の照合が、N に印を書く前に落ちた状態を直すのに使う。通常は None。
    #[serde(default)]
    pub replaces: Option<ChatKey>,
    /// 組織実行の依頼整理（DESIGN_ORG_INTAKE §1.4）。None＝依頼整理をしていない。引き継ぎで新しい先頭へ写す。
    #[serde(default)]
    pub intake: Option<crate::backend::intake::IntakeRecord>,
    /// 受付係の印（DESIGN_ROUTER §3.1）。None＝受付係ではない。引き継ぎで新しい先頭へ写す。
    #[serde(default)]
    pub router: Option<RouterRecord>,
}

/// 受付係の記録（DESIGN_ROUTER §3.1）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterRecord {
    /// 固定の指示をこの thread へ届けた版。thread が替われば None から。
    #[serde(default)]
    pub instructions_delivered_rev: Option<u32>,
    /// ルール帳をこの thread へ届けた版。
    #[serde(default)]
    pub rules_delivered_rev: Option<u32>,
    /// 一覧の安定部分の指紋をこの thread へ届けた値。
    #[serde(default)]
    pub catalog_delivered_digest: Option<String>,
    /// 圧縮を観測したら真（次の送信で指示・ルール・一覧を全部添え直す）。
    #[serde(default)]
    pub compacted_since_delivery: bool,
    /// 振り分けの記録（新しい順、最大200件）。
    #[serde(default)]
    pub dispatches: Vec<Dispatch>,
    /// 送信ごとの一覧の連番とチャットの対応（新しい順、最大 [`ROUTER_CATALOGS_MAX`] 件）。返答の解析に使う。
    #[serde(default)]
    pub catalogs: Vec<SentCatalog>,
    /// 受付係の返答の解析結果（新しい順、最大 [`ROUTER_TURNS_MAX`] 件）。再起動後も画面が描き直せるように残す。
    #[serde(default)]
    pub turns: Vec<RouterTurnRecord>,
}

/// 送った一覧の連番とチャットの対応を残す件数の上限。
pub const ROUTER_CATALOGS_MAX: usize = 20;
/// 振り分けの記録の上限。
pub const ROUTER_DISPATCHES_MAX: usize = 200;
/// 返答の解析結果を残す件数の上限。
pub const ROUTER_TURNS_MAX: usize = 50;

impl RouterRecord {
    /// 一覧の対応を先頭へ足し、古いものを捨てる（同じ送信IDは置き換える）。
    pub fn push_catalog(&mut self, c: SentCatalog) {
        self.catalogs.retain(|x| x.client_message_id != c.client_message_id);
        self.catalogs.insert(0, c);
        self.catalogs.truncate(ROUTER_CATALOGS_MAX);
    }
    /// 振り分けの記録を先頭へ足し、上限を超えた古いものを捨てる。
    pub fn push_dispatch(&mut self, d: Dispatch) {
        self.dispatches.insert(0, d);
        self.dispatches.truncate(ROUTER_DISPATCHES_MAX);
    }
    /// 返答の解析結果を先頭へ足し、古いものを捨てる（同じ turn は置き換える）。
    pub fn push_turn(&mut self, t: RouterTurnRecord) {
        self.turns.retain(|x| x.turn != t.turn);
        self.turns.insert(0, t);
        self.turns.truncate(ROUTER_TURNS_MAX);
    }
}

/// 受付係の1つの返答（turn）の解析結果。画面が返答文・ルール更新の行を描くのに使う。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterTurnRecord {
    pub turn: String,
    /// 解析できた返答文。解析できなければ None（原文は通常の本文として見せる）。
    pub reply: Option<String>,
    /// 解析できなかった理由。
    #[serde(default)]
    pub parse_error: Option<String>,
    /// 検査で落とした振り分け案の理由。
    #[serde(default)]
    pub route_rejects: Vec<String>,
    #[serde(default)]
    pub notices: Vec<RuleNoticeRecord>,
}

/// 返答の下に出すルール更新の1行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleNoticeRecord {
    /// 書いたときの `rules.json` の版。書かなかった（理由の表示）ときは None。
    pub revision: Option<u32>,
    pub text: String,
}

/// ある送信に添えた一覧の連番（c1, c2…）とチャットの対応。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SentCatalog {
    pub client_message_id: String,
    /// （連番, チャット）。
    pub entries: Vec<(String, ChatKey)>,
    pub at: UnixMillis,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dispatch {
    pub id: LocalId,
    /// どの受付係の返答（turn）から出た案か。
    pub source_turn: String,
    pub route_index: u32,
    pub target: DispatchTarget,
    /// 受付係の案（利用者が送り先を変えたら target と異なる）。
    pub suggested: DispatchTarget,
    pub state: DispatchState,
    /// 次の受付係への送信で「前回の振り分け結果」として伝えたか。
    #[serde(default)]
    pub reported: bool,
    /// 受付係の案の中身（送る本文・理由・新規チャットの内容）。古い記録にはない。
    #[serde(default)]
    pub proposal: Option<DispatchProposal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct DispatchProposal {
    pub text: String,
    pub text_is_original: bool,
    pub reason: String,
    /// "high" | "medium" | "low"。
    pub confidence: String,
    /// 新規チャットの案（送り先が新規のとき）。
    pub new_chat: Option<NewChatProposal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(rename_all = "camelCase")]
pub struct NewChatProposal {
    pub title: String,
    pub folder: String,
    pub purpose: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DispatchTarget {
    Existing { chat: ChatKey },
    /// 新規チャット。作成できたら chat が入る。
    New { chat: Option<ChatKey> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DispatchState {
    Proposed,
    Sent { at: UnixMillis, client_message_id: String },
    /// 受理不明（送れたか確認できない）。自動再送しない。
    Unconfirmed { at: UnixMillis, client_message_id: String },
    Dismissed { at: UnixMillis },
}

/// 目的メモ（利用者が書く固定文）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurposeNote {
    pub text: String,
    /// 保存のたびに1増やす（届けたかの判定と、同時編集の検出に使う）。
    pub revision: u32,
    pub updated_at: UnixMillis,
    /// 由来。最小版は常に Chat。
    #[serde(default)]
    pub source: PurposeSource,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PurposeSource {
    #[default]
    Chat,
}

/// 以前の thread 1本。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSegment {
    pub chat: ChatKey,
    /// 引き継いだ時刻（この thread が先頭でなくなった時刻）。
    pub handed_off_at: UnixMillis,
    /// 引き継ぎ時点の表示名（一覧から隠れた後の区切り線に出す）。
    #[serde(default)]
    pub title: Option<String>,
}

/// 引き継がれた側に付ける印。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Superseded {
    pub by: ChatKey,
    pub at: UnixMillis,
}

/// 新しい先頭へ入れた引き継ぎ資料。再起動後も同じ資料で送り直せるよう、組み立てた本文をそのまま持つ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffSeed {
    /// 引き継ぎ元。
    pub from: ChatKey,
    /// 組み立てた資料の本文（目的メモは含めない。目的メモは送るときの版を使う）。
    pub material: String,
    pub rounds_included: u32,
    pub truncated: bool,
    pub created_at: UnixMillis,
    pub state: SeedState,
    /// 資料に入れた目次の往復数。None＝目次のない資料（台帳導入前に作った資料）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toc_rounds: Option<u32>,
    /// 書き出せた会話台帳のパス。None＝台帳なし（書けなかった・導入前の資料）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SeedState {
    /// まだ受理を確認していない（次の手動送信に資料を付ける）。受理不明もここ（照合か履歴で確定する）。
    Pending,
    /// 資料を含む送信の受理を確認した、または履歴で資料つきの依頼を観測した。
    Delivered,
}

/// 文脈の使用量の観測値。推定値は持たない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsageSample {
    /// 直近のモデル呼び出しのトークン数（`last.totalTokens`）。
    pub used: u64,
    /// 文脈の上限。バックエンドが示さなければ None（割合は出さない）。
    pub window: Option<u64>,
    pub observed_at: UnixMillis,
    pub turn: Option<ExternalId>,
    /// この値の後で文脈の圧縮を観測した（次の値が来るまで割合を出さない）。
    #[serde(default)]
    pub stale_after_compaction: bool,
}

/// 組織実行が作ったチャットの所有記録。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgBinding {
    pub workflow_id: String,
    pub task_id: String,
    pub role_id: String,
    /// 「手動に切り替える」を押した時刻。Some なら通常チャットとして送信できる（エンジンは二度と送らない）。
    #[serde(default)]
    pub released_at: Option<UnixMillis>,
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
            next_folders: None,
            accepted_folders: None,
            unconfirmed_folders: None,
            org: None,
            conversation: None,
            superseded_by: None,
            context_usage: None,
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
        f.fork_of = Some(ForkOrigin { chat: key(), through_turn: Some(ExternalId("turn-2".into())), purpose: ForkPurpose::default() });
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

    /// 継続チャットの追加フィールドを持たない旧ファイルが読め、追加分は既定値になる。
    #[test]
    fn chat_local_without_continuity_fields_still_reads() {
        let mut v: serde_json::Value = serde_json::from_slice(&to_bytes(&sample_local()).unwrap()).unwrap();
        let o = v.as_object_mut().unwrap();
        for k in ["conversation", "supersededBy", "contextUsage"] {
            assert!(o.remove(k).is_some(), "{k}");
        }
        let back: ChatLocalFile = parse_versioned(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(back, sample_local());
        assert!(back.conversation.is_none() && back.superseded_by.is_none() && back.context_usage.is_none());
    }

    #[test]
    fn continuity_fields_round_trip() {
        let mut f = sample_local();
        f.conversation = Some(ConversationRecord {
            id: LocalId("c1".into()),
            purpose: Some(PurposeNote { text: "目的".into(), revision: 3, updated_at: UnixMillis(4), source: PurposeSource::Chat }),
            threads: vec![ThreadSegment { chat: key(), handed_off_at: UnixMillis(5), title: Some("前".into()) }],
            seed: Some(HandoffSeed {
                from: key(),
                material: "資料".into(),
                rounds_included: 3,
                truncated: true,
                created_at: UnixMillis(6),
                state: SeedState::Pending,
                toc_rounds: Some(4),
                ledger: Some("x/会話台帳.md".into()),
            }),
            purpose_delivered_rev: Some(2),
            compacted_since_delivery: true,
            replaces: Some(key()),
            intake: None,
            router: None,
        });
        f.superseded_by = Some(Superseded { by: key(), at: UnixMillis(7) });
        f.context_usage = Some(ContextUsageSample { used: 12_345, window: Some(200_000), observed_at: UnixMillis(8), turn: Some(ExternalId("t".into())), stale_after_compaction: true });
        let bytes = to_bytes(&f).unwrap();
        let back: ChatLocalFile = parse_versioned(&bytes).unwrap();
        assert_eq!(back, f);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["schemaVersion"], 1);
        assert_eq!(v["conversation"]["seed"]["state"], "pending");
        assert_eq!(v["conversation"]["threads"][0]["handedOffAt"], 5);
        assert_eq!(v["contextUsage"]["staleAfterCompaction"], true);
        // 内側の項目が欠けた古い形（purposeだけ）も読める。
        let min: ConversationRecord = serde_json::from_str(r#"{"id":"c"}"#).unwrap();
        assert!(min.purpose.is_none() && min.threads.is_empty() && min.seed.is_none() && !min.compacted_since_delivery);
        assert!(min.router.is_none(), "router を持たない古い記録も読める");
    }

    #[test]
    fn router_record_round_trips_and_caps_catalogs_and_dispatches() {
        let d = |i: u32| Dispatch {
            id: LocalId(format!("d{i}")),
            source_turn: "t".into(),
            route_index: i,
            target: DispatchTarget::New { chat: None },
            suggested: DispatchTarget::Existing { chat: key() },
            state: DispatchState::Sent { at: UnixMillis(1), client_message_id: "m".into() },
            reported: false,
            proposal: None,
        };
        let mut r = RouterRecord::default();
        for i in 0..25 {
            r.push_catalog(SentCatalog { client_message_id: format!("m{i}"), entries: vec![("c1".into(), key())], at: UnixMillis(i as i64) });
        }
        r.push_catalog(SentCatalog { client_message_id: "m24".into(), entries: vec![], at: UnixMillis(99) });
        assert_eq!((r.catalogs.len(), r.catalogs[0].client_message_id.as_str(), r.catalogs[0].at), (ROUTER_CATALOGS_MAX, "m24", UnixMillis(99)));
        for i in 0..210 {
            r.push_dispatch(d(i));
        }
        assert_eq!((r.dispatches.len(), r.dispatches[0].route_index), (ROUTER_DISPATCHES_MAX, 209));
        let back: RouterRecord = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
        assert_eq!(back, r);
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["dispatches"][0]["state"]["kind"], "sent");
        assert_eq!(v["dispatches"][0]["target"]["kind"], "new");
        let empty: RouterRecord = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, RouterRecord::default());
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
