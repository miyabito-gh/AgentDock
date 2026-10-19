# DESIGN_ORG_INTAKE: 依頼整理フェーズ・Gitの準備・アプリからのプッシュ（設計書 v0.1、2026-10-19）

対象: PLAN_ORG_REWORK §6 の追加3点。前提: DESIGN_ORG（Q9）・DESIGN_ORG_GRAPH §5・§11.1・DESIGN_CONT §2（I1〜I6）。
守る合意: `~/.codex` を触らない／自動実行・自動再送・自動プッシュなし／未確認の操作を成功表示しない／通信断を成否に変換しない／確認は選択式でまとめる。

## 0. 結論

1. **会話の相手は「依頼整理チャット」＝利用者の普通のチャット**（`org` 所有記録なし。Q9 の対象外）。専用の軽い対話は作らない。作業フォルダを cwd、権限は `ReadOnly`、継続チャットの**目的メモ**に「依頼整理の進め方」（固定文）を入れて、既存の資料枠で届ける（adapter に developerInstructions を足さない）。
2. 固まった内容は **依頼要約（IntakeSummary）** として `chat.json` の `conversation.intake` に保存する。要約は利用者が押す「要約を作る」の turn（outputSchema 付き、1回だけ送る）で作り、画面で編集できる。「手で書く」でも作れる。
3. **承認して開始**で初めて `org_start` を呼ぶ。Planner への依頼文は要約から純関数で決定的に組み立てる。会話中はエンジン・ドライバを一切作らない。開始時の要約の写しを `org\<wf>\intake.json` に保存する（保存できなければ始めない）。
4. Gitでないフォルダ: 開始前の読取り検査を構造化し、`NotRepo`／`NoCommit` なら **「Gitを準備」ダイアログ**で `git init`＋初回コミットを提案する。AgentDock が作るのは **`.git` フォルダだけ**（`.gitignore` は作らない。除外は `.git\info\exclude` に書く）。
5. プッシュ: **作業フォルダの今のブランチ**（利用者のブランチ）だけを対象にし、`agentdock/` ブランチは送れない。確認ダイアログで送り先・コミット一覧を見せ、確認時のコミットIDを固定して送る。force 系は型で作れない。資格情報は Git に任せる。結果が分からなければ「確認できません」とし、読取りの「リモートを確認」だけを出す。

---

## 1. 依頼整理フェーズ

### 1.1 窓口の比較（推奨: A）

|案|内容|Q9|継続チャット・既存作成との整合|判断|
|---|---|---|---|---|
|**A 依頼整理チャット**|`start_chat`（cwd＝作業フォルダ、ReadOnly）＋目的メモ。普通に会話|対象外（所有記録なし）|継続・引き継ぎ・履歴・検索・承認応答がそのまま使える。既存チャットからも作れる|**採用**|
|B 専用ダイアログ（AIなし／単発AI）|フォームか1問1答|対象外|会話が残らない・引き継げない。新規UIが大きい|不採用|
|C Planner と対話|役割チャットで質問|Q9 と衝突|エンジンを先に動かす必要がある（要件違反）|不採用|

### 1.2 画面フロー

1. 「組織実行を始める…」→ **開始ダイアログ（簡略）**: 作業フォルダ（参照…）と Git の状態行（§2.1。`NotRepo`／`NoCommit` なら「Gitを準備…」）。ボタンは「依頼を整理する」（主）／「依頼が決まっているので書く」（副）。テンプレート・並列数・turn 上限・見積りは**ここから外して §1.2-3 の要約カードへ移す**。
2. 「依頼を整理する」→ `org_intake_begin` が依頼整理チャットを作り、そのチャットを開く。上部の帯: 「依頼を整理しています（組織実行はまだ始まっていません）　[要約を作る] [手で書く] [やめる]」。利用者は普通に送信して AI と詰める。
   - 「依頼が決まっているので書く」も同じチャットを作り、送信せずに要約カードを空で開く（全経路を要約カードで承認させる）。
   - 既存の自分のチャット（`org` 所有でない・組織実行中でない）の「…」メニュー「この会話から組織実行の依頼を作る」→ `org_intake_attach`（目的メモは変えない）。
3. 「要約を作る」→ 固定文＋outputSchema の turn を1回送る（履歴に残る）。完了後、その turn の最終回答を解析して **要約カード**（会話の末尾に固定表示、編集可）: 目的／成果物の形／入力ファイル（パスと検査結果）／制約／完了条件（1件以上必須）／未解決の点。下段に テンプレート・並列数・turn 上限・見積り（現 `OrgStartBody` の該当部分を移植）。
4. 「この内容で開始…」→ **開始確認ダイアログ**: Planner へ送る依頼文の全文プレビュー（§1.5）、作業フォルダ、テンプレート、「計画は承認カードで確認できます／自動でマージしません」。「開始」で `org_intake_start`。
5. 開始後、帯は「この依頼でワークフローを開始しました → ワークフローを開く」。チャットは普通に続けられるが、**ワークフローへは何も伝わらない**ことを帯に書く（追記は段階2の入口で行う）。要約を直して再度開始すると別のワークフローになる（同時1件の制約は既存どおり）。

### 1.3 状態（`IntakeState`、`rules/org_intake.rs` の純関数で遷移）

`Drafting` →（要約を作る）`Summarizing{clientMessageId, sentAt}` →（turn 完了・解析成功）`Ready` ／（解析失敗）`Drafting`＋`lastError` →（手で書く・編集）`Ready` →（開始成功）`Started{workflowId, rev, at}`。どこからでも `Abandoned`（やめる。チャットは消さない）。
- `Summarizing` 中の切断・受理不明: 状態は変えず「要約の結果を確認できません」。出すのは「履歴から読み直す」（読取りのみ、同じ clientMessageId の turn を探す）と「もう一度要約を作る」（利用者の明示操作で新しい送信）。自動再送しない。turn が失敗・中断なら `Drafting`＋理由。
- `Started` 後に要約を編集すると `Ready`（rev+1）へ戻り、`started[]` に履歴を残す。
- 能力: バックエンドが構造化出力を持たない（`Capabilities` に `structuredOutput` がなければ追加。Codex＝対応）ときは「要約を作る」を出さず「手で書く」だけ。

### 1.4 保存

- `store/records.rs` `ConversationRecord` に `#[serde(default)] intake: Option<IntakeRecord>`（SCHEMA_VERSION は上げない）。会話の記録に置くので、引き継ぎ（`handoff_conversation`・`redo_conversation`）で**新しい先頭へ写す**（純関数の変更＋テスト）。
- `IntakeRecord { id: LocalId, state, summary: Option<IntakeSummary>, rev: u32, prefs: IntakeStartPrefs{folder, graphId, maxWriters, maxTotal, maxTurns}, lastError: Option<String>, started: Vec<IntakeStarted{workflowId, rev, at}> }`。
- `IntakeSummary { goal, deliverable, inputs: Vec<{path, note}>, constraints: Vec<String>, doneCriteria: Vec<String>, openQuestions: Vec<String> }`（文字列と配列だけ。Codex の型を入れない）。
- 開始時: `org\<wf>\intake.json` ＝ `{schemaVersion, chat: ChatKey, intakeId, rev, summary, composedRequest}`。`org_start` の「定義の写し」と同じ箇所で、ドライバ登録の**前に**書く。書けなければ始めない。
- 開始後に chat.json の `Started` 書込みが失敗した場合: ワークフローは動かしたまま警告を出す。起動時に `intake.json` から chat の記録を `Started` へ直す（純関数 `reconcile_intake_started`）。

### 1.5 依頼文の組立て（`compose_planner_request`、純関数）

```
# 依頼（依頼整理で確定・第{rev}版）
## 目的
## 成果物の形
## 入力（作業フォルダからの相対パス。各行に検査結果）
## 制約
## 完了条件
## 未解決の点（あれば。Planner は推測で埋めず計画に明記すること）
```
空の節は「（なし）」。全体 16KB を超えたら開始しない（理由表示）。本文中のフェンスは `rules/compose.rs` の `fenced` を使う。入力ファイルの検査（`classify_input`）: 作業フォルダ内でコミット済み＝OK／未コミット・変更あり＝「Workerの作業場所には最新が入りません」警告／フォルダ外・存在しない＝警告（開始は止めない。止めるのは goal 空・完了条件0件・16KB超だけ）。

### 1.6 要約の turn

- 送る本文（固定）: 「（AgentDock: 依頼要約の作成）ここまでの会話から、組織実行に渡す依頼を決められた形で出してください。分からない点は推測で埋めず openQuestions に入れてください。ファイルは変更しないでください。」
- outputSchema: `IntakeSummary` と同形（`rules/org_intake.rs` の定数）。送信は既存の利用者送信経路（`SendAuth::User`）に `output_schema` を通す。資料枠（目的メモ）の扱いは通常送信と同じ。
- 解析（`parse_summary_output`）: 最終回答を JSON として読み、型と長さ上限（各文字列 2,000字、配列 30件）で検査。失敗は `Drafting`＋「要約を読み取れませんでした（手で書くこともできます）」。

### 1.7 Q9・既存機能との関係

- 依頼整理チャットは利用者のチャット。送信・承認応答・中断・削除は既存どおり。ワークフローの役割チャットは引き続き手入力不可（Q9 不変）。
- エンジンはこのチャットを読まない・送らない。ワークフローからこのチャットへ自動で送らない（M11）。
- 正本 §3.15 に「開始前の依頼整理（利用者のチャット、承認後に開始）」を追記（司令塔、版更新）。

---

## 2. Gitでないフォルダの準備

### 2.1 検査（読取りのみ、`gitops/prep.rs` `repo_status`）

`RepoStatus = Ready{root, branch} | Nested{root}（上位リポジトリの一部。そのまま使う・準備しない） | NotRepo | NoCommit{root} | OwnerMismatch | Unavailable{message}`。`org_start` の `precheck_repo_has_commit` はこれを使い、`NotRepo`／`NoCommit` を `BlockedReason::RepoNotPrepared` で返す（UIが準備ダイアログへ誘導できる）。

### 2.2 プレビュー（書かない）

- `NotRepo`: AgentDock 領域の一時 gitdir（`%LOCALAPPDATA%\com.agentdock.app\tmp\gitprep-<id>\`）で `git --git-dir=<tmp> --work-tree=<folder> init` → `info\exclude` に除外を書く → `status --porcelain=v2 -z --untracked-files=all` で**実際に入るファイル**を得る（フォルダ内の既存 `.gitignore` も反映される）。一時 gitdir は終わったら消す（AgentDock 自身の一時領域）。
- `NoCommit`: 利用者の既存リポジトリで `--no-optional-locks status`（読取り）だけ。除外は書かない。
- 集計（`plan_prep`、純関数）: ファイル数・合計サイズ・大きいファイル上位10件・除外したもの。
  - 既定の除外（`info\exclude` に書く。`NotRepo` のときだけ）: `Thumbs.db` `desktop.ini` `~$*` `*.tmp` `.DS_Store`、および **50MB 超のファイル**（個別パスで列挙）。
  - 断る: ファイル 20,000 超、合計 1GB 超、ドライブ直下・ユーザープロファイル直下・`Windows`・`Program Files` 配下（既存の「強い確認」判定を流用し、ここでは確認ではなく拒否）。`NoCommit` で 50MB 超があれば断る（利用者の .gitignore を書き換えないため）。
  - 注意だけ: OneDrive 等の同期フォルダ内（`.git` の同期で不具合が出ることがある）。
- `previewToken` ＝ ファイル一覧（パス・サイズ・更新時刻）の SHA-256。実行時に再計算して一致しなければ実行しない。

### 2.3 確認ダイアログ（`GitPrepDialog`）

表示: 対象フォルダ／作るもの「`.git` フォルダ（AgentDock はこれ以外のファイルを作りません）」／最初のコミットに入るファイル N件・合計 X MB・大きいもの上位／除外するもの（理由つき。「除外したファイルは Worker の作業場所に入りません」）／ブランチ名（Git の既定 `init.defaultBranch` に従う。指定しない）／作者（下記）／「このフォルダの既存ファイルは移動・変更しません」。ボタン: 「Gitを準備する」／「やめる」。

作者: そのフォルダで `git config --get user.name`・`user.email`（読取り）が両方あればそれを使う。なければ `-c user.name=AgentDock -c user.email=agentdock@localhost`（設定は書かない）とし、ダイアログに「後で `git commit --amend --reset-author` で変えられます」と出す。署名なし・フックなし（既存 `CommitStaged` と同じ `-c core.hooksPath=/dev/null -c commit.gpgsign=false --no-verify`）。メッセージ「AgentDock: 最初のコミット」。

### 2.4 実行と巻き戻し（`gitops/prep.rs` `prepare_repo`）

1. 再検査（`repo_status` が同じ／`.git` が無いこと／`previewToken` 一致）。違えば何もしない。
2. `git init -q` → 直後に `.git\agentdock-init.json`（`{prepId, at}`）を書く＝「AgentDock が作った」印。
3. `info\exclude` へ AgentDock 節を追記 → `add -A` → 初回コミット → `rev-parse HEAD` で確認。
4. `%LOCALAPPDATA%\com.agentdock.app\git-ops.jsonl` に記録（種類・フォルダ・コミット・除外一覧・結果）。

巻き戻し（`rollback_decision`、純関数）:
- 手順2〜3が**明確に失敗**（非0終了）し、印が自分の `prepId` と一致 → 作った `.git` だけを削除し「元に戻しました」。削除に失敗したら「途中まで作った .git が残っています」＋「開く」。
- **タイムアウト・起動不能など結果が分からない** → 削除しない。`repo_status` を読み直して実際の状態を表示（「完了を確認できません」）。
- `NoCommit`（利用者の既存 `.git`）→ どんな場合も削除しない。失敗を表示するだけ（索引に `add -A` の結果が残り得ることも書く）。
- 次回の検査で印付き・コミットなしの `.git` を見つけたら「前回の準備が途中です: [続きから（コミットだけ）] [取り除く（確認つき）]」。

---

## 3. アプリからのプッシュ

### 3.1 対象と入口

- 対象: 作業フォルダのリポジトリの**今のブランチ**（取り込み先＝利用者のブランチ）。`agentdock/` で始まるブランチ・detached HEAD は断る。他のブランチは選ばせない（推奨。取り込み後に押す前提のため）。
- 入口: ワークフロー画面ヘッダーの「…」→「プッシュ…」（作業フォルダが `Ready` のとき）。段階3の成果物パネルの「取り込み」完了表示から同じダイアログを開く。自動では開かない・送らない。

### 3.2 プレビュー（読取り、ネットワークなし。`gitops/push.rs` `push_preview`）

`git remote`／`remote get-url`、`rev-parse --abbrev-ref --symbolic-full-name @{u}`、送るコミット `log --format=%H%x09%an%x09%s <upstream>..HEAD`（追跡先なしは `HEAD --not --remotes=<remote>`、最大200件＋総数）、未コミット変更の件数。fetch はしない（遅れは push の拒否で分かる）。
`PushBlocker`（`push_blocker`、純関数）: `NoRemote`（案内:「リモートがありません。GitHub 等でリポジトリを作り、`git remote add origin <URL>` を実行してから再度開いてください」。AgentDock はリモートを追加しない）／`Detached`／`InternalBranch`／`NothingToPush`／`Unavailable`。リモートが複数で追跡先なしなら選択式（既定 `origin`）。

### 3.3 確認ダイアログ（`GitPushDialog`）

送り先: リモート名と URL（`mask_url` で `user:token@` を伏せる）／`<local> → <remote>/<branch>`／送るコミット一覧（短いID・件名・作者）と件数／「未コミットの変更 N件は送られません」／追跡先なしなら「追跡先として設定する（-u）」チェック（既定オン。リポジトリの設定に追跡先が書かれる旨を明記）／「強制プッシュはしません。認証は Git の設定（資格情報マネージャ）を使い、AgentDock は資格情報を扱いません」。ボタン「プッシュする」／「やめる」。

### 3.4 実行（`push`）

- 再検査: HEAD がプレビューのコミット（`expectedSha`）と同じでなければ送らない。
- コマンド: `git push --porcelain [-u] -- <remote> <expectedSha>:refs/heads/<branch>`。`GitOp::Push` は `force`・`+`・`--force-with-lease`・`--mirror`・`--all`・`--tags`・削除 refspec を**表現できない型**にする（refspec は `valid_ref` ＋ 先頭 `+`・`:` 単独を拒否）。
- 環境: `GIT_TERMINAL_PROMPT=0`（端末入力で固まらない。資格情報マネージャの画面は出てよい）。タイムアウト 5分。pre-push フックは利用者のものとして走らせる（出力は失敗時に表示）。
- 結果（`classify_push`、純関数。`--porcelain` の行と終了コード・stderr から）: `Pushed{sha}`／`Rejected{NonFastForward|HookDeclined|Other(text)}`／`AuthFailed`／`NetworkError`／`Unknown`（タイムアウト・出力が読めない）。`Unknown` は「送れたか確認できません」と表示し、「リモートを確認」（`ls-remote <remote> refs/heads/<branch>`、読取り）で `Confirmed`／`NotThere`／`Unavailable` を出す。再送はボタンを押し直したときだけ（再プレビューから）。
- `git-ops.jsonl` に記録。

---

## 4. IPC・型（ts-rs で `src/ipc/gen/`。状態を変えるものは `UserConfirmed`）

|コマンド|引数 → 戻り|
|---|---|
|`org_intake_begin`|`{folder, model?, mode: "converse"\|"write"}` → `{chat: Chat, intake: IntakeView}`|
|`org_intake_attach`|`{chat}` → `IntakeView`|
|`org_intake_view`|`{chat}` → `IntakeView`（`state, summary, rev, prefs, inputChecks[], blockers[], composedPreview`）|
|`org_intake_summarize`|`{chat}` → `SendAttempt`|
|`org_intake_reload_summary`|`{chat}` → `IntakeView`（履歴から読み直す、読取り）|
|`org_intake_save`|`{chat, baseRev, summary, prefs}` → `IntakeView`（`baseRev` 不一致は拒否）|
|`org_intake_start`|`{chat, rev}` → `StartOrgResult`（`rev` が保存値と同じときだけ）|
|`org_intake_abandon`|`{chat}` → `IntakeView`|
|`git_repo_status`|`{folder}` → `RepoStatusView`|
|`git_prep_preview` / `git_prepare_repo`|`{folder}` → `RepoPrepPreview` ／ `{folder, previewToken}` → `RepoPrepResult{outcome: Prepared{commit}\|RolledBack{reason}\|Failed{reason, leftover?}\|Unknown{status}}`|
|`git_push_preview` / `git_push` / `git_push_verify`|`{folder}` → `PushPreview` ／ `{folder, remote, branch, expectedSha, setUpstream}` → `PushResult` ／ `{folder, remote, branch, expectedSha}` → `PushVerify`|

イベント: `intake-updated {chat}`（要約の完了・失敗で発火）。`OrgStartArgs` に `origin: Option<IntakeOrigin{chat, intakeId, rev, summary, composedRequest}>` を追加（`org_intake_start` だけが使う。旧 `org_start` は「書く」経路の内部用として残す）。

---

## 5. 実装タスク（実装担当1名が順に）

|#|内容|主なファイル|
|---|---|---|
|IN-1|純粋ロジックと型: 要約の検査・解析・依頼文の組立て・入力の分類・状態遷移・`reconcile_intake_started`／準備の計画・除外・拒否条件・`rollback_decision`／`push_blocker`・`classify_push`・`mask_url`・refspec 検査。`ConversationRecord.intake` と引き継ぎでの写し|`src-tauri/src/rules/org_intake.rs`・`rules/git_prep.rs`・`rules/git_push.rs`（新規）、`store/records.rs`、`host/continuity_handoff.rs`、`backend/ipc.rs`|
|IN-2|gitops: `GitOp` に `Init`・`ConfigGet{key}`・`StatusAllUntracked`・`Remotes`・`RemoteUrl`・`Upstream`・`LogRange`・`LogNotRemote`・`CommitStagedAs{message, identity}`・`Push`・`LsRemoteBranch` を追加。環境変数・長いタイムアウトの実行口（`snapshot.rs` の env 付き実行を参考）。`prep.rs`（`repo_status`・プレビュー・`prepare_repo`）、`push.rs`|`src-tauri/src/gitops/mod.rs`・`gitops/prep.rs`・`gitops/push.rs`（新規）|
|IN-3|ホスト（Git）: IPC 6本、`git-ops.jsonl`、`precheck_repo_has_commit` を `repo_status` に置換し `RepoNotPrepared` を返す|`host/gitprep.rs`（新規）、`host/org/mod.rs`、`commands.rs`、`lib.rs`|
|IN-4|ホスト（依頼整理）: begin（`start_chat`＋目的メモ＋intake 記録）・attach・summarize（利用者送信に outputSchema）・turn 完了の観測と解析・保存・start（`origin` 付き `org_start`、`intake.json`）・abandon・起動時の照合・能力 `structuredOutput`|`host/org/intake.rs`（新規）、`host/org/mod.rs`、`store/org.rs`（`INTAKE_FILE`）、`host/mod.rs`（送信経路の output_schema）、`backend/backend.rs`（Capabilities）|
|IN-5|UI: 開始ダイアログの簡略化、依頼整理の帯・要約カード・開始確認、チャット「…」の項目、Gitを準備／プッシュのダイアログ、ワークフロー画面の「プッシュ…」|`src/ui/OrgDialogs.tsx`、`src/ui/OrgIntake.tsx`・`src/ui/GitPrepDialog.tsx`・`src/ui/GitPushDialog.tsx`（新規）、`src/ui/OrgPane.tsx`、`src/ipc/client.ts`|
|IN-6|文書: 正本 §3.15 の版更新、`LIVE_CHECK_INTAKE.md`|司令塔|

順序: IN-1 → IN-2 → IN-3 → IN-4 → IN-5。IN-3 と IN-4 は IN-2 後に並行可（触るファイルが別。`host/org/mod.rs` の変更は IN-3 が precheck、IN-4 が `org_start` の origin で衝突しやすいので順にする方が安全）。

## 6. 受入条件

1. 依頼整理中（`Drafting`〜`Ready`）は `org_rt.driver_active()` が偽のまま、`org\` 配下に新しいワークフローができない。
2. 依頼整理チャットは `chat.json.org` が None で、通常送信・承認応答ができる。役割チャットは従来どおり送信不可。
3. 「要約を作る」は押した回数だけ送る。切断・受理不明で再送しない。結果不明時は「確認できません」と2つの明示操作だけを出す。
4. 開始は `rev` 一致時のみ。`intake.json` を書けなければ開始しない。依頼文は確認ダイアログのプレビューと1文字も違わない。
5. 引き継ぎ後も依頼要約が新しい先頭に残る。
6. Git準備: `NotRepo` で作られるのは `.git` 配下だけ（作業フォルダの他のファイルは更新時刻も変わらない）。`.gitignore` を作らない。`~/.gitconfig` を変えない。
7. 準備の明確な失敗では自分の作った `.git` だけを消す。結果不明・既存 `.git` では消さない。`previewToken` 不一致では何もしない。
8. プッシュは確認ダイアログの「プッシュする」以外から起きない。送るのは `expectedSha` だけ。force 系の引数は生成されない（単体テストで args を検査）。`agentdock/` ブランチ・detached は送れない。
9. 結果不明を成功と表示しない。URL の資格情報は表示・記録に出ない。AgentDock は資格情報を保存・入力しない。
10. `cargo check`・`tsc --noEmit`・追加した純関数のテストが通る。

## 7. 単体テストの対象（純関数）

`parse_summary_output`（正常・欠落・型違い・長さ超過）、`summary_blockers`、`compose_planner_request`（決定性・空節・16KB・フェンス）、`classify_input`、`IntakeState` の遷移（切断・失敗・再要約・開始後の編集）、`reconcile_intake_started`、`handoff_conversation` が intake を写す、`plan_prep`（上限・除外・拒否フォルダ・`NoCommit` の大きいファイル）、`preview_token` の一致判定、`rollback_decision`（明確な失敗／不明／既存 `.git`／印の不一致）、`push_blocker`、`GitOp::Push::args`（force 系が出ない・refspec 検査）、`classify_push`（porcelain の `=`・`*`・`!`、認証失敗、ネットワーク、タイムアウト）、`mask_url`。

## 8. 実機確認（ユーザー）

1. 依頼整理: 会話→「要約を作る」→要約カードが埋まる／未解決の点が出る。編集→開始→確認ダイアログの依頼文で Planner の計画承認カードまで進む。会話中にワークフロー一覧へ何も増えない。
2. 要約の turn 中に App Server を止める→「確認できません」表示、再送されない。再接続後「履歴から読み直す」で取り込める。
3. 長い依頼整理チャットで引き継ぎ後も要約が残る。
4. Gitなしの業務フォルダ（Office ファイル・50MB 超を含む）で「Gitを準備」→ `.git` だけができる、除外が一覧どおり、初回コミットの作者表示が正しい（user.name 設定あり／なしの両方）。
5. 読取り専用ファイルのロック等で準備を失敗させ、`.git` が消えて元に戻ること。
6. プッシュ: GitHub の個人リポジトリへ。資格情報マネージャの画面が出て通る／未認証で「認証に失敗」／リモートを先に進めて「先に進んでいます（pull が必要）」／リモートなしで案内／ネット切断中で失敗表示（成功にならない）。

## 9. 要判断（実装は推奨で進める）

- U1 会話の相手: 依頼整理チャット（推奨）／専用ダイアログ。
- U2 Git準備の除外: `.git\info\exclude` に書き `.gitignore` は作らない（推奨）／`.gitignore` を作る（共有されるが利用者のファイルが増える）。
- U3 作者: user.name があれば利用者、なければ AgentDock 名義（推奨）／常に AgentDock 名義。
- U4 プッシュ対象: 今のブランチだけ（推奨）／ブランチを選べる。リモート未設定は案内のみ（推奨）／URL を入れて追加。

## 追記: U2 の決定（2026-10-19 ユーザー）
- `git init` の初期作成時に除外ファイル（`.git/info/exclude`・`.gitignore`）は**作らなくてよい**（推奨案の `.git/info/exclude` への書込みも必須ではない）。
- ただし AgentDock 側で**必要に応じて**除外ファイルを作成・追記してよい（例: 巨大ファイル・生成物・一時ファイルをコミットから外す必要が出たとき）。その場合も、何を書くかを利用者に示してから行い、既存の除外ファイルは上書きせず追記のみ・前後を照合する。
- 実装（IN-3）では、初期作成の除外書込みを任意（既定オフまたは省略）にし、「必要に応じた追記」は独立した小さな部品として切り出す。
