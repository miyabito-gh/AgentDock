# part-4: A13 / A14 / A15 / A16 / A17 / A20 / A22

調査日 2026-10-23。対象 feature/org-engine。判定はコードとテストのみを根拠とした。パスは `app/` 起点（`src-tauri/src` は S と略記、`org-engine/src` は E と略記）。

## 動作確認で実行したコマンド（変更なし・読み取り専用の cargo test）

| コマンド | 結果 |
|---|---|
| `cd app/org-engine; cargo test --lib persist` | 10 passed（再起動分類 `restart_makes_running_needs_reconcile_and_halts`、`restart_keeps_unknown_and_unconfirmed_and_never_cancels`、壊れた末尾・途中行の扱い、スナップショット破損時の全再生） |
| `cd app/src-tauri; cargo test --lib -- lifecycle:: store::atomic grant:: codex::tree stop::` | 25 passed（強制終了記録、終了前の作業数え、`GrantRegistry` の失効・手動切替、子孫ツリー組立て（循環・重複・孤児）、停止確認、原子書込み・追記の途中切れ復旧） |

制約: 実 App Server・実画面・Job Object の実機動作は未確認（ユーザーの実機確認事項）。UI（tsx）のテストは実行していない。

## 判定一覧

| ID | 判定 |
|---|---|
| A13 | 実装済み |
| A14 | 実装済み（Codex の thread API を利用＋独自の継続・台帳層） |
| A15 | 一部実装 |
| A16 | 一部実装 |
| A17 | 実装済み |
| A20 | 一部実装 |
| A22 | 一部実装 |

## A13 状態表示

| 項目 | 内容 |
|---|---|
| 判定 | 実装済み（バックエンド＋UI） |
| 概要 | 独自実装。Codex の原状態を `AgentState`（initializing/running/waiting/idle/done/failed/interrupted/closed/unknown）へ正規化し、`raw.label` に原文を保持。鮮度は別軸の `Freshness`（live/historyOnly/needsReconcile/disconnected/unsupported）。notLoaded・通信断・timeout は unknown に落とし、done にしない。親子は `codex/tree.rs` が組立て（循環・重複・親不明は孤児枠に分け、推測で繋がない）。右ペイン Dock は状態不明を「live未確認」と別ラベルで出し（履歴で終端確認できたものは状態を書き換えず注記）、不明件数を要約に付ける。 |
| 根拠 | S/backend/model.rs:198 `AgentState`、:289 `Freshness`（`allows_auto_send` :301）、:261 `AgentStatus`（turn・wait・evidence）／S/codex/tree.rs:68 `assemble`（テスト `unknown_parent_goes_to_orphan_frame_without_guessing`、`cycles_and_self_parent_do_not_loop_and_are_reported`）／S/host/mod.rs:428 `start_scan`、:456 `scan_once`（子孫の対象集合を読み取りのみで走査）、:515 `recheck_unknown_agents`／src/ui/Dock.tsx:50-87（`historyOutcomeOf`・「live未確認・取得…（履歴の読取りで確認）」）、:106 `unknownCount`／`unknownNote`、src/ui/chatState.ts（`sortByDockRank`） |
| 動作確認 | `codex::tree` のテスト 5 件合格。Dock の描画・並び替えは未確認（UI テスト未実行）。 |
| 不足・制約 | 現在の作業（`currentActivity`）は Codex が通知した item に依存し、未取得は「未確認」と表示（0・空で代用しない方針）。実機での大規模な子孫数の挙動は未確認。重要度 推奨／難易度 小（実機検証のみ）。 |

## A14 セッション・スレッド管理

| 項目 | 内容 |
|---|---|
| 判定 | 実装済み（履歴・再開・アーカイブ・削除・ピンは Codex API 利用＋独自の保存層。継続チャット・台帳・目的メモは独自実装） |
| 概要 | 履歴・再開・アーカイブは Codex の `thread/resume`（excludeTurns）・`thread/archive` を呼ぶ（S/codex/adapter.rs:872, :892）。resume は利用者操作の中だけで、監視目的では呼ばない。ピン・下書き・目的メモ・モデル設定・削除保留は AgentDock の `chat.json`（`%LOCALAPPDATA%\com.agentdock.app\chats\<dirId>\`）に保存。アーカイブは停止確認後に Codex へ送る作業中アーカイブ対応。削除は確認付きで、停止未確認なら削除しない（強制終了記録が削除を止めるテストあり）。継続チャット: 目的メモ（`plan_purpose_edit`、版つき）、引き継ぎ（`handoff_and_send`: 新 thread を作り、会話台帳.md を送信より前に書き、書けなくても引き継ぎは止めず資料にその旨を明記）、目的メモの下書き補助（`purpose_draft.rs`: 一時 read-only thread、構造化出力、引用検証、利用者確認後に保存）。コンテキスト継承は台帳＋目的メモ＋添付を資料として新 thread へ渡す方式で、Codex のコンテキストそのものの引継ぎではない。 |
| 根拠 | S/host/manage.rs:201 `archive_chat`、:280 `unarchive_chat`、:308 `preview_delete`、:357 `delete_chat`／S/host/mod.rs:1356 `set_pinned`／S/host/continuity.rs:34 `plan_purpose_edit`、:69 `assemble_preview`、:165 `apply_chain_resolution`／S/host/continuity_handoff.rs:290 `handoff_and_send`、:475 `write_ledger_file`、:123 `attachment_blocker`／S/host/purpose_draft.rs:285 `start_purpose_draft`／S/store/layout.rs:12-30（配置）、:32（索引ファイルを持たず `chat.json` 走査） |
| 動作確認 | 未実行（該当テストを個別に走らせていない）。`lifecycle::tests::force_kill_without_confirmation_leaves_a_record_that_blocks_deletion` は合格。 |
| 不足・制約 | chat.json を失った開発チャットと段階①の Roaming 領域の会話は外部扱い（CLAUDE.md の合意。コードでの確認は未実施）。台帳は 1 チャット内の原文往復であり、チャット横断の検索・要約は未実装（未確認：検索機能の有無は調べていない）。重要度 推奨／難易度 中。 |

## A15 中断・復旧

| 項目 | 内容 |
|---|---|
| 判定 | 一部実装（通常チャットの受理不明・wake 復旧・強制終了は実装済み。AgentDock 自体が落ちた後の通常チャット実行の自動再開は意図的に無し） |
| 概要 | 独自実装。(1) 受理不明: timeout・切断は `OutcomeUnknown`/`AcceptanceUnknown` とし再送しない。`reconcile_once` は履歴の読取りだけで照合（S/host/queue_driver.rs:989）。未解決送信は `queue.json` に永続化し、再起動後に `restore_unresolved` で復元（:932）。(2) wake: `on_resume` が鮮度を needsReconcile にして履歴を読取り照合、live 通知を受けたものだけ live に戻す。自動 resume なし（S/host/lifecycle.rs:591）。(3) 強制終了: App Server を Job Object に割り当て（`KILL_ON_JOB_CLOSE` は付けない＝AgentDock 異常終了で作業を無断停止しない）、確認付きで `TerminateJobObject`、終了後も `active_processes()==0` を確認するまで停止未確認（S/win/job.rs 冒頭, S/host/lifecycle.rs:488 `preview_force_kill`、:500 `force_kill`）。(4) 組織実行の再起動後: `recover_on_restart` が running/reviewing かつ Confirmed のタスクを `NeedsReconcile(Restart)` に再分類、SendUnknown・InterruptRequested・StopUnconfirmed は維持、cancelled/failed に変換しない、`Halted{Restart}` で送信を止め利用者の再開操作まで待つ。計画承認待ちは上書きしない。(5) 二重実行防止: `Dispatched` を送信の前に永続化（`send_is_always_preceded_by_dispatched_persist`）、受理不明・未決の試行があるタスクにはディスパッチしない（E/core.rs:1489 のコメント）、送信権は epoch 失効（一時停止・取消・終了・再起動・wake で revoke_all）。多重起動は `tauri_plugin_single_instance`（S/lib.rs:73）。 |
| 根拠 | E/persist.rs:824 `classify_after_restart`、:871 `recover_on_restart`／E/core.rs:1489-1520／S/host/org/grant.rs:50 `revoke_all`、:120 以降の `is_released` ガード／S/host/lifecycle.rs:591 `on_resume`（`org_note_stale`、`org_request_reconcile` を含む）／S/host/org/driver.rs:376 `reconcile_read_only`／S/win/job.rs／S/host/stop.rs:13 `derive_summary`（停止確認は期限と証拠から導出） |
| 動作確認 | 上記コマンドで合格：`restart_makes_running_needs_reconcile_and_halts`、`restart_keeps_unknown_and_unconfirmed_and_never_cancels`、`corrupt_snapshot_falls_back_to_full_replay`、`restore_ignores_broken_tail_and_reports_it`、`force_kill_without_confirmation_leaves_a_record_that_blocks_deletion`、`turn_end_confirms_and_late_end_after_deadline_still_confirms`、`failed_replace_keeps_old_content`、`append_after_a_torn_last_line_starts_on_a_new_line`。Job Object の実プロセス終了と wake は未確認（実機）。 |
| 不足・制約 | 「再開位置」は、Codex の thread 履歴を正とし AgentDock 側にチェックポイントは持たない（組織実行はタスク単位でイベントから復元）。通常チャットは AgentDock 再起動後に作業が自動で続くことはない（resume は利用者操作）。管理外（AgentDock が起動していない）プロセスは強制終了の対象外。通常チャットの `Dispatched` 相当の送信前記録は `record_presend`（queue_driver.rs:894）が担うが、その二重送信抑止の個別テストは今回走らせていない（未確認）。重要度 推奨／難易度 中。 |

## A16 実行ログ・監査

| 項目 | 内容 |
|---|---|
| 判定 | 一部実装（記録は豊富だが、改ざん耐性・横断検索・保持方針の仕組みは無い） |
| 概要 | 独自実装。保存先は全て `%LOCALAPPDATA%\com.agentdock.app\` 配下。`chats\<dirId>\activity.jsonl`（状態変化・turn 終端・完了 item の短い本文・鮮度変化。delta は保存しない）、`decisions.jsonl`（承認・質問・MCP 許可の応答記録。要求元エージェント・要約・結果・要求時刻・応答時刻）、`baselines\segments.jsonl`（変更の控え・戻し）、`queue.json`、`compactions\`、`side\`、`会話台帳.md`。ルートに `extension-ops.jsonl`（拡張管理操作。認証情報・起動コマンドは含めない）。組織実行は `org\...\events.jsonl`（状態遷移イベント、seq つき）、`workflow.json`（原子的スナップショット）、`usage.jsonl`、`imports.jsonl`。書込みは `append_line`（1 回の書込み＋flush＋sync_data、前回の途中切れ行には改行補填）と `write_atomic`（一時ファイル→sync_all→rename、共有違反は再試行）。読み取りは壊れた行・新しい版の行を飛ばす。決定・活動は「自動では削除しない」。 |
| 根拠 | S/store/layout.rs:9-30／S/store/atomic.rs:22 `write_atomic`、:60 `append_line`／S/store/mod.rs:381 `read_activity`、:392 `append_decision`、:418 `append_extension_op`、:436 `append_baseline`／S/store/records.rs:302 `ActivityLine`／S/backend/decision.rs:50 `DecisionRecord`／S/store/org.rs:25-32, :104 `append_event`／E/persist.rs（`parse_events`、`restore`） |
| 動作確認 | `store::atomic` 6 件、`persist` 10 件合格（途中切れ・破損の復元）。 |
| 不足・制約 | (a) ハッシュ連鎖・署名・追記専用 ACL なし：ファイルは平文 JSONL で、同一ユーザーなら編集可能（改ざん検知は無い）。重要度 推奨（監査用途なら必須）／難易度 中。(b) 保持期限・ローテーションなし（自動削除しない方針。容量不足でも削除しない）。(c) ツール呼び出しの全出力は保存しない（完了 item の短い本文のみ）。依頼・計画の本文は Codex 側 thread 履歴＋組織実行のイベントで追えるが、チャット横断の統合監査ビューは未確認／無いと見られる。(d) 承認の代行記録は無い（承認は常に利用者）。重要度 推奨／難易度 中。 |

## A17 成果物管理

| 項目 | 内容 |
|---|---|
| 判定 | 実装済み（通常チャットの変更の控え・戻す／組織実行の成果物・取り込み・取り戻し。いずれも独自実装） |
| 概要 | 通常チャット: Git 基準の「変更の控え」を専用 git オブジェクト置き場（`baselines\objects`）に turn 区間ごとに保存し、差分表示・「変更を戻す」（確認付き。戻す前の控えを `revert-backup\<ms>\` に保存、`revert_blocker` で戻せない条件を判定）。旧方式（Codex 報告の差分の逆適用）は読まず消さない。添付はコピーを `attachments\<attId>\` に保持し、欠損時は引き継ぎを `attachment_blocker` で止める。組織実行: 成果物パネル（`org_artifacts`）・差分・プレビュー・「開く」対象検証、確認付き取り込み（`org_import`、事前プレビュー、`imports.jsonl` に台帳、`org_import_verify` で照合）、取り込みの取り消し（`org_import_undo`、HEAD 一致を確認）。自動マージはしない。 |
| 根拠 | S/host/baseline.rs, baseline_snap.rs／S/host/changes.rs:1046 `revert_blocker`、:1112 `revert_changes`／S/store/layout.rs:17-22／S/host/org/artifacts.rs:182 `org_artifacts`、:449 `org_artifact_diff`、:465 `org_artifact_preview`、:505 `org_artifact_open_target`、:550 `org_import_preview`、:649 `org_import`、:714 `org_import_verify`、:747 `org_import_undo`／S/gitops/import.rs:96 `prepare`、:273 `execute`、:343 `undo_execute`／S/host/attachments.rs |
| 動作確認 | 該当 module のテストは今回未実行（`host::changes`、`host::baseline`、`gitops::import` に多数のテストがあることを確認したが走らせていない）。 |
| 不足・制約 | 「根拠」（成果物と調査根拠・出典の紐付け）を保持する専用構造は未確認。成果物は Git 差分・ファイル単位で、業務文書の版管理は共有サーバー連携（A18）に依存。観測できない変更（Git 管理外等）は戻さず案内する設計。重要度 推奨／難易度 中。 |

## A20 業務ごとの再利用性

| 項目 | 内容 |
|---|---|
| 判定 | 一部実装 |
| 概要 | 独自実装（組織実行）：役割定義ファイル `org\roles\<id>.json`（標準 Planner/Worker/Reviewer を `create_new` で一度だけ生成、既存は上書きしない、壊れたファイルは `.corrupt-<ms>` へ退避、権限を広げる定義は利用者の確認まで無効、確認の印は権限指紋に紐付く）、グラフテンプレート `org\graphs`（`standard`、`quality-loop`、定義ファイルで追加可能）、開始時にグラフ・役割の写しをワークフローに保存し、定義変更を検出（`definitions_changed`）。目的別チャット：目的メモ（ConversationRecord）で長期チャットに業務目的を持たせる。Skill は Codex 側の機能で、AgentDock は選択 UI（SkillPicker.tsx）と AGENTS.md 周辺を接続（役割＝実行主体、Skill＝手順の分離は Codex の Skill 機構に依存し、AgentDock が役割定義と Skill を紐付ける仕組みは未確認）。 |
| 根拠 | S/host/org/roles.rs:1-9, :50 `ensure_standard_roles`、:77 `load_roles`、:155 `enable_role`／S/host/org/graphs.rs:59 `ensure_builtin_graphs`、:118 `load_graphs`、:246 `save_copies`、:284 `definitions_changed`／E/graph/templates.rs:1-12（TEMPLATE_STANDARD/QUALITY_LOOP）、E/roledef.rs／S/host/continuity.rs:34／src/ui/SkillPicker.tsx |
| 動作確認 | 未実行（`host/org/graphs/tests.rs` 等は走らせていない）。 |
| 不足・制約 | 業務別（障害調査・問い合わせ対応等）の標準ワークフローテンプレートは同梱されていない（同梱は汎用 2 種のみ）。役割定義に業務別 Skill を束縛する欄の有無は未確認。共有サーバー経由の役割・テンプレート配布は未実装（A18 参照）。重要度 推奨／難易度 中。 |

## A22 UI操作とAI制御の整合

| 項目 | 内容 |
|---|---|
| 判定 | 一部実装（単一の正・送信ロック・手動切替は実装済み。承認の同時二重応答はホスト側の排他が薄い） |
| 概要 | 独自実装。状態の正はホスト（`HostData`）で、UI は `HostEvent` の通知で確定し先取りしない（コメント「UIの状態はこの通知で確定する」）。組織実行のチャットは UI 側で送信欄を無効化（`ORG_SEND_LOCK`）、ホスト側も同判定で拒否（`check_not_org_owned`：キュー・送信・再開など 6 箇所で呼び出し）、取り込み中（`org_chat_adopting`）も通常送信を断る。「手動に切り替え」は確認付きの片方向操作で、以後 `GrantRegistry` がそのチャットに許可を出さない（`ChatReleased`）。送信権は `OrgGrant`（Clone/Serialize なし、利用者操作で作成）と epoch 失効。承認・質問は `respond_request` が Pending 以外を `RequestAlreadyResolved` で拒否し、結果を Answered／ResolvedElsewhere／AnswerUnconfirmed／Unanswerable に区別して状態に反映（未確認を成功表示しない）。エンジンは Codex の承認要求に応答しない。 |
| 根拠 | S/host/mod.rs:920 `check_not_org_owned`、:1211-1240 `respond_request`／S/host/org/grant.rs:1-9, :31 `GrantError`、:50 `revoke_all`、:120 以降／S/host/org/mod.rs:490 `org_release_chat`、:521 `org_chat_adopting`／S/codex/rpc.rs:183（未知または回答済みサーバー要求は Protocol エラー）／S/codex/adapter.rs:1076 `AlreadyResolved`／src/ui/OrgPane.tsx:101 `ORG_SEND_LOCK`、:427, :482（手動に切り替え確認）／src/ui/Dock.tsx |
| 動作確認 | `host::org::grant::tests` 5 件合格（`a_released_chat_never_gets_a_permit`、`a_newer_grant_revokes_the_older_one`、`permit_is_issued_only_while_the_grant_is_current` ほか）。UI ロック表示は未確認。 |
| 不足・制約 | `respond_request` は Pending 確認と `backend.respond` の間にロックを保持しないため、2 つの窓や 2 回の素早いクリックでの同時応答では、後者が rpc 層で「unknown or already answered server request」エラーになる経路に依存する（同時実行の専用テストは確認できず、未確認）。複数窓・コンパクト監視窓間の状態は同一ホストのイベントなので不一致は起こりにくいが、検証テストは未確認。重要度 推奨／難易度 小。 |

## 構成図用の事実

- 実行主体: 単一の Tauri ホスト（Rust）が `codex app-server`（stdio JSON-RPC）を子プロセスとして起動。Job Object に割り当て（KILL_ON_JOB_CLOSE なし）。UI（React）はホストの IPC とイベント通知（`HostEvent`）だけで動き、状態の正はホストの `HostData`。
- AI バックエンド境界: S/backend の trait とモデルがあり、Codex 固有はS/codex のアダプターに閉じる。
- 保存ルート: `%LOCALAPPDATA%\com.agentdock.app\`。`settings.json`、`windows.json`、`worktrees.json`、`cloud-tasks.json`、`extension-ops.jsonl`、`diag\`、`worktrees\`、`EBWebView\`、`org\roles\`、`org\role-confirmations.json`、`org\graphs\`、ワークフロー別ディレクトリ（`workflow.json`・`events.jsonl`・`usage.jsonl`・`imports.jsonl`・`graph.json`・`roles.json`）。
- チャット別 `chats\<dirId>\`: `chat.json`、`queue.json`、`activity.jsonl`、`decisions.jsonl`、`changes.jsonl`（旧、読まない）、`baselines\{objects,segments.jsonl,tmp}`、`revert-backup\<ms>\`、`compactions\`、`side\`、`attachments\<attId>\`、`会話台帳.md`、`workspace\`。
- 書込み方式: 全体は一時ファイル→`sync_all`→`rename`（`write_atomic`）、ログは 1 行ずつ追記＋`sync_data`（`append_line`）。空き容量確認あり、不足時も自動削除しない。
- データの流れ（通常チャット）: UI→IPC→Host→Backend(Codex) 送信。受理不明は `queue.json` に記録して照合（読取りのみ）。Codex 通知→正規化→`HostData`→`HostEvent`→UI、同時に `activity.jsonl`／`decisions.jsonl` へ追記。
- データの流れ（組織実行）: 利用者の開始／再開（`UserConfirmed`）→`OrgGrant`→`OrgDriver` が `OrgCore`（org-engine：DAG・スケジューラ・回数制限）を所有→`Dispatched` 永続化→ポート経由で Codex へ送信→観測をイベント化→`events.jsonl`＋スナップショット。再起動時は `recover_on_restart`（要照合化＋停止）。
- 共有サーバー・LangGraph 相当はここでは未確認。AI 実行は全てローカルのホストで完結。
