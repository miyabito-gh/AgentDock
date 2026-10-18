# 組織ワークフローの削除・アーカイブ（設計 v0.1、2026-10-18）

対象: DEFERRED.md「組織ワークフローの削除・アーカイブが未実装」。DESIGN_ORG §(組織チャットの保護)「削除・アーカイブはワークフロー単位でだけ行い、既存の停止確認・削除保留に従う」の具体化。
前提の正本: §3.4（M46 停止未確認の削除は保留）、§3.6（M33・M43）、§3.15.4「worktree・ブランチはワークフローの削除でも自動で消さない」。

## 1. 対象の判定（純粋ロジック `rules::org_manage::verdict`）

入力: タスクごとの (phase, confirm)、司令taskがこのワークフローで動いているか、計画承認待ちか、削除途中の記録、対象チャットごとの `ChatWork`・削除保留・保存失敗、接続状態。

| 条件 | アーカイブ | 削除 |
|---|---|---|
| 全タスクが Completed／Cancelled／Failed（空のときは計画承認待ちでないこと） | 必須 | 必須 |
| 全タスクの確認状態が Confirmed（SendUnknown・NeedsReconcile・InterruptRequested・StopUnconfirmed は不可＝通信断・停止未確認を除外） | 必須 | 必須 |
| 司令taskがこのワークフローで動いていない（`driver_active()` かつ同じID なら不可） | 必須 | 必須 |
| 削除する役割チャットごとに `delete_decision == Proceed`（未完了・停止未確認・所有不明・受理不明・初回送信中・キュー送信中なし。状態不明は `confirm_unknown_terminals(force)` で履歴の終端を読んでから判定） | — | 必須 |
| 対象チャットに保存失敗がない（`save_blocks_chat`）、引き継ぎ中・作業領域の共用がない | — | 必須 |
| Codex に接続している（履歴削除が要るため） | — | 必須 |

- Failed を含む「終了」も対象にする（エンジンの Failed は確認済みの結果だけで、通信断からは作らない）。→要判断1。
- 満たさないときは理由の一覧を返して拒否する。**削除のために中断はしない**（対象は終わったワークフローだけなので、停止未確認なら削除保留を作らず拒否。§3参照）。
- 再起動後に読取り専用で出る「未終了の古いワークフロー」は対象外（理由「終わっていないワークフローです」）。→要判断3。

## 2. 削除で消すもの・消さないもの

| 対象 | 扱い |
|---|---|
| `org\<wf>\`（workflow.json・events.jsonl・usage.jsonl・graph.json・roles.json・slots\） | 消す。最後に消す（§3の順序） |
| 未解放の役割チャット（`chat.json.org.workflow_id == wf` かつ `released_at == None`。タスクの `chat` 参照と chat.json の所有記録の**和集合**。作成結果が失われたものも含む） | 既定: 既存の削除と同じ（Codex履歴の削除→チャット専用領域。side相談も `forget_deleted_chat` が消す）。利用者が「残す」を選んだら削除せず `released_at` を付けて通常チャットに戻す |
| 手動に切り替え済みの役割チャット | 消さない（利用者が引き取ったもの）。`org` 記録はそのまま、表示は「元のワークフローは削除済み」 |
| 所有worktree（`WorktreeRecord.org.workflow_id == wf`）とブランチ `agentdock/org/…` | **消さない**（§3.15.4）。台帳の記録も残す。削除は既存の worktree 管理（`remove_worktree`、未コミットは2段目の確認、ブランチは `-d` のみ）で個別に行う |
| 役割定義・グラフ定義（ルートの定義ファイル）、利用者の作業ツリー・ブランチ | 触れない |

確認画面に出す内容（`OrgDeletePreview`、読取りだけ）:
- 消える役割チャットの件数と一覧（役割・タスク・チャット名）、Codex の履歴も消えること、手動に切り替え済みで残るチャットの件数。
- 残る worktree の一覧: パス、ブランチ、未コミットの変更件数（`dirty_count`＋`ignored_count`）、**未取込みのコミット数**（`git rev-list --count HEAD..refs/heads/<branch>`。読めなければ「未取得」と表示し0で代用しない）。未取込みがある行は強調し「ブランチに残ります。取り込む場合は削除の前後どちらでも可能」と添える。
- 記録の容量（`org\<wf>` の合計。読めなければ未取得）。
- 判定が拒否なら理由の一覧と、無効の「削除」ボタン。

## 3. 削除保留との関係と実行順

- 停止未確認の役割チャットがあれば**拒否**し、M46 の削除保留（chat.json の `delete_pending`）は作らない。ワークフローを終わらせる・停止を確認する手段は既存の「取り消す」と停止確認であり、削除はその後に利用者が再度選ぶ（M46 の「停止確認後に再度削除を選んだときだけ削除」と同じ形）。
- 既に `delete_pending` がある役割チャット: `PartialFailure` は続きから実行（既存の `execute_delete` が `done` を引き継ぐ）。それ以外の理由の保留は拒否（停止確認が先）。
- 実行順:
  1. `org\<wf>\delete.json` を書く（`{requestedAt, deleteChats, chats[], done[], failed[]}`。原子的書換え）。**書けなければ何もしない**。
  2. 役割チャットを1件ずつ削除（または解放）。各チャットは既存の `delete_chat` の排他（引き継ぎガード・`deleting`・控えのキュー）を通る。結果を `delete.json` に追記。
  3. 全チャットが済んだら、`org\<wf>` を slots\ → graph.json・roles.json → usage.jsonl → events.jsonl → workflow.json → delete.json → フォルダ の順で消す。
  4. どこかで失敗したら完了と偽らず `delete.json` に `failed` を残して `Partial` を返す。画面は「削除の途中です（済: …／失敗: …）」と「削除を続ける…」。続きは同じコマンドで、`done` は飛ばす。
  5. 成功したら、`org_rt.view` が同じIDなら `None` に、observer の bind を外し、grants の解放表から外す。
- 再起動で途中のものは、`delete.json` があれば `workflow.json` が読めなくても最小の行（ID・削除途中・済／失敗）として過去一覧に出す（自動では続けない）。`org_restore` は `delete.json` のあるフォルダを復元対象にしない。

## 4. アーカイブ

- 保存場所: `org\<wf>\listing.json`（`{schemaVersion, archivedAt}`、原子的書換え）。チャット側には書かない。削除でフォルダごと消える。
- アプリ内だけの操作。Codex の `thread/archive` は役割チャットへ送らない（役割チャットは元々一覧から隠れているため。可逆性を優先）。→要判断2。
- 効果: 左一覧の「組織実行」行・過去の行から外す。役割チャットは引き続き通常一覧から隠す（`shownOrgIds` にアーカイブ分のIDも含める）。
- 復元あり: 左一覧下の「アーカイブ（N）」ダイアログに「組織ワークフロー」節を足し、各行に「開く」「アーカイブを解除」「削除…」。解除は `listing.json` を消すだけ（確認なし）。
- 過去一覧の件数（`HISTORY_KEEP=10`）はアーカイブを除いた後に数える。アーカイブ一覧は新しい順に最大50件（読めないものは飛ばす）。
- アーカイブ中でも削除の判定・手順は同じ。

## 5. IPC（`backend/org.rs`、ts-rs生成、`commands.rs`、`client.ts`）

```rust
/// 削除確認の内容（読取りのみ）。
pub struct OrgDeletePreview {
    pub workflow_id: String,
    pub title: Option<String>,
    pub verdict: OrgManageVerdict,
    pub chats: Vec<OrgDeleteChatRow>,        // 消える（または解放される）未解放の役割チャット
    pub kept_released: Vec<ChatKey>,         // 手動に切り替え済みで残るもの
    pub worktrees: Vec<OrgKeptWorktreeRow>,  // 残る所有worktree
    pub record_bytes: Known<u64>,
    pub deletion: Option<OrgDeletionView>,   // 途中の削除があれば
}
pub enum OrgManageVerdict { Allowed, Blocked { reasons: Vec<String> } }
pub struct OrgDeleteChatRow { pub chat: ChatKey, pub task_id: String, pub role_id: String, pub name: Option<String>, pub blocked: Option<String> }
pub struct OrgKeptWorktreeRow { pub path: String, pub branch: String, pub access: OrgWorktreeAccess, pub exists: bool, pub dirty: Known<u32>, pub unmerged_commits: Known<u32> }
pub struct OrgDeletionView { pub requested_at: UnixMillis, pub done: Vec<String>, pub failed: Vec<String> }
pub struct OrgDeleteArgs {
    pub workflow_id: String,
    /// false なら役割チャットを消さず、手動に切り替えて通常チャットとして残す。
    pub delete_chats: bool,
    /// 確認画面で見せたチャット。実行時の集合と違えば拒否（確認し直し）。
    pub expect_chats: Vec<ChatKey>,
}
pub enum OrgDeleteOutcome { Deleted, Partial { done: Vec<String>, failed: Vec<String> } }
```
- 追加フィールド: `OrgWorkflowView.archived_at: Option<UnixMillis>`、`OrgWorkflowView.deletion: Option<OrgDeletionView>`、`OrgSnapshotView.archived: Vec<OrgWorkflowView>`（すべて `#[serde(default)]`）。
- コマンド: `org_preview_delete(OrgWorkflowArgs) -> OrgDeletePreview`、`org_delete_workflow(OrgDeleteArgs) -> OrgDeleteOutcome`（`UserConfirmed::from_user_command()`）、`org_archive_workflow(OrgWorkflowArgs) -> ()`、`org_unarchive_workflow(OrgWorkflowArgs) -> ()`。
- エラー: 判定の拒否は `Blocked`（理由文を連結）、集合の不一致は `Rejected`、未接続は `NotConnected`、`delete.json` を書けないときは `Io`。

## 6. UI

- ワークフロー画面（`OrgPane.tsx`）の「…」メニュー: 「ワークフローを取り消す…」の下に区切り線、「アーカイブ」「削除…」。読取り専用の過去ワークフローでもこの2つは出す（今は `ro` でメニューごと消えているので、`ro` のときは2項目だけのメニューを出す）。判定は表示用に `finished`・確認状態で無効化し、最終判定はホスト。無効時の title「実行中・停止未確認・通信断のタスクがあるため、まだ削除できません」。
- 削除ダイアログ（`OrgDialogs.tsx` に `OrgDeleteDialog`、`ConfirmBlock`・`FootActions` を使う）:
  - 見出し「ワークフローを削除」。本文「このワークフローの記録（計画・経過・使用量）を削除します。元に戻せません。」
  - 「役割チャット N件の会話も削除する（Codex の履歴を含む）」チェック（既定オン）。オフの説明「オフにすると、手動に切り替えて通常のチャットとして一覧に残します」。
  - 「残るもの」: 手動に切り替え済みのチャット N件／worktree M件（各行に パス・ブランチ・未コミット件数・未取込みコミット数）。添え書き「worktree とブランチは削除しません。不要なら worktree の管理から個別に削除してください」。
  - 押下: 「削除する」（danger）。結果 Partial なら済／失敗を一覧し「削除を続ける」。
- アーカイブ: 確認なしで即実行し、トースト「ワークフローをアーカイブしました（アーカイブ一覧から戻せます）」。
- 左一覧（`LeftPane.tsx`）: アーカイブ済みは `OrgListRow`・`OrgHistoryRows` に出さない。「アーカイブ（N）」の N に組織ワークフローを含め、ダイアログに「組織ワークフロー」節。削除途中の行は「削除の途中」チップ。
- worktree 管理（`WorktreeDialogs.tsx`）: `org` 所有で元のワークフローが見つからない行に「元のワークフローは削除済み」。

## 7. 実装タスク（実装担当1名で順に）

| # | ファイル | 内容 | 受入条件 |
|---|---|---|---|
| D1 | `src-tauri/src/rules/org_manage.rs`（新規）、`rules/mod.rs` | `verdict(kind, input) -> OrgManageVerdict`、`delete_targets(tasks, bindings) -> (delete, kept_released)`（和集合・解放済み除外・重複なし）、`next_steps(record) -> Vec<Step>`（`done` を飛ばす） | 単体テスト（下記） |
| D2 | `store/org.rs`、`host/org/history.rs` | `delete.json`・`listing.json` の読書き、順序付きのフォルダ削除（各段の失敗を返す）、stamp を5ファイルに、アーカイブを除いてから `keep`、アーカイブ一覧、`delete.json` があり読めないときの最小の行。`org_restore` は `delete.json` のあるフォルダを飛ばす | 一時フォルダのテスト: 途中失敗で `delete.json` が残る、アーカイブが件数を消費しない |
| D3 | `gitops/worktree.rs` | `unmerged_count(git, repo, branch)`（読取りのみ。許可された GitOp に追加） | `cargo check` |
| D4 | `host/manage.rs` | `delete_chat` を `delete_chat_core(chat, confirmed, mode)` に分割。`DeleteMode::OrgWorkflow{workflow_id}` は所有記録の workflow 一致時だけ組織ガードを通し、`StopFirst`／保留が要る判定では中断も保留記録もせず `Blocked` を返す | 既存の削除の挙動が変わらない（既存テスト） |
| D5 | `host/org/manage.rs`（新規）、`host/org/mod.rs` | `org_preview_delete`・`org_delete_workflow`・`org_archive_workflow`・`org_unarchive_workflow`。§3の順序、`expect_chats` 照合、成功後の view・observer・grants の片付け。`org_view` に `archived`・`deletion` を詰める | 未接続・未終了・停止未確認で拒否、記録を書けなければ何も消さない |
| D6 | `backend/org.rs`、`commands.rs`、`lib.rs`（登録）、`src/ipc/client.ts`、`src/ipc/gen/` | §5の型とコマンド | `cargo check`、`tsc --noEmit` |
| D7 | `src/ui/OrgPane.tsx`、`OrgDialogs.tsx`、`LeftPane.tsx`、`orgHidden.ts`、`WorktreeDialogs.tsx` | §6 | `tsc --noEmit`。`orgHidden.test.ts` にアーカイブIDで隠れ続けるケース |

単体テスト対象（純粋ロジック）:
- `verdict`: 全終端＋Confirmed で可／1件でも Running・Blocked・Reviewing で不可／SendUnknown・NeedsReconcile・StopUnconfirmed で不可／司令task稼働中で不可／空タスク＋計画承認待ちで不可／削除のみ: 未接続・チャットの `has_unfinished`・`stop_unconfirmed`・`unresolved_send` で不可、アーカイブはこれらに依らない。
- `delete_targets`: タスク参照だけ・所有記録だけ・両方のチャットが1件ずつ、解放済みは kept、別ワークフローの記録は含めない。
- `next_steps`: `done` に入った段を再実行しない、チャットが全部済むまでフォルダ削除へ進まない。
- `shownOrgIds`: アーカイブ済みIDを含める（役割チャットが通常一覧へ漏れない）。

## 要判断（推奨案で設計済み）

1. 失敗（Failed）を含む終了ワークフローも削除・アーカイブの対象にする（推奨: 対象。Failed は確認済みの結果だけ）。
2. アーカイブで役割チャットに Codex の `thread/archive` を送らない（推奨: 送らない。アプリ内だけ）。
3. 再起動前から未終了のまま残った読取り専用の古いワークフローは、今回は削除不可（操作の手段がない。推奨: 別課題として DEFERRED に記録）。
4. 役割チャットの削除は既定オン（チェックで外せば通常チャットとして残す）。
