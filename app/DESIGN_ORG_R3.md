# DESIGN_ORG_R3: 成果物パネル（R6）と「取り込む」（R7）設計書 v0.1（2026-10-19）

対象: `PLAN_ORG_REWORK.md` 段階3（R6・R7）と §6（取り込み後にアプリからプッシュ）。前提: `DESIGN_ORG_INTAKE.md` §3（プッシュは作成済み）、`DESIGN_ORG_R1.md` §2.4（手動結果の取込み）。`DESIGN_ORG_R2.md` は未作成のため、画面は `OrgPane.tsx` の現状に合わせ、段階2のタイムライン（「経過」付近）とは重ならない位置に置く。
守る合意: 自動マージしない／worktree・ブランチ・台帳を自動で消さない／未確認の操作を成功表示しない／`~/.codex` を触らない／利用者の作業フォルダへの書込みは確認つきの明示操作（`UserConfirmed`）だけ。

## 0. 結論

1. **成果物の単位は「書込みタスクのコミット」**（`Task.commit`、ブランチ `agentdock/org/<wf>/<task>`、始点は `WorktreeRecord.base_commit`）。差分は常に Git から読む（Worker の `changedFiles` は自己申告として併記し、食い違いを表示）。
2. **取り込みは「作業ツリーに触れずに合成 → 確認 → 早送りで1回反映」**。`git merge-tree --write-tree` で結果を計算し（どの作業ツリー・索引・ブランチにも書かない）、`commit-tree` で取り込みコミットを作り、確認後に利用者のフォルダで `merge --ff-only <そのコミット>` を1回だけ実行する。重なり（衝突）・未コミット変更との重なり・開かれているファイルがあれば**何も書かずに**理由を出す。
3. **控え＝取り込み前のHEAD**を台帳（`org\<wf>\imports.jsonl`）に先に書いてから実行する。「取り込み前に戻す」は、HEAD が取り込み結果のままで未送信のときだけ `reset --keep <前のHEAD>`（確認つき）。
4. 取り込み後の結果表示から**既存の「プッシュ…」（GitPushDialog）**を開く。自動では開かない・送らない。
5. worktree・ブランチは取り込み後も残す。削除は既存のワークフロー「削除…」だけ（取り込み済みなら未マージ0件として表示される）。

## 1. R6 成果物パネル

### 1.1 状態（`ArtifactStatus`、純関数 `artifact_status` で導く。保存しない。取り込みの記録だけ保存）

|状態|条件|表示|
|---|---|---|
|`Working`|書込みタスクが非終端でコミットなし、または Running・Rework|作業中|
|`AiReviewing`|書込みタスクが `Reviewing`|レビュー中（AI）|
|`ReadyForReview`|`Completed`＋コミットあり、今のブランチに含まれない、上位の成果に含まれない|**レビュー待ち**（AIの確認は終わり、あなたの確認と取り込みを待っています）|
|`NotPassed`|`Failed`／`Cancelled` だがコミットあり（差し戻し上限など）|合格していない成果（見る・取り込むは可、強い注意）|
|`Superseded`|コミットが別の候補のコミットの祖先（グラフで後の書込みが前を含む）|後の成果に含まれています|
|`Imported`|台帳に `Applied` があり、コミットが今のHEADの祖先|取り込み済み（日時・ブランチ）|
|`IncludedElsewhere`|台帳なしで、コミットが今のHEADの祖先|すでに今のブランチに含まれています|
|`NoChanges`|`Completed`、コミットなし（NothingToCommit）|変更なし|
|`Unknown`|Gitの読取り失敗・コミットが見つからない|未確認（理由）|

- 台帳に `Applied` があるのに今のHEADに含まれない（利用者が戻した等）は `ReadyForReview`＋注記「取り込みの記録はありますが、今のブランチには含まれていません」。
- 鮮度は別軸: `OrgArtifactsView.fetched: Known<UnixMillis>`。祖先判定が取れなければ `Unknown`（`ReadyForReview` で代用しない）。
- 「レビュー待ち」はエンジンの「レビュー中」（Reviewer AI 作業中）と別の語として扱う。件数はワークフロー一覧の行とパネル見出しに出す。

### 1.2 中身

- **ファイル一覧**: `base_commit..commit` の `diff --numstat -z -M`＋`--name-status`。行ごとに 種類（A/M/D/R）・パス・`+a −d`（バイナリは「バイナリ」、数えられないものは未確認）。
- **要約**: タスクごと「N ファイル　+a −d（バイナリ k）」、ワークフロー全体は候補（`ReadyForReview`・`NotPassed`）の合計。
- **プレビュー**（`preview_kind(path)` 純関数）: `md`＝既存 `Markdown` 部品（生HTMLは文字・リモート画像なし・リンクは確認後 http/https）／`txt log json yaml yml xml ini toml`＝等幅テキスト／`csv tsv`＝表（先頭500行）／`png jpg jpeg gif webp bmp svg`＝`<img src="data:...">`（10MB まで。svg も img 経由のみ）／`docx xlsx pptx pdf` 等＝プレビューなし、「開く…」。テキストは 1MB まで（超過は先頭＋「先頭のみ」）、UTF-8（BOM可）・UTF-16（BOMあり）のみ。他は「文字コードを判定できません」＋「開く…」。
- **差分**: 文書（md・txt・csv）は「プレビュー｜差分」のタブ（追加ファイルはプレビュー既定、変更はプレビュー既定）。それ以外のテキストは差分既定。差分は `diff base commit -- path`（移動は両パス）。
- **「開く…」**: blob を `%LOCALAPPDATA%\com.agentdock.app\tmp\artifacts\<wf>\<task>\<元のファイル名>` に書き出して読取り専用属性を付け、既存の「成果物を開く」確認を経て関連付けアプリで開く（worktree 内のファイルは開かない＝誤編集防止。tmp は AgentDock 領域）。
- **Reviewer の所見**: その書込みタスクを検証した Reviewer／verifier の最新の判定（合格／差し戻し）と `findings` を併記。試行が複数なら「差し戻し n 回」と過去分を折りたたみ。Worker の `summary`・`tests{ran,passed}` も併記。
  - 対応付け: plannedTasks は `deps.first()==書込みタスク` の reviewer。グラフは `target` が書込みノードの検証ノードの、対象コミットが一致する活性化（実装時に `graphrun.rs` で確認）。
  - 出どころ: グラフは既存スロット。plannedTasks は**今は保存されていない**（`artifacts`・`findings` はメモリのみ）ため、§3.1 の `PutOutput` で保存する。旧ワークフロー・保存失敗は、役割チャットの受理turnの最終回答を履歴から読む（読取りのみ。取れなければ「未取得」）。
- **申告との照合**（`report_vs_diff`）: 報告にあって差分にない／差分にあって報告にない パスを「報告と差分の違い」として列挙（判定には使わない）。
- Worker の作業場所に未コミットの変更が残っていれば「コミットされていない変更 N 件は成果に含まれません」（`dirty_count`、読取り）。

### 1.3 置き場所と文言

- `OrgPane.tsx` の「タスク」節の**直後**に `section.org-artifacts`「成果物」: 見出し「成果物　レビュー待ち N・取り込み済み M」、候補ごとに1行（タスクID・タイトル・状態チップ・`N ファイル +a −d`・所見の要約1行「Reviewer: 合格（所見2件）」）、ボタン「中身を見る」「取り込む…」。下に「取り込み後も作業場所とブランチは残ります（削除はワークフローの『削除…』から）」。
- 「中身を見る」→ `OrgArtifactsDialog`（大きいダイアログ。左: タスク別ファイル一覧／右: プレビュー・差分・所見タブ）。表の「成果物」列（`artifactText`）もこのダイアログを該当タスクで開くリンクにする。
- 完了バナー（`wf.finished`）を「成果物を確認して取り込んでください。AgentDock は自動でマージしません。[成果物を見る]」に変更。`OrgNotice::WorkflowCompleted` の文言も同様（`view.rs`）。
- `OrgListRow` に印「レビュー待ち N」（`OrgWorkflowView.artifactHint`。エンジン状態＋台帳だけから導く軽い値。Gitの事実はパネルで確認）。

## 2. R7 取り込む

### 2.1 方式の比較（推奨: D）

|案|内容|未コミット変更との関係|戻し方|判断|
|---|---|---|---|---|
|A `merge --no-ff` を作業フォルダで直接|一般的|衝突時に作業フォルダが「マージ中」になり、利用者に Git の後始末を要求|`reset`|不採用（失敗時に作業フォルダを汚す）|
|B `cherry-pick`|コミットを再適用|同上（途中停止の状態が残る）。チェーン時に重複|同上|不採用|
|C ファイルコピー|worktree から上書き|利用者の変更を黙って上書きし得る。削除・移動を表せない。履歴が残らない|控えコピー|不採用（段階4の Gitなしモードで再検討）|
|**D 合成してから早送り**|`merge-tree --write-tree`→`commit-tree`→確認→`merge --ff-only`|重なれば何も書かず案内。重ならない未コミット変更はそのまま残る|`reset --keep <前のHEAD>`|**採用**|

- 取り込みコミットは常に作る（候補ごとに親＝[直前, 候補]の合流コミット。1件でも早送りで済ませない＝取り込みの区切りが履歴に残り、戻し先が1つに決まる）。メッセージ「AgentDock: 組織実行の成果を取り込み（<ワークフロー名>）」＋本文に `- <task> <title> <sha7>`。作者は `user.name`/`user.email` があれば利用者、なければ AgentDock 名義（INTAKE U3 と同じ。設定は書かない）。署名なし。
- 複数候補を選んだときは、選んだ順に `merge-tree` を連鎖させ、全部が衝突なしのときだけ最後のコミットへ1回早送りする（1つでも衝突なら何も書かない）。
- Git 2.38 未満（`merge-tree --write-tree` なし）は取り込みを出さず「Git 2.38 以降が必要です」と案内（`merge_tree_supported`）。

### 2.2 取り込みの前提（`import_blockers`、純関数）

対象＝`wf.repo` の**今のブランチ**。拒否: detached／`agentdock/` ブランチ／マージ・リベース・cherry-pick 途中（gitdir の `MERGE_HEAD`・`rebase-merge`・`rebase-apply`・`CHERRY_PICK_HEAD`）／ワークフロー未完了（`wf.finished` でない、J1）／削除保留中／候補が `ReadyForReview`・`NotPassed` 以外／Git 2.38 未満。
合成後の拒否: 衝突（ファイル一覧）／**取り込みで変わるファイルと、未コミット変更（追跡中の変更・未追跡）の重なり**（`dirty_overlap`）／Windows で書込み対象が他のアプリに開かれている（共有なしで開けるかを試すだけ。内容は変えない）。重ならない未コミット変更は「N 件はそのまま残ります」と表示して進める。

### 2.3 確認ダイアログ（`OrgImportDialog`）

表示: 取り込み先「<フォルダ>　ブランチ <branch>（今のコミット <sha7>）」／取り込む成果（タスク・状態、`NotPassed` は赤字「合格していません」＋同意チェック必須）／**変わるファイル一覧と +a −d**（`diff HEAD <結果ツリー>`。プレビュー・差分は 1.2 の部品で開ける）／「あなたの未コミットの変更 N 件には触れません」／止める理由があればそれだけを出し、ボタンを無効にする。
文言（Gitの語を前に出さない）: 「選んだ成果を、作業フォルダの今のブランチに取り込みます。取り込み前の状態は記録し、送信（プッシュ）前なら『取り込み前に戻す』で戻せます。AgentDock は自動で取り込みません。」
衝突時: 「同じ箇所を、あなたのブランチと成果の両方が変えているため、自動では合わせられません（ファイル: …）。何も変更していません。取り込む成果を減らすか、ブランチ <agentdock/…> を手動で取り込んでください。」＋ブランチ名のコピー。
ボタン「取り込む」／「やめる」。

### 2.4 実行（`gitops/import.rs`）

1. プレビューで得た `token`（前HEAD `H`・結果コミット `R`・候補コミット・ファイル一覧のハッシュ）を照合。HEAD・ブランチが `H` のままで、`R` を再計算して同じでなければ何もしない（「状態が変わりました。もう一度確認してください」）。
2. 台帳に `begin{id, folder, branch, before:H, result:R, tasks[{task,commit}]}` を追記。**書けなければ実行しない**。
3. `git -c core.hooksPath=/dev/null merge --ff-only -q R`（作業フォルダで）。
4. HEAD を読み戻して分類（`classify_import`）: `Applied`（HEAD==R）／`NotApplied{reason}`（HEAD==H・対象ファイルが前と同じ）／`Partial{files}`（HEAD==H だが対象ファイルが変わった。ロック等）／`Unknown`（タイムアウト・読めない）。台帳に `end{id, outcome}`、`git-ops.jsonl` に記録。
5. 結果表示: `Applied`＝「取り込みました（N ファイル）。[プッシュ…] [閉じる]」。`Unknown`＝「取り込めたか確認できません」＋「状態を確認」（読取りのみ。同じ分類をやり直す）。成功表示は `Applied` を読み戻しで確かめたときだけ。
- 起動時・パネル表示時に `begin` だけの記録は `reconcile_import`（HEAD==R→Applied、H→NotApplied、他→Unknown）で表示を決める（自動で再実行しない）。

### 2.5 取り込み前に戻す（`undo_decision`、純関数）

- 可: 今のブランチが記録と同じ、HEAD==R、`R` がどのリモート追跡ブランチにも含まれない（`branch -r --contains`）、対象ファイルに未コミット変更がない。→ 確認「取り込み前（<sha7>）に戻します。取り込んだ成果は agentdock ブランチに残っているので、あとでまた取り込めます。」→ `git -c core.hooksPath=/dev/null reset --keep -q H` → HEAD==H を読み戻して `undone`。
- `Partial`（HEAD==H で一部だけ書き換わった）: 対象ファイルは取り込み前に未変更だったことを 2.2 で確かめているので、確認後 `checkout H -- <書き換わったファイル>` で戻せる。
- 不可（理由を出す）: 送信済み（「送信済みのため戻せません。必要なら新しい変更で打ち消してください」。強制プッシュは使わない）／HEADがその後進んだ／ブランチ違い／重なる未コミット変更。

### 2.6 保存と消さない合意

- `org\<wf>\imports.jsonl`（追記のみ。`IMPORTS_FILE`）。行: `{schemaVersion:1, kind: begin|end|undoBegin|undoEnd, id, at, folder, branch, before, result, tasks, outcome?}`。ワークフローの記録と一緒にだけ消える（利用者の「削除…」。`remove_workflow_files` の順に追加）。
- 取り込みは worktree・ブランチ・`WorktreeRecord`・台帳 `worktrees.json` を変更しない。取り込み後は既存の削除プレビューの未マージ数が0になり、利用者が削除を選べる（自動では消さない）。

## 3. 状態・型・IPC

### 3.1 エンジン（`org-engine`）

- `OrgAction::PutOutput { task, attempt: u32 /*= rework_count*/, value: Value }` を `planned.rs` `complete_output` の `Report`・`Verdict` で出す（イベントは増やさない。値は表示用の控え）。ホスト `driver.rs` は `outputs\<task>.<attempt>.json` に書く。**失敗しても以後の副作用は止めない**（スロットと違い経路判定に使わない。diag ログのみ）。

### 3.2 型（`backend/org.rs`、ts-rs で `src/ipc/gen/`）

- `OrgArtifactsView { workflowId, target: OrgImportTargetView{folder, branch: Known<String>, head: Known<String>, dirty: Known<u32>, blockers: Vec<String>}, items: Vec<OrgArtifactItem>, fetched: Known<UnixMillis>, gitSupportsImport: Known<bool> }`
- `OrgArtifactItem { task, title, role, branch, base: Option<String>, commit: Option<String>, status: OrgArtifactStatus, statusLabel, files: Known<Vec<OrgArtifactFile>>, totals: Known<OrgArtifactTotals>, report: Known<OrgWorkerReportView>, reviews: Known<Vec<OrgReviewView>>, reportMismatch: Vec<String>, uncommitted: Known<u32>, importable: bool, importRefusal: Option<String>, imports: Vec<OrgImportRecordView> }`
- `OrgArtifactFile { path, kind: ChangeKind, moveTo, additions: Known<u32>, deletions: Known<u32>, binary, preview: OrgPreviewKind(markdown|text|table|image|none) }`（UI の差分部品は構造的にこの形と `ChangedFile` の両方を受ける）。
- `OrgImportPreview { token, files, totals, conflicts, dirtyOverlap, lockedFiles, dirtyOther: u32, notPassed: Vec<String>, blockers: Vec<String>, message }`、`OrgImportResult { recordId, outcome: Applied{head}|NotApplied{reason}|Partial{files}|Unknown{headNow: Known<String>} }`、`OrgUndoPreview { allowed, refusal, mode: reset|restoreFiles, files }`。
- `OrgWorkflowView.artifactHint: { ready: u32, imported: u32 }`。

### 3.3 IPC（状態を変えるものは `UserConfirmed`）

|コマンド|引数 → 戻り|
|---|---|
|`org_artifacts`|`{workflowId}` → `OrgArtifactsView`（読取り）|
|`org_artifact_diff` / `org_artifact_preview`|`{workflowId, task, path}` → `UnifiedDiff` 同形 ／ `OrgArtifactPreview{kind, text?, dataUrl?, truncated, bytes, refusal?}`|
|`org_artifact_open`|`{workflowId, task, path}` → `{path}`（tmp へ書出し後、既存の確認つき「開く」）|
|`org_import_preview`|`{workflowId, tasks: Vec<String>}` → `OrgImportPreview`（作業フォルダには書かない。オブジェクトだけ増える）|
|`org_import`|`{workflowId, token, acceptNotPassed}` → `OrgImportResult`|
|`org_import_verify`|`{workflowId, recordId}` → `OrgImportResult`（読取り）|
|`org_import_undo_preview` / `org_import_undo`|`{workflowId, recordId}` → `OrgUndoPreview` ／ `{workflowId, recordId, expectedHead}` → `OrgUndoResult`|

`token` はホストのメモリに10分保持（再起動で無効→再プレビュー）。取り込み・戻す・プッシュは同じリポジトリ単位のロック（`import_rt`）で直列化。

### 3.4 Git 操作の追加（`GitOp`、許可リスト方式・引数検査つき）

読取り: `DiffCommitsNumstat{from,to}`（`--numstat -z -M`）、`DiffCommitsFile{from,to,path,old_path?}`、`DiffTreeNames{from, tree}`、`BlobSize{commit,path}`（`cat-file -s`）、`ShowBlob{commit,path}`（`cat-file blob <40桁>:<path>`。commit は40桁16進のみ）、`IsAncestor{a,b}`（`merge-base --is-ancestor`、終了コード 0/1 を値に）、`RemoteContains{commit}`（`branch -r --contains`）、`MergeTree{ours,theirs}`（`merge-tree --write-tree -z --name-only --no-messages`。終了 0＝成功、1＝衝突）。
書込み（AgentDock の取り込みだけが使う）: `CommitTree{tree, parents, message, identity}`（オブジェクトだけ。ref を書かない）、`MergeFfOnly{commit}`、`ResetKeep{commit}`、`RestorePaths{commit, paths}`。いずれも `core.hooksPath=/dev/null`、`--hard`・`--force` は型で表せない。実行は「今のブランチが `agentdock/` でない・本体の作業フォルダである」を `gitops/import.rs` が直前に確かめる（`gitops/org.rs` の逆向きの保護）。

## 4. 実装タスク（実装担当1名が順に）

|#|内容|主なファイル|
|---|---|---|
|R3-1|純関数と型: `artifact_status`・`supersede`（祖先関係から上位を判定）・`preview_kind`・`decode_text`・`report_vs_diff`・`totals`・`parse_numstat_z`・`parse_merge_tree`・`merge_tree_supported`・`import_blockers`・`dirty_overlap`・`classify_import`・`reconcile_import`・`undo_decision`・取り込みメッセージの組立て・台帳行の型|`src-tauri/src/rules/org_artifacts.rs`（新規）、`rules/mod.rs`、`backend/org.rs`|
|R3-2|エンジンの `PutOutput` とホストでの保存・読出し、`IMPORTS_FILE`・`OUTPUTS_DIR`・削除順|`org-engine/src/types.rs`・`planned.rs`、`host/org/driver.rs`、`store/org.rs`|
|R3-3|Git 操作（3.4）と `gitops/artifacts.rs`（読取り）・`gitops/import.rs`（プレビュー・実行・確認・戻す、ロック中ファイルの検査）。一時リポジトリでの結合テスト（org.rs と同形式）|`src-tauri/src/gitops/mod.rs`・`artifacts.rs`・`import.rs`（新規）|
|R3-4|ホスト: IPC 8本、token 保持、`import_rt` ロック、台帳・`git-ops.jsonl`、所見の対応付けと履歴フォールバック、`artifactHint`、完了通知の文言、tmp 書出し。コマンド登録と ts-rs 再生成|`host/org/artifacts.rs`（新規）、`host/org/mod.rs`・`view.rs`・`ipc.rs`、`commands.rs`、`lib.rs`|
|R3-5|UI: 差分部品の切出し（`DiffText`・種類アイコン・行数）→`DiffParts.tsx`、ChangesDialog はそれを使う。成果物節・`OrgArtifactsDialog`・`OrgImportDialog`（確認・結果・状態を確認・戻す）、完了バナー、表の列リンク、`OrgListRow` の印、結果からの「プッシュ…」（既存 `gitPush:` 経路）|`src/ui/DiffParts.tsx`・`OrgArtifacts.tsx`・`OrgImportDialog.tsx`（新規）、`ChangesDialog.tsx`、`OrgPane.tsx`、`Dialogs.tsx`、`App.tsx`、`org.ts`、`src/ipc/client.ts`|
|R3-6|文書: 正本 §3.15.4 に「成果物パネル・確認つきの取り込み・戻す」を追記（版更新）、`LIVE_CHECK_ORG_R3.md`|司令塔|

## 5. 受入条件

1. パネルの差分・行数は Git から読んだ値で、取れないものは「未確認」。0 や空で代用しない。Worker の申告との違いが出る。
2. `org_artifacts`・`preview`・`diff`・`open`・`import_preview` は利用者の作業フォルダ・索引・ブランチ・worktree を変えない（結合テストで HEAD・`status`・`for-each-ref` が前後同じ）。
3. 取り込みは確認ダイアログの「取り込む」以外から起きない。token 不一致・HEAD 変化・衝突・未コミット変更との重なり・ロック中ファイル・`agentdock/` ブランチ・detached・途中状態では何も書かない。
4. 取り込みで動くのは今のブランチの ref・索引・対象ファイルだけ。重ならない未コミット変更は残る。フックは走らない。
5. 台帳の `begin` を書けなければ実行しない。`Applied` は読み戻しで確かめたときだけ表示。結果不明は「確認できません」と読取りの確認だけ。
6. 「取り込み前に戻す」は条件を満たすときだけ有効で、送信済みでは使えない。`--hard`・force 系は生成されない（args の単体テスト）。
7. worktree・ブランチ・`worktrees.json` は取り込み・戻すで変わらない。成果物パネルからは削除できない。
8. 取り込み後の「プッシュ…」は既存ダイアログを開くだけで、自動では送らない。
9. plannedTasks の Worker 報告と Reviewer 判定が `outputs\` に保存され、再起動後もパネルに出る（旧データは履歴から読むか「未取得」）。
10. `cargo check`・`tsc --noEmit`・追加テストが通る。

## 6. 単体テスト（純関数）

`artifact_status`（全行＋台帳ありで HEAD に含まれない場合・祖先判定未取得で Unknown）、`supersede`、`preview_kind`、`decode_text`（UTF-8/BOM/UTF-16/判定不能/1MB 切詰め）、`report_vs_diff`、`parse_numstat_z`（バイナリ `-`・移動）、`parse_merge_tree`（成功・衝突・壊れた出力）、`merge_tree_supported`（`git version 2.37.1`・`2.45.2.windows.1`）、`import_blockers`、`dirty_overlap`（移動・未追跡・大文字小文字）、`classify_import`、`reconcile_import`、`undo_decision`、`GitOp` の新規 args（force・`--hard` が出ない、40桁以外の commit 拒否、パスは `--` の後ろ）。結合テスト（一時リポジトリ）: 取り込み成功・衝突で無変更・未コミット重なりで無変更・重ならない変更が残る・戻す・`agentdock/` ブランチで拒否。

## 7. 実機確認（ユーザー）

1. 標準テンプレートで md・csv・画像・コードを作らせ、完了後に成果物パネルでファイル一覧・行数・プレビュー・差分・Reviewer の所見が見える。docx は「開く…」で開ける。
2. 「取り込む…」→ 確認の一覧どおりに作業フォルダへ反映される。Git の履歴に取り込みコミットが1つ（候補ごと）できる。worktree とブランチは残る。
3. 作業フォルダで同じファイルを未コミットで編集してから取り込む → 何も変わらず、重なったファイルが示される。別のファイルの編集だけなら取り込め、その編集は残る。
4. 取り込み対象の xlsx を Excel で開いたまま取り込む → 「開かれています」で止まる。
5. 自分のブランチで同じ行を先に変更・コミットしてから取り込む → 衝突の案内、作業フォルダは無変更。
6. 取り込み後「取り込み前に戻す」→ 元に戻り、再び「レビュー待ち」。取り込み→「プッシュ…」→ 送信後は「戻す」が無効になる。
7. 再起動後もパネルの所見・取り込み済みの印が残る。

## 8. 要判断（実装は推奨で進める）

- J1 取り込める時期: ワークフロー完了後だけ（推奨。実行中の Worker は始点に今のHEADを使うため、途中で動かすと差分の基準が分かりにくい）／完了した成果はいつでも。
- J2 取り込みコミット: 常に作る（推奨）／早送りできるときは作らない（履歴は短いが、戻し先・取り込みの区切りが曖昧）。
- J3 合格していない成果（`NotPassed`）: 同意チェックつきで取り込み可（推奨）／取り込み不可。
- J4 Git 2.38 未満: 取り込みを出さず案内のみ（推奨）／作業フォルダで直接 merge する旧方式を用意（失敗時に作業フォルダを汚すため非推奨）。
- J5 文字コード: UTF-8・UTF-16 のみプレビュー（推奨。Shift_JIS は「開く…」）／`encoding_rs` を追加して Shift_JIS も表示。
- J6 送信後の「戻す」: 不可（推奨。force を使わない合意のため）／打ち消しコミット（`revert -m 1`）を確認つきで作る。
- J7 取り込み時のフック: 走らせない（推奨。既存の組織コミットと同じ）／利用者の post-merge を走らせる。

## 9. 実装で確かめた点・設計との差分（2026-10-20、R6・R7 実装）

要判断 J1〜J7 は推奨のとおりに実装した（J1 完了後だけ、J2 取り込みコミットは常に作る、J3 同意チェックつきで取り込める、J4 Git 2.38 未満は案内のみ、J5 UTF-8・UTF-16のみ、J6 送信後は戻せない、J7 フックなし）。

### 9.1 確かめた点（quality-loop の検証工程と書込み工程のコミットの対応付け）

- 組込みテンプレート `quality-loop` は `build`（`capture: commit`）と `verify`（`target: build`）で、検証工程のタスクは**ノードごとに1つ**（`task_of_node` で再利用）。周回しても検証の活性化は増えず、判定はスロット `verdict` の版（`SlotWritten`）が増えるだけ。
- 検証役は、送られる前に必ず対象の最新の確定コミットへ移される（`graphrun.rs` の `graph_launch`、`Launch::Refresh`）。書込み工程が次にコミットするのは、判定の辺（`feedback`）が通って再実行されたあとなので、**ある判定の対象コミット＝その `SlotWritten` より前に書込み工程が最後にコミットしたもの**で取り違えない。
- 実装: `rules::org_artifacts::review_rounds`（イベント列から判定の版と対象コミットを出す純関数）。書込み工程がまだコミットしていない判定は対象コミットを None にする（推定しない）。単体テスト `graph_verdicts_are_matched_to_the_commit_the_writer_made_just_before`。
- 計画経路（standard）は検証役のタスクが書込みごとに1つで、判定は試行 k（= 書込みの差し戻し回数）ごとに `outputs\<検証役>.<k>.json`。保存のない回は `FindingsRecorded`（差し戻しの所見）→ 最新の回だけ検証役の最終回答（履歴）→ 判定「未取得」。合格・不合格は推定しない。

### 9.2 設計との差分

- **確認の控え（token）の再検証**: 設計は「`R` を再計算して同じでなければ何もしない」だが、取り込みコミットは時刻を含むため再計算では一致しない。代わりに、確認時の `R` を使い、実行の直前に「HEAD・ブランチが `H` のまま」「`R` が存在し `H` の子孫」「途中の操作なし」「重なる未コミットの変更・ロック中ファイルなし」を再検査する（`gitops::import::recheck`）。`H` と `R` が不変なので、変わる内容は確認時と同じ。控えは1回だけ使え、10分で切れる。
- **`GitOp`**: `DiffTreeNames` は作らず、`DiffCommitsNamesM`／`DiffCommitsNumstat` がコミットの代わりにツリーIDも受ける。`--name-status -M` の専用操作 `DiffCommitsNamesM` を追加（既存の `DiffCommitsNames` は移動を検出しないため）。
- **無視ファイル**: Git は無視ファイル（`.gitignore` 対象）を確認なしに上書きするため、取り込みで新規追加されるパスに、追跡されていない既存のファイルがあれば「重なり」として止める。
- **取り込み先の拒否**: 取り込み先のフォルダが AgentDock の worktree（所有記録のあるパス）に当たるときも拒否する。
- **ledger**: `imports.jsonl` の行に `undoOutcome` を追加（`outcome` は取り込みの結果、`undoOutcome` は戻すの結果）。`begin` だけ・`undoBegin` だけの行は、表示のたびに今のHEADで見分ける（自動で再実行しない）。
- **`PutOutput`** はエンジンの `OrgStore` に `put_output`／`read_output`（既定は書けない／未取得）を追加して保存する。書けなくても以後の副作用は止めない（`PutSlot` と違い、経路の判定に使わない）。
- **ロック**: リポジトリ単位の直列化は、既存の準備・プッシュと同じ `gitprep_rt`（同じフォルダのGit操作は同時に1本）を共用した。`import_rt` は別に作っていない。
- **「開く…」**: 実行形式の確認は既存の `attach::is_executable_path` と `BlockedReason::OpenNeedsConfirm` を使い、書き出し先は `%LOCALAPPDATA%\com.agentdock.app\tmp\artifacts\<wf>\<task>\<元のファイル名>`（読取り専用）。前のコピーが開かれたままなら置き換えず、その旨を返す。
- **大きなファイル**: Git の出力上限（16MiB）を超えるファイルは、プレビューも「開く…」も断る（先頭だけ読む手段がないため）。画像は10MBまで。

### 9.3 実機確認で見てほしい点（追加）

- 取り込み先が入口で作ったリポジトリ以外の「サブフォルダ」のとき（リポジトリ本体のサブフォルダを作業フォルダにしている場合）の取り込み・重なり検査。
- Excel などで取り込み対象を開いたままにしたときの「開かれています」（共有違反の検出）。ウイルス対策ソフトの一時的なロックで誤って止まる場合は、閉じ直して再試行する。
