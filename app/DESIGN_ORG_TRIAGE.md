# DESIGN_ORG_TRIAGE — 停止トリアージ（止まったときの原因の読み解きと次の一手の提案）

版 0.1（2026-10-24）。対象: 組織実行（正本 §3.15）の計画経路。前提: 診断カード（`OrgStopView{kind,headline,facts,evidence,actions,hint}`、`OrgPane.tsx`／`orgText.ts`）は別担当が実装中で、本書はその**最上部に1枠足す**。

変えない合意: 承認要求に代行で答えない／自動で再送しない／通信断・受理不明を done・failed にしない／監視目的で resume・プロンプト追加をしない／`~/.codex` を書かない／自動マージしない（Q7）／上限で止まったタスクの選択は利用者（§3.15.5）。

---

## 0. 結論

1. 止まったら**読み取り専用の分析役 `triager`（新しい標準役割）**が**新しいチャットで1回だけ**原因を調べ、構造化出力で「診断・推奨1つ・根拠・確信度・リスク」を返す。
2. 起動は**自動（ワークフロー単位の設定、既定オン）**。ただし送信権 `OrgGrant` がある間だけ、turn上限に数え、残り2以上・回数上限内のときだけ。条件を満たさないときは［原因を調べる］ボタン（利用者操作）。→ 正本の版更新が要る（要判断 J-T1）。
3. **実行は必ず利用者の1操作**。提案はエンジン・ホストのどちらも自動で適用しない。低リスクの自動適用オプションは**入れない**（§3.15.5「上限で止まったら利用者が選ぶ」と衝突するため）。
4. 実装は最小で効く順: **段階A＝ReworkLimit／CheckFailed／NothingWritten の3種**、段階B＝TurnFailed・InvalidArtifact・調査報告が読めない、段階C＝計画の部分修正（Plannerの吸収）とグラフ経路。

## 1. 分析役: Planner 再利用か triager 新設か

|案|利点|欠点|
|---|---|---|
|P1 Planner の既存チャットへ送る|計画の意図を会話として持っている|完了済みタスクへ送ることになり「ready のタスクだけ送る」に反する／自分の計画の誤りを認めにくい（自己弁護）／文脈が肥大し再計画のたびにチャットが変わる／Planner の出力スキーマと混ざる|
|P2 Planner の**定義**で新チャット|定義ファイルが増えない|役割IDが Planner なので計画タスクと区別できず、表・所有判定・policy に特例が要る|
|**T 新しい役割 `triager`（推奨）**|毎回まっさらな文脈で、渡す材料をエンジンが決められる（上限つき）／読み取り専用を定義で強制できる／計画・作業タスクと所有・表示・集計を分けられる|定義ファイルが1つ増える|

- 定義ファイル `org\roles\triager.json`（初回・欠落時だけ作成、既存は上書きしない）。`sandbox=readOnly` 固定（`workspaceWrite` 指定は狭めて読込み、理由を表示）、承認方針は役割の値（既定 on-request、承認は利用者へ。エンジンは答えない）、モデル未指定＝チャット既定、effort は Planner と同じ既定。
- 作業場所: 対象 Worker の worktree を**読取りで**使う（Worker は止まっていて書かない）。worktree が無い（Planner・調査・未作成）なら作業フォルダを読取りで。新しい worktree は作らない（片付けの負担を増やさない）。
- 「Planner による上位での吸収」は、triager が計画の誤りと診断したときに**計画の部分修正案**を出すこと（§5）で満たす。計画の再作成そのものは Planner へ回さない（turn と操作が1つずつ増えるため）。

## 2. 起動（いつ・何を渡すか）

### 2.1 自動か手動か（推奨: 自動・既定オン）

|案|コスト|合意との整合|利用者の手間|
|---|---|---|---|
|M 押したときだけ|止まった回数×押した分|利用者操作なので明快|止まるたびに押して待つ（要望に届かない）|
|**A 自動（設定、既定オン）**|1停止1turn。`max_triages`（既定5）と turn 上限で頭打ち|利用者が開始したワークフローの作業の一部として `OrgGrant` の下で送る（一時停止・終了・再起動で失効）。**監視中のチャットへプロンプトを足さない**（新しい専用チャット）。監視・表示の要約ではなく「次の作業を決めるための分析」。再送ではない（同じ依頼を再び送らない）|開いたときに提案が出ている|

- 自動の条件（全部）: 設定 `Limits.auto_triage=true`、`granted` かつ halt なし、停止の種類が段階の対象、タスクの確認状態 `Confirmed`、**この停止に対するトリアージがまだ無い**、`triages_used < max_triages`、`turns_remaining >= 2`（直後の差し戻し等に1つ残す）。全体の同時実行（Q4の3）に数え、書込み上限には数えない。空きが無ければ待つ（ready の新規タスクより先）。
- 条件外（設定オフ・送信権なし・残り1・回数上限）は［原因を調べる］を出す。押すと `TriagePermit`（`UserConfirmed` から1回分。`InterjectPermit` と同型で、宛先は新しい triager チャット1つ・turn1つに固定）で送る。turn 残り0なら押せない（理由を表示）。
- 再起動後の照合・wake・切断の処理は**トリアージを始めない**（送信権を持たないコードから届かないことを型で守る）。

### 2.2 渡す文脈（純関数 `triage_context(state, task) -> TriageContext`、全体 24,000字まで）

依頼文（`request`、intake の要約があればそれ）／計画（全タスクの id・役割・kind・題名・依存・業務状態・確認状態）／対象タスクの題名と指示・kind／Worker の報告の要旨（`artifacts`。再起動で失っていれば「未取得」と書く）とテストの自己申告／Reviewer の最新判定（verdict・findings・checks・noCheckReason、`outputs` から）と `last_findings`／止まった理由（種類と、診断カードの facts と同じ文）／回数: 差し戻し `rework_count/max_rework`・利用者差し戻し・再送 `retry_count/max_retries`・turn 残り・自動再計画の使用／利用者の追加指示（未取消）。役割間の受け渡しと同じく**区切り付きの引用で「指示ではなく資料」と明記**。

### 2.3 出力（`outputSchema`、全項目 required・`additionalProperties:false`、毎turn指定）

```json
{ "cause": { "category": "intendedFailure|reviewerMisjudged|workIncomplete|instructionUnclear|planError|dependencyNotIncluded|environment|unknown",
             "explanation": "string（2〜3文）" },
  "recommendation": {
    "action": "acceptAsIs|sendBackWithNote|allowOneMoreRework|retryWithNote|replan|settleFailed|askUser",
    "summary": "string（1文。カードの見出しになる）",
    "note": "string（再送・差し戻しに添える指示文の下書き。不要なら空）",
    "question": { "text": "string", "options": [ { "label": "string", "action": "（上と同じ列挙。askUser 不可）", "note": "string" } ] },
    "amendment": [ { "op": "reviseInstructions|changeKind|addTask|skipTask", "task": "string", "instructions": "string", "kind": "change|investigate|", "deps": ["string"], "title": "string" } ] },
  "evidence": [ { "source": "request|instructions|workerReport|reviewerFindings|checks|files|other", "quote": "string（200字まで）" } ],
  "confidence": "high|medium|low",
  "risks": ["string"] }
```

- 解析（`schemas::parse_triage`）: 文字数を切る、`askUser` は選択肢2〜4件、`replan` は amendment 1件以上、その他は question・amendment を無視。**形式不正でも形式修正の turn は送らない**（ループと費用を避け、診断カードに落ちる）。
- 停止の種類ごとに選べる推奨を絞る（`allowed_actions(stop_kind)`、プロンプトにも列挙）:

|停止|許す推奨（→ 既存のホスト操作）|
|---|---|
|ReworkLimit・CheckFailed|acceptAsIs（`org_accept_as_is`）／sendBackWithNote（`org_send_back{note}`）／allowOneMoreRework（`org_send_back{note: None}`＝保存済み所見で1周）／settleFailed（`org_fail_task`）／replan／askUser|
|NothingWritten|retryWithNote（`UserAddNote{task}`＋`org_retry`の新しい試行）／acceptNoChange＝acceptAsIs（新 `UserAcceptNoChange`、変更なしで完了・検証役はスキップ）／replan（`changeKind→investigate` 等）／settleFailed／askUser|

## 3. 実行は利用者の1操作

- 入口は1つ: IPC `org_apply_triage{workflowId, triageId, choice: Main|Option(n), editedNote?}`（`UserConfirmed`）→ エンジン `OrgInput::UserApplyTriage{triage, choice, edited_note}`。エンジンが**その場で鮮度（§6）と既存の可否判定**（`send_back_check`・`accept_as_is_check`・`retry_kind`・`fail` 条件・amendment の検証）を再確認し、内部で既存の処理（`UserSendBack` 等と同じ関数）を呼ぶ。拒否は既存の `Refused{理由}`。`TriageApplied{id, choice}` を記録。
- エンジンにも driver にも**提案を自分で適用する経路を作らない**（`UserApplyTriage` を作るのは IPC だけ。単体テストで「`TriageRecorded` の直後の出力に Send・状態変更が無い」を確認）。
- 低リスク自動適用（既定オフ）は**今回入れない**: 対象になりうる acceptAsIs・allowOneMoreRework はどちらも正本 §3.15.5 で利用者の選択と決めた操作で、オプションでも合意の変更になる。必要になれば正本の版更新から（要判断 J-T3、推奨は見送り）。

## 4. 画面（診断カードの最上部に「AI の提案」1枠）

|状態|表示|
|---|---|
|調べています|「AI が原因を調べています…」＋［調査の会話を開く］。下の診断カードはそのまま使える（待たせない）|
|提案あり（単一）|「{summary}　理由: {explanation の1文目}」＋［はい、{動作の短い言い方}］［ほかの方法を選ぶ］。ボタン直上に効果の1文（例「検証で不合格のまま t2 を完了にします。取込みはあなたが行います」）。**このボタン自体が確認**で、2段目のダイアログは出さない|
|提案あり（質問）|質問文＋選択肢（ラジオ、2〜4）＋［これで進める］［ほかの方法を選ぶ］|
|計画の部分修正|修正の差分（タスクごとに「指示: 前→後」「種類: 変更→調査」「追加: t7 …」「スキップ: t5」）＋自動承認できない理由があれば列挙＋［はい、計画を直して進める］|
|提案なし|「提案を出せませんでした（{理由}）。下の選択肢から選んでください」＋［もう一度調べる］（利用者操作。turn を1つ使う旨）|
|古くなった|「この提案は古くなりました（{変わったこと}）」。はいボタンは出さず［もう一度調べる］|
|調べていない|［原因を調べる］（押せない理由があれば title）|

- 折りたたみ「根拠と詳しい説明」: 原因の分類（利用者の語）、説明全文、根拠の引用（出典ラベル付き。リンクにしない）、確信度（低いときは見出しに「自信は低めです」）、リスク、指示文の下書き（**編集可**。編集したら［はい］は編集後の文で送る）。
- ［ほかの方法を選ぶ］は既存の診断カードの選択肢へスクロール・フォーカスするだけ。
- triager チャットは通常一覧から隠し（役割チャットと同じ）、提案枠から開ける。ドックのカードに役割バッジ「調査役」。
- 文言は「turn」でなく既存の「やりとり」に合わせる。Codex の語・型を出さない。

## 5. Planner 相当の吸収: 計画の部分修正（段階C）

- 今の `user_replan` は作業開始後を拒否するため、作業中の「計画を作り直す」は**部分修正 `PlanAmendment`** として新設する。許す編集: 未着手（Pending・Ready・保留）か止まっている対象タスクの `reviseInstructions`／`changeKind`（未着手か NothingWritten の対象だけ）、`addTask`（依存は既存か同じ修正内のタスク）、`skipTask`（既存 `skip_plan` の条件）。完了・実行中のタスクは変えない。
- 検証: 既存 `parse_plan` の規則（DAG・既知役割・kind 規則）＋`evaluate_plan`（`PlanKind::Replan{scope_changed:true}`）。§4.3 の「常に利用者確認」理由は差分と一緒に表示する。適用は利用者の［はい］だけで、これが計画承認を兼ねる（理由を見た上での明示操作）。
- Q5-B（自動再計画1回、未配線）との関係: 部分修正は**利用者承認なので `auto_replans_used` に数えない**。Q5-B の自動再計画は今回も配線しない（自動で計画を変える経路を作らない）。将来配線するなら、トリアージの replan 提案を「自動再計画の1回」に使う案が自然（要判断 J-T4）。
- 適用イベント `PlanAmended{triage, edits}`（イベントで再生でき、旧データには出ない）。

## 6. 保存・再起動・ループ防止

```rust
// org-engine/src/triage.rs（新）
pub struct TriageId(pub String);
pub enum TriageTrigger { Auto, User }
/// The stop this triage is about: the task and the seq of the StatusChanged that blocked it.
pub struct StopRef { pub task: TaskId, pub stop_seq: u64, pub kind: BlockedReason, pub nothing_written: bool }
pub enum TriagePhase { Creating, Sending, Running { turn: TurnRef }, Reading, Proposed, Unavailable(TriageGap), Applied, Dismissed }
pub enum TriageGap { NotLive, SendUnknown, TurnFailed, TurnInterrupted, FinalUnreadable, Malformed(String), Restarted, NoTurns }
pub struct Triage { pub id: TriageId, pub stop: StopRef, pub trigger: TriageTrigger, pub chat: Option<ChatRef>,
                    pub attempt: Option<AttemptRef>, pub phase: TriagePhase, pub proposal: Option<TriageProposal> }
pub fn triage_fresh(state: &WorkflowState, t: &Triage) -> Result<(), String>; // 理由文＝「この提案は古くなりました（…）」
pub fn auto_triage_due(state: &WorkflowState, granted: bool, now_turns_remaining: u32) -> Vec<StopRef>;
pub fn allowed_actions(stop: &StopRef) -> &'static [TriageAction];
```

- 保存: `WorkflowState.triages: Vec<Triage>`（`#[serde(default)]`）、`Limits.auto_triage: bool`（serde 既定 **false**＝旧ワークフローは自動にしない。新規は設定値、設定の既定オン）、`Limits.max_triages: u32`（serde 既定5）、`Counters.triages_used`。イベント `TriageRequested{id, stop, trigger}`・`TriageChatBound{id, chat}`・`TriageDispatched{id, attempt}`・`TriageSettled{id, outcome}`・`TriageRecorded{id, proposal}`（提案本体をイベントに入れる。8KB以内に切る）・`TriageUnavailable{id, gap}`・`TriageApplied{id, choice}`・`TriageDismissed{id}`・`PlanAmended`。`SCHEMA_VERSION` は据え置き（R1・R2 と同じ扱い）。`chat.json.org` に `triage: Option<String>`（serde default）。
- 送信: `OrgAction::TriageCreateChat{id, spec}`／`TriageSend{id, chat, attempt, client_message_id, text, output_schema}`／`TriageReadFinal{id, chat, turn}`。結果は `TriageChatCreated`・`TriageSendSettled`・`TriageFinal`。`TurnEnded{chat,…}` はエンジンが chat→triage 表を先に引く（タスクの試行照合と混ぜない。R2 の割り込みと同じ作法）。`TurnKind::Triage` を `spend_turn` で数える（送る前）。
- 1停止1回: `auto_triage_due` は同じ `StopRef` のトリアージが1件でもあれば返さない。「もう一度調べる」は利用者操作だけで新しい id（同じ停止に何件でも可、`max_triages` と turn 上限に数える）。トリアージは**タスクの状態を一切変えない**。
- トリアージ自身の失敗・通信断・受理不明・turn の失敗・形式不正は `Unavailable(gap)` にし、タスクを failed にしない。**受理不明は再送しない**（「もう一度調べる」は新しいチャットへの利用者操作で、カードに「前の調査が続いている可能性があります」と出す）。
- 再起動: `Creating/Sending/Running/Reading` は「確認中」として読取りだけで照合する（既存の再起動照合と同じ経路。triager チャットの履歴で、受理した turn が完了していれば最終回答を読んで `Proposed`、それ以外は `Unavailable(Restarted)`）。resume・送信はしない。`Proposed` は再起動後もそのまま表示し、毎回 `triage_fresh` で古さを判定。
- 古さ: 対象タスクの業務状態・確認状態が `stop` 時点から変わった、`stop_seq` 以降に対象タスクへ差し戻し・再送・追加指示（Task scope）が入った、計画が修正された、のどれか。turn 残りが減っただけでは古くしない（適用時の可否判定で拒否する）。

## 7. 安全の確認点

triager は readOnly 固定・承認要求へは応答しない（利用者の承認カード）・停滞通知は承認待ちを除く既存規則のまま／Codex の承認要求をトリアージの材料にも推奨にも入れない（AwaitingApproval は対象外）／`~/.codex` を読まない・書かない／UsageLimit は対象外（分析役も同じ上限に当たるため。既存の決まった案内で足りる）／SendUnknown・NeedsReconcile・StopUnconfirmed は対象外（事実が未確定）／提案の本文由来のコマンドをホストで実行しない／通知は既存の「止まりました」即時通知だけ（提案の到着で追加通知しない）。

## 8. 実装タスク（実装担当1名が順に）

|#|内容|ファイル|
|---|---|---|
|T-1|型・イベント・保存（§6）、`triage_fresh`・`auto_triage_due`・`allowed_actions`、`TurnKind::Triage`、`Limits` 追加|`org-engine/src/triage.rs`（新）・`types.rs`・`limits.rs`・`persist.rs`・`lib.rs`|
|T-2|`triage_schema`・`parse_triage`、`triage_context`・`triage_prompt`（停止の種類で許す推奨を列挙）|`schemas.rs`・`prompt.rs`|
|T-3|エンジンの流れ: 自動起動、Create→Send→TurnEnded→ReadFinal→Recorded／Unavailable、chat→triage 表、`UserRequestTriage`・`UserApplyTriage`・`UserDismissTriage`、`UserAcceptNoChange`、再起動時の分類|`core.rs`・`planned.rs`・`intervene.rs`|
|T-4|役割定義 `triager.json`（作成・readOnly 強制）、`TriagePermit`、driver の3動作、`chat.json.org.triage`、所有・隠し表示、再起動照合への追加|`host/org/roles.rs`・`grant.rs`・`driver.rs`・`ports.rs`・`observe.rs`・`mod.rs`|
|T-5|IPC `org_request_triage`・`org_apply_triage`・`org_dismiss_triage`、`OrgStopView.triage: Option<OrgTriageView>`（状態・見出し・効果文・選択肢・根拠・確信度・リスク・下書き・古さ）、設定「止まったら AI に原因を調べさせる」|`backend/org.rs`・`host/org/view.rs`・`ipc.rs`・ts-rs 再生成|
|T-6|画面: 提案枠（§4の全状態）、折りたたみ、下書き編集、［ほかの方法を選ぶ］|`src/ui/OrgPane.tsx`・`orgText.ts`・`org.ts`|
|T-7（段階B）|TurnFailed・InvalidArtifact・調査報告が読めないへ拡張（許す推奨: retryWithNote／settleFailed／askUser）。DependencyBlocked は上流の停止の提案へリンク（新たに調べない）|上記|
|T-8（段階C）|`PlanAmendment` の検証・差分表示・適用、`PlanAmended`、グラフ経路の RoundLimit|`planned.rs`・`policy.rs`・`schemas.rs`・`view.rs`・`OrgPane.tsx`|

## 9. 受入条件

1. ReworkLimit・CheckFailed・NothingWritten で止まると、設定オンなら triager が**1回だけ**送られ、提案が診断カード最上部に出る。同じ停止で2回目は自動で送られない。
2. 提案が出ても、［はい］を押すまでタスクの状態・送信は一切変わらない。［はい］1回で該当の既存操作が実行され、`TriageApplied` が残る。
3. 一時停止中・再起動後（再開前）・turn 残り1以下・`max_triages` 到達では自動で送らず、［原因を調べる］（または押せない理由）が出る。
4. triager の turn 失敗・形式不正・切断・受理不明で、対象タスクも triager も failed にならず、診断カードの選択肢がそのまま使える。受理不明で再送されない。
5. 再起動後も提案が表示され、対象タスクの状態が変わっていれば「この提案は古くなりました」になり［はい］が出ない。
6. triager チャットは readOnly で作られ、承認要求に AgentDock が応答しない。`~/.codex` に差分が出ない。
7. 旧データ（triages なし）が読め、旧ワークフローは自動トリアージしない。

## 10. 単体テスト（純粋ロジック）

`auto_triage_due`（条件の各欠落・1停止1回・残り2未満）、`triage_fresh`（状態変化・stop_seq 以降の介入・turn 減少だけでは古くしない）、`allowed_actions` と `parse_triage`（種類外の推奨を拒否、askUser の選択肢数、文字数切り）、`UserApplyTriage` の写像（各推奨→既存処理、古い提案・可否不可は Refused）、`TriageRecorded` の後に Send・状態変更が出ないこと、トリアージの失敗系がタスク状態を変えないこと、再起動時の分類、（段階C）amendment の検証。

## 11. 実機確認（ユーザー）

1. テストが失敗することが依頼どおりのタスク（「失敗するテストを書く」）で ReworkLimit／CheckFailed に止め、提案「このまま受け入れる」と理由が出て［はい］1回で完了になる。
2. 何も書かない Worker（調査的な依頼を change で計画させる）で NothingWritten に止め、「調査に切り替える」または「指示を添えて再送」の提案と下書きが出る。下書きを編集して［はい］で、その文が送られる。
3. 提案が出た後に別の操作（差し戻し）をすると「古くなりました」になる。
4. 調査中に App Server を落とす（または完全終了→再起動）と、提案枠が「確認できません／出せませんでした」になり、診断カードの3択が使え、自動で送り直されない。
5. 設定をオフにすると自動で調べず、［原因を調べる］で1回だけ送られる。

## 12. 要判断

- **J-T1 自動トリアージ（推奨: 既定オンの設定で採用）**: 正本 §3.15.2「エンジンが送れるのは ready タスクの指示だけ」と §1.3「監視・要約のための追加モデル呼出をしない」に、分析役の送信を明記する版更新が要る。文案:「タスクが止まったとき、利用者が開始したワークフローの送信権の下で、読み取り専用の分析役に原因の分析を1回だけ依頼できる（設定で既定オン、turn 上限に数える）。分析は提案だけで、適用は利用者の操作に限る。監視中のチャットへは送らない」。保守的にするなら既定オフ（押したときだけ）。
- **J-T2 turn の数え方**: トリアージを turn 上限（Q11）に数え、さらに `max_triages=5` で頭打ちにする（推奨）。数えない案は費用の上限が見えなくなる。
- **J-T3 低リスク自動適用**: 推奨は入れない（§3）。
- **J-T4 Q5-B 自動再計画**: 今回も配線しない（推奨）。部分修正は利用者承認なので自動再計画の回数に数えない。
