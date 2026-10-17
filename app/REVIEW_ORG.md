# REVIEW_ORG — 組織実行の差分レビュー（R-ORG）

- 対象: `feature/org-engine`（`git diff master...HEAD`、P-ORG-2〜8・7a・7b）。組織実行関連のファイルを中心に読んだ。
- 基準: 正本 §3.15（M53〜M60・A135〜A150）、§4.2、§4.5、CLAUDE.md の確定事項（Q1〜Q11・J1・J2）。
- 件数: **高 4／中 6／低 10**。P-ORG-9 では高→中の順に直す。要判断は2点（M3・M6）。

---

## 高

### H1. 再起動すると計画承認待ちが消え、「再開」だけで未承認の計画が動き出す
- 場所: `app/org-engine/src/persist.rs:525-527`（`classify_after_restart`）、`app/org-engine/src/core.rs:576-587`（`user_resume`）
- 到達経路: 計画が利用者承認の条件に当たる → `Halted{AwaitingPlanApproval}` → アプリを終了・再起動 → `recover_on_restart` が `halt != Restart` なので `Halted{Restart}` で**上書き** → 承認カードが消える（`ipc.rs:139-146` は `AwaitingPlanApproval` でだけ承認を受ける）→ 利用者が「再開」→ `user_resume` の `Some(_) => clear_halt()` → 未承認の Worker タスクが promote・dispatch される。A136／§3.15.1 の承認を迂回できる。
- 推奨修正: `classify_after_restart` は既存の halt が `AwaitingPlanApproval` のときは上書きしない（再起動の理由は別のフィールドか通知で持つ）。加えて `user_resume` は `AwaitingPlanApproval` を決して解除しない（今は `Some(AwaitingPlanApproval)` 分岐で拒否するが、上書き後は効かない）。テスト: 承認待ち→`recover_on_restart`→`UserResume` で Send が出ないこと。

### H2. 切断・wake・再起動のあと、レビュー待ち（Reviewing）や中断要求済みのタスクが永久に止まる
- 場所: `app/org-engine/src/core.rs:977-993`（`freshness` は `Running | Reviewing` を `NeedsReconcile` にする）、`persist.rs:514-519`（再起動でも同じ）、`app/src-tauri/src/host/org/driver.rs:230-235`（照合は `status == Running && NeedsReconcile` だけ）、`core.rs:1012-1016`／`scheduler.rs:26-28`（依存は `confirm == Confirmed` 必須）
- 到達経路: Worker がコミット済みで `Reviewing`（自分のturnは終わっている）→ sleep/wake・切断・再起動 → 確認状態が `NeedsReconcile` になる → 照合対象は `Running` だけなので戻らない → Reviewer は `dep_ok` 不成立で始まらない（始まっていても Pass で対象を `Completed` にするが confirm が残り、`check_done` も後続の `default_dep_ok` も満たさない）。再起動時に `InterruptRequested` だったタスクも照合対象外で同様に残る。UI からは取消し以外に出口がない（手動完了は `Confirmed` が条件）。
- 推奨修正: (a) `freshness`／`classify_after_restart` は、Codex 上で実行中の可能性がない `Reviewing`（自分の受理turnの終端を観測済み＝`observed.turn == accepted_turn`）を要照合にしない。(b) `reconcile_read_only` の対象に `Reviewing` と `InterruptRequested`／`StopUnconfirmed` を加え、履歴で終端を確認できたら確認状態を戻す入力（例 `OrgInput::Reconciled { task, end }`）をエンジンに足す。テスト: Reviewing＋`Freshness{Wake}`→照合→Reviewer が dispatch されること。

### H3. Reviewing の Worker が枠を占有し、全体の並列数が小さいとレビューが始まらずに停止する
- 場所: `app/org-engine/src/scheduler.rs:36-42`（`occupies` が `Reviewing` を実行中と数える）
- 到達経路: 設定で全体並列を1（1〜4は正当な値）→ Worker が `Reviewing` になる → 全体1枠を占有 → Reviewer は `total >= max_total` で永久に出ない。既定（書込み2・全体3）でも「全体 ≤ 同時に Reviewing の Worker 数」になれば同じ（例: 2・2 で Worker 2件が同時にレビュー待ち）。エラーも通知も出ない。
- 推奨修正: `Reviewing` で確認状態が `Confirmed` かつ自分の受理turnが終端済みのタスクは枠に数えない（Codex 上で動いていない）。A146 の意味（同時に走るturn数）と一致させる。テスト: `max_total=1` で worker→reviewer が順に dispatch されること。

### H4. 司令taskの登録が解除されず、1件終わった後は新しいワークフローを開始できない
- 場所: `app/src-tauri/src/host/org/mod.rs:218`（開始時の拒否）、`:249`・`:262`（役割の読み直し・有効化の拒否）、`:438-444`（登録）。`driver` を `None` に戻す箇所が無い。
- 到達経路: ワークフローが完了／全件取消し → `OrgPane.tsx:40,258` の「新しいワークフローを開始…」は有効になる → `org_start` は「組織実行はすでに動いています」で拒否。役割の有効化・読み直しも拒否され続ける。再起動すると終わったワークフローは復元されない（`mod.rs:394-397`）ので、アプリ再起動でしか抜けられない。司令taskが panic した場合も同じ（閉じた送信口が残る）。
- 推奨修正: 「終わった」（全タスクが終端、またはワークフロー取消し）を判定した時点で司令taskを終了させ、`DriverHandle` を外す（写し `view` は残して画面に結果を出し続ける）。開始時の判定は「送信口が生きている かつ 未終了」に変える。`org_start` の判定と登録は1回のロック区間で行う（L6）。

---

## 中

### M1. 差し戻し後の再レビューが、古いコミットに固定された worktree で行われる
- 場所: `app/org-engine/src/core.rs:1118-1126`（チャットがあれば `continue_launch` で再利用）、`:1139-1149`（Reviewer の detached worktree は初回だけ作る）、`app/src-tauri/src/host/worktree/org.rs:42-45`
- 到達経路: Reviewer が Rework → Worker が再作業して新しいコミット → Reviewer は既存チャット（cwd＝前回コミットに固定した detached worktree）へ新しいコミットIDを含む指示を送る → 作業ツリーは古いまま。Reviewer が作業ツリーを見て Pass すると、未レビューの変更で Worker が完了になる（M59 の「成果物の検証」が形骸化）。
- 推奨修正: 再レビューのたびに対象コミットで新しい detached worktree と新しい Reviewer チャットを作る（試行ごとに `reviewer-<task>-<n>`）、または Reviewer の再利用を禁止し、`target.commit()` と worktree の HEAD が一致しないときは送らない。

### M2. 「送る／取り消す」の「送る」が無く、要対応で止まったタスクの出口が取消しだけになる
- 場所: `core.rs:690-693`（拒否・照合で見つからない → `Blocked(NeedsAttention)`、文言は「送る／取り消すを選んで」）、`:713`（`retry_ready` は NotLive だけ・メモリだけ）、`:589-598`、`:536-541`（`ChatRecovered`）、`app/src-tauri/src/host/org/ipc.rs:222-243`
- 到達経路:
  1. 送信が拒否・`ReconciledNotFound` → 再試行の操作が無い（§3.15.2「確認できなければ送る／取り消すを示す」に反する）。
  2. 作成結果不明（`block_unknown_create`: `Blocked`＋`SendUnknown`）→ 利用者が「チャットを選ぶ」で結び付けても、`ChatRecovered` は `ChatBound` を記録するだけで状態も確認状態も戻らず、以後進まない（UI は「結び付けました」と出る）。
  3. `Dispatched` 追記後・ホストの送信前記録（`record_presend`）前に落ちると、ホストに照合対象が無く `SendUnknown` が永久に残る。
  4. `NotLive` の再試行は `retry_ready`（メモリ）なので再起動で失われる。コミット失敗・worktree作成失敗・利用上限も再試行の操作が無い。
- 推奨修正: 利用者操作 `org_retry_task`（`UserConfirmed` 必須・確認ダイアログ付き）をエンジン入力 `UserRetryTask { task }` として追加し、`Blocked(NeedsAttention)` かつ未送信が確定（`Rejected`／`ReconciledNotFound`／作成拒否・失敗）のタスクだけ `Ready` に戻す（新しい試行。自動再送はしない）。`ChatRecovered` 受理時は `SendUnknown`→`Confirmed`、`Blocked(NeedsAttention)`→`Ready` に戻す。3. は再起動時の照合で「ホストに記録が無い `SendUnknown`」を履歴の `client_message_id` で読取り照合し、見つからなければ利用者へ「送る／取り消す」を出す。`retry_ready` は記録（イベント）から導く。

### M3. 完了判定（M59）と §4.2 の差：子孫の判定値が鮮度の変化で古いまま使われる（**要判断**を含む）
- 場所: `core.rs:837-844`（`desc_state` は `descendants`／`open_requests` の写しを読む）、`:971-994`（`freshness` は `live`・`early_turns` を消すが `descendants`・`open_requests` を消さない）、`driver.rs:240-242`
- 評価: 通常の経路では、子孫の判定（`org_descendants` → キューの判断）が「親が live でなければ結論を出さない」（`rules/org.rs:140-147`）ため、履歴で終端を確認しただけでは完了しない。M59 はおおむね守られている。ただし穴が2つある。
  1. 待機中（`WaitDesc`）に切断 → 照合で履歴の終端 → 最終回答の読直し → `WaitDesc` に入った瞬間、切断前の `(working 0, unknown 0, scan_complete)` の写しで即完了する（親が非liveのまま完了。§4.2 1.4 は親が非liveなら保留）。
  2. 解決済みの承認が `open_requests` に残ると、`RequestResolved` が来ない限り完了しない（停止側の誤りなので安全側だが止まる）。
- 推奨修正: `freshness` と `TurnEnded` 受理時に、そのチャットの `descendants`／`open_requests` を消す（次の読取り結果だけで判断）。
- **要判断**: 再起動後のタスクは、利用者の「再開」で resume → 親は live だが turn の終端そのものは履歴でしか見ていない、という状態で完了する。M59 の文言（live で turn の完了を確認）を厳密に取ると完了できない。選択肢: (a) 現状を許容し、画面・記録に「終端は履歴で確認」と印を付ける（推奨。§4.2 の「親が live かつ完了確認済み」の読みと整合）、(b) 履歴で終端を確認したタスクは「要確認」で止め、利用者の「完了として受け入れる」を要する。

### M4. 役割定義の項目の多くが反映されず、独自の役割は黙って動かない
- 場所: `core.rs:1154-1170`（`role_cfg` は sandbox・model・effort だけ）、`:196-203`／`:1118-1120`（`role_kind` が標準3IDだけ。他は `launch` が何もせず戻る）、`app/src-tauri/src/host/org/ports.rs:253-258`（承認方針は sandbox からの固定値）
- 到達経路: (1) 役割で `approval: Untrusted`（狭める指定）にしても、チャットは `OnRequest` で作られる＝利用者の定義より広い権限で動く（§3.15.3）。`instructions`・`workplace`・`extraWritableDirs`・`allowNetwork` も使われない。確認ダイアログで「広げる定義を有効化」しても実際には効かない（表示と挙動が食い違う）。(2) `worker` 以外のIDで書込み役を定義すると、計画の検証は通り、承認後もタスクは `Ready` のまま一度も動かず、通知も出ない。
- 推奨修正: 承認方針（Untrusted を反映）と `instructions`（各プロンプトの前置き）を反映する。反映しない項目（作業場所・追加フォルダ・ネットワーク）は、読込み時に「この版では未対応」として無効扱いにし理由を出す。標準3役以外のIDは、`kind`（planner/worker/reviewer 相当）を定義に持たせるか、未対応として読込み時に問題扱いにし、計画の検証で「未知の役割」として不合格にする。

### M5. 所有worktreeのコミットが、worktree内の `.git` ファイルの書換えで別のgitdir・ブランチへ向く余地がある
- 場所: `app/src-tauri/src/gitops/org.rs:77-91`、`app/src-tauri/src/gitops/mod.rs`（`CommitStaged`）
- 根拠: 所有・ブランチの確認はリポジトリ本体の `git worktree list` で行うが、`StageAll`／`CommitStaged` は `dir`（worktree）で実行され、gitdir は worktree 直下の `.git` ファイル（作業領域内＝Worker の sandbox で書ける）に従う。Worker（またはプロンプト注入）が `.git` を `gitdir: <本体>/.git` に書き換えると、確認は通ったまま利用者のブランチへコミットされ得る（A143）。また `--no-verify` は pre-commit／commit-msg だけを止め、post-commit フックは走る（コメント「フックは走らせない」と不一致）。symlink／junction を実際に使ったテストも無い。
- 推奨修正: コミット直前に `dir` で `git rev-parse --absolute-git-dir` と `git symbolic-ref HEAD` を読み、一覧の該当 worktree の管理フォルダ（`<common>/.git/worktrees/<name>`）と記録のブランチに一致しなければ拒否（新しい `Refusal::GitDirMismatch`）。より確実には `--git-dir=<管理フォルダ> --work-tree=<path>` を明示して実行する。`-c core.hooksPath=<空の専用フォルダ>` でフックを全部止める。junction（`mklink /J`、管理者不要）で置き場所の外を指すケースのテストを追加。

### M6. 「再開」で resume する範囲が「送信直前」より広く、再開後の照合も頼まない（**要判断**）
- 場所: `app/src-tauri/src/host/org/mod.rs:284-311`
- 根拠: 終端以外の全タスクのチャット（`Running`・`Reviewing`・止まっているもの）を resume する。§3.15.2「resume は送信直前に限る」より広い。一方で、再起動後に実行中だったタスクを live で完了させるには resume が要る（M3 と関係）。また resume 後に `Reconcile` を頼まないので、照合と再開の間に終わった turn は `NeedsReconcile` のまま残る。
- 推奨修正: resume 後に `DriverMsg::Reconcile` を送る（読取りだけ。確定でよい）。
- **要判断**: resume の範囲。選択肢: (a) 現状（再開の確認ダイアログに「実行中のタスクのチャットも再開して状態を受け取ります」と明記して利用者操作の範囲とする。推奨）、(b) これから送るタスク（`Ready`／差し戻し）のチャットだけ resume し、実行中だったものは履歴照合と M3(b) の利用者確認で完了させる。

---

## 低

- **L1** `app/src-tauri/src/host/lifecycle.rs:304-306`: 終了の**要求**の時点で送信権を失効させ `Shutdown` で止める。確認で「やめる」を選んでもワークフローは止まったまま（再開操作が要る）。§3.15.6「終了を選ぶと新しい送信を止め」との差。推奨: 確認を出す場合は `StopAndQuit`（`:335`）でだけ止める。確認なしで終了する分岐（`:308-311`）は今のままでよい。
- **L2** `lifecycle.rs:307-312`: 実行中のturnが無ければ確認なしで終了するため、未終了のワークフロー（承認待ち・タスク間）があっても「組織実行中」の併記が出ない。推奨: `org_running_notes()` が空でなければ確認を出す。
- **L3** Q5-B（自動再計画1回）は未実装で、再計画は常に利用者の操作（計画承認待ちの間だけ）。上限「1回まで」を超えない方向なので**許容**。`auto_replans_used` は常に0と表示される旨を `app/DEFERRED.md` に記録する。
- **L4** §3.15.5「依存先が再実行で新しいコミットになったら後続のレビュー結果を陳腐化と示す」は未実装。現在の依存規則（完了＋確定が条件）では起きにくいが、M1 の修正と合わせて「対象コミットとレビュー時コミットの不一致」を表示に出す。
- **L5** `core.rs:542-547`: エンジンは `StopUnconfirmed` を設定しない（10秒経っても「中断要求済み」のまま）。表示は「停止は未確認」と出るので誤表示ではないが、ホストの停止記録（停止未確認）から `StopUnconfirmed` へ移す入力があると §4.5 と揃う。
- **L6** `app/src-tauri/src/host/org/mod.rs:218-234`: 二重押しで `org_start` が並ぶと、判定と登録の間に隙がある（後の登録が前を上書き、前の司令taskは送信権を失って宙に浮く）。判定〜登録を1つのロック区間に。
- **L7** `app/src-tauri/src/backend/backend.rs:338`: `UserConfirmed::from_user_command` は `pub(crate)` で、型保証は「呼ぶのは commands.rs だけ」という約束頼み（現状の呼出しは commands.rs・テスト・`host/mod.rs:727`（既存、名前付け）だけで、組織の経路からの生成は無いことを確認）。`OrgGrant::issue` も `pub(in crate::host)`。推奨: commands.rs とテスト以外で `from_user_command` と `OrgGrant::issue` が出ないことを確かめるテスト（ソース走査）を1本足す。
- **L8** `mod.rs:394-397`: 終わったワークフローは再起動後に復元されず、完了時のブランチ・コミット・差分（Q7「完了時に示す」）を見る手段が消える。読取り専用の写しとして最新1件を表示する。
- **L9** `app/src-tauri/src/host/org/roles.rs:100-111,128-138`: 読めない標準役割ファイルは退避され、既定の内容で作り直される。上書きではない（A142）が、編集途中の保存ミスで黙って既定に戻る。作り直したことを問題一覧に明示するか、同IDの退避があるときは作り直さず無効のまま示す。
- **L10** 設定の「役割の有効化」確認ダイアログで Esc を押すと設定画面ごと閉じる（既知の軽微点）。ダイアログ側で `stopPropagation`／`preventDefault` し、既存の `e.defaultPrevented` 判定（`OrgPane.tsx:105` と同じ形）を設定側にも入れる。

---

## 問題なしと確認した観点

- 送信権: `OrgGrant`／`DispatchPermit`／`CreatePermit` は `Clone`・`Default`・`Serialize` なし・フィールド非公開・許可は `OrgGrant` からだけ（`grant.rs`）。`SendAuth::Org` を作るのは `ports.rs:344`（`org_send`）だけで、許可の再確認は `org_send` と `dispatch_send_as` の2か所。司令task以外（監視・照合・再起動・wake・キュー）は `GrantRegistry`（失効の管理）しか触れない。
- 利用者側の保護: 送信・再送・キュー（積む・編集・再開・今すぐ送る・自動送信）・モデル・権限・作業フォルダ・目標・レビュー・分岐再送・圧縮・管理・変更を戻す、に `check_not_org_owned` が入っている。
- resume: `resume_chat_live` の呼出しは `org_resume` だけ。復元・照合・wake は読取りだけ。
- 承認・質問: エンジンは件数（`RequestPending`）を受けるだけで、応答の経路が無い（A138）。
- 状態変換: 切断・Gap は `Freshness`、再起動は `NeedsReconcile`、中断は `Blocked(ManualStop)`。Cancelled は利用者操作（`UserCancelTask`）だけ、Failed は turn 失敗・成果物不正・差し戻し上限だけ。通信断・終了を取消・失敗にする経路は無い（A147）。
- 受理不明: エンジンに再送の入力が無い。`Dispatched` は送信 action より前に追記され、追記失敗後は副作用を実行しない。`DispatchSettled` は `apply` で `AttemptRef` を照合し、別試行・重複を拒否する（A139）。手動に切り替えたチャットへは許可が出ない（A140）。
- 終了・ロック: 送信口は unbounded で満杯にならず、閉じていれば送信を捨てるだけ。`org_flush` は3秒で打ち切る（司令taskが送信待ちでも終了は止まらない）。ロック順は observer → host data の一方向で、逆順の取得は見当たらない。`events.jsonl` 追記失敗で送信権を手放し、以後は送らない。
- 書込み: `authorize_commit` の拒否（記録なし・所有外・別タスク・読取り用・未完成・本体・置き場所外・`agentdock/org/` 以外）と Git 一覧のブランチ一致は実リポジトリのテストあり。マージ・push・利用者ブランチ操作の GitOp は無い（A143・A144、Q7）。`~/.codex` への書込みは無く、config.toml は `ext_lock` 内で前後を読むだけ（J2）。役割ファイルは `create_new` で既存を上書きしない（A142）。
- 通知: 計画承認待ち・停止系は即時、完了・失敗はワークフロー単位の2秒集約。本文は定型文、名前はIDの末尾4桁で依頼文を含まない（A150）。
- UI: 閲覧・選択（一覧の行・ドックの印・タスク表・「チャットを見る」）は何も送らない。取消し・再計画・手動完了・手動切替・上限変更・再開は確認ダイアログ経由。状態の表示はアイコン＋文言で色だけに依存しない。
- 確定事項: Q1（別crate・接続共有）、Q2（定義ファイル・標準3役）、Q3（条件外は自動承認）、Q4（既定2・3、1〜4。ただし H3）、Q5（差し戻し2回。自動再計画は L3）、Q6、Q7、Q8（照合は自動・送信は再開後。ただし H1）、Q9、Q10（停滞は通知だけ）、Q11（初期30・上限で停止と引上げ）、J1、J2 は実装と一致。
