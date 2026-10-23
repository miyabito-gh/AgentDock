# 調査 part-3（A08〜A12）2026-10-23 / feature/org-engine

判定語彙: 実装済み／一部実装／未実装／確認不能／外部Codex機能で対応。種別: UIだけ／バックエンドだけ／設計のみ／Codex側に存在するが未接続。
行番号は調査時点のもの。「未確認」は読んでいない・実機で見ていない点で、推測で埋めていない。

## 判定一覧

| ID | 判定 |
|---|---|
| A08 Skills連携 | 一部実装（読取り・明示指定は実装済み。共有・更新・組織実行での指定は未実装） |
| A09 MCP連携 | 一部実装（読取り・OAuth・再読込み・CLI補助の追加削除は実装済み。ツール呼出しはCodex任せ。組織実行の役割へのMCP指定は未実装） |
| A10 ツール操作・権限制御 | 一部実装（権限プリセット・承認カード・計画承認は実装済み。読取／書込／削除／外部送信の独自区分は計画の宣言ベース） |
| A11 独立レビュー・テスト | 一部実装（Reviewer分離・pass/rework・差し戻しは実装済み。テスト実行専用役は未実装） |
| A12 再試行・ループ制御 | 実装済み（上限・停止・通知。トークン量や時間での上限は未実装） |

## A08 Skills連携

| 項目 | 内容 |
|---|---|
| 判定 | 一部実装 |
| 実装の概要 | 発見・一覧は Codex App Server の `skills/list` を読むだけ（Codex機能を呼ぶ）。AgentDock独自は「一覧から選んで入力欄のチップにし、送信時に `skill` 入力として明示呼出し」する部分と、定義ファイル冒頭のプレビュー。Skillの有効／無効切替、作成、共有、更新、同期は作っていない（`~/.codex` 配下は読むだけ）。AGENTS.mdは start/resume/fork の応答が示した指示ファイルのパスを表示するだけ（読むためにresumeしない）。組織実行（org-engine）にはSkill指定の仕組みがない。Codex自身の暗黙のSkill適用が役割スレッドで起きるかは未確認。 |
| 根拠 | 一覧: `app/src-tauri/src/codex/parity.rs:184` `list_skills`（`skills/list`、`forceReload` は明示操作時のみ）、`:510` `skill_list_of`（読めない項目は errors へ、欠けた項目は未取得）。指示ファイル: `parity.rs:189` `instruction_sources`（未示なら `Known::NotFetched`）。指定: `app/src-tauri/src/host/compose.rs:86` `add_skill_attachment`（コピーせず定義ファイルの実在確認）、`:69` `read_skill_preview`（一覧に含まれるパスだけ読む）。送信: `app/src-tauri/src/codex/adapter.rs:132-136` `build_turn_input`（`{"type":"skill","name","path"}`。名前が無ければ拒否）。能力表: `parity_table.rs:30-31`（`ParityOp::Skills`・`InstructionFiles`）。画面: `app/src/ui/SkillPicker.tsx`、`skillPick.ts`。組織実行側は `app/org-engine/src` に skill の記述なし（Grep 0件）。 |
| 動作確認 | `cd app/src-tauri; cargo test --lib -- codex::parity codex::events codex::parity_table` → 43件 ok（`skill_list_keeps_unreadable_entries_as_errors_and_never_invents_fields` を含む）。adapter の `skill_attachment_becomes_a_skill_input_without_a_copy_and_needs_a_name`（adapter.rs:1253）は上の絞込みには含めていない（この項目の単体実行は未実施）。実機でのApp Server接続は禁止指示のため未実施。 |
| 不足・制約 | (1) 共有Skillの同期・更新（共通サーバー→各PC）: 未実装。重要度 推奨、難易度 中（Git等で `~/.codex/skills` 相当へ配る運用か、アプリ内機能かは未決）。 (2) 組織実行の役割やタスクへのSkill指定: 未実装。推奨、中。 (3) Skill有効／無効の切替・作成: 意図的に未実装（読取りのみの合意）。将来、小〜中。 (4) 「業務別Skill」を選ぶ手段は、チャットごとの手動選択のみ。 |

## A09 MCP連携

| 項目 | 内容 |
|---|---|
| 判定 | 一部実装 |
| 実装の概要 | 区分は次のとおり。(a) App Server読取り（Codex機能の呼出し）: `mcpServerStatus/list`（名前・接続状態・認証状態・ツール数）、`plugin/installed`（Plugins一覧）。(b) App Serverの操作: `mcpServer/oauth/login`（認可URLを返し、開くのはUIの明示クリック）、`config/mcpServer/reload`（受付のみ。既存会話への反映は確認できないと明示）。(c) CLI補助（独自実装）: `codex mcp add`（stdioのみ）／`mcp remove`／`plugin add`／`plugin remove` を、確認済み操作でだけ実行し、前後の config.toml を読み直して照合（`judge_config`）。自動rollbackなし、CLIが終わらない・異常終了は `ResultUnconfirmed`。(d) ツール呼出し・MCP承認（elicitation）はCodex本体が行い、AgentDockは `mcpServer/elicitation/request` を承認カードとして受けて応答するだけ（form入力ありの要求は選択肢に出さない）。ツール一覧は件数のみで、個々のツール名・スキーマは取得しない。組織実行の役割（standard Planner/Worker/Reviewer）にMCPを指定する仕組みはない（org-engine に mcp の記述なし）。また「sandbox競合回避」オプション有効時は、その起動に限り `features.plugins=false` と `mcp_servers.node_repl.enabled=false` を渡す（`~/.codex` は変更しない）。 |
| 根拠 | `app/src-tauri/src/codex/parity.rs:198` `list_tool_servers`、`:220` `login_tool_server`、`:228` `reload_tool_servers`、`:232` `list_extensions`、`:238` `manage_extension`（前後照合）、`:801` `tool_server_from_wire`、`:920` `judge_config`、`:864-866`（`McpAddStdio`/`McpRemove` の `ExpectedScope`）。CLI種別: `app/src-tauri/src/codex/cli.rs:20-30`（`CliCommand`）。通知: `codex/events.rs:298,303`（`mcpServer/startupStatus/updated`・`oauthLogin/completed`）。承認要求: `events.rs:520-528,641-648`。起動引数: `codex/process.rs:52-54` `sandbox_workaround_args`。画面: `app/src/ui/ExtensionsDialogs.tsx:70` `ExtensionsBody`。能力表 `parity_table.rs:32`（ToolServers）。 |
| 動作確認 | `cargo test --lib -- codex::parity ...` で `tool_server_and_extension_conversion_keep_unknowns_unknown`、`plugin_scope_covers_name_at_marketplace`、`judge_config` 系を含め 43件 ok（上記A08と同じ実行）。実際のMCP接続・OAuthは未実施（禁止指示）。 |
| 不足・制約 | (1) 組織実行の役割へのMCP指定・制限: 未実装。推奨、中。MCPはチャット／Codex全体設定（config.toml）で効くため、役割別に絞る手段は現状ない（未確認: 役割スレッドでCodexが全設定のMCPを使えるか）。 (2) MCPの個別ツール一覧・スキーマ表示: 未実装（件数のみ）。将来、小。 (3) 追加はstdioのみ（HTTP/SSE型は未対応）。推奨、小〜中。 (4) 設定変更後の既存会話への反映確認なし（仕様上明示）。 (5) 読取／書込／外部送信の区分でMCPツールを制御する独自層はなく、Codexの承認に依存。必須（A10と共通）、中。 |

## A10 ツール操作・権限制御

| 項目 | 内容 |
|---|---|
| 判定 | 一部実装 |
| 実装の概要 | チャット単位の権限は `PermissionPreset` 6種（WorkspaceWriteOnRequest／ReadOnly／WorkspaceWriteUntrusted／ReadOnlyUntrusted／ReadOnlyNoPrompt／FullAccess）を `(approvalPolicy, sandbox)` に写して `thread/start` 等へ渡す（Codex機能に権限判断を委ねる）。既定は WorkspaceWriteOnRequest。FullAccess は利用者選択のみで無断で使わない。承認要求は5種（コマンド実行・ファイル変更・追加権限・ユーザー質問・MCP elicitation）を `PendingRequest` に変換して画面カードで利用者が選ぶ。未対応の要求は自動応答せず「拒否」だけ選べる。選択肢に無いIDは送らず拒否。承認の代行はしない（組織実行は承認要求に応答しない。ipc.rs の冒頭に明記）。「読取／書込／削除／外部送信」の独自分類は、組織実行の計画で Planner が宣言する `Effect`（userTreeWrite／pushMergeTag／outsideWorkdir／networkOrInstall／secretsOrAuth／deletionPrimary）にあり、いずれか宣言があれば計画承認が必須（宣言が無ければ自動承認）。宣言はAIの自己申告で、実際のツール操作との突合せはない。プッシュ（`git_push.rs`）・取り込み・変更を戻す等は確認画面＋`UserConfirmed`（コマンド層だけが作る）を要する。組織実行の役割: Planner＝ReadOnly共有、Worker＝workspace-write＋on-request＋専用worktree、Reviewer＝ReadOnly＋コミット固定のdetached worktree。役割定義が既定より広い（書込み許可・追加フォルダ・ネットワーク・作業場所）と無効化し、権限指紋に結び付いた利用者確認で有効化。`approval: never` は定義の読込みで拒否。 |
| 根拠 | `app/src-tauri/src/backend/backend.rs:180-188` `PermissionPreset`、`codex/convert.rs:707-714` `permission_params`、`:720-726` `sandbox_policy_json`、`codex/adapter.rs:164,798`（approvalPolicy/sandbox送信）、`host/mod.rs:697`（既定）。承認: `codex/events.rs:505-529`（変換）、`:584-673` `build_reply`/`build_response`、`:530-535`（未対応要求）。組織: `app/org-engine/src/policy.rs:50` `evaluate_plan`、`roledef.rs:147` `compare_breadth`、`:128` `baseline`、`:174` `permission_fingerprint`、`:238`（never拒否）、`host/org/ports.rs:275` `role_permission`、`:307`。読取り専用の共有: `rules/org_worktree.rs:82,106`（Detached worktree へのコミット拒否）。ipc不変条件: `host/org/ipc.rs:3-8`。追加の用途: `host/purpose_draft.rs:382`（目的メモ下書きは ReadOnlyNoPrompt の一時スレッド）。UI: `app/src/ui/RequestGap.tsx`、`CenterPane.tsx`、`OrgPane.tsx`、`OwnerAllowPanel.tsx`。 |
| 動作確認 | `cargo test --lib -- host::org::ports rules::thread_ops host::org::stall` → 21件 ok（`the_approval_policy_of_the_spec_makes_the_chat_permission` を含む）。`cd app/org-engine; cargo test --lib` → 198件 ok（policy の `each_effect_requires_confirmation`、roledef の広がり判定を含む）。`codex::events` のテストは上記A08実行に含まれ ok。 |
| 不足・制約 | (1) 読取／書込／削除／外部送信を実際のツール呼出しから判定する独自の層はない（Codexのsandbox・承認に依存。Effectは自己申告）。必須ではなく推奨、大。 (2) 組織実行ではWorkerが承認待ちで止まった場合、承認は利用者が役割チャットのカードで行う設計（AgentDockは代行しない）。通知・状態表示（AwaitingApproval）はあるが、常時承認待ちが滞留した場合の自動処理はない（仕様）。 (3) ReadOnlyNoPrompt は組織役割には使われていない（目的メモ下書きのみ）。 (4) 実機でのsandbox挙動（Windowsでの競合など）は未確認。 |

## A11 独立レビュー・テスト

| 項目 | 内容 |
|---|---|
| 判定 | 一部実装 |
| 実装の概要 | 組織実行（独自実装）: Reviewer は Worker とは別のスレッド・別役割で、Workerのコミットに固定したdetached worktree（読取り専用）で動く。出力は JSONスキーマ強制の判定 `{verdict: pass|rework, findings[]}`。pass で対象Workerタスクと自身が完了。rework で findings を記録し、差し戻し回数を数えて対象を Ready に戻す（対象タスクごとの上限 `max_rework`＝2）。上限到達は失敗にせず `ReworkLimit` で停止し、利用者が「もう一度直させる／このまま受け入れる／失敗として確定」を選ぶ。findings は event ログ（`FindingsRecorded`）に残り、再起動で失われない。グラフ実行（graph）には `verdict3`（pass/rework/replan＋score）のスキーマもある。組織外: チャット単位の `/review` 相当を Codex の `review/start` で起動（Codex機能。対象は未コミット変更／ベースブランチ／コミット／カスタム。`delivery: inline` 固定で、組織管理下のチャットでは拒否）。テスト実行専用の役割（Tester/Verifier）や、テスト結果を判定に使う仕組みは標準役割になく未実装。Reviewerは read-only のため書込みはできないが、コマンド実行（テスト）が可能かは未確認。 |
| 根拠 | `app/org-engine/src/schemas.rs:74-80` `reviewer_verdict_schema`、`:103-111` `VerdictKind`/`ReviewerVerdict`、`:183` `parse_verdict`（rework で findings 空は拒否）。`planned.rs:148,182-193`、`:199-243` `complete_verdict`（pass／rework／ReworkLimit）、`:253-256` `dep_ok`（Reviewerはコミット確認済みの対象を待つ）。`limits.rs:62-75` `rework_limit_reached`/`try_rework`。`outschema.rs:280` `verdict3`。standard 役割: `roledef.rs:128-144` `baseline`/`standard_workplace`（Reviewer＝DetachedAtCommit）。組織外: `codex/parity.rs:149` `start_review`、`:477-489` `review_target_json`/`review_params_json`、`host/thread_ops.rs:205-206` `start_review`（`check_not_org_owned`）、`backend/parity.rs:46`。 |
| 動作確認 | org-engine `cargo test --lib` 198件 ok（`report_and_verdict_validation`、`rework_twice_then_limit` を含む）。codex側 `review_params_are_always_inline_and_never_detached` ok。実際のReviewer実行は未実施。 |
| 不足・制約 | (1) テスト実行役（Tester/Verifier）と合否ゲート（テスト通過を完了条件にする）: 未実装。必須（品質ゲートの要件次第）、中。 (2) 独立性: ReviewerとWorkerは別スレッドだが同一モデル設定が既定（役割ごとの `model`/`effort` は定義可）。 (3) レビュー所見の保存は workflow の event／outputs ログのみ。組織外の `/review` 結果はチャット履歴の中で、構造化保存はしていない。 (4) 業務別の必須品質ゲートは未決定事項。 |

## A12 再試行・ループ制御

| 項目 | 内容 |
|---|---|
| 判定 | 実装済み（組織実行内）。チャット単体にはループ制御なし（利用者操作のみ） |
| 実装の概要 | `Limits`（既定）: 同時Worker 2／全体3、差し戻し 2回／タスク、自動再計画 1回／ワークフロー、turn 30、タスク 20、再送 `max_retries`（既定値は `DEFAULT_MAX_RETRIES`）。turn は送信前に数え（Plan／Task／Review／FormatFix／Rework／Replan／Retry を全て1と数える）、上限到達時は送らない。turn上限は引き上げて再開できる。自動再計画は上限超過で利用者確認。ReworkLimit は利用者選択。失敗の再送は利用者操作のみで、送信または停止結果が未確認なら拒否（受理不明の自動再送なし）。利用上限（`usageLimitExceeded`）での失敗は、そのタスクを `Blocked(UsageLimit)` にしてワークフロー全体を `HaltReason::UsageLimit` で止め、通知する。cancelled／failed には変換せず、利用者の「再開」でのみ再開。停滞（出力が一定時間無い running タスク）は通知のみで、自動中断はしない（既定時間は設定、1〜1440分）。グラフ実行には周回上限 `max_rounds`（辺ごと、上限あり）と `RoundLimit` 停止がある。 |
| 根拠 | `app/org-engine/src/limits.rs:6-24` `TurnKind`/`LimitError`、`:38` `spend_turn`、`:47` `try_auto_replan`、`:56` `raised_turn_limit`、`:62` `rework_limit_reached`。`types.rs:397-450` `Limits`（max_writers 2、max_total 3、max_rework 2、max_auto_replans 1、max_turns 30、max_tasks 20、max_retries）。`core.rs:261` `retryable`、`:313` `retry_kind`（`RetryRefusal`）、`:1109-1114`（利用上限でhalt＋`OrgNotice::UsageLimit`）。`codex/convert.rs:594-601`（`usageLimitExceeded` → `FailureClass::UsageLimit`）。停滞: `host/org/stall.rs:65` `StallWatch::check`、`ports.rs:141`、`host/org/notify.rs:56`、`ipc.rs:481` `org_set_stall_minutes`、`view.rs:960-961`。グラフ: `graph/validate.rs:53,286,295`。 |
| 動作確認 | `cd app/org-engine; cargo test --lib` → 198 passed, 0 failed（`default_turn_limit_is_30`、`turn_limit_counts_all_kinds_and_stops`、`auto_replan_once`、`rework_twice_then_limit`、`failures_usage_limit_and_interrupts_are_not_cancellations`、`a_usage_limit_stop_is_resumed_by_the_user_...` を含む）。停滞の単体テストは `host::org::stall` を絞込みに含めて実行した範囲で ok（個別の成否名は未確認）。 |
| 不足・制約 | (1) トークン量・コスト・実時間による上限はない（turn数と回数のみ）。推奨、中。 (2) 利用上限停止からの自動復帰（リセット時刻での再開）はない（利用者操作のみ。合意事項に沿う）。 (3) 停滞は通知のみで自動中断しない（合意）。 (4) 並列数・turn上限の値は未決定事項（現行値は仮の既定）。 (5) チャット単体（組織外）のループ防止は無く、利用者が中断する前提。 |

## 構成図用の事実

- 権限・承認・Skills・MCP の実行主体はすべて Codex（App Server／CLI）。AgentDock は呼出し・表示・確認・照合の層で、Codex固有の型は `backend` 境界（`AiBackend`／`ParityOps`）の中立型に変換してUIへ出す（`Known<T>` で未取得を明示）。
- App Server 呼出し（読取り）: `skills/list`、`mcpServerStatus/list`、`plugin/installed`、`thread/goal/get`、`collaborationMode/list`。操作: `mcpServer/oauth/login`、`config/mcpServer/reload`、`review/start`、`thread/compact/start`、`thread/goal/set|clear`。
- CLI補助（独自）: `codex mcp add|remove`、`codex plugin add|remove`、`codex cloud exec|list|status|diff|apply`。実行前後に config.toml を読んで照合。確認済み（`UserConfirmed`）の後のみ、アプリ内で直列化。
- サーバー要求の受信: `item/commandExecution/requestApproval`、`item/fileChange/requestApproval`、`item/permissions/requestApproval`、`item/tool/requestUserInput`、`mcpServer/elicitation/request`。これ以外は拒否のみ選択可。
- 権限プリセット→Codex引数: WorkspaceWriteOnRequest=(on-request, workspace-write)、ReadOnly=(on-request, read-only)、WorkspaceWriteUntrusted=(untrusted, workspace-write)、ReadOnlyUntrusted=(untrusted, read-only)、ReadOnlyNoPrompt=(never, read-only)、FullAccess=(never, danger-full-access)。
- 組織実行（`app/org-engine` crate＋`host/org`）: 標準役割 Planner（共有・読取り）／Worker（専用worktree・書込み・on-request）／Reviewer（コミット固定detached・読取り）。役割は定義ファイルで拡張可（既定より広い権限は無効化し確認で有効化。approval never は拒否）。
- 組織実行は Codex の承認要求に応答しない。承認は利用者が役割チャットのカードで行う。計画承認は独自（宣言された Effect があれば必須）。
- 判定は JSONスキーマ強制の出力（reviewerVerdict／verdict3）。上限は turn 30／差し戻し 2／自動再計画 1／再送 `max_retries`／グラフ周回。
- 利用上限停止はワークフロー全体の halt（cancelled／failed に変換しない）。停滞は通知のみ。
- Skills は組織実行と未接続（チャットの添付としてのみ指定可）。MCPも組織役割とは未接続。ともにCodexのグローバル設定に依存。
- 確認コマンド結果: org-engine `cargo test --lib` 198件 ok、`src-tauri` `cargo test --lib -- codex::convert::tests::permission codex::parity codex::events`（実行時の出力は末尾の43件ok）、`-- host::org::ports rules::thread_ops host::org::stall` 21件 ok。ソース・設定の変更なし。
