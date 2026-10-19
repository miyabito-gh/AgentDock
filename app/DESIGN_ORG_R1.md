# 組織実行 改修 段階1 設計（R1〜R3）v0.1（2026-10-19）

対象: `PLAN_ORG_REWORK.md` 段階1と、`DEFERRED.md` の2026-10-19の3項（承認待ちに答えられない／上限停止から再開できない／手動切替後に成果を取り込めない）。
変えない合意: エンジンは承認要求に代行で答えない／自動で再送しない／通信断を cancelled・failed にしない／停止を確認できないまま削除しない／`~/.codex` を無断で変更しない。Q9（組織チャットへの手入力不可）は段階2で扱うため、今回は触らない。

## 0. 方針（判断の要点）

1. **待機理由は第3の軸にする**（業務状態・確認状態とは別）。承認・質問の待ちは live の事実で、turn の追跡は `status==Running` を前提にしている。業務状態を Blocked に変えると turn 終了の受け取りが壊れるため、**業務状態は変えず、表示用に導き出す**（保存しない）。
2. **turn の失敗は終端（Failed）にしない**。`Blocked(TurnFailed)`／`Blocked(UsageLimit)` にして、利用者が「新しい試行で再送」「失敗として確定」「取消し」「手動に切替」のどれかを選ぶ。終端状態は終端のまま残す（表の不変条件を守る）。
3. **手動切替後の取込みは「利用者が選んだturnを取り込む」明示操作**（案B）。observer は外したまま（§3.3 で比較）。

## 1. 状態遷移の変更

### 1.1 型の追加（`app/org-engine/src/types.rs`・`limits.rs`・`persist.rs`）

|追加|内容|互換|
|---|---|---|
|`BlockedReason::UsageLimit`|利用上限で turn が失敗（今は NeedsAttention）|新しい列挙子。旧データには出ない|
|`BlockedReason::TurnFailed`|turn が失敗（上限以外、または区分不明）|同上|
|`TurnEndKind::FailedUnclassified`|履歴で失敗を確認したが、原因の区分が取れない|同上|
|`HaltReason::UsageLimit`|上限で全体を止めた（今は `User` を流用しているため「一時停止中」と誤表示）|同上|
|`TurnKind::Retry`|新しい試行の turn（turn数に数える）|同上|
|`Limits.max_retries: u32`|タスクごとの再送の上限。`#[serde(default)]` で **3**。設定範囲 0〜10|旧データは3|
|`Task.retry_count: u32`・`Task.retry_pending: bool`・`Task.adopted_manual: bool`|再送回数／次の送信が再送か／手動結果の取込み中か|`#[serde(default)]`|
|`LimitError::RetryLimit { used, max }`|上限を超える再送を拒否||
|イベント `RetryRequested { task }`|`retry_count+=1`、`retry_pending=true`。上限超過は `ApplyError::Limit`|新イベント|
|イベント `ManualTurnAdopted { task, turn }`|`accepted_turn=turn`、`observed=Some(Completed)`、`adopted_manual=true`。`Blocked(Manual)`かつ`Confirmed`でなければ拒否|新イベント|

`Dispatched` の適用で `retry_pending=false` にする（kind に関係なく）。

### 1.2 遷移の変更表（`core.rs`）

|起点|きっかけ|今|変更後|
|---|---|---|---|
|Running|live の turn 失敗 `Failed{usage_limit:true}`|Blocked(NeedsAttention)＋halt User|**Blocked(UsageLimit)**＋**halt UsageLimit**＋通知 UsageLimit（即時）|
|Running|live の turn 失敗 `Failed{usage_limit:false}`|**Failed(ExecutionFailed)**（終端）|**Blocked(TurnFailed)**＋通知 TaskBlocked（即時）。halt しない|
|Running|履歴照合の失敗（区分あり）|Failed(ExecutionFailed)|上の2行と同じ（`rules/org.rs` が区分を渡す）|
|Running|履歴照合の失敗（区分なし）|Failed(ExecutionFailed)|**`FailedUnclassified` → Blocked(TurnFailed)**、理由「turnは失敗で終わりました（原因の区分は取得できません）」|
|Blocked(UsageLimit/TurnFailed/ManualStop)|利用者「新しい試行で再送」|不可|`RetryRequested`→**Ready**（Cause::User）。次の送信は `TurnKind::Retry`|
|Blocked(UsageLimit/TurnFailed/ManualStop/NeedsAttention)|利用者「失敗として確定」|不可|**Failed(ExecutionFailed)**（Cause::User。表に既にある遷移）|
|Blocked(Manual)|利用者「結果を取り込む」|不可（完了にするだけ）|`ManualTurnAdopted`→**Running**（User）→最終回答の読取り→通常の完了判定|
|Running（adopted_manual）|最終回答が形式不合格・読めない|（該当なし）|**Blocked(Manual)** に戻す（形式修正の turn は送らない）|
|halt UsageLimit|利用者「再開」|—|halt を解除（既存の `Some(_) => clear_halt`）|

通信断・再起動・wake の扱いは変えない（Running は NeedsReconcile、送信は「再開」まで止める）。`state.rs` の ALLOWED 表は変更不要（Blocked→Ready／Failed／Running、Running→Blocked は既存）。

### 1.3 再送の可否（純粋関数、`core.rs`）

`pub fn retry_kind(t: &Task, limits: &Limits, turns_remaining: u32) -> Result<RetryKind, RetryRefusal>`
- `RetryKind::Unsent` … 今の `retryable(t)` が真（送っていないことが確か）。今の挙動のまま。
- `RetryKind::NewAttempt { reason: UsageLimit|TurnFailed|Interrupted }` … 次を全部満たす: 状態が `Blocked(UsageLimit|TurnFailed|ManualStop)`、確認状態 `Confirmed`、`accepted_turn` があり `observed.turn == accepted_turn` かつ終端が Completed 以外（その turn が終わったことを確認済み）、開いた試行がない（`last_settle` が None/Unknown でない）、`chat` がある、`retry_count < max_retries`、`turns_remaining > 0`。
- `RetryRefusal` … `SendUnknown`／`NotEnded`（終端未確認）／`RetryLimit{used,max}`／`NoTurns`／`NotBlocked`。画面の理由文に使う。
- core 側の追加条件: `flows`・`launching` に無いこと。
- `OrgInput::UserRetryTask` は `retry_kind` で分岐する（Unsent は今の処理、NewAttempt は `RetryRequested`→Ready）。拒否は `Refused` 通知（理由つき）。

### 1.4 再送の文面

`ready_turn` の先頭で `retry_pending` を見る（グラフ・計画の両方）。`prompt::retry_prompt(reason, title, instructions)`:
「前回の turn は{利用上限／失敗／中断}で途中で終わりました。作業フォルダとこれまでの会話を確認し、未完了の作業を続けて完了させ、最後に指定の形式で結果を返してください。（タスク: {title}）」
同じチャットへ送るため、元の依頼の全文は付けない。outputSchema は通常どおり付ける。`format_fix_used` は戻さない（形式修正は1回のまま）。

## 2. ホスト側の変更

### 2.1 R1: 回答できない要求を黙殺しない

- `backend/backend.rs` `RespondOutcome` に `NotDeliverable { cause: RespondGap }` を追加。`RespondGap = ConnectionChanged | NotHeld`（バックエンド中立）。
- `codex/adapter.rs` `respond`: pending に無いとき、`request.source` が今の接続と違えば `ConnectionChanged`、同じなら `NotHeld`。**AlreadyResolved にしない**。rpc の Protocol エラー（Codex側で解決済み）は今のまま AlreadyResolved。
- `backend/model.rs` `RequestState` に `Unanswerable { at, cause: RespondGap }`。`host/mod.rs` respond_request で対応付け、`RequestUpdated` を出す（印は awaiting から外れる）。
- 文言（App.tsx の案内とカード上の常設表示）:「この要求には回答できません（接続が切り替わったため、Codex へ届けられません）。回答は送っていません。」／（NotHeld）「AgentDock がこの要求を保持していないため、回答を届けられません（原因は特定できません）。」＋案内「Codex 側で turn が待ち続けている場合は中断してください。組織のタスクは中断後に『新しい試行で再送』できます。」
- 切断で `Expired` になった要求のカードにも同じ案内を出す（今は文言のみで次の手が無い）。

### 2.2 R1: 待機理由の導出と表示

- 件数は **UI のスナップショットから導く**（`snap.requests`。既に `orgPendingRequests` がこの方式）。保存しない。`app/src/ui/org.ts` に純粋関数 `taskRequestWait(snap, task) -> { approvals, questions, unanswerable, first: PendingRequest|null }`。questions＝`userInput`/`toolElicitation`、unanswerable＝`unanswerable` と、`expired` のうち turn がそのタスクの今の turn のもの。
- 停滞通知の抑止: `host/org/driver.rs` `check_stalls` は、エンジンの `open_requests` が1以上のタスクを除く（`OrgCore::open_requests_of(chat)` を公開）。利用者の回答待ちを「停滞」と通知しない。
- 通知: 承認・質問の即時通知（既存）が組織チャットでも出ることを確認し、本文の名前に「組織・{タスクID}」を付ける（`host/notifier.rs`。会話内容は入れない）。押すと役割チャットを開く。

### 2.3 R2: 履歴照合での区分

- `backend/backend.rs` `FailureClass` に `Other` を追加。`backend/model.rs` `TurnRecord` に `failure: Known<FailureClass>`（serde 既定は NotFetched）。
- `codex/convert.rs` `turn_to_record`: status が failed のとき `error.codexErrorInfo` が `"usageLimitExceeded"` → `Value(UsageLimit)`、他の非null値 → `Value(Other)`、error か codexErrorInfo が null → `Missing`。失敗以外は `NotFetched`。
- `host/org/ports.rs` `ReconcileRead` に `turn_failure: Known<FailureClass>`。`rules/org.rs` `turn_end_kind_of(end, &failure)`: UsageLimit→`Failed{usage_limit:true}`、Other→`Failed{usage_limit:false}`、それ以外→`FailedUnclassified`。

### 2.4 R3: 手動切替後の取込み

- `org_manual_result(OrgTaskArgs) -> OrgManualResultView`（読取りのみ）: 役割チャットの履歴を読み、最新 turn の `{ turn_id, end: completed|running|failed|interrupted|unknown, complete, final_head(先頭400字), problem }` を返す。`final_message_of` を使う。
- `org_adopt_manual(OrgAdoptManualArgs{workflow_id, task, turn_id}, UserConfirmed)`: 履歴を読み直し、その turn が**最新で、完了で、全件読めて、最終回答がある**こと、チャットに実行中の turn が無いことを確かめる（どれか欠けたら拒否）。送る前に `RequestPending{chat, open}`（今の件数）を入れ、`OrgInput::UserAdoptManual{task, turn}` を送る。
- エンジン `UserAdoptManual`: Blocked(Manual)・Confirmed・flow/launching なしを確認 → `ManualTurnAdopted` → Running → `ReadFinalMessage`（既存の読取り）→ 解析 → WaitDesc → 通常の完了（Worker はコミット。送信権と halt なしが要るのは今と同じ）。`parse_failed`・最終回答 None のとき `adopted_manual` なら NeedFix を作らず Blocked(Manual) に戻し、理由を通知する。
- 取込み中の誤送信防止: 解放済みチャットでも、そのタスクが Running（adopted_manual）の間は通常送信を拒否する（`rules/org.rs` に `adopting` 判定を追加、送信経路で参照）。文言「結果の取り込み中です。終わるまで送信できません。」
- 「手動で完了にする」（成果物なし）は残す。

### 2.5 比較: 手動切替後の turn 終了の読み方

|案|内容|長所|短所|
|---|---|---|---|
|A 観測を外さない|解放後も observer に残し、turn 終了で自動取込み|自動で進む|どの turn を結果とするか曖昧（手動で何回も送る）。手動を選んだ意図に反して勝手に完了する。再起動時は解放済みチャットを結び直さない現行と食い違う|
|**B 明示取込み（推奨）**|利用者が turn を選んで取り込む。読取りは履歴から|対象が明確・再起動後も同じ手順・既存の読取り経路を再利用・Q9に触れない|1操作増える|

## 3. IPC・型（`backend/org.rs`、`src/ipc/gen/` は ts-rs で再生成）

- `OrgTaskView` 追加: `retry: Option<OrgRetryView>`（`kind: unsent|newAttempt`、`reason_label`、`used`、`max`）、`retry_refusal: Option<String>`、`can_fail: bool`、`can_adopt_manual: bool`。`can_retry` は `retry.is_some()` と同値で残す。
- `OrgReason` に `UsageLimit`・`TurnFailed`、`OrgHaltKind` に `UsageLimit`（ラベル「利用上限で停止中」）。`status_view` のラベル:「止まっています（利用上限）」「止まっています（turnが失敗）」。
- 新コマンド: `org_fail_task(OrgTaskArgs)`、`org_manual_result(OrgTaskArgs)`、`org_adopt_manual(OrgAdoptManualArgs)`。`org_resume` の引数に `retry_tasks: Vec<String>`（既定空）。ホストは `UserResume` の後に各タスクの `UserRetryTask` を送る。
- `RespondOutcome::NotDeliverable`、`RequestState::Unanswerable`、`RespondGap`、`FailureClass::Other`、`TurnRecord.failure`。

## 4. 画面（置き場所と文言）

|置き場所|表示・操作|
|---|---|
|左一覧のワークフロー行（`OrgListRow`）|印「回答待ち N」（wait）、「回答不可 N」（fail）。件数は `LeftPane` から props で渡す|
|ドック|回答待ちの帯は既存（全チャット対象）。組織チャットのカードに `OrgBadge` を必ず出し、`unanswerable`／`expired` の要求は「回答できません（理由）」の1行で残す|
|組織画面の表の上（新）|**塊の帯**:「あなたの対応待ち N（回答 a・上限 b・失敗 c・手動 d）」「作業中 N」「完了 N／全 M」。各塊を押すと表の該当行へ移動|
|組織画面の表「待ち先」列|回答待ちがあれば「あなたの回答待ち（承認a・質問b）」を最優先で表示|
|タスクの注記行|**承認カード（`RequestCard`）をそのまま埋め込み**、組織画面から直接回答できる（App.tsx の onRespond を渡す）。回答不可は理由と案内|
|タスクの操作列|「新しい試行で再送…」（retry が newAttempt のとき。`再送 used/max`）、「やり直す…」（unsent）、「失敗として確定…」、「結果を取り込む…」（Manual）、「手動で完了にする…」。不可のときは `retry_refusal` を title に|
|再開の確認|halt が UsageLimit のとき、上限で止まったタスクをチェック付きで並べる（既定でオン）。「チェックしたタスクを新しい試行として送ります（自動では送りません。この操作が送信の指示です）」|
|取込みの確認|最新 turn の終端と最終回答の先頭を表示。「この回答を成果物として取り込み、通常の完了判定（形式の検証・子孫の確認・Workerはコミット）に進めます。形式に合わなければ手動のまま戻します。」形式が合わないときのために「形式の説明をコピー」ボタン（クリップボードへ。送信はしない）|
|止める操作（新）|今の「一時停止」を **「止める…」メニュー**にして3つに分ける:「新しい送信だけ止める（実行中は続ける）」＝今の一時停止／「実行中も止める」＝一時停止＋実行中タスクへ既存の中断（10秒で停止未確認の案内。取消・失敗にしない。止まったタスクは Blocked(ManualStop) で再送できる）／「アーカイブ」＝既存の作業中アーカイブ（一覧から隠すだけ、作業は続く）|

比較（塊の帯）: 列だけ（変更小さいが、表が長いと待ちを見落とす）／**塊の帯＋列（推奨）**（上部で件数が分かり、押すと該当行へ。段階2のタイムラインとも重ならない）。

## 5. 実装タスク（実装担当1名が上から順に）

1. **エンジンの型とイベント**: `types.rs`・`limits.rs`・`persist.rs`（1.1 の追加、apply、serde 既定）。`cargo test -p org-engine`（追加分）。
2. **遷移と再送**: `core.rs`（turn_ended の変更、`retry_kind`、`UserRetryTask` 分岐、`UserFailTask`、`UserAdoptManual`、`parse_failed`/`final_message` の adopted 分岐、`open_requests_of`）、`planned.rs`/`graphrun.rs` の `ready_turn` 先頭、`prompt.rs` `retry_prompt`。
3. **履歴の区分**: `backend/backend.rs`・`backend/model.rs`・`codex/convert.rs`・`host/org/ports.rs`・`rules/org.rs`・`host/org/driver.rs`（reconcile_read_only で区分を渡す）。
4. **回答不可の表示**: `backend/backend.rs`・`backend/model.rs`・`codex/adapter.rs`・`host/mod.rs`・`host/state.rs`（印）。
5. **ホストのIPC**: `backend/org.rs`・`host/org/view.rs`（ラベル・retry 表示）・`host/org/ipc.rs`（fail・manual_result・adopt・resume 引数）・`host/org/mod.rs`・`rules/org.rs`（adopting の送信拒否）・`driver.rs`（停滞抑止）・`host/notifier.rs`（名前）。Tauri コマンド登録。ts-rs 再生成。
6. **画面**: `src/ui/org.ts`（`taskRequestWait`、summary に件数）、`OrgPane.tsx`（塊の帯・列・注記行の RequestCard・操作・確認文・止めるメニュー）、`OrgDialogs.tsx`（取込み・再送・再開の確認）、`LeftPane.tsx`、`Dock.tsx`、`CenterPane.tsx`（RequestCard の unanswerable/expired 表示）、`App.tsx`（案内文）、`src/ipc` の呼び出し。`tsc --noEmit`。

## 6. 受入条件

- U1 live の上限失敗で、タスクは「止まっています（利用上限）」、ワークフローは「利用上限で停止中」。Failed にならない。
- U2 履歴照合で失敗を見つけたとき、区分ありは U1／TurnFailed、区分なしは TurnFailed（区分不明の注記）。どちらも Failed にならない。
- U3 「新しい試行で再送」は利用者の操作1回で1回だけ送る。`max_retries` 到達・turn残り0・受理不明・終端未確認では拒否され、理由が出る。再起動しても回数は保たれる。
- U4 通信断・再起動・wake で Blocked(UsageLimit/TurnFailed) のタスクが勝手に Ready にならず、再開でも自動では送らない（再開の確認でチェックしたものだけ）。
- U5 pending に無い要求への回答は「回答できません（理由）」と表示され、AlreadyResolved（解決済み）とは出ない。何も送らない。
- U6 回答待ちが、組織画面の塊の帯・表・注記行、左一覧の行、ドックに件数つきで出る。組織画面から直接答えられる。回答待ちの間は停滞通知を出さない。
- U7 手動切替後、「結果を取り込む」で選んだ完了turnの最終回答が検証され、合格なら通常どおり完了（Workerはコミット）、不合格なら手動のまま理由が出る。エンジンはそのチャットへ送らない。取込み中は通常送信を拒否する。
- U8 「失敗として確定」は利用者の操作だけで Failed にする。
- U9 「止める…」の3項目がそれぞれ別の範囲で働き、「実行中も止める」でタスクを取消・失敗にしない。

## 7. 単体テスト（純粋ロジックだけ）

- `org-engine`: `retry_kind` の全分岐／turn_ended の4種（上限・失敗・区分不明・中断）の行き先と halt／`RetryRequested` の上限と再起動後の再生／`Dispatched` で retry_pending が落ちる／`UserAdoptManual` の前提と、形式不合格で Blocked(Manual) に戻り Send が出ないこと／`UserFailTask` の可否／Cause::Engine で Failed・Cancelled にならない既存テストの維持。
- ホスト: `turn_end_kind_of` の3区分、`turn_to_record` の failure、adapter の `NotDeliverable` の判定関数（source 比較を純粋関数に切り出す）、adopting の送信拒否判定。
- TS（任意）: `taskRequestWait`。

## 8. 実機確認（ユーザー）

1. 承認が要るタスク（Worker にネットワーク取得をさせる等）で、組織画面の注記行のカードから承認 → 進む。左一覧・ドックに件数が出ていたこと。
2. 承認待ちのまま App Server を再起動（またはネット切断）→ カードを押す → 「回答できません（接続が切り替わった）」が出る。中断 → 「新しい試行で再送」で続く。
3. 利用上限に当たったら（再現できるときだけ）タスクとワークフローの表示を確認 → 上限解除後に「再開」でチェックしたタスクが1回だけ送られる。
4. 実行中のタスクを「手動に切替」→ チャットで続きを指示し、形式どおりの結果を出させる → 「結果を取り込む」→ 完了・コミット・次のタスクの開始を確認。形式外の回答では手動のまま戻ること。
5. 「止める…」の3項目をそれぞれ試し、範囲どおりかを確認。

## 9. 要判断

- J-R1 旧版で `Failed(ExecutionFailed)` になった既存タスク（上限停止が失敗扱いになったもの）は、終端の不変条件を守るため**復活させない**（推奨）。復活させる場合は「失敗を見直す」専用の利用者操作と遷移の追加が要る。
- J-R2 正本 §3.15.2／§3.15.5 に「turn の失敗・上限は止めて利用者へ示し、利用者の操作で新しい試行として送れる（自動では送らない、回数上限あり）」を追記する版更新（司令塔）。
- J-R3 `max_retries` 既定3・範囲0〜10（推奨）。
