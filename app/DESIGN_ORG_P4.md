# DESIGN_ORG_P4: P-ORG-4 実装指示書（ホストのポート・OrgGrant・組織チャットの保護・sandbox確認・trust照合）

- 作成: 2026-10-17（設計担当）。入力: `app/DESIGN_ORG.md` v0.4（§1・§3.2・§4.1・§4.7・§4.10・§5.2・§5.4）、`app/org-engine/src/*`、`app/src-tauri/src/{backend,host,codex,rules,store}/*`、schema 0.162.1（`TurnStartParams`・`ThreadStartParams`・`CommandExecParams`・`WindowsSandboxReadinessResponse`・`ThreadShellCommandParams`）。
- 位置付け: DESIGN_ORG を実コードに合わせて具体化したもの。食い違いは §1 で解決し、DESIGN_ORG の該当箇所は本書を優先する（次の版更新で DESIGN_ORG へ戻す）。
- 表記: 【変更】＝既存コードの変更、【新規】＝新しいファイル・型。関数名は現時点のもの。

---

## 0. 結論（要点）

1. P-ORG-4 は3つに分ける: **4a** エンジン中核（sans-IO、純ロジック）、**4b** ホストのポート・`OrgGrant`・組織チャットの保護、**4c** sandbox確認・起動オプション・trust照合。4a→4b、4c は並行可。
2. エンジンは async・I/O を持たない（P-ORG-2の方針を維持）。`OrgCore::handle(OrgInput) -> Vec<OrgAction>` の **sans-IO** 型にし、ポートtrait（`AgentRuntime` 等）は**ホスト側**に置く。DESIGN_ORG §1.1 の「`OrgCommand`／`OrgEvent`」は `OrgInput`／`OrgAction` に改名する（`OrgEvent` は P-ORG-3 で events.jsonl の行の名前に使用済み）。
3. 送信の型保証: `OrgGrant`（非Clone・非Serialize・private構築子）は `UserConfirmed` を**値で消費**してだけ作れ、`OrgDriver`（ホストの専用タスク）だけが持つ。送信は `DispatchPermit`（`OrgGrant::permit` からだけ作れ、1回で消費）を要求する `Host::org_send` だけ。`Host` にも監視・照合のコードにも `OrgGrant` を置かない。
4. エンジンのポートに **interrupt・respond・resume は入れない**。中断は利用者の既存 `interrupt_chat` から、承認は既存の承認カードから、resume は利用者操作の中だけ（§1 D6）。
5. `Task.status/confirm/rework_count` と `WorkflowState.tasks` 等を crate 外から書けなくし、変更は `WorkflowState::record(OrgEventKind)`（遷移検査＋イベント生成）だけにする。これで「全変更が events.jsonl に残る」も同時に保証する。

---

## 1. 設計書と実コードの食い違い・衝突と解決

|#|食い違い・衝突|解決（本書の決定）|
|---|---|---|
|D1|DESIGN_ORG §1.1・§1.2 はエンジンがポートを呼ぶ形。`org-engine` は「no I/O, no async」で、ポートもランナーもまだ無い|sans-IO にする（§2）。ポートはホスト側 `host/org/ports.rs`。直列化可能な `OrgInput`／`OrgAction` で将来の別プロセス化も満たす|
|D2|名前の衝突: `org_engine::OrgEvent` は events.jsonl の1行（P-ORG-3）|エンジン↔ホストのメッセージは `OrgInput`／`OrgAction`、UIへのイベントは `orgUpdated`（P-ORG-7）|
|D3|§5.2 のタスク表に**オーケストレーション本体**（Planner→計画検証→承認→Worker→コミット→Reviewer→判定、指示文の組立て、出力スキーマ、成果物検証）を担当するタスクが無い|P-ORG-4a に入れる（純ロジックなのでテストできる）|
|D4|`Workspace` ポート（worktree作成・コミット・差分）の**ホスト実装の担当**が無い（P-ORG-4はAgentRuntime/RuntimeEventsだけ、P-ORG-5はgitops関数だけ）|P-ORG-5 に「`Workspace` ポートのホスト実装」を追加する。4b の `OrgDriver` は `OrgAction::Workspace*` を受けたら P-ORG-5 完了まで `Unsupported` を返して `blocked{要対応}` にする|
|D5|events.jsonl にタスクとチャットの結び付き・送信の識別（`client_message_id`）が無い。再起動後に「どのチャットのどの送信を照合するか」を復元できない|`OrgEventKind` に `ChatRequested`・`ChatBound`・`Dispatched`・`DispatchSettled`・`TurnObserved` を追加（§2.2）。`SCHEMA_VERSION` は 1 のまま（未リリース・保存データなし）|
|D6|§1.3「resumeは送信直前の `ensure_live` に限る」。しかし `ensure_live` は `&UserConfirmed` を要し、エンジンの自動送信から resume すると既存合意「wake後の自動resumeなし」「キューの自動送信は親live必須」と食い違う|**組織の自動送信は live 必須**（キューと同じ）。live でなければ送らず、ワークフローを `halt=NeedsResume` で止める。resume は利用者の「ワークフローを再開」（P-ORG-7のIPC、`UserConfirmed`あり）の中で、送信対象のチャットにだけ `ensure_live` を行う。新しい役割チャットは `thread/start` 直後なので live。→ **U1 確定（2026-10-17、ユーザー）**。設計書 §1.3 の「送信直前に resume」より厳しい側に倒す|
|D7|§1.2 `AgentRuntime` に「利用者操作による停止」が入っている|エンジンのポートから外す。利用者の中断は既存 `Host::interrupt_chat`（`UserConfirmed`）で行い、ホストが `OrgInput::InterruptRequested` をエンジンへ知らせるだけ|
|D8|`UserConfirmed::from_user_command()` は `pub(crate)` で、IPC以外（`host/mod.rs:729` の名前付け、`host/changes.rs:1640`）でも作られている。型だけでは「利用者操作」を保証できていない|`OrgGrant` は `UserConfirmed` を**値で消費**して作る（参照では作れない）。加えて受入条件で `host/org/` 配下に `from_user_command` が現れないことを grep で確認する（§3.7）|
|D9|`check_sendable`（`host/mod.rs:853`）は利用者の送信と組織の送信の両方が使う前提になる|`check_sendable_core`（現在の中身）と `check_not_org_owned` に分割。利用者経路は両方、`org_send` は core だけ（§3.4）|
|D10|§4.10 は健全性チェックを「計画承認後・最初のWorker/Reviewer送信前」に置く。Planner も sandbox 内でコマンドを使い、競合は `thread/start` 時の MCP 起動で起きる（§5.5）|**Planner の `thread/start` 直後・最初の `turn/start` 前**に移す（App Server の起動単位で1回、`SourceId` が変わったら再確認）。計画の利用枠を無駄にしない（§5）|
|D11|健全性チェックの手段が未定。`thread/shellCommand` は schema に「unsandboxed with full access」と明記|`thread/shellCommand` は**使わない**。`windowsSandbox/readiness`（読取り）＋`command/exec`（`sandboxPolicy: readOnly`）を使う（§5.1）|
|D12|`thread/tokenUsage/updated` は `codex/events.rs` で捨てている（テスト `events.rs:698` が空を期待）|中立イベント `BackendEvent::TokenUsage` を追加（§3.6）。テストを書き換える|
|D13|`SendRequest` に `outputSchema` が無い（§5.4 で毎turn指定に確定済み）|`SendRequest.output_schema: Option<serde_json::Value>` を追加し `build_send_call` で `outputSchema` に写す。構築箇所（5ファイル10か所）は `None`|
|D14|config.toml の前後照合は CLI 管理操作（`manage_extension`）専用で、`thread/start` を囲んでいない。`config/read` に `projects` が現れる保証がなく、toml crate も無い|config.toml の**本文を読むだけ**（保存しない）の行差分で「trust追記だけ」を判定する純関数を `rules/org.rs` に置き、組織の役割チャット作成を前後で囲む（§6）|
|D15|`start_chat` の timeout は `OutcomeUnknown`。`thread/start` には照合用IDが無く、作られたかもしれないthreadを特定できない|`ChatRequested{cwd}` を先に記録→作成→`chat.json` 保存→`ChatBound`。結果不明は `blocked{要対応}`＋確認状態 `SendUnknown`。照合は `list_chats(cwd, include_external)` の読取りだけで、**候補が1件でも自動で結び付けない**（推定しない）。利用者が「このチャットを使う／作り直す／取り消す」を選ぶ（UIはP-ORG-7）|
|D16|§5.4 の実測は `approvalPolicy=never`。本番の `PermissionPreset` は `ReadOnly`／`WorkspaceWriteOnRequest` とも `on-request`（`codex/convert.rs:692`）|そのまま（on-request）。組織チャットでも承認要求は出得て、既存の承認カードで利用者が答える。`thread/start` に `approvalsReviewer` は**指定しない**|
|D17|`dispatch_send_as` は受理時に `touch_chat_used`（利用者の利用として最近利用時刻を更新）する|`org_send` 経由では呼ばない（`SendAuth::Org` のとき省く）|
|D18|業務完了の条件「子孫に作業中なし」で、状態不明の子孫をどう扱うかが未記載|既存の送信条件（2026-10-12合意: 親が live で完了確認済みなら不明の子は止めず注記）に揃える。注記はタスクの記録に「子N件の状態は未確認」と残す。作業中の子がいれば `running` のまま待つ|

---

## 2. P-ORG-4a: エンジン中核（`app/org-engine/`、純ロジック）

### 2.1 フィールドの保護（「transition経由」の強制）【変更】

- `types.rs` `Task`: `status`・`confirm`・`rework_count` を `pub(crate)` にし、読取り用 `status()`・`confirm()`・`rework_count()` を足す（serde は private でも動く）。
- `state.rs` `Task::transition`・`Task::set_confirm` を `pub(crate)` にする。
- `persist.rs` `WorkflowState`: `tasks`・`counters`・`halt`・`limits`・`last_seq` を `pub(crate)`、読取り用 getter（`tasks() -> &[Task]` 等）を足す。`workflow_id`・`schema_version` は読取りだけ。
- crate 外の唯一の変更経路は `WorkflowState::record(kind, at_ms) -> Result<OrgEvent, ApplyError>`（検査付き適用＋追記すべき行を返す）と `classify_after_restart`／`recover_on_restart`。
- テストでの直接代入（`scheduler.rs:113,152-162`、`state.rs:140,202`、`persist.rs:562-564`）は同じ crate 内なので `pub(crate)` で通る。
- 確認: ホスト側に `.status =`・`.confirm =`・`.tasks.push` が書けない（コンパイルエラーになる）。

### 2.2 イベントの追加（`persist.rs` `OrgEventKind`）【変更】

不透明な参照型を `types.rs` に追加（中身はホストだけが解釈。エンジンは比較・保存だけ）:

```rust
id_type!(ChatRef);    // ホストの ChatKey を可逆に文字列化したもの（例 "codex:<id>"）
id_type!(TurnRef);    // turn の中立ID
id_type!(AttemptRef); // ホストの送信試行ID（LocalId）
```

追加する `OrgEventKind`（すべて `apply` で検査。未知タスクは `UnknownTask`）:

|イベント|意味|apply の検査|
|---|---|---|
|`ChatRequested { task, cwd: String }`|役割チャットの作成を始める（結果不明でも記録が残る）|同じタスクに未結び付けの `ChatRequested` が既にあれば拒否|
|`ChatBound { task, chat: ChatRef }`|作成に成功し `chat.json.org` を保存できた|1タスク1チャット。別チャットへの付け替えは拒否|
|`Dispatched { task, attempt: AttemptRef, client_message_id: String, kind: TurnKind }`|送信前に記録（**送る前に追記**）。`TurnSpent` も同時に記録|タスクが `ChatBound` 済み|
|`DispatchSettled { task, attempt, outcome: SettleOutcome }`|`Accepted{turn}`／`Rejected`／`Unknown`／`ReconciledAccepted{turn}`／`ReconciledNotFound`|対応する `Dispatched` がある|
|`TurnObserved { task, turn: TurnRef, end: TurnEndKind }`|liveで観測した終端（`Completed`／`Failed{usage_limit: bool}`／`Interrupted`）|`turn` が直近の受理turnと一致しなければ記録だけして状態は変えない（後着の古いturn）|

`HaltReason` に `NeedsResume`（live でないチャットへの送信が要る、D6）と `SandboxUnhealthy`（§5）を追加。どちらも利用者の「再開」でだけ外す。

`Task` に `chat: Option<ChatRef>`・`last_attempt: Option<AttemptRef>`・`accepted_turn: Option<TurnRef>` を `#[serde(default)]` で追加（上のイベントの apply だけが書く）。

### 2.3 sans-IO 中核【新規 `app/org-engine/src/core.rs`・`schemas.rs`・`prompt.rs`】

```rust
/// ホスト→エンジン。すべて直列化可能。到着順は保証しない（turn は TurnRef で照合）。
pub enum OrgInput {
    Start { workflow: WorkflowId, request: String, repo: String, limits: Limits }, // grant はホストが持つ
    UserResume, UserPause, UserCancelTask { task: TaskId }, UserManual { task: TaskId },
    UserAcceptManual { task: TaskId }, UserApprovePlan, UserReplan, UserRaiseTurnLimit { to: u32 },
    ChatCreated { task: TaskId, result: CreateResult },          // Ok(ChatRef) / Rejected / Unknown
    SendSettled { task: TaskId, attempt: AttemptRef, outcome: SettleOutcome },
    TurnEnded { chat: ChatRef, turn: TurnRef, end: TurnEndKind },
    FinalMessage { task: TaskId, turn: TurnRef, text: Option<String> }, // 読めなければ None（未取得）
    Descendants { chat: ChatRef, working: u32, unknown: u32, scan_complete: bool },
    RequestPending { chat: ChatRef, open: u32 },                 // 承認・質問の件数（応答はしない）
    Freshness { cause: ReconcileCause },                         // 切断・wake・Gap
    NotLive { task: TaskId },                                    // 送信時に live でなかった
    WorkspaceResult { task: TaskId, result: WorkspaceOutcome },  // P-ORG-5
    SandboxChecked { ok: bool, detail: String },
    InterruptRequested { chat: ChatRef },                        // 利用者の中断ボタン
    Tick { now_ms: i64 },
}

/// エンジン→ホスト。ホストは実行して結果を OrgInput で返す。送信・作成の Action を出すのは ready のタスクだけ。
pub enum OrgAction {
    Persist(OrgEvent),                         // 先に追記してから次の Action を実行する（順序を守る）
    CreateRoleChat { task: TaskId, spec: RoleChatSpec },     // cwd・sandbox(readOnly|workspaceWrite)・model・effort
    CheckSandbox { cwd: String },
    Send { task: TaskId, chat: ChatRef, attempt: AttemptRef, client_message_id: String, text: String, output_schema: serde_json::Value },
    ReadFinalMessage { task: TaskId, chat: ChatRef, turn: TurnRef },
    Workspace(WorkspaceAction),                // P-ORG-5 のポート
    Notify(OrgNotice),                         // 即時／2秒集約の区別を持つ
    Publish,                                   // 画面へ状態を出す合図（P-ORG-7）
}
```

- `OrgCore` は `WorkflowState`・役割表（MVPは標準3役を定数で持つ。定義ファイルは P-ORG-8）・回数を持つ。`handle()` の中で `record()` を使い、生成した `OrgEvent` は必ず `OrgAction::Persist` として**送信系 Action より前**に返す。
- 送信の流れ（D5・D6）: `ready` → `record(StatusChanged Running)` → `record(ConfirmChanged SendUnknown)` → `record(Dispatched)` → `Persist`×3 → `Send`。結果で `Accepted`→`ConfirmChanged Confirmed`、`Rejected`→`Confirmed` にして `Blocked{要対応}`、`Unknown`→**何もしない**（`SendUnknown` のまま。スケジューラは触らない）、`ReconciledNotFound`→`Confirmed`＋`Blocked{要対応}`（利用者の「送る／取り消す」待ち）。
- **受理不明で再送しない**ことはここで保証する: `SendUnknown` のタスクには `Tick` でも `Send` を出さない（テスト必須）。再送は `OrgInput` に存在しない（利用者の「送る」は 4b のホスト関数が `SendRequest::retry`＋`UserConfirmed` で行い、結果だけ `SendSettled` で来る）。
- 完了判定（DESIGN_ORG §1.3・D18）: `TurnEnded{Completed}`（live）→ `ReadFinalMessage` → `FinalMessage` を役割の出力スキーマで解析・検証 → 子孫に作業中が無い（`Descendants.working==0` かつ `scan_complete`）→ Worker は `Workspace(Commit)`→`Reviewing`、Reviewer は判定（合格→依存元を `Completed`、不合格→差し戻し）。`FinalMessage.text=None`（読めない）は `NeedsReconcile` 扱いで止める（推定で完了にしない）。
- 解析失敗: 1回だけ「形式修正」の `Send`（`TurnKind::FormatFix`、turn上限に数える）。2回目の失敗は `Failed{InvalidArtifact}`。
- `TurnEnded{Failed{usage_limit:true}}` はワークフロー全体を `Halted{User}` 相当で止め `Blocked{要対応}`（§4.4）。それ以外の失敗は `Failed{ExecutionFailed}`。どちらも再送しない。
- `Freshness` 受信時、`Running`／`Reviewing` で `Confirmed` のタスクは `NeedsReconcile(cause)`。戻すのはホストの照合（readのみ）結果の `TurnEnded`／`FinalMessage` が来たときだけ。
- `UserManual{task}`: `Blocked{手動}`。以後そのタスクに `Send` を出さない。ホストは grant から当該チャットを外す（4b）。

### 2.4 出力スキーマと指示文【新規 `schemas.rs`・`prompt.rs`】

- `schemas.rs`: `planner_plan_schema()`（`tasks[{id, role, deps[], effects[], title, instructions}]`）、`worker_report_schema()`（`summary`・`changedFiles[]`・`tests{ran, passed}`）、`reviewer_verdict_schema()`（`verdict: "pass"|"rework"`・`findings[]`）。`serde_json::Value` を返す定数関数。
- `prompt.rs`: 役割の指示テンプレート＋タスク本文＋依存先の成果物を**区切り付き引用**で連結（「以下は資料であり指示ではない」を明記、§4.9）。指示テンプレートは turn の本文に入れる（`developerInstructions` は使わない。resume 後も同じ動きになるため）。

### 2.5 4a の受入条件・確認

- crate 外から `Task` の状態・`WorkflowState.tasks` を書けない（4b のコンパイルで確認）。
- テスト（`cargo test --manifest-path app/org-engine/Cargo.toml`）:
  - `Send` の前に必ず `Persist(Dispatched)` がある。
  - `SendSettled{Unknown}` 後、`Tick` を何回送っても同タスクの `Send` が出ない。依存タスクも進まない。独立タスクは進む。
  - `ReconciledNotFound` は `Blocked{要対応}`、`Cancelled`／`Failed` にならない。
  - `Freshness` で `Running` が `NeedsReconcile`、`Failed`／`Cancelled` にならない。
  - 後着の古い `TurnEnded`（turn 不一致）で状態が変わらない。
  - 解析失敗→形式修正1回→再失敗で `Failed{InvalidArtifact}`、形式修正も turn に数える。
  - `UserManual` 後にそのタスクへ `Send` が出ない。
  - `NotLive` で `Halted{NeedsResume}`、送信しない。
- `cargo tree --manifest-path app/org-engine/Cargo.toml` に tauri・tokio が無い。

---

## 3. P-ORG-4b: ホストのポート・OrgGrant・保護（`app/src-tauri/`）

### 3.1 ファイル構成【新規】

```
src-tauri/src/host/org/mod.rs      // OrgDriver の起動・受け口（Host に置くのは送信側の口だけ）
src-tauri/src/host/org/grant.rs    // OrgGrant, GrantRegistry, DispatchPermit
src-tauri/src/host/org/driver.rs   // OrgCore を所有し OrgAction を実行する専用 tokio タスク
src-tauri/src/host/org/ports.rs    // AgentRuntime / RuntimeEvents の trait と Host 実装
src-tauri/src/host/org/observe.rs  // BackendEvent・送信結果 → OrgInput の変換（非同期待ちなし）
src-tauri/src/rules/org.rs         // 純関数: 保護の判定、ChatRef の符号化、trust 差分（4c）
```

### 3.2 OrgGrant と DispatchPermit（型の保証）

```rust
/// ワークフロー単位の送信権。利用者の「開始」「再開」操作でだけ作る。
/// Clone・Serialize・Default を実装しない。フィールドは private。OrgDriver だけが所有する。
pub struct OrgGrant { workflow: WorkflowId, epoch: u64 }

impl OrgGrant {
    /// `UserConfirmed` を値で消費する（参照からは作れない）。IPC の org_start / org_resume だけが呼ぶ（P-ORG-7）。
    pub(in crate::host) fn issue(confirmed: UserConfirmed, workflow: WorkflowId, reg: &GrantRegistry) -> Self;
    /// 1回分の送信許可。失効（epoch 不一致）・手動に切り替えたチャット・別ワークフローなら Err。
    pub(in crate::host::org) fn permit(&self, reg: &GrantRegistry, task: &TaskId, chat: &ChatKey) -> Result<DispatchPermit<'_>, GrantError>;
}

/// 1回の送信に1つ。Clone しない。`Host::org_send` が消費する。
pub struct DispatchPermit<'g> { grant: &'g OrgGrant, task: TaskId, chat: ChatKey }

/// 失効の管理（Host が持つのはこれだけ。OrgGrant 自体は持たない）。
pub struct GrantRegistry { epoch: AtomicU64, current: Mutex<Option<(WorkflowId, u64)>>, released: Mutex<HashSet<ChatKey>> }
impl GrantRegistry {
    pub fn revoke_all(&self);                 // 一時停止・取消・完全終了の開始・再起動・wake で呼ぶ（P-ORG-6 で配線、4b では関数とテスト）
    pub fn release_chat(&self, chat: &ChatKey); // 手動に切替
}
```

- 失効は epoch で行う。古い `OrgGrant` を持っていても `permit` が通らない。
- 再起動直後の `OrgDriver` は `grant: None` で起動する。照合（読取り）はできるが送信はできない（Q8）。
- `#[cfg(test)] pub fn issue_for_test(...)` を用意し、テストでだけ使う。

### 3.3 ホストのポート（`ports.rs`）

```rust
/// 書く側。OrgDriver からだけ呼ぶ。interrupt / respond / resume を持たない（D6・D7）。
#[async_trait]
pub trait AgentRuntime {
    async fn create_role_chat(&self, permit: CreatePermit<'_>, spec: RoleChatSpec) -> CreateResult;
    async fn send(&self, permit: DispatchPermit<'_>, req: OrgSendSpec) -> OrgSendResult; // live でなければ NotLive を返し送らない
}
/// 読む側。grant 不要。再起動後の照合・監視と共用してよい（送信・resume・承認をしない）。
#[async_trait]
pub trait RuntimeRead {
    async fn read_final_message(&self, chat: &ChatKey, turn: &TurnKey) -> Result<Option<String>, BackendError>;
    async fn list_by_cwd(&self, cwd: &str) -> Result<Vec<ChatKey>, BackendError>; // D15 の候補表示用
    async fn sandbox_check(&self, cwd: &str) -> SandboxCheck;                      // 4c
}
```

`CreatePermit` は `DispatchPermit` と同形（タスク単位、チャット未定）。`Host` に `impl AgentRuntime for Arc<Host>`、`impl RuntimeRead for Arc<Host>`。

#### `create_role_chat` の手順（`host/org/ports.rs`）
1. driver が `Persist(ChatRequested)` を済ませている前提。
2. trust 照合の「前」を取る（4c。4c 未完了なら省略し `Unverified`）。
3. `self.backend.start_chat(StartChatParams { kind: Development, cwd, model, permission: ReadOnly|WorkspaceWriteOnRequest, extra_folders: vec![] })`。**`host/mod.rs` の `start_chat`（IPC用、一般チャット領域・最初の依頼・名前付けを含む）は使わない**。
4. 成功: `update_local(&key, …, |f| { f.hosted = true; f.org = Some(OrgBinding{..}); f.permission = Some(..); f.model = ..; f.cached_meta = .. })` → `save_now(SaveScope::ChatLocal)` が `Saved` でなければ `CreateResult::SaveFailed`（送らない、`blocked{要対応}`）。`HostData` へは `start_chat` と同じく `hosted.insert`・`upsert_chat`・`set_live` を行う（`host/mod.rs:690-697` を関数に切り出して共用）。
5. `Err(OutcomeUnknown)` → `CreateResult::Unknown`。`Err(Rejected|Unsupported|…)` → `CreateResult::Rejected`。どちらも再試行しない。

#### `send` の手順
1. `send_lock(chat)` を取る（`queue_rt.send_lock`。手動操作と同時に turn/start しない）。
2. `check_sendable_core(chat)`（停止未確認・受理不明・削除保留・外部実行）。NG なら `OrgSendResult::Held(reason)`。
3. live でなければ `NotLive`（resume しない、D6）。
4. `SendRequest { mode: NewTurn, text, output_schema: Some(schema), permission: None, cwd: None, work_folders: None, model: role の値, client_message_id: (driver が Dispatched に記録した値), .. }`。
5. `self.dispatch_send_as(request, attempt, SendAuth::Org(permit))`（下記 3.4）。結果の `SendAttempt.state` を `OrgSendResult` に写す。

### 3.4 既存の送信経路の変更（`host/mod.rs`・`host/queue_driver.rs`）【変更】

- `host/mod.rs`
  - `check_sendable` を分割: `fn check_sendable_core(&self, chat)`（現在の中身）、`fn check_sendable(&self, chat) { self.check_not_org_owned(chat)?; self.check_sendable_core(chat) }`。
  - `dispatch_send_as(request, attempt)` に第3引数 `auth: SendAuth<'_>` を追加:
    ```rust
    pub(super) enum SendAuth<'a> { User, Org(DispatchPermit<'a>) }
    ```
    - `User` で宛先が組織チャット（`org` あり・未解放）なら送らず `Rejected("組織実行中…")`（二重の保護。`retry_send` は `UserConfirmed` を `SendRequest::retry` で消費済みなので参照を持たせない）。
    - `Org(permit)` は `permit.chat == request.chat` を確認（違えば送らない）。`touch_chat_used` を呼ばない（D17）。
    - 呼出し元: `send_message`・`retry_send`・`start_chat` の最初の依頼は `User`、`org_send` は `Org`。`dispatch_send`（引数2つ）は `User` 固定の薄い包みとして残す。
  - `emit_send(chat, attempt)` の末尾で `self.org_observe_send(chat, attempt)`（`Sending` 以外のとき、組織チャットなら `OrgInput::SendSettled` を送る）。`reconcile_once` も `emit_send` を通るので照合結果も同じ口で届く。**照合経路はエンジンへ知らせるだけで送信しない**。
  - `handle_backend_event` の `mutate` の**後**（ロック外）で `self.org_observe_event(&env.event)`。
  - `interrupt_chat` の末尾で、組織チャットなら `OrgInput::InterruptRequested`。
- `host/queue_driver.rs`
  - `run_entry_send` の `self.backend.send(request)`（757行）の前に、組織チャットなら送らず `Rejected`。`enqueue`（1081）・`resume_queue`（1152）・`send_queue_entry_now`（563）の先頭で `check_not_org_owned`。
  - `rules/queue.rs::evaluate` の後半（162行以降）を `pub fn evaluate_gate(input: &GateInput, baseline_at: Option<UnixMillis>, awaiting: bool) -> GateDecision` に切り出し、`evaluate` はそれを呼ぶ（挙動不変。既存テストで確認）。組織の完了判定・送信判定は `gate_input` と `evaluate_gate` を使い、`Descendants` 入力を作る。
- `check_not_org_owned` を呼ぶ利用者操作（組織チャット・未解放のとき `blocked(BlockedReason::OrgOwned, "組織実行中のチャットです。介入はワークフロー画面の「手動に切り替える」から")`）:
  `send_message`・`retry_send`・`enqueue`・`edit_queue_entry`・`resume_queue`・`send_queue_entry_now`・`set_goal`・`clear_goal`・`start_review`・`compact_chat`・`resend_as_fork`・`revert_changes`・`set_chat_model`・`set_chat_permission`・`set_next_work_folders`・`set_chat_cwd`・`set_work_mode`・`archive_chat`・`unarchive_chat`・`delete_chat`（ワークフロー単位でだけ行う、§1.4）。
  許すもの: 開く・読む・`interrupt_chat`・`fork_chat`（元を変えない）・`open_side`（別thread）・`respond_request`（承認は利用者が答える）・`set_pinned`・名前変更。
- `backend/ipc.rs` `BlockedReason` に `OrgOwned` を追加（ts-rs 生成を更新）。

### 3.5 `chat.json.org`（`store/records.rs`）【変更】

```rust
/// 組織実行が作ったチャットの所有記録。所有はこの記録だけで判定する（推定しない）。
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
// ChatLocalFile に追加
#[serde(default)]
pub org: Option<OrgBinding>,
```

- `ChatLocalFile::new` に `org: None`。`ChatLocalView`（`backend/local.rs`）にも `org: Option<OrgBindingView>` を出す（左一覧で隠す・ドックのバッジは P-ORG-7）。
- 手動に切替: `Host::org_release_chat(chat, confirmed: &UserConfirmed)`（IPCはP-ORG-7）→ `update_local(|f| f.org.released_at = Some(now))`＋`save_now`→ 成功後に `GrantRegistry::release_chat`→`OrgInput::UserManual`。保存に失敗したら解放しない（再起動後に食い違わないように）。
- 再起動時: `ChatRequested` だけで `ChatBound` が無いタスクは、`chat.json.org` で同じ workflow・task を指すチャットがあれば `ChatBound` を記録して結び付ける（記録どうしの突合で推定ではない）。無ければ `list_by_cwd` の候補を出して利用者に選ばせる（P-ORG-6/7）。

### 3.6 `RuntimeEvents`（`observe.rs`）

`OrgObserver` は `HashMap<ChatKey, (WorkflowId, TaskId)>`（`chat.json.org` から作る、未解放のみ）と `UnboundedSender<OrgInput>` を持つ。**待ちを作らない**（`send` は非同期待ちなし。満杯も無い）。

|BackendEvent|OrgInput|
|---|---|
|`TurnEnded { turn, end, .. }`（ルートのみ。`turn.agent` がチャットのルート）|`TurnEnded`（`end` と、`Failed` なら利用上限か）|
|`RequestOpened`／`RequestResolved`（チャット内の全エージェント。子孫も `AgentView.agent.chat` で親チャットに寄せる）|`RequestPending{open}`（件数だけ。**応答はしない**）|
|`Connection { Disconnected | … }`、`Gap`|`Freshness{Disconnect}`|
|`TokenUsage`（新規）|usage 行の材料（driver が `OrgStore::append_usage`）|
|子孫の発見・状態変化|`Descendants`（`gate_input` から数える。driver が必要時に読む）|

- 新規 `BackendEvent::TokenUsage { agent: AgentKey, turn: Option<ExternalId>, last_total: Option<u64>, thread_total: Option<u64>, context_window: Option<u64> }`。`codex/events.rs` の `thread/tokenUsage/updated` を写す（取れない値は None、0 で代用しない）。`events.rs:698` のテストを更新。既存の `state.rs::apply_event` では無視（表示しない）。
- 利用上限の判定: `TurnEnded.error` の文字列で判断しない。アダプターが turn 失敗の `codexErrorInfo` が利用上限のときに `BackendEvent::TurnFailureClass { turn, class: FailureClass::UsageLimit }` を併送する（新規、中立）。届かなければ通常の失敗扱い。

### 3.7 4b の受入条件・確認

- **型**: `OrgGrant`・`DispatchPermit` に `Clone`・`Serialize`・`Default` が無い。`SendAuth::Org` は `DispatchPermit` を値で持つ（`grant.rs` の外で構築できない）。`OrgGrant::issue` の引数は `UserConfirmed`（値）。`Host` のフィールドに `OrgGrant` が無い。
- **送信は permit 必須**: `AgentRuntime::send` と `SendAuth::Org` 以外に組織チャットへ送る経路が無い。`check_not_org_owned`／`SendAuth::User|Queue` の保護の単体テスト（`rules/org.rs` の純関数 `org_guard(binding, op) -> Result<(), &str>` をテスト）。
- **照合・監視から呼べない**（grep で確認、0件であること）:
  ```
  rg -n "OrgGrant|DispatchPermit|org_send|AgentRuntime" app/src-tauri/src/host/queue_driver.rs app/src-tauri/src/host/mod.rs app/src-tauri/src/host/state.rs app/src-tauri/src/host/lifecycle.rs
  ```
  （`host/mod.rs` は `SendAuth` の定義・`dispatch_send_as` の分岐だけ許可。上の語が出たら行を確認）
- **エンジンが Codex の要求に応答しない**（0件であること）:
  ```
  rg -n "respond|RequestAnswer|from_user_command|\.resume\(|ensure_live|interrupt" app/src-tauri/src/host/org app/org-engine/src
  ```
  （`InterruptRequested` の受信だけは許可。`observe.rs` の `RequestPending` は件数のみ）
- **受理不明で再送しない**: 4a のテスト＋ `org_send` に再送の分岐が無い（`SendRequest::retry` の呼出しが `host/org/` に無い）。
- `rules/queue.rs` の既存テストが `evaluate_gate` 切出し後も通る。
- 確認コマンド:
  ```
  cargo check --manifest-path app/src-tauri/Cargo.toml
  cargo test --manifest-path app/src-tauri/Cargo.toml --lib rules::org
  cargo test --manifest-path app/src-tauri/Cargo.toml --lib rules::queue
  cargo test --manifest-path app/src-tauri/Cargo.toml --lib host::org
  cargo test --manifest-path app/src-tauri/Cargo.toml --lib codex::events
  cargo test --manifest-path app/src-tauri/Cargo.toml --lib store::records
  ```

---

## 4. 送信まわりのアダプター変更（4b に含める）

- `backend/backend.rs` `SendRequest` に `pub output_schema: Option<serde_json::Value>` を追加。`SendRequest::retry` は元の値を引き継ぐ。`UnconfirmedSend::restore` は `None`。
- `codex/adapter.rs` `build_send_call` の `NewTurn` に `if let Some(s) = &req.output_schema { p["outputSchema"] = s.clone(); }`。`Steer` では付けない。テスト1件（付く・Steerで付かない）。
- 構築箇所（`rg -n "SendRequest \{" app/src-tauri/src`）すべてに `output_schema: None`。

---

## 5. P-ORG-4c-1: sandbox健全性チェック

### 5.1 手順（1ワークフロー開始あたり。結果は `SourceId` ごとに覚え、再接続で捨てる）

1. Planner の役割チャットを作る（`thread/start`。ここで MCP が起動し、競合が起きる条件がそろう）。
2. `windowsSandbox/readiness`（読取り）。`ready` 以外（`notConfigured`／`updateRequired`）は失敗。`setupStart` は**呼ばない**（利用者がCodex側で行う）。
3. `command/exec`:
   ```json
   {"command": ["cmd.exe", "/d", "/c", "cd"], "cwd": "<repo>", "sandboxPolicy": {"type": "readOnly", "networkAccess": false},
    "timeoutMs": 20000, "outputBytesCap": 4096}
   ```
   書込みなし・ネットワークなし。`exitCode == 0` で合格。
4. 判定: 合格→ Planner の最初の `turn/start` へ。不合格・応答なし（`OutcomeUnknown`）・`Unsupported`→ `Halted{SandboxUnhealthy}`、ワークフローは `blocked{要対応}`。**自動再試行・自動回避なし**。利用者の「再開」で手順2から再確認。
5. 二重の保護: 実行中に `commandExecution` の失敗本文に `setup refresh had errors` を観測したら（アダプターが `BackendEvent::Unrecognized` ではなく中立の `SandboxFailureObserved { agent }` を出す）、そのタスクを `NeedsReconcile` ではなく `Blocked{要対応}` にし同じ案内を出す。

- 中立型（`backend/parity.rs` に追加）:
  ```rust
  pub struct SandboxCheck { pub readiness: Known<String>, pub exit_code: Known<i32>, pub stderr_head: Known<String> } // stderr は先頭1KBだけ、保存しない
  ```
- 実装: `codex/parity.rs` に `async fn sandbox_check(&self, cwd: &str) -> BackendResult<SandboxCheck>`（`ParityOps` に追加、能力表 `parity_table.rs` に1行。0.162.1 で Supported、それ以前は Unknown）。
- **実機確認項目（4c の中、ユーザー）**: 回避オプションなしの環境で `org-probe.mjs` に `sandboxcheck` モードを足し、①`thread/start` 後の `command/exec` が失敗する（再現する）②回避オプションありで成功する、を確認。①で失敗しない（再現しない）場合は手順5の実行時検出を主にし、手順3は補助と位置付け直す（報告して判断）。

### 5.2 失敗時の案内（ワークフロー画面の帯。P-ORG-7 が表示、文言は4cで定数化）

> Codex の sandbox を起動できないため、組織実行を始めていません。
> 原因の候補: Codex のプラグイン（node_repl／computer-use）と sandbox の更新が競合している（`os error 32`）。詳しくは `%USERPROFILE%\.codex\.sandbox\` のログを確認してください。
> 回避: 設定 →「Codex」→「sandbox の競合を回避する」をオンにし、AgentDock を再起動してから「再開」。

## 6. P-ORG-4c-2: J1-A 起動オプション

- 保存先: `settings.json`（`AppSettings`、`backend/local.rs:491`）に `#[serde(default)] pub sandbox_workaround: bool`（既定 false）。TS名 `sandboxWorkaround`。Codex固有の型は持たない（真偽値だけ。意味はアダプターが解釈）。
- `backend/backend.rs` `ConnectConfig` に `pub sandbox_workaround: bool`（doc: 「バックエンドが定義する sandbox 競合回避。Codex では起動引数で plugins と node_repl を無効にする」）。`host/mod.rs:523` で設定値を入れる。
- `codex/process.rs` `CodexProcess::launch` に `extra_args: &[&str]` を足し、オンのとき `["app-server", "-c", "features.plugins=false", "-c", "mcp_servers.node_repl.enabled=false"]`。**`mcp_servers.cua_repl.*` は渡さない**（起動失敗する、§5.5）。`~/.codex` は変更しない。
- 有効になるのは次の App Server 起動から。接続時の値を `SourceInfo` に `sandbox_workaround: bool` として残し、設定と違えば設定画面に「AgentDock の再起動後に有効になります」を出す。切替えで App Server を自動再起動しない（実行中の作業を止めない）。
- 設定画面（UIは最小。P-ORG-7 か 4c のどちらでもよい。4c で入れるなら設定「Codex」節にチェック1つ）:
  - ラベル: 「sandbox の競合を回避する（プラグインを無効にして Codex を起動）」
  - 説明: 「組織実行の開始前チェックが失敗したときだけ使います。オンの間は computer-use などのプラグインが全チャットで使えません。AgentDock の再起動後に有効になります。」
- 単体テスト: 起動引数の組立て（オン・オフ、`cua_repl` を含まない）。

## 7. P-ORG-4c-3: trust 追記の前後照合（J2-A）

- 純関数（`rules/org.rs`）:
  ```rust
  pub enum TrustCompare {
      Unchanged,
      /// 追加された行が `[projects.'<path>']` 見出しと `trust_level = "trusted"` だけ。削除・変更なし。
      CodexTrustAppended { projects: Vec<String> },
      /// それ以外の変化がある（Codex本体・他の操作・利用者の編集の可能性。AgentDock は書いていない）。値は返さない。
      Other { changed_lines: usize },
      /// 前後どちらかを読めなかった。一致とは扱わない。
      Unverified { reason: String },
  }
  pub fn classify_trust_append(before: Option<&str>, after: Option<&str>) -> TrustCompare;
  ```
  判定は行の多重集合の差（空行は無視、前後の空白は無視）。見出しは `[projects."…"]`・`[projects.'…']` の両方を受ける。
- 呼び出し: `create_role_chat` の前後で `cli::config_toml_path()` の本文を読む（**読むだけ・保存しない・ログに本文を出さない**）。前後の読取りは `ext_lock()`（`manage_extension` と同じ直列化）を取った中で行い、AgentDock 自身の設定操作と混ざらないようにする。外部（Codex アプリ・利用者）の同時変更は防げないので `Other` として表示するだけ。
- 結果は `CreateResult` に `trust: TrustCompare` として付け、エンジンへは `OrgNotice::ConfigObserved` で渡す。表示（P-ORG-7）: `CodexTrustAppended` は「Codex が作業フォルダを信頼済みとして config.toml に追記しました（AgentDock は変更していません）」を情報として、`Other` は「config.toml に想定外の変更があります（AgentDock は変更していません）」を注意として出す。どちらも自動で戻さない・送信を止めない。
- 既存の `judge_config`／`ConfigCompare`（CLI管理操作用）は変えない。
- テスト: 追記だけ／見出しだけ（値なし）／他キーの変更混在／削除あり／読めない、の5件。

### 7.1 共通親フォルダ案の検証手順（実機。ユーザーが行う。利用枠を使わない＝turn なし）

`probes/app-server/org-probe.mjs` に `trust` モードを足す（4c の実装担当がスクリプトを書く。scratch で可）。各ケースで「config.toml のハッシュと `[projects.*]` 見出しの一覧」を前後で記録し、`thread/start` だけ行う（turn は送らない）。最後に作った thread を `thread/archive` する。

|#|cwd|期待を確かめること|
|---|---|---|
|V1|既に trust 済みのリポジトリの git worktree を `%LOCALAPPDATA%\com.agentdock.app\worktrees\probe-<時刻>` に作り、そこを cwd|見出しが増えるか。増えなければ Codex が worktree を元リポジトリとして扱っている → **共通親フォルダ案は不要**、本件は「Planner の cwd が未trustのときだけ1件増える」で確定|
|V2|同フォルダ配下の git でない普通のフォルダ|見出しが増えるか（比較用）|
|V3（V1で増えた場合だけ）|利用者が自分で `config.toml` を控えたうえで `[projects.'<LOCALAPPDATA>\com.agentdock.app\worktrees']` `trust_level = "trusted"` を手で追加し、V1 を新しい worktree で再実行|見出しが増えないなら共通親フォルダ案は有効。AgentDock は書かないので、採るなら「利用者が手で1回追加する」案内になる（改めて報告）|

- 任意の追加確認（§4.10 の thread 単位の上書き）: V1 の `thread/start` に `"config": {"features": {"plugins": false}, "mcp_servers": {"node_repl": {"enabled": false}}}` を付け、§5.1 の `command/exec` が回避なしの環境で通るか。通れば J1 を thread 単位に狭める案を報告（前提にはしない）。
- 手順書（PowerShell）は司令塔が S-ORG とは別に1枚にまとめる。

### 7.2 4c の受入条件・確認

- `cargo check --manifest-path app/src-tauri/Cargo.toml`
- `cargo test --manifest-path app/src-tauri/Cargo.toml --lib rules::org`（trust 分類・起動引数）
- `cargo test --manifest-path app/src-tauri/Cargo.toml --lib codex::parity`（能力表に `sandbox_check`）
- `rg -n "cua_repl" app/src-tauri/src` が起動引数に現れない（コメント・テストの否定確認だけ）。
- `rg -n "write|create|OpenOptions" app/src-tauri/src/rules/org.rs` が0件（config.toml を書かない）。
- 実機: §5.1 の確認と §7.1 の V1・V2（ユーザー）。

---

## 8. 分割案と順序

|#|内容|担当|依存|規模の目安|
|---|---|---|---|---|
|P-ORG-4a|§2 全部（フィールド保護、イベント追加、`OrgCore` sans-IO、スキーマ・指示文、テスト）|implementer|P-ORG-2・3|中〜大（純ロジック）|
|P-ORG-4b|§3・§4（grant、ports、observe、送信経路の分割と保護、`chat.json.org`、`TokenUsage`、`output_schema`、`evaluate_gate` 切出し）|implementer|P-ORG-4a|大|
|P-ORG-4c|§5〜§7（sandbox確認、起動オプション、trust照合、probe の `trust`／`sandboxcheck` モード）|implementer|なし（4a・4b と並行可。`create_role_chat` への組込みだけ 4b の後で1関数）|中|
|P-ORG-5（追加）|`Workspace` ポートのホスト実装（D4）|implementer|P-ORG-4b|—|

順序: (4a ∥ 4c) → 4b → 4c の組込み（`create_role_chat` で trust 照合と sandbox 確認を呼ぶ1か所）。R-ORG の対象に 4a・4b を含める。

## 9. 実装担当が守ること（抜粋）

- 組織の送信は `org_send`（`DispatchPermit` 消費）だけ。`host/org/` で `backend.send`・`backend.respond`・`backend.resume`・`from_user_command`・`SendRequest::retry` を使わない。
- `Persist`（events.jsonl 追記）が成功してから `Send`・`CreateRoleChat` を実行する。追記に失敗したら送らない（`blocked{要対応}`）。
- `Host::mutate` のロックの中で `OrgInput` を作って送らない（ロック外で送る。チャネルは unbounded、待たない）。
- 受理不明・作成不明・読めない・切断は、`Failed`／`Cancelled`／`Completed` にしない。
- config.toml は読むだけ。本文を保存・ログ出力しない。`windowsSandbox/setupStart`・`thread/shellCommand` を呼ばない。

---

## 10. 要判断（ユーザー）

**U1（確定 2026-10-17、ユーザー）. 組織実行中に会話の live 購読が切れたとき（切断・wake・App Server 再起動）**（D6）

- 自動では resume せず止める。ワークフローを「再開待ち」（`HaltReason::NeedsResume`）にし、利用者が「ワークフローを再開」を押したときだけ、その操作の中で必要なチャットを resume して続ける。既存のキュー（親 live 必須）・wake の合意と同じ。組織の自動送信は live 必須。
- 「送信直前に自動で resume」（DESIGN_ORG §1.3 の旧文言）は採らない。
- 4a の実装: エンジンは `OrgInput::Live { chat, live }` を覚え、live でないチャットには `Send` を出さずに `Halted{NeedsResume}` にする。`Freshness` 受信で live の記憶を全て消す。再開時は、ホストが先に必要なチャットを resume して `Live { live: true }` を渡し、そのあと `UserResume` を渡す。

ほかに新しいユーザー判断はなし（D10・D18・設定名・表示文言は設計裁量で決定済み）。
