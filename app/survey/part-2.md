# 調査 part-2（A03〜A07）

調査日 2026-10-23 / ブランチ feature/org-engine / 最新コミット 0e1b11e。判定はコードとテストによる。ソース・設定は変更していない。
パスは `app/` 起点（例 `org-engine/src/…` = `app/org-engine/src/…`）。

## 実行した確認コマンド（変更を伴わない）

- `cd app/org-engine && cargo test --lib` → 198 passed / 0 failed
- `cd app/org-engine && cargo test --lib -- scheduler policy::tests limits graph::templates graphrun` → 52 passed（scheduler・policy・limits・graphrun の該当テスト）
- `cd app/src-tauri && cargo test --lib -- codex::tree codex::convert::tests host::org::graphs` → 39 passed
- 制約: 単体テスト（純粋ロジック・疑似ポート）のみ。実際のApp Server・実Codexでの並列実行は未実行（未確認）。

## 判定一覧

| ID | 判定 |
|---|---|
| A03 | 一部実装 |
| A04 | 一部実装 |
| A05 | 一部実装 |
| A06 | 実装済み（並列数・依存・worktree分離。リソース競合の自動検出は対象外） |
| A07 | 一部実装 |

## A03 メインAIによる計画・タスク分解

| 項目 | 内容 |
|---|---|
| 判定 | 一部実装 |
| 実装の概要 | **独自実装（組織実行）**。Planner役の別チャット（read-only）に outputSchema（plan）で依頼を渡し、`PlanTask`（id・role・deps・effects・title・instructions）の表を受け取る。形式検証（空・重複・循環・未知役割・依存欠落・タスク数上限）→ ポリシー判定（宣言された effect・無効役割・並列要求超過・turn不足のいずれかがあれば利用者承認、無ければ自動承認）。利用者は計画を差し戻して再計画できる（理由メモ付き。未着手のときのみ）。quality-loop では検証役の `replan` 判定が plan ノードへ戻るフィードバック辺（最大1回）として動く。**Codex自身の計画（plan collaborationMode）は別機能**で、AgentDockはモード指定のみ接続（`codex/adapter.rs:180`、`codex/parity.rs:573 collaboration_mode_json`）。 |
| 根拠 | Planner投入: `org-engine/src/prompt.rs:27 planner_prompt`、`:42 replan_prompt`。出力解析・判定: `org-engine/src/planned.rs:121 parse_output`（Planner分岐）、`:153 complete_output`（Plan → TaskAdded、`Halted{AwaitingPlanApproval}`）。形式検証: `org-engine/src/dag.rs:18 validate_plan`。承認ポリシー: `org-engine/src/policy.rs:50 evaluate_plan`（`ConfirmReason` 6種）。ユーザー再計画: `org-engine/src/planned.rs:72 user_replan`（`core.rs:173 OrgInput::UserReplan`）。回数上限の定義: `org-engine/src/types.rs:418 Limits::default`（max_tasks以外に max_auto_replans=1, max_turns=30, max_rework=2）、`org-engine/src/limits.rs:47 try_auto_replan`。グラフでの再計画: `org-engine/src/graph/templates.rs:30 QUALITY_LOOP_JSON`（verify→plan, when verdict=replan, maxRounds 1）、`org-engine/src/outschema.rs:283`（verdict3 = pass/rework/replan）。 |
| 動作確認 | `cargo test --lib`（org-engine）で `policy::tests::{clean_plan_auto_approved, each_effect_requires_confirmation, replan_conditions, parallelism_and_turn_limits, task_count_over_limit_is_format_error}`、`limits::tests::auto_replan_once`、`dag` の循環・欠落・重複検出テストが通過。 |
| 不足・制約 | (1) **AIによる自動再計画（失敗・結果を見ての再分解）は実質未配線**。`OrgEventKind::AutoReplanSpent`（`persist.rs:283,412`）は復元時に数えるだけで、発行箇所は非テストコードに見当たらない（`rg AutoReplanSpent` は persist.rs・org_timeline.rs:274 のみ）。`policy.rs` の Replan 判定入力も `planned.rs:134` で `scope_changed: true` 固定（利用者再計画のみ）。standard テンプレートでは再計画は利用者操作のみ。重要度 必須／難易度 中。(2) quality-loop の replan は検証役の判定による自動周回（上限1回）で、これが唯一の自動再計画。(3) 計画の粒度・最大タスク数は `Limits.max_tasks`（既定値は `types.rs` を要確認＝未確認）。(4) 実機でPlannerがスキーマ通りの計画を返す成功率は未確認。推奨／難易度 小。 |

## A04 サブエージェント起動・制御

| 項目 | 内容 |
|---|---|
| 判定 | 一部実装（AgentDock独自の役割チャット方式＝実装済み。Codex自身の子・孫エージェントは「監視のみ」で起動・制御は未接続） |
| 実装の概要 | **方式1（独自）**: 組織実行がタスクごとに役割チャット（別Codex thread）を `thread/start` 相当で作り、`turn/start` で依頼を送る。1ノード＝1タスク＝1チャット。役割ごとの sandbox・承認方針・model・effort を定義ファイルから反映。完了は outputSchema の最終メッセージを読んで収集。終了・照合は履歴の read のみ（resumeしない）。**方式2（Codex側）**: Codexが自分で spawn_agent 等で作った子・孫は、`thread/list` の `subAgent*` ソース種別で走査し、`source.subAgent.thread_spawn.parent_thread_id` で明示親だけを結んでツリー化。ニックネーム・役割・起動時のmodel/effort・担当（`collabAgentToolCall`/`subAgentActivity` の記録）を表示用に取り出す。AgentDockから子エージェントを起動・停止・指示する経路はコードに存在しない（`spawn_agent`/`spawnAgent` は `codex/convert.rs` の読取りのみ。UI側 `app/src` にも該当なし）。組織実行は子孫の状態を `descendants()` で参照し、未終了の子孫があるタスクを完了扱いにしない。 |
| 根拠 | 役割チャット作成: `src-tauri/src/host/org/ports.rs:170 AgentRuntime::create_role_chat` → `:298 org_create_role_chat`（`StartChatParams` に permission・model を設定、config.toml前後照合）。送信: `ports.rs:174 send`、`:178 steer`。結果収集: `ports.rs:185 read_final_message`、`:206 read_reconcile`。ポート定義: `ports.rs:68`。Codex子孫走査: `codex/adapter.rs:36 SUBAGENT_KINDS`、`:947 scan_descendants`（read専用。冒頭コメント `:5` に「resume・送信・承認をしない」）。ツリー組立て: `codex/tree.rs:68 assemble`（親推定なし、循環・親不明は orphans）。子の担当・起動指定の抽出: `codex/convert.rs:442`（spawnAgentの担当）、`:465`（task_name・model・reasoning_effort・fork_turns）、`:491`（subAgentActivity started）。組織側の子孫参照: `ports.rs:202 descendants` → `:460 org_descendants`。 |
| 動作確認 | `codex::tree::tests`（6件: 入れ子組立て・親不明・遅延親・循環・重複）、`codex::convert::tests`（child_assignment_and_path_come_from_its_own_thread、spawn_spec_is_taken_only_for_one_unambiguous_child_and_never_filled_in、child_model_and_effort_are_recorded_values 等）が通過。`org-engine` の `core::tests` に疑似ポートでのチャット作成・送信フロー。 |
| 不足・制約 | (1) Codex組込みのサブエージェントを AgentDock から起動・停止・指示する機能は無い（Codex側に spawn_agent が存在しても未接続）。Codexが自律的に作った子の存在はプロンプト指示次第で、AgentDockは制御しない設計（監視目的のresume・停止・承認代行をしない方針）。重要度 推奨／難易度 大（方針変更を伴う）。(2) 役割チャットの実Codexでの起動成功率・遅延は未確認（実機確認資料は未参照、判定対象外）。(3) 子孫を持つ役割チャットを持つ組織タスクの「完了後の子孫待ち」の挙動は `rules/org.rs`（`descendants_from_gate`）を要読み＝詳細未確認。 |

## A05 動的エージェント選択

| 項目 | 内容 |
|---|---|
| 判定 | 一部実装 |
| 実装の概要 | **独自実装**。役割は定義ファイル（`<root>\org\roles\*.json`）で拡張でき、標準5役（planner・worker・reviewer・verifier・writer）が初回起動時に作られる（既存は上書きしない）。ファイルの検証・権限の広がり判定（標準より広い権限は無効化し、内容の指紋で利用者確認後に有効）。Plannerは計画の各タスクに role を割り当てるが、**選べるのは役割表にあり有効なもの**。standard グラフでは plannedTasks ノードの `allowedRoles = [worker, reviewer]` が上限。グラフ（固定手順）側では `agent` ノードに役割を固定指定する。「案件に応じて役割を追加・省略」は、Plannerが計画に載せる／載せない形でのみ可能。エンジンが実行できる役割の種類は固定（`planned.rs:35 node_kind` は planner・worker・reviewer のみ識別。それ以外は `parse_output` で「未知の役割です」）。 |
| 根拠 | 役割定義: `org-engine/src/roledef.rs:61 RoleDefinition`、`:187 validate`、`:255 load_role`、`:266 standard_definitions`、`:147 compare_breadth`。ホスト側読込み: `src-tauri/src/host/org/roles.rs:51 ensure_standard_roles`、`:77 load_roles`。許可役割: `org-engine/src/graph/templates.rs:22`（allowedRoles）、`org-engine/src/graph/validate.rs:469-483`（V10）、`graph/def.rs:116 NodeKind::PlannedTasks{allowed_roles}`、`:85 NodeKind`（Agent/Gate/Router/PlannedTasks）。計画時の役割検証: `dag.rs:18 validate_plan`（UnknownRole）。役割識別: `planned.rs:35 node_kind`、`:149`。 |
| 動作確認 | `roledef::tests`（standard_definitions_are_valid_and_within、read_only_role_made_writable_is_widened_and_disabled_with_reasons、validation_rejects_bad_definitions、the_engine_takes_only_a_complete_standard_set 等）、`host::org::graphs::tests::a_definition_that_uses_a_disabled_or_missing_role_is_not_usable` が通過。 |
| 不足・制約 | (1) 標準3役（planner/worker/reviewer）以外の役割は、グラフの `agent` ノード経由なら実行可能（verifier・writer は `graphrun` 経由）だが、standard の plannedTasks（Planner自動計画）は worker/reviewer のみ。Explorer・Researcher・Tester 等の役割をPlannerが動的に選ぶ仕組みは未実装。重要度 推奨／難易度 中。(2) 役割選択の判断（なぜその役割か）は記録されない（計画の role 欄のみ）。(3) Planner のプロンプトが役割を worker／reviewer に固定して記述（`prompt.rs:31`）。役割定義ファイルを増やしても Planner に伝わる経路は未確認。 |

## A06 並列実行・依存関係

| 項目 | 内容 |
|---|---|
| 判定 | 実装済み（独自。リソース競合の自動検出は対象外） |
| 実装の概要 | **独自スケジューラ（純粋関数）**。依存タスクが完了かつ確認済みのとき pending→ready、ready のうち、全体同時数・書込み役同時数・役割別上限・残りturn数・一時停止ゲートの範囲で送信可を決定。既定は Worker同時2・全体3（画面入力は1〜4）。状態不明（未確認）のタスクはスロットを保持し、それに依存するタスクは進めない。レビュー待ちで自分のturnが終わった Worker はスロットを占有しない。競合対策は **worktree分離**: 書込み役（Worker）はタスクごとに専用worktree、Reviewer は対象コミットに固定した detached worktree で read-only。コミットは AgentDock が所有worktree内で行い、マージ・pushはしない。グラフでは書込み役の連鎖で前の書込み役の最新コミットから開始。 |
| 根拠 | `org-engine/src/scheduler.rs:48 promote_ready`、`:65 next_dispatch`、`:71 next_dispatch_with`（total/writers/per_role/turns_remaining の判定）、`:36 occupies`。上限値: `types.rs:418-425 Limits::default`（max_writers=2, max_total=3）、`roledef.rs:18 MAX_CONCURRENT_CAP=4`。画面入力: `src/ui/OrgIntake.tsx:219-220`、表示 `OrgPane.tsx:641`。worktree作成: `planned.rs:297-315`（Worker=Head起点、Reviewer=DetachedAt(commit)）、`graphrun.rs:256,265`、`core.rs:85 WorkspaceAction::CreateWorktree`、ホスト実装 `src-tauri/src/host/worktree/org.rs:22`。所有worktreeの照合: `host/org/ports.rs:114 Workspace`、`:440 org_worktree_finding`。グラフの静的見積り: `graph/validate.rs:20 V11`。依存検証: `dag.rs:18`。 |
| 動作確認 | `scheduler::tests`（promotes_only_when_deps_completed、respects_writer_and_total_limits、never_exceeds_total_even_with_many_ready、role_cap_gate_and_turn_budget、unconfirmed_task_is_skipped_and_holds_a_slot、a_review_waiting_worker_whose_turn_ended_holds_no_slot）、`graphrun::tests::a_writer_starts_from_the_nearest_earlier_writers_newest_commit` が通過。 |
| 不足・制約 | (1) 実Codexでの2〜3並列実行の安定性（レート制限・sandbox競合）は未確認（疑似ポートのテストのみ）。(2) 同一ファイルを触る並列Workerの競合は、worktree分離で衝突を避けるのみで、取込み時のマージ衝突検出・調整は利用者側（自動マージしない設計）。重要度 推奨／難易度 中。(3) 並列度のAIによる動的調整はない（上限内でスケジューラが決める）。(4) トークン使用量の上限は無く、上限は turn 数（既定30）のみ。重要度 推奨／難易度 小〜中。 |

## A07 標準業務ワークフロー

| 項目 | 内容 |
|---|---|
| 判定 | 一部実装（汎用の2テンプレートのみ。業務別テンプレートは未実装） |
| 実装の概要 | **独自実装（有向グラフ定義）**。組込み2種: `standard`（planner → plannedTasks）と `quality-loop`（plan → build → verify、検証が rework なら build へ最大3回、replan なら plan へ最大1回、上限到達時は利用者に尋ねる）。グラフは JSON ファイル（`<root>\org\graphs\<id>.json`）で、利用者が追加でき、検証V1〜V12（到達性・循環はフィードバック辺のみ・分岐・周回の入れ子・役割と作業場所の整合・条件・回数見積り等）を通ったものだけ使える。ノード種別は agent / gate（人の判断）/ router（決定的分岐）/ plannedTasks。開始時にグラフと役割の写しをワークフロー領域へ保存し、定義ファイルを後で変えても実行中の流れは変わらない。**障害調査・問い合わせ回答・影響調査など業務別の手順テンプレートは組込みに無い**（標準Skill・業務別手順の組込みもコード上に無し）。必須品質ゲートはグラフ作者が gate／verifier ノードを置くことで表現でき、エンジンが強制する固定ゲートは無い。 |
| 根拠 | `org-engine/src/graph/templates.rs:10 STANDARD_JSON`、`:30 QUALITY_LOOP_JSON`、`:71 builtin_templates`。定義型: `graph/def.rs:37 GraphDef`、`:85 NodeKind`、`:146 EdgeDef`、`:165 Feedback`。検証: `graph/validate.rs`（`validate`、V1〜V12）。実行: `graphrun.rs`（`take_feedback :470`、`apply_feedback :517`）、`graph/route.rs`。ホスト: `src-tauri/src/host/org/graphs.rs:3`（置き場所・create_new）、`:118 load_graphs`、`:265 load_copies`。UI: `src/ui/OrgIntake.tsx`（グラフ選択）、`OrgPane.tsx`。Skillsとの結び付けは未確認（`memory` フィールドが `MemoryUse::{None,Pinned,All}` だが `validate.rs:369` で none のみ許可）。 |
| 動作確認 | `host::org::graphs::tests`（first_start_creates_the_builtin_templates_and_never_overwrites、a_bad_definition_is_reported_with_reasons_and_left_where_it_is、a_workflow_restores_from_its_copy_even_when_the_definition_files_change_later）、`graphrun::tests`（three_rounds_then_pass_completes_the_body、ask_at_the_limit_blocks_the_target_and_an_extra_round_adds_one、fail_at_the_limit_fails_the_target、a_gate_is_decided_only_by_the_user）が通過。 |
| 不足・制約 | (1) 障害調査（受付→調査→統合→追加調査の要否→回答案→独立レビュー）・開発（要求→影響→計画→実装→テスト→修正→レビュー）の業務別テンプレートは無い（利用者がJSONで書けば可能。ただし扇状の分岐＝並列ブランチはグラフでは未対応（V6: 無条件の辺は1本まで）、並列は plannedTasks 側のみ）。重要度 必須（方針の標準処理のため）／難易度 中。(2) グラフ内の「並列調査→統合」を宣言的に書けない。重要度 推奨／難易度 大。(3) 業務別の必須品質ゲートと承認条件は未決定事項のため、固定実装もない。(4) 手順内でのSkill参照（A08）との結び付けは未確認。 |

## 構成図用の事実

- コンポーネント: UI（`app/src/ui/Org*.tsx`、IPC） ⇄ ホスト（`src-tauri/src/host/org/`: `driver.rs` エンジン駆動、`ports.rs` ポート実装、`graphs.rs`・`roles.rs` 定義ファイルの読込み、`ipc.rs`、`view.rs`、`notify.rs`、`stall.rs`、`artifacts.rs`）⇄ エンジン crate `org-engine`（`core.rs` 状態機械、`planned.rs` standard、`graphrun.rs` グラフ実行、`scheduler.rs`、`policy.rs`、`limits.rs`、`dag.rs`、`persist.rs` 事象記録）。エンジンは純粋ロジックで、入力 `OrgInput` に対し出力 `OrgAction`（送信・チャット作成・Workspace操作・値保存・通知）を返し、ホストが実行。
- 接続: ホスト → Codexバックエンド（`codex/adapter.rs`、stdio JSON-RPC、App Server）。既存Codex接続を共有。役割チャットは `start_chat`/`turn/start`（outputSchema付き）で作成・送信し、結果は履歴の read で取得。
- データの流れ（standard）: 利用者の依頼 → Planner チャット（read-only）→ plan JSON → `parse_plan`・`validate_plan`・`evaluate_plan` → 自動承認 or 利用者承認（`Halted{AwaitingPlanApproval}`）→ タスク表（pending）→ `promote_ready`→ `next_dispatch` → Worker：専用worktreeを作成してチャット作成・送信 → 報告JSON → AgentDockがworktreeでコミット → Reviewer：コミットに固定したdetached worktreeでチャット → verdict（pass/rework）→ rework は最大2回差戻し → 完了。マージ・pushはしない（利用者が取り込む）。
- データの流れ（graph/quality-loop）: plan(spec) → build(report, commit) → verify(verdict3) → pass で完了／rework で build へ（最大3）／replan で plan へ（最大1）。上限到達は利用者に確認。各スロット値は `slots\` のファイルに保存。
- 保存: ワークフロー別領域に `workflow.json`・`events.jsonl`（`persist.rs`）、`graph.json`/`roles.json`（開始時の写し）、定義ファイルは `<root>\org\graphs\` と `<root>\org\roles\`。
- Codex側の子孫: `thread/list`（`SUBAGENT_KINDS`）の走査 → `convert.rs` で Agent 化 → `tree.rs:assemble` で親子ツリー → 画面の子孫表示・通知。組織実行は `descendants()` で参照のみ。AgentDockからの子孫の起動・制御経路は無い。
- 未確認: 自動再計画の発火経路（`AutoReplanSpent` の発行元は非テストコードに見当たらず）、Plannerが役割定義ファイル追加分を認識する経路、実Codexでの並列実行・子孫付きタスクの挙動、`Limits.max_tasks` の既定値。
