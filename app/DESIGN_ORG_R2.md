# 組織実行 改修 段階2 設計（R4 進行タイムライン・R5 介入）v0.1（2026-10-19）

対象: `PLAN_ORG_REWORK.md` 段階2と §6 の決定。前提: `DESIGN_ORG_R1.md`（待機理由・再送・手動取込み・「止める…」メニュー・塊の帯、実装済み）、`DESIGN_ORG_INTAKE.md`（実装済み）。
変えない合意: エンジンは承認要求に代行で答えない／自動で再送しない／通信断・受理不明を done・failed にしない／監視目的でプロンプトを足さない（**利用者が書いた指示を利用者の操作で届けるのは可**）／`~/.codex` を触らない。

## 0. 結論（判断の要点）

1. **turn を始めるのは引き続きエンジンだけ**。利用者の指示は2つの道で届ける: ①実行中の turn への**割り込み**（Codex `turn/steer`、`expectedTurnId` 付き。0.162.1 stable、adapter は `SendMode::Steer` 実装済み）②**次にエンジンが送る turn に添える**（既存の `UserNote` を拡張）。どちらもエンジンのイベントとして `events.jsonl` に残す。Q9 は「通常送信不可・手動切替だけ」→「通常送信不可。割り込み指示と追加指示は可。手動切替も残す」に変わる。
2. 割り込みが**確実に届かなかった**（拒否・非live）ときだけ、送信時に利用者が選んだ「届かなければ次の送信に添える」（既定オン）で注記へ回す。**受理不明は何もしない**（照合の結果を見て利用者がボタンで選ぶ）。
3. ワークフロー単位の追加指示は「今後エンジンが送る全 turn に、タスクごと1回だけ添える」。実行中の turn には触らない。計画（タスクの分け方）は変えない。変えたいときは既存の「計画を作り直す」に理由として渡す（作り直せる間だけ）。
4. 差し戻し上限は**終端（Failed）にせず `Blocked(ReworkLimit)`** にして利用者に選ばせる（R1 の方針と同じ）。利用者の差し戻しは自動の回数に数えず、turn 上限には数える。
5. タイムラインはホストが events を人間向けに畳み込んで保持し、差分を1秒に1回だけ配る。「いまの動き」は UI がスナップショット（タスク＋役割チャットの `current_activity`・鮮度）から1タスク1行で作り、表示は3秒保持・1秒スロットル。

## 1. R4 進行タイムライン

### 1.1 データの流れ
- 純関数 `rules/org_timeline.rs` `TimelineFold`: `push(&OrgEvent, &TaskLookup) -> FoldChange`（追加・直前行の更新・なし）。タスクごとに「今の業務状態に入った時刻」`since` と確認状態も持つ（「照合で確認」の判定に使う）。
- `host/org/timeline.rs` `TimelineBuf`（実行中ワークフローだけ。最新500行、`VecDeque`）。driver が `Persist` の**追記成功後**に `push`。復元時は `store.read_events()` 全体を畳み込んで作る（壊れた末尾行は既存どおり無視）。履歴（読取り専用）のワークフローは開いたときに読んで畳み込み、保持しない。
- driver はバッチ終わりに `HostEvent::OrgTimelineUpdated{workflowId, lastSeq}` を出す（**1秒に1回まで、末尾は必ず出す**）。UI は `org_timeline{afterSeq}` で差分だけ取る。

### 1.2 整形規則（`render`。文は120字まで、引用は先頭80字＋「…」）

|イベント|表示（主体）|
|---|---|
|Created＋Requested|「ワークフローを開始（依頼: …）」（あなた）|
|TaskAdded（連続）|1行に畳む「計画: t1『…』ほか N件を追加」（Planner）|
|Dispatched|「送信しました（作業／差し戻し／検証／再送／形式修正／計画）」（役割・タスク）|
|StatusChanged|Completed「完了」／Blocked(r)「止まっています（理由）」／Failed「失敗（理由）」／Cancelled「取消し」または skipped なら「スキップ」／Reviewing「検証待ちへ」／Rework「差し戻し（n回目）」／Cause::User の Ready「あなたが再開」。Engine の Pending・Ready・Running は出さない|
|ConfirmChanged|SendUnknown「受理を確認できません」／NeedsReconcile(c)「接続断・スリープ復帰・再起動のため照合待ち」／StopUnconfirmed「停止を確認できません」／非確定→Confirmed のときだけ「照合で確認」|
|DispatchSettled|Rejected「送信が断られました」／Reconciled*「照合: 届いていました／届いていませんでした」。Accepted・Unknown は出さない（他の行と重複）|
|TurnObserved|「turn 完了（結果を確認中）」「利用上限で終了」「turn が失敗」「中断で終了」（Codex）|
|Committed／Halted／LimitsChanged|「コミット abc1234」「全体を停止: ラベル」「再開」「turn 上限を N に」（AgentDock／あなた）|
|グラフ系（SlotWritten・EdgeTaken・GateDecided 等）|「{task} が {slot} を出力（第v版）」「{from} → {to}」「あなたの判断: {choice}」|
|UserNoteAdded／Withdrawn|「追加指示（このタスクへ／全体へ）: …」「追加指示を取り消し」（あなた）|
|InterjectRequested／Settled|「割り込み指示: …」→「届きました／届きませんでした（理由）／確認できません」（あなた）|
|RetryRequested・TaskSkipped・UserReworkRequested・TaskHeld|「新しい試行を指示（n/max）」「スキップ」「あなたが差し戻し」「保留／保留を解除」（あなた）|
|ChatRequested・ChatBound・TurnSpent・FindingsRecorded|出さない|

行の型: `OrgTimelineEntry{ seq, seqTo, at, task?, actor: {kind: user|role|engine|codex, label}, tone: info|good|warn|bad|wait, text, count }`。

### 1.3 「いまの動き」（役割ごとの現在の活動1行）
- `src/ui/orgActivity.ts` `activityLines(wf, snap, now)`（純関数）: 終わっていないタスクを1行ずつ、R1 の塊（あなたの対応待ち → 作業中 → 順番待ち（件数だけ、展開可）→ 完了（件数だけ））に並べる。
- 行の形:「Worker・t2『ログイン画面』｜作業中｜コマンド: cargo test｜12分」。
  - 3列目: 作業中は役割チャットのルートエージェントの `current_activity`（Command「コマンド: …」、FileChange「ファイル変更: …」、Reasoning「考えています」、AgentMessage「回答を書いています」、ToolCall／WebSearch／SubAgent 各ラベル）。summary が未取得なら「（内容は未取得）」。止まっているものは R1 の待機理由（`taskRequestWait`・状態ラベル）、順番待ちは「t1 の完了待ち」「保留中」。
  - **鮮度は別の印**で出す（live でなければ「最新ではありません（接続断等）」の印を足す。状態の文字は変えない）。
  - 経過は `OrgTaskView.since` から分単位（「1分未満」「12分」「2時間5分」）。since が無い（古い記録で辿れない）なら出さない。
- チカチカ防止: Dock の `useThrottled` を `src/ui/useThrottled.ts` へ移して共用し、入力を1秒スロットル（承認・質問の発生は即時通過＝Dock と同じ bypass）。`stabilize(prev, next, now)`: 業務状態が同じなら活動文は**最低3秒は替えない**。経過時間の再描画は30秒ごと。1行は CSS で省略、全文は title。
- 押すと: 行＝タスク表の該当行へ移動、チャット名＝役割チャットを開く（閲覧）。

### 1.4 置き場所
組織画面: 帯（R1）の直下に「いまの動き」。その下を**タブ「タスク｜経過」**にし、「経過」が `OrgTimeline.tsx`（新しい順、200行表示＋「さらに表示」、絞り込み「すべて／あなたの操作と止まった所／このタスク」）。上へスクロールしていない間は新しい行を差し込まず「新しい記録 N件」を出す（位置が飛ばない）。タスク表の行「…」に「このタスクの経過」。

## 2. R5 介入

### 2.1 (a) 役割チャットへの割り込み指示

経路の判定（純関数 `rules/org_interject.rs` `interject_route(task, live, active_turn, caps) -> Steer{turn} | NextTurn | Refuse(reason)`）:

|タスクの状況|経路|画面の文言（入力欄の上）|
|---|---|---|
|Running・Confirmed・受理済み turn T が終わっていない・live・チャットの実行中 turn が T・能力 `steer`=Supported|**Steer{T}**|「実行中の turn に今すぐ届けます」|
|Running で送信の受理待ち（`last_settle` なし）|Refuse|「送信の受理を確認中です。少し待ってから送ってください」|
|Running で T は終了済み（結果の確認・コミット中）、非live、能力なし、Pending・Ready・Blocked・Rework・Reviewing、確認状態が未確定|**NextTurn**|「次にこのタスクへ送るとき（差し戻し・再送など）に添えます。このタスクにもう送信がなければ届きません」|
|終端／Blocked(Manual)・解放済み／取込み中|Refuse|「終わったタスクです。ワークフローへの追加指示を使ってください」／「手動に切り替え済みです。チャットに直接書けます」／「結果の取り込み中です」|

流れ（Steer）: UI `org_interject{workflowId, task, text, fallback}` → エンジン `UserInterject` が経路を再確認 → `InterjectRequested{id, task, turn, text, fallback}` を記録 → `OrgAction::Steer{task, id, chat, turn, clientMessageId, text}` → driver が `ports.steer` を実行。本文は `prompt::interject_prompt`:「（利用者からの追加指示）\n{text}\n\nこの指示を踏まえて今の作業を続けてください。最後の結果は、これまでどおり指定の形式で返してください。」（outputSchema は付けない＝Steer には付かない既存仕様）。
- 送信の権限: 新しい `SendAuth::OrgInterject(InterjectPermit{chat, turn})`。**`SendMode::Steer{turn}` で宛先と turn が一致するときだけ通す**（NewTurn は型で拒否）。ワークフローの送信権（一時停止で失効）は要らない（新しい turn を作らないため）。`check_sendable_core` の判定は通常どおりかける（停止未確認・受理不明のチャットへは送らず NotDelivered）。
- 結果の受け口: `observe.rs` に「割り込みの試行ID → (タスク, id)」表を持ち、`on_send` で**先にこの表を見て** `InterjectSettled` にする（通常の `SendSettled` にしない。混ぜるとエンジンの試行照合が壊れる）。Accepted→Delivered（応答の turnId が T と違えば Delivered＋「別の turn に入りました」注記）、Rejected／NotLive→NotDelivered{理由}、AcceptanceUnknown→Unknown、照合後→ReconciledDelivered／ReconciledNotFound。
- NotDelivered かつ fallback=真 → エンジンが同じ文を `UserNoteAdded{scope: Task}` にする（**確実に送っていないときだけ**）。Unknown・ReconciledNotFound は自動では何もせず、行に「次の送信に添える」「破棄」を出す（`UserInterjectFallback{id}`）。
- 再起動: 決着していない割り込みは「届いたか確認できません」。再起動後の照合（driver の読取り照合）で、そのチャットの履歴を `find_client_message` で探して Reconciled* にする。再送はしない。
- Codex 側の履歴にも利用者の発言として残る（steer の userMessage）。

### 2.2 (b) ワークフロー単位の追加指示
- 既存の `UserNote`（`NoteScope::Node`・`AllFollowing`）を使う。追加: `NoteScope::Task{task}`、`UserNote.withdrawn`。
- 配る規則（純関数 `org-engine/src/notes.rs` `notes_for(state, task) -> (Vec<&UserNote>, upto)`）: 取り消していない注記のうち、`seq > max(task.notes_seen_seq, task.last_output_seq)` で、scope が AllFollowing／Task{自分}／Node{自分のノード}（計画経路の再計画は従来どおり Node{タスクID}）。**グラフ・計画・再送・再計画・Planner の全 prompt** に「## 利用者からの追加指示」節として付ける。形式修正の turn には付けない。
- 配達の記録: `Dispatched.notesUpto`（`#[serde(default)]`）→ `Task.notes_sent_upto`。`DispatchSettled` が Accepted／ReconciledAccepted のとき `notes_seen_seq = notes_sent_upto`。拒否・不明では進めない（次の送信で再び添える。受理不明のタスクは送らないので二重にならない）。
- 実行中の turn には触らない。入力欄の下に配り先の予告:「次に送るタスクへ添えます: t3, t4（これから計画されるタスクにも添えます）。実行中の t2 には届きません（届けるなら t2 の割り込み指示を使ってください）」。
- 計画との関係: Planner 実行中＝「計画に反映するなら Planner への割り込み指示を」。計画承認待ち（`AwaitingPlanApproval`）で計画後に追加指示がある＝承認カードに「計画の作成後に追加した指示が N件あります（計画には反映されていません。各タスクには届きます）」と「この指示を理由に計画を作り直す…」（既存 `UserReplan{note}`、作り直せる間だけ）。グラフ経路は計画がないので注記だけ。
- 取り消し: まだどのタスクにも届いていない、または一部に届いた注記を「取り消す」→ 以後は添えない（届いた分は戻せない旨を表示）。
- 依頼整理チャットの帯（INTAKE §1.2-5）に「ワークフローへ追加指示…」を足し、同じ入力欄を開く（本文を写すだけ。自動では送らない）。

### 2.3 (c) スキップ・差し戻し・保留（タスク単位）／止める範囲

|操作|対象（計画経路の Worker・Reviewer。グラフ経路と Planner は拒否）|結果|
|---|---|---|
|**スキップ…**|Pending・Ready・Blocked(*)・Rework（待機中の検証）で、確認状態 Confirmed、開いた試行・作成中・flow なし|`TaskSkipped`＋Cancelled(User)。Worker を飛ばすと、それを検証する Reviewer も同時に飛ばす。Reviewer だけを飛ばし対象 Worker が Reviewing なら Worker は**検証なしで完了**(User)。後続の Worker は飛ばしたタスクを「満たした」とみなし、prompt の依存欄に「（スキップ: 成果なし）」|
|**差し戻す…**（文必須、上限停止時は任意）|Worker が Reviewing で検証タスクが実行中でない、または `Blocked(ReworkLimit)`|`UserNoteAdded{Task}`（利用者の指摘）＋`UserReworkRequested`（`user_rework_count+1`、`rework_count` は増やさない）→ Rework→Ready(User)。検証タスクは待機へ（Ready→Pending、Blocked→Rework）。turn 残り0なら拒否|
|**このまま受け入れる…**|`Blocked(ReworkLimit)`|Worker を Reviewing→Completed(User)、その Reviewer をスキップ。表示「検証不合格のまま受け入れ」|
|**保留／保留を解除**|Pending・Ready（未送信）|`TaskHeld{held}`。スケジューラは保留中を送らない。依存先は待つ|
|**このタスクだけ止める…**|Running|そのチャットへ既存の中断（`OrgOp::Stop`）。他のタスクとワークフローは続く。10秒で停止未確認の案内。止まれば Blocked(ManualStop)＝R1 の再送ができる|

- 差し戻し上限: `complete_verdict` で上限到達 → **`Blocked(ReworkLimit)`**（今は Failed(LimitReached)）。指摘は `FindingsRecorded{task, findings}` で保存し `Task.last_findings` に保つ（今のメモリ上の `findings` は再起動で消えるため置き換える）。選択肢は「もう一度直させる…」「このまま受け入れる…」「失敗として確定…」（R1 の `UserFailTask` に ReworkLimit を許可）。旧データの Failed(LimitReached) は復活させない（J-R1 と同じ）。
- `committed` の「検証役がいるか」は**スキップ・取消し済みの Reviewer を数えない**（今は数えるため、取消し後に Reviewing で止まる）。
- 止める範囲は R1 の「止める…」（全体: 新しい送信だけ／実行中も）＋上表のタスク単位で分ける。メニュー見出しを「ワークフロー全体」「このタスク」に分ける。

## 3. 型・イベント・状態の追加（`org-engine`）

|追加|内容|互換|
|---|---|---|
|`NoteScope::Task{task}`、`UserNote.withdrawn`|2.2|serde 既定|
|`Task.notes_sent_upto: Option<u64>`・`notes_seen_seq: u64`・`skipped`・`held: bool`・`user_rework_count: u32`・`last_findings: Vec<String>`|上記|`#[serde(default)]`|
|`Dispatched.notes_upto: Option<u64>`|配達の記録|`#[serde(default)]`|
|`BlockedReason::ReworkLimit`|差し戻し上限|新列挙子|
|イベント `InterjectRequested{id: NoteId, task, turn, text, fallback}`・`InterjectSettled{id, outcome}`・`UserNoteWithdrawn{id}`・`TaskSkipped{task}`・`UserReworkRequested{task}`・`FindingsRecorded{task, findings}`・`TaskHeld{task, held}`|`WorkflowState.interjects: Vec<Interject>`（outcome は None＝未決着）|新イベント、`SCHEMA_VERSION` は2のまま（R1 と同じ扱い）|
|`InterjectOutcome = Delivered{note?} | NotDelivered{reason} | Unknown | ReconciledDelivered | ReconciledNotFound | FallbackNoted | Discarded`|決着は1回。`Unknown` の後の Reconciled* と、Unknown／NotFound の後の FallbackNoted／Discarded だけ上書き可|—|
|`OrgInput::UserInterject{task, text, fallback, active_turn: Option<TurnRef>}`・`InterjectSettled{id, outcome}`・`UserInterjectFallback{id, add: bool}`・`UserAddNote{text, task: Option<TaskId>}`・`UserWithdrawNote{id}`・`UserSkipTask{task}`・`UserSendBack{task, note: Option<String>}`・`UserAcceptAsIs{task}`・`UserHoldTask{task, held}`|利用者操作はすべて Cause::User。拒否は `Refused{理由}`|—|
|`OrgAction::Steer{task, id, chat, turn, client_message_id, text}`|turn を作らない。`TurnSpent` なし|—|
|純関数 `skip_plan(state, task) -> Result<SkipPlan{skip: Vec<TaskId>, complete_unreviewed: Option<TaskId>}, String>`、`send_back_check(...)`|画面の可否と確認文にも使う|—|

ALLOWED 表は変更不要（Blocked→Reviewing・Reviewing→Completed・Ready→Pending・Blocked→Rework は既存）。文字数上限: 割り込み・注記とも4,000字（`NOTE_MAX_CHARS`）、注記の未配達は全体で50件まで。

## 4. IPC（`backend/org.rs`、ts-rs 再生成）

- 新コマンド: `org_timeline{workflowId, afterSeq?, limit}` → `OrgTimelineView{entries, lastSeq, truncated}`／`org_interject{workflowId, task, text, fallback}` → `OrgInterjectAck{id, route}`／`org_interject_fallback{workflowId, id, add}`／`org_add_note{workflowId, text, task?}`／`org_withdraw_note{workflowId, id}`／`org_skip_task`・`org_send_back{…, note?}`・`org_accept_as_is`（`UserConfirmed`）／`org_hold_task{workflowId, task, held}`。
- `OrgTaskView` 追加: `since: Option<UnixMillis>`、`interject: OrgInterjectRouteView{route: steer|nextTurn|refused, label}`、`skip: Option<OrgSkipView{also: Vec<String>, completesUnreviewed: Option<String>}>`・`skipRefusal`、`canSendBack`・`canAcceptAsIs`、`held`、`notesPending: u32`。`OrgWorkflowView` 追加: `notes: Vec<OrgNoteView{id, text, scope, at, deliveredTo: Vec<String>, pendingFor: Vec<String>, withdrawn}>`、`interjects: Vec<OrgInterjectView{id, task, textHead, at, outcome}>`（決着していないもの＋最新20件）、`notesAfterPlan: u32`。
- `HostEvent::OrgTimelineUpdated{workflowId, lastSeq}`。

## 5. 画面（置き場所と文言）

|置き場所|内容|
|---|---|
|役割チャット（`CenterPane.tsx`、所有中）|今の送信ロック文（`ORG_SEND_LOCK`）を**割り込み入力欄**（`InterjectComposer`）に置き換え。経路ラベル（2.1 の表）、チェック「届かなければ次の送信に添える」（既定オン）、ボタン「指示を送る」。送ったものはカード「割り込み指示（届きました／確認できません…）」で残す。添付・キュー・モデル変更は出さない|
|組織画面の下端|**ワークフローへの追加指示**の入力欄＋配り先の予告（2.2）＋「追加する」。直上に未配達・取り消せる注記の一覧（「取り消す」）|
|タスク表の操作列「…」|「指示を挟む…」（InterjectComposer をポップオーバーで）、「スキップ…」「差し戻す…」「このまま受け入れる…」「保留」「このタスクだけ止める…」「このタスクの経過」。不可は title に理由|
|確認ダイアログ（`OrgDialogs.tsx`）|スキップ:「t3 をスキップします。あわせて t4（検証）も飛ばします。成果は後続へ渡りません。元に戻せません。」／差し戻し:「t2 を、次の指摘で直させます（自動の差し戻し回数には数えません。turn を1つ使います）」＋入力欄／受け入れ:「検証で不合格のまま t2 を完了にします。取込みは利用者が行います（自動マージしません）」|
|承認カード|計画後の追加指示の件数と「この指示を理由に計画を作り直す…」|

## 6. 版更新の文言案（司令塔）

- 正本 §3.15.2 の「組織チャットへの通常送信…（Q9）」を置換:「組織チャットへの通常送信・完了後キューは無効で、turn を始めるのはエンジンだけ。利用者は、実行中の turn への割り込み指示（バックエンドが対応する場合のみ、宛先の turn を指定して1回）と、タスク・ワークフローへの追加指示（次にエンジンが送る turn に、タスクごと1回添える）を送れる（版2.2）。いずれも利用者の操作で送り、ワークフローの記録に残す。割り込みは確実に届かなかったときだけ、利用者が選んだ場合に追加指示へ回し、受理不明では何もしない。介入として『このタスクを手動に切り替える』も残す（Q9 変更）。中断は利用者の操作で行える。」受入に A151（割り込み・追加指示が記録に残り、受理不明で再送・転記されない）。
- §3.15.5 差し戻しの行に追記:「上限に達したタスクは失敗にせず止め、利用者が『もう一度直させる』『このまま受け入れる』『失敗として確定』を選ぶ。利用者の差し戻しは自動の回数に数えず turn 上限に数える。スキップ・保留は未着手か止まっているタスクだけ、利用者の確認で行う（版2.2）。」受入 A152。
- §3.15.8 に追記:「ワークフロー画面に、タスクごとの現在の活動1行（状態・待機理由・経過時間。鮮度は別の印）と、記録を整形した経過（時系列）を出す。表示は1秒に1回までに間引く（版2.2）。」受入 A153。
- `DESIGN_ORG.md` 表 Q9 の行と §1.4 第3項:「介入は手動切替に加えて、割り込み指示（steer）と追加指示（次の turn に添える）。turn の開始はエンジンだけ（2026-10-19 変更、`DESIGN_ORG_R2.md`）」。

## 7. 実装タスク（実装担当1名が順に）

1. **R2-1 エンジンの型・イベント**: `org-engine/src/types.rs`・`persist.rs`（apply・serde 既定・interjects）・`notes.rs`（新規 `notes_for`）・`lib.rs`。`cargo test -p org-engine`（追加分）。
2. **R2-2 エンジンの遷移**: `core.rs`（新 OrgInput、Steer action、InterjectSettled と fallback、Dispatched の notes_upto・settle での notes_seen_seq）、`planned.rs`（ReworkLimit・FindingsRecorded・`committed` の検証役判定・dep_ok のスキップ・`ready_turn` に注記）、`graphrun.rs`（`notes_for` へ置換）、`scheduler.rs`（held・skipped）、`prompt.rs`（注記節・`interject_prompt`・依存欄のスキップ表示）。
3. **R2-3 ホストの割り込み経路**: `rules/org_interject.rs`（新規 `interject_route`）、`host/mod.rs`（`SendAuth::OrgInterject`）、`host/org/ports.rs`（`steer`）、`host/org/driver.rs`（Steer 実行・再起動照合で未決着の割り込みを探す）、`host/org/observe.rs`（試行表の振り分け）、`host/org/grant.rs`（`InterjectPermit`）。
4. **R2-4 タイムライン**: `rules/org_timeline.rs`（新規、fold と render）、`host/org/timeline.rs`（新規 buf）、`driver.rs`（追記後の push・1秒の合図）、`host/org/view.rs`（since・interject・skip 等）。
5. **R2-5 IPC**: `backend/org.rs`、`host/org/ipc.rs`、`commands.rs`、`lib.rs`、ts-rs 再生成、`src/ipc/client.ts`。
6. **R2-6 画面**: `src/ui/useThrottled.ts`（Dock から移動）、`src/ui/orgActivity.ts`（新規）、`src/ui/OrgTimeline.tsx`・`src/ui/InterjectComposer.tsx`（新規）、`OrgPane.tsx`、`OrgDialogs.tsx`、`CenterPane.tsx`、`OrgIntake.tsx`（帯のリンク）、`Dock.tsx`。`tsc --noEmit`。
7. **R2-7 文書**（司令塔）: 正本 2.2、`DESIGN_ORG.md`、`LIVE_CHECK_ORG_R2.md`。

## 8. 受入条件

- V1 割り込みは利用者の操作1回で `turn/steer` を最大1回だけ送る。`expectedTurnId` は受理済みの turn。新しい turn は作らず、turn 数・送信権に影響しない。
- V2 拒否・非live のときだけ、チェックがあれば追加指示へ回る。受理不明・照合で見つからないでは自動で何もせず、2つのボタンが出る。再起動後は照合だけで決着させる。
- V3 割り込みの結果がエンジンの `SendSettled` に混ざらない（同じチャットの通常送信の照合が壊れない）。
- V4 追加指示は実行中の turn に送られず、各タスクの次の turn に1回だけ入る（拒否・受理不明の送信では消費しない）。形式修正には入らない。取り消した注記は以後入らない。
- V5 差し戻し上限で Failed にならず `Blocked(ReworkLimit)`。3つの選択肢がそれぞれ働き、利用者の差し戻しは `rework_count` を増やさない。
- V6 スキップは確認つきで、検証役の連鎖と「検証なしで完了」が確認文どおり。実行中・受理不明・作成中のタスクは拒否。取消し・スキップ済みの検証役しかない Worker はコミット後に完了する。
- V7 タイムラインに 1.2 の行が出て、隠す行は出ない。画面の更新は1秒に1回まで、活動文は3秒保持、スクロール中に位置が飛ばない。状態と鮮度が別に出る。
- V8 Q9 以外の合意（承認の代行なし・自動再送なし・通信断を成否にしない）を変えない。`cargo check`・`tsc --noEmit`・追加テストが通る。

## 9. 単体テスト（純粋ロジック）

- org-engine: `notes_for`（scope・閾値・取り消し・グラフ互換の last_output_seq）／notes_seen_seq が Accepted だけで進む／Interject の決着の上書き規則と fallback が NotDelivered だけで動く／`skip_plan`（連鎖・検証なし完了・拒否条件）／`UserSendBack` の可否と rework_count 不変／ReworkLimit の行き先と3選択肢／`committed` が取消し済み検証役を数えない／held が送られない／再起動の再生で interjects・last_findings が戻る。
- ホスト: `interject_route` の全行、observe の割り込み振り分け、`InterjectPermit` が NewTurn・別 turn を拒否、`TimelineFold`（畳み込み・隠す行・照合で確認・since）。
- TS（任意）: `activityLines` の並び、`stabilize`。

## 10. 実機確認（ユーザー）

1. Worker 実行中に役割チャットから「READMEにも一言追記して」を割り込み → 同じ turn の中で反映され、最終回答が形式どおり、タイムラインに「届きました」。
2. turn が終わる直前に割り込み（拒否を起こす）→「届きませんでした」→ 次の差し戻し・再送の turn に添えられる。承認待ち中の turn への割り込みも挙動を記録（Codex の対応を確認）。
3. 割り込み送信中に App Server を止める → 「確認できません」、自動では何も送らない。再接続後に照合で決着。
4. ワークフロー追加指示を2件 → 実行中のタスクには入らず、次に始まるタスクの依頼文に1回だけ入る。計画承認待ちで追加すると承認カードに件数と作り直しが出る。
5. Reviewer に2回差し戻させて上限 →「止まっています（差し戻し上限）」→ 3つの選択肢を試す。
6. 未着手の Worker をスキップ → 検証役も飛び、後続が進む。保留したタスクが送られない。
7. 3並列の間、「いまの動き」がチカチカせず、経過時間が進み、接続断で「最新ではありません」の印だけが付くこと。

## 11. 要判断

- J-R2-1 割り込みのフォールバック既定: 「届かなければ次の送信に添える」**既定オン（推奨）**／既定オフ。確実に送っていない場合に限るので重複はない。
- J-R2-2 スキップ・差し戻しを**計画経路だけ**にする（推奨。グラフ経路は既存のゲート・周回で代替）／グラフにも広げる（辺の選択が成果に依存するため設計が別に要る）。
- J-R2-3 完了済み Worker の差し戻しは**扱わない（推奨。終端の不変条件を守り、追加指示で後続へ伝える）**／「追加の作業タスクを作る」操作を段階3以降で検討。
- J-R2-4 正本の版番号 2.2・受入番号 A151〜A153 は司令塔が確定する。
