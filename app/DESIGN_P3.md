# P3 設計メモ: VS Code拡張との同等性18操作（段階③）

対象: 正本 §3.14（18操作）、関連 §2.3・§3.5・§3.6・§3.7・§3.13・§4・§5・§6・§8（EXP・B03・CFG）、A6、受入 A99〜A116。本書の内容はすべて**設計案**。前提は `DESIGN_T1.md`・`DESIGN_P2.md`。schema は 0.160.0（`ts-stable/v2`、experimental は `ts-experimental/v2`）。

**前提とする修正（並行中の段階②持ち越しバッチの完了後）**: `unresolved_sends` へ送信前に Sending 相当を保存（R2-L6）、成果物「開く」の実行形式確認、`HostSnapshot.quit`、画像貼付けIPCの上限、`AppSettings.executables: {backend → path}`、削除工程の `DeleteStep` 列挙化。本書の保存形式・設定・削除工程はこの修正後の形に**追加する**。衝突しうる箇所は各節に「衝突注意」と書く。P3-0 の開始時に修正バッチが master にあることを確認する。

---

## 0. 共通方針

### 0.1 経路の3区分
| 区分 | 意味 | 例 |
|---|---|---|
| App Server API | バックエンド実装（`codex/`）が JSON-RPC で行う | review・fork・compact・goal・skills・MCP |
| AgentDock管理 | バックエンドに依存しないホスト処理（Git・ファイル・保存） | 差分のGit照合、変更を戻す、worktree、参考指定、side相談の境界 |
| CLI補助 | `codex.exe` のサブコマンドを明示操作時だけ起動（バックエンド実装の内側） | `codex plugin`・`codex mcp`・`codex cloud` |

### 0.2 操作の結果は「受付」と「観測した結果」を分ける（A99〜A116）
- 状態を変える操作はすべて `&UserConfirmed` を要求し（T1）、監視・自動処理からは呼べない。
- 戻り値は共通の `OpAck`（`Accepted` / `Rejected{message}` / `Unknown{message}`）。`Accepted` は「Codexが要求を受け付けた」であり、結果（圧縮済み・レビュー完了等）は後続イベントで観測したときだけ表示する。
- `Unknown`（timeout・切断）は**再送しない**。操作ごとの読取り専用の照合（§1 の各操作の「照合」）だけを行い、UIは「受理不明」と「状態を確認」（readのみ）を出す。
- turn を開始する操作（review・compact）は、送る前に `PendingOp` をチャットの保存へ書く（R2-L6 と同じ考え）。異常終了後は受理不明として残り、同じ操作の重複実行と、そのチャットのキュー自動送信を止める（`QueueHold::OperationUnconfirmed`）。
- 通信断・取得不能は done/failed にしない。操作の結果も同様（「完了」は終端の明示イベントか履歴の終端からだけ）。

### 0.3 experimental・非推奨・未確認の扱い
- 能力は「宣言（Support）」と「確認状況（Verification）」の2軸で持つ（§2）。Support は schema 上の有無、Verification は実機で確認済みかどうか。
- `Verification::Unverified` の操作は、メニュー・ボタンに「未確認」ラベルを付け、結果は「要求は受け付けられた／〜を観測した」のように観測事実だけを書く。「成功しました」と書かない。
- experimental は接続時の `experimentalApi`（`ConnectConfig.enable_experimental`、初期値有効）で使う。拒否されたら能力を `Unsupported` に降格し、理由を表示する。
- 非推奨（`personality`、review の `detached`、`additionalSpeedTiers`、`contextCompacted` 通知、resume の全履歴展開）は使わない。`personality` は「非推奨（このCodex版では選べない）」と表示する。
- 確認状況は**コード内の版別の表**（`codex/parity_table.rs`、`(codex版, ParityOp) → Verification`）で持つ。初期値はすべて `Unverified`、ただし #11 の明示Skill呼出しは実測成立（§47）なので `Verified`。実機確認の結果は司令塔がこの表を更新する。ヘルプ「同等性の確認状況」で18操作の経路・宣言・確認状況を一覧する（A6 の台帳）。

### 0.4 合意との整合（全操作共通）
- 監視目的で resume・プロンプト追加・承認代行・停止をしない。本書の操作はすべてユーザーの明示操作。ユーザー操作の中でだけ `ensure_live`（resume、`excludeTurns: true`）を含めてよい（送信と同じ扱い）。
- 表示のために resume しない（Goal の取得・差分表示・参考指定は read だけ。読めなければ「未取得」）。
- 外部実行中の会話（閲覧のみ）には、状態を変える操作を出さない（`Blocked{ExternalRunning}`）。削除保留中のチャットも同様（`Blocked{DeletePending}`）。
- `~/.codex` を変えうる操作（Plugins・MCPの管理、memory のリセット）は明示確認のうえ、前後で `config/read` を取り直して照合し、不一致は記録・表示する。自動rollback・自動再信頼をしない（B03・CFG）。

---

## 1. 18操作の設計

各項目: **経路** / **状態** / **IPC** / **保存** / **失敗・受理不明** / **合意**。IPC名はコマンド（snake）とイベント（camel）。

### #1 変更ファイル一覧と差分表示
- **経路**: App Server（`turn/diff/updated` 安定＝turnの集約diff、`fileChange` item の `changes[]{path, kind, diff}` 安定）＋AgentDock管理（Git読取り）。
  - 「Codexが報告した変更」: live中は `turn/diff/updated` の最新値（turnごとに上書き）、履歴からは read した `fileChange` item を集める。どちらも統一diff文字列で、`rules::diff::split_files` でファイル別に分ける。
  - 「現在のファイル（Git）」: `git --no-optional-locks status --porcelain=v2 -z` と `git diff`（読取りのみ、索引を書かない）。Gitがない・リポジトリでなければこの区分を「非対応（Gitなし）」と表示。
  - 2つは混ぜずに出所を表示する（「Codex報告」「Git上の現在の差分」）。コマンド実行による変更は fileChange に出ない場合があることを注記する。
- **状態**: なし（表示のみ）。`turn/diff/updated` は描画用で状態判定に使わない。
- **IPC**: `get_change_list{chat, scope: Turn{turn} | Chat | WorkingTree}` → `ChangeList{files: [ChangedFile{path, kind, moveTo, source, additions, deletions}], basis}`、`get_file_diff{chat, path, source, turn?}` → `UnifiedDiff`。イベント `changesUpdated{chat, turn}`（中身は送らず、開いているUIが取り直す）。
- **保存**: §#2 の `changes.jsonl` を兼ねる（live観測分）。turn集約diffは保存しない（再起動後は item から組み直す）。
- **失敗**: Git失敗・タイムアウト（30秒）は「Git上の差分は未取得」。Codex報告が取れなければ「未取得」。空diffと未取得を区別。
- **合意**: 読取りだけ。resumeしない（履歴は read）。

### #2 変更を戻す
- **経路**: AgentDock管理。`thread/revert` は会話履歴の巻戻しでファイルを戻さない（schemaの注記どおり）ので**使わない**。方式は**要判断Q1**。以下は推奨案（Q1-1）で書く。
  - live中に `fileChange` item が完了したら、ホストが `ChangeRecord{chat, agent, turn, item, path, kind, moveTo, diff, postHash: Known<sha256>, observedAt}` を `changes.jsonl` に追記する。`postHash` は観測直後にファイルを読んで取る（読めなければ `notFetched`）。
  - 戻す単位はファイル（turn単位の「このturnの変更をすべて」はファイル別の一括）。`preview_revert` で計画を作り、各ファイルの判定を示す。
  - 帰属の判定（`rules::revert::plan`、純粋ロジック）: ①そのファイルの**全チャットの記録**のうち最新が当該チャットのもの、②現在の内容のハッシュがその記録の `postHash` と一致、③戻す対象turn以降の同ファイルの記録を新しい順に逆適用できる（diffの文脈が完全一致）。どれかが欠けたら「戻せない（理由）」として**止めて案内**（例: 「別の会話またはエディタで変更された可能性」「この変更はAgentDockが観測していない」「コマンドによる変更は対象外」）。
  - 実行: 対象ファイルの現内容を `chats\<dirId>\revert-backup\<ms>\` へ控えてから原子的に置換（追加→削除、削除→再作成、移動→戻す）。控えは削除しない（チャット削除まで保持、使用量に計上）。
- **状態**: 計画 `RevertPlan{id, items: [RevertItem{path, verdict: Revertible | Blocked{reason}}]}`（メモリのみ、5分で失効）。結果 `RevertResult{reverted, failed: [path, reason], backupDir}`。
- **IPC**: `preview_revert{chat, turn?, paths?}` → `RevertPlan`、`revert_changes{chat, planId, paths}` → `RevertResult`。実行前に計画を再判定し、変わっていれば `Blocked{PlanStale}`。
- **保存**: `chats\<dirId>\changes.jsonl`（追記、`ActivityLine` と同じく最終行の欠けは無視）、`revert-backup\`。
- **失敗**: ファイルごとに成否を出し、部分成功を完了と偽らない。対象チャット（および同じ作業フォルダで作業中のチャット）が作業中・停止未確認なら `Blocked{ChatBusy}`。
- **合意**: 推定で戻さない（帰属不明は止める）。元ファイルの変更はユーザーの明示操作だけ。

### #3 コードレビュー ／ #4 レビュー結果の保存先
- **経路**: App Server `review/start`（安定）。対象 `ReviewTarget`: 未コミット（`uncommittedChanges`）／基準ブランチ（`baseBranch{branch}`）／コミット（`commit{sha, title}`）／指示（`custom{instructions}`）。ブランチ候補は Git の `for-each-ref`（読取り）。作業フォルダがGitリポジトリでなければ未コミット・ブランチ・コミットを無効化して理由を表示。
- **#4 保存先**: 「現在のチャット」＝`delivery: inline`。「別チャット」は schema が `detached` を非推奨としているので**使わず**、`thread/start`（同じcwd・モデル・権限、アプリ管理＝hosted、名前「レビュー: 元の名前」）→ 新チャットで inline の `review/start`。新チャットの `chat.json` に `reviewOf: ChatKey` を記録して「レビュー元」を表示する。
- **状態**: レビューは通常のturnとして扱う（running→終端）。`enteredReviewMode`／`exitedReviewMode{review}` item を活動として表示し、`exitedReviewMode.review` をレビュー結果カードにする。結果カードは item を観測したときだけ出す。
- **IPC**: `start_review{chat, target, delivery: CurrentChat | NewChat}` → `ReviewStarted{chat, turn?, ack: OpAck}`。新チャット作成は成功しレビュー開始が失敗した場合は `NewChatCreatedReviewFailed{chat, message}`（チャットは残し、「レビューを開始」ボタンを出す。自動で再試行しない）。
- **保存**: `PendingOp{op: Review, since}`（送信前）。`reviewOf`（chat.json、追加フィールド）。
- **失敗・受理不明**: 応答なし→受理不明。照合は read で `since` 以降の turn に `enteredReviewMode` item があるか。キューは照合まで保留。
- **合意**: 実行中のチャットには出さない（`Blocked{ChatBusy}`）。送信待ちがあれば「この操作の完了後に送信待ちN件が送られます」を確認に出す。

### #5 Plan（計画／実行の切替）
- **経路**: App Server experimental。`turn/start.collaborationMode{mode: plan | default, settings}`（experimental版の `TurnStartParams` にだけある）、プリセットは `collaborationMode/list`（experimental）。受理は安定の `thread/settings/updated.threadSettings.collaborationMode` で確認する。
- **状態**: チャット設定 `workMode: Plan | Default`（選択値）と `acceptedWorkMode: Known<…>`（受理値）。モデル設定と同じく「選択／受理済み」を分けて表示（§3.9）。次のturnから適用。`turn/plan/updated`・`item/plan/delta`（安定）は計画の表示に使う。
- **IPC**: `set_work_mode{chat, mode}` → `SettingsImpact`（送信待ちへの影響、§3.7）。`list_work_modes` → プリセット。
- **保存**: `chat.json.workMode`（追加フィールド）。
- **失敗**: experimental 拒否→能力を `Unsupported` に降格し、設定欄を「このCodex接続では非対応」にする。受理を観測できなければ「受理未確認」。
- **合意**: 実行中の変更は適用時期（次のturn）を表示。キュー項目には送信時点の値を適用（DESIGN_P2 §2.6）。

### #6 会話分岐
- **経路**: App Server `thread/fork`（安定、`excludeTurns: true`、任意で `lastTurnId`＝その turn まで）。「チャットを分岐」（最新の終端turnまで）と、メッセージの「ここから分岐」（そのturnまで）。進行中のturnは指定できないので、実行中なら最後に終端したturnまでを明示して確認する。
- **状態**: 新チャットはアプリ管理（hosted）、種別・cwd は元と同じ。名前「元の名前（分岐）」。`forkedFromId` は spawn の親とは別項目で表示（§3.5）。元チャットは変わらない。
- **IPC**: `fork_chat{chat, throughTurn?}` → `ForkOutcome{ack, chat?: ChatKey}`。イベントは既存の一覧更新。
- **保存**: 新チャットの `chat.json` に `forkOf{chat, throughTurn}`（Codexの `forkedFromId` が読めない場合の表示用）。
- **失敗・受理不明**: 受理不明は再送しない。照合は `thread/list`（cwd・作成時刻 ≥ since）で `forkedFromId == 元` の会話が**ちょうど1件**ならそれを採用し hosted にする。0件・複数件は「分岐先を確認できません」（一覧の更新を案内）。
- **添付**: 分岐先の履歴は元チャット領域の添付コピーのパスを参照する。コピーはしない。元チャットの削除確認に「分岐先N件がこのチャットの添付を参照しており、削除後は欠損表示になる」を出す。
- **合意**: 元チャットのresumeは不要（forkはパス不要の thread_id で行う）。

### #7 文脈の圧縮（圧縮前本文も閲覧可）
- **経路**: App Server `thread/compact/start`（安定、応答は空＝受付のみ）。完了は `thread/compacted` 通知（安定）または `contextCompaction` item の完了で観測する。
- **圧縮前本文**: 正本はCodex履歴。ユーザー操作の圧縮では、送る前にそのチャットの本文を turns/items の一覧で取得し、`chats\<dirId>\compactions\<ms>.json` に「圧縮前の控え（AgentDock保存・表示専用）」として保存する（空き確認つき）。圧縮後もCodex履歴に過去turnが残るかは**未確認**で、実機で確認する。残る場合も控えは保持する（チャット削除まで）。Codexの自動圧縮は事前に控えを取れないので、履歴に残らなければ「圧縮前の本文はCodex履歴から取得できません」と表示する。
- **状態**: 圧縮中は通常のturn（running）。「圧縮済み」は観測時だけ表示。
- **IPC**: `compact_chat{chat}` → `OpAck`、`list_compaction_snapshots{chat}`、`read_compaction_snapshot{chat, id}`。
- **保存**: `PendingOp{op: Compact}`、`compactions\`。
- **失敗**: 控えの保存に失敗したら圧縮を送らない（「控えを保存できないため中止」）。受理不明の照合は read で `contextCompaction` item の有無。
- **合意**: 実行中は不可。再送しない。

### #8 過去会話の参考指定
- **経路**: AgentDock管理＋App Server `thread/read`（resumeしない）。専用の参照入力契約は使わない（未確認のため）。
- **振る舞い**: 添付メニュー「過去の会話を参考に…」→ 会話の選択（アーカイブ・外部会話を含む）→ turn の選択（初期値は直近3turn）→ ユーザー依頼とエージェント応答の本文を機械的に抜き出し、出所（会話名・日時）付きの引用ブロックとして**入力欄へ挿入**。送信前に確認・編集できる。AIで要約しない（§6）。文字数を表示し、2万字超は警告（止めない）。
- **状態**: なし（入力欄の文章になるだけ。下書きとして既存の仕組みで保存・復元）。
- **IPC**: `list_reference_turns{chat}` → `[ReferenceTurn{turn, at, userPreview, hasAgentText}]`、`build_reference{chat, turns}` → `{text, chars, missing}`（取れなかったturnは `missing` に列挙し本文に「未取得」と書く）。
- **保存**: 追加なし（下書きに含まれる）。
- **合意**: 参照先の会話を変更しない。元ファイルとの対応を推定しない（A98と同じ）。

### #9 Goal
- **経路**: App Server `thread/goal/get`・`set`・`clear`（安定）、通知 `thread/goal/updated`・`cleared`（安定）。
- **状態**: `Goal{objective, status: GoalStatus, tokenBudget: Known<u64>, tokensUsed, timeUsedSecs, updatedAt}`、`GoalStatus = active | paused | blocked | usageLimited | budgetLimited | complete | unknown(raw)`（中立名に写像。未知値は unknown＋原文）。トークン数は表示だけで料金・残量に換算しない（§3.10）。
- **表示**: 入力欄の上の常設欄（モック）。開いたときに get（read相当）を試み、threadが未ロード等で取れなければ「未取得」。**表示のためにresumeしない**。設定・更新・解除はユーザー操作なので `ensure_live` を含めてよい。
- **IPC**: `get_goal{chat}` → `Known<Goal>`、`set_goal{chat, objective?, status?, tokenBudget?}`・`clear_goal{chat}` → `OpAck`。イベント `goalUpdated{chat, goal: Known<Goal> | cleared}`。
- **保存**: なし（Codexが正本）。表示用に `cachedMeta.goal` を持ってもよいが鮮度は「切断」扱い（任意）。
- **失敗**: 受理不明は get で照合（読めた値をそのまま表示）。
- **合意**: Goalの status 変化（complete等）をエージェント状態（§4.1）に流用しない。

### #10 side相談
- **経路**: App Server `thread/fork{ephemeral: true, excludeTurns: true, lastTurnId: 最後の終端turn, sandbox: read-only}`＋AgentDock管理の相談境界。専用経路はない。
- **振る舞い**: 「side相談を開く」で主会話の最後の終端turnまでを一時forkし、右側（狭い画面ではオーバーレイ）の専用パネルで対話する。sandbox は read-only を初期値にし、主作業のファイルに触れない（承認方針は主会話と同じ）。主会話の実行・キューはそのまま続く。
- **引渡し**: side の発言を選んで「主会話へ渡す」→ 主会話の入力欄へ引用ブロックとして挿入（自動送信しない）。
- **状態**: `SideSession{id: LocalId, main: ChatKey, thread: ChatKey, state: Open | Ended{reason}}`。side の thread は一覧に出さず、主会話の子孫にも数えない（キューの自動送信条件に入れない）。ただし**停止対象には含める**: 完全終了・強制終了の確認、主会話の削除の停止照合に列挙する。side内の承認・質問は side パネルで回答し、通知に「side相談」と付ける。
- **持続**: ephemeral はディスクに残らずプロセス終了で消える。AgentDockは確定した item の短い本文を `chats\<主dirId>\side\<id>.json` に表示用として保存し、閉じた後・再起動後は「終了（再開不可）」の読取り専用記録として主会話から開ける。保持は主会話の削除まで（自動削除しない）。App Server再起動・切断時は鮮度を切断にし、状態を done にしない。
- **IPC**: `open_side{chat}` → `SideSession`、送信は既存 `send_message`（chat に side の ChatKey、`ChatRole::Side` をホストが判定）、`close_side{side}`（実行中なら中断を確認してから）、`handoff_side{side, item}` → `{text}`（入力欄へはUIが挿入）。イベント `sideUpdated`。
- **保存**: `chat.json.sideSessions: [SideSessionMeta]`、`side\<id>.json`。
- **失敗**: fork の受理不明は再送しない（ephemeral は一覧で照合できないので「開けたか不明」と表示し、閉じて開き直すかをユーザーに委ねる）。

### #11 Skills・AGENTS.md
- **経路**: App Server `skills/list{cwds, forceReload}`（安定）、通知 `skills/changed`。明示呼出しは `UserInput.skill{name, path}`（実測成立）。AGENTS.md は `thread/start`・`resume`・`fork` 応答の `instructionSources`（読み込まれた指示ファイルのパス）＋AgentDockのファイル実在確認。
- **振る舞い**:
  - 確認: 設定「Skills・AGENTS.md」タブで、選択チャットのcwdのSkill（名前・説明・scope・有効・エラー）と、読み込まれた指示ファイルの一覧（開く＝opener）。
  - 明示呼出し: 添付メニュー「Skillを指定…」→ 入力欄にSkillチップ。送信時に `skill` 入力になる。
  - 初期化: 作業フォルダに AGENTS.md がないとき、(a)「雛形を作成」＝AgentDockが最小の雛形を書く（**既存があれば書かない**、作成後に開く）、(b)「Codexに作成を依頼」＝定型の依頼文を入力欄へ挿入（送信はユーザー）。Skillの有効・無効の切替（`skills/config/write`）は要件外なので段階③では作らない。
- **IPC**: `list_skills{chat, forceReload}` → `[SkillInfo{name, description, scope, enabled, path, errors}]`、`get_instruction_files{chat}` → `Known<[path]>`、`create_instruction_template{chat}` → `{path}` | `Blocked{AlreadyExists}`。境界の添付種別に `AttachmentKind::Skill`（`model::Attachment` に `Skill{name, path}`）を追加し、能力 `attachment_kinds` に含める。
- **保存**: 下書きの添付に Skill 参照を含める（既存の送信前添付と同じ扱い。コピーはしない）。
- **合意**: `~/.codex` 配下のSkill・AGENTS.mdは読むだけ。

### #12 MCP・Plugins
- **経路（決定。正本§3.14・B03の方針どおり）**:
  - MCP: App Server `mcpServerStatus/list`・`mcpServer/oauth/login`（`authorizationUrl` を返す）・`config/mcpServer/reload`、通知 `mcpServer/startupStatus/updated`・`mcpServer/oauthLogin/completed`（いずれも安定）。サーバーの追加・削除は CLI補助 `codex mcp add/remove`。
  - Plugins: 一覧は App Server の読取り（`plugin/list`・`plugin/installed`、管理系 `plugin/install`・`uninstall` は使わない）、導入・削除は CLI補助 `codex plugin add/remove`。
- **状態（§4.4）**: `ExtensionView{id, installed: Known<bool>（CLI結果）, enabledInConfig, cachePresent, advertisedInChat, hookTrust, auth, connection}` を項目ごとの `Known` で持ち、一覧に出ることを既存会話への反映と扱わない。MCPは `ToolServerView{name, connection: NotStarted|Starting|Connected|AuthRequired|Failed|Cancelled|Disabled|Unknown, auth: NotLoggedIn|Bearer|OAuth|Unsupported|Unknown, toolCount, toolsError}`。
- **管理操作の手順（`host/extensions.rs`、アプリ内で直列化）**: 明示確認 → `config/read` と `config.toml` のハッシュを記録 → CLI実行（引数配列、シェルなし、タイムアウト120秒）→ 終了コード・出力を記録 → `config/read` を取り直して照合 → 一致／不一致（想定外の変更・変更なし）を表示。`config/mcpServer/reload` はユーザーのボタンでだけ送り、「既存会話への反映は未確認」を出す。自動rollbackなし。
- **OAuth**: URLは明示クリックで既定ブラウザ（opener）。完了通知で更新、5分来なければ「完了未確認」。トークン・URLのクエリを診断ログ・活動履歴へ書かない。
- **IPC**: `list_tool_servers{chat?}`、`login_tool_server{name}` → `{authorizationUrl}`、`reload_tool_servers`、`list_extensions`、`manage_extension{op: Install{id} | Remove{id} | AddToolServer{…} | RemoveToolServer{name}}` → `ExtensionOpResult{exitCode, configCompare: Unchanged | ChangedAsExpected | Unexpected{keys}, outputSummary}`。イベント `toolServersUpdated`・`extensionOpUpdated`。
- **保存**: ルートの `extension-ops.jsonl`（操作・前後ハッシュ・照合結果。認証情報を含めない）。
- **失敗**: CLIのタイムアウト・異常終了は「結果未確認」。出力が想定形式でなければ要約せず原文の先頭を表示。

### #13 クラウド委任
- **経路**: CLI補助 `codex cloud`（0.160.0 で `exec --env <ENV> [--branch] <QUERY>`・`list --json`・`status`・`diff`・`apply` があり、いずれも [EXPERIMENTAL]）。App Server に経路なし。範囲は**要判断Q2**。推奨案（Q2-1）:
  - 委任: 依頼文・環境ID（リポジトリごとに記憶）・ブランチを入力して明示確認 → 送る前に `CloudTaskRecord{state: Submitting}` を保存 → `cloud exec` → 出力からタスクIDを取れたら `Submitted`、非0終了は `Rejected`、タイムアウト・異常終了は `Unknown`（**再送しない**。`cloud list --json` で照合を案内）。
  - 状態: パネルを開いたときとユーザーの「更新」で `cloud list --json`/`status`。定期取得はしない。状態語は原文を表示し、AgentDockの状態（§4.1）へ写像しない（鮮度は「取得時刻」で示す）。
  - 取込み: `cloud diff` で表示、`cloud apply` は作業フォルダで作業中のチャットがないことを確認し明示確認後のみ。結果は終了コードと `git status` の照合で示す。
- **保存**: `chat.json.cloudTasks` ではなくルートの `cloud-tasks.json`（チャットに属さない操作のため。起点チャットは参照として持つ）。設定に `cloudEnvByRepo: {repoRoot → envId}`。
- **能力**: `ParityOp::CloudDelegation` を `Experimental`＋`Unverified`。CLIにサブコマンドがない版では `Unsupported`。

### #14 Git worktree
- **経路**: AgentDock管理（`git worktree add/list/remove`）。置き場所とブランチ名は**要判断Q3**。推奨案（Q3-1）:
  - 新規チャットで作業フォルダがGitリポジトリなら「通常（そのフォルダ）／分離（worktreeを作成）」を選べる（初期値は通常）。
  - 作成: `WorktreeRecord{id, repoRoot, path, branch, baseCommit, createdAt, createdForChat, state: Creating}` を保存 → `git -C <repo> worktree add -b <branch> <path> <base>`（base初期値はHEAD）→ 成功で `Ready`、失敗は `Failed`（作られた分は削除しない。表示して手動の削除を案内）。元の作業ツリーの未コミット変更は持ち込まれないことを確認画面に書く。
  - 削除（別操作）: AgentDockが作った（記録がある）worktreeだけ。`git worktree list --porcelain` で実在を照合 → そのworktreeを使うチャットに作業中・停止未確認・送信待ち・受理不明がないこと → `git worktree remove <path>`（`--force` なし）。未コミット変更があれば止め、「変更を破棄して削除」は2段目の確認で `--force`。ブランチは残す。「マージ済みならブランチも削除」を選べば `git branch -d`（`-D` は使わない）。
  - 外部で作られたworktreeは一覧に「外部」として出し、作業フォルダとして選べるが削除しない。
  - チャットの削除ではworktreeを消さない（削除確認に「worktreeは残る」と表示）。
- **IPC**: `git_info{folder}` → `{isRepo, root, branch, head, dirtyCount, worktrees}`、`create_worktree{repoRoot, branch?, base?}`、`preview_remove_worktree{id}`、`remove_worktree{id, force, deleteBranch}`。イベント `worktreeUpdated`。
- **保存**: ルートの `worktrees.json`、`chat.json.worktree: Option<LocalId>`。
- **衝突注意**: `DeleteStep` にworktreeを加えない（チャット削除とは別操作）。

### #15 status・Fast・personality・memories
- **status**: ヘルプ「Codexの状態」（モック）。版・実行ファイル・接続・experimental有効・`account/read`（種類・プラン。トークン類は出さない）・`account/rateLimits/read`・`account/usage/read`（取れなければ「不明」、換算しない）・選択チャットのモデル／effort／速度（選択値と受理値）・MCP件数・設定警告（`configWarning`）。すべて読取り。
- **Fast（速度）**: `Model.serviceTiers[]{id, name, description}`・`defaultServiceTier` から選択肢を作り（固定一覧にしない）、チャット設定 `speedTier: Option<String>`（不透明ID）を `turn/start.serviceTier` で渡す。受理は `thread/settings/updated.threadSettings.serviceTier` で確認（§3.9と同じ選択／受理の区別）。`additionalSpeedTiers`（非推奨）は使わない。
- **personality**: 非推奨（schema注記: 常に効果なし）。設定欄に「非推奨（このCodex版では選べない）」と表示し、操作は置かない。能力は `Unsupported`＋`deprecated: true`。
- **memories**: `memory/status`（experimental、読取り）を表示。チャット単位の `thread/memoryMode/set`（experimental）を設定に置く。`memory/reset`（experimental）は `~/.codex` のデータを消すため、影響を書いた明示確認の後だけ実行し、結果は `memory/status` を取り直して観測値を表示する。
- **IPC**: `get_backend_status` → `BackendStatus`、`set_chat_model` に `speedTier` を追加（既存コマンドの引数拡張）、`get_memory_status`、`set_memory_mode{chat, mode}`、`reset_memory`。
- **保存**: `ModelChoice.speedTier`（追加フィールド、既定 None）、`chat.json.memoryMode`。

### #16 主要操作のボタン＋コマンドメニュー
- **UI**: 中央ヘッダーの主要ボタン（差分・戻す・レビュー・分岐・Goal、モックどおり）と、上部メニュー「作業」（モックの項目順）、`Ctrl+K` のコマンドメニュー（検索つき一覧）。
- 3つとも**同じ操作台帳** `app/src/ui/commands.ts`（`ParityCommand{id, label, op: ParityOp, enabled(state) → Enabled | Disabled{reason}}`）から作る。能力 `Unsupported` は出さないか「このAIでは非対応」、`Unverified` は「未確認」ラベル、無効時は理由（作業中・削除保留・切断・外部実行中・Gitなし）を表示する。
- キー操作でフォーカスを奪わない（入力欄のEnter設定と衝突しない `Ctrl+K`）。メニューは文字のみ（§3.13）。
- ホスト変更なし（能力と各チャットの状態は既存snapshotから導出）。

### #17 狭い画面の配置（D07）
- 境界（§11の設計裁量）: 幅 ≥1280px は3領域、960〜1279px は右パネルを一時パネル（オーバーレイ）、<960px は左右とも一時パネル。中央を優先する。
- side相談・差分表示・レビュー結果は、≥1280px では右側パネル／ダイアログ、狭いときは全面オーバーレイ（閉じると会話へ戻る）。
- 通知・新しい活動でパネルを開かない、入力フォーカスを奪わない。折り畳み状態は既存の記憶を使う。段階①②の実装を確認し、足りない分だけ足す。

### #18 同時利用（1〜5チャット、多数も一覧で）
- **ホスト**: 新しいthread（fork・side・レビュー用チャット）も既存の監視に乗るだけで、チャット数で増える定期処理を作らない。readの同時実行を最大4に制限（セマフォ、`host/` 共通）。notLoaded の終端確認は既存（10秒・8件）のまま。
- **UI**: ホストイベントの適用を `requestAnimationFrame` 単位にまとめる。`ActivityDelta` は同一itemを50msで合流。チャット一覧は200件を超えたら固定行高の自前ウィンドウ表示（依存追加なし）。右パネルは表示範囲の規則（§3.5）どおりで、全チャット表示でも終了済みは既定で出さない。
- **負荷確認**: 偽バックエンド（`BackendEvent` を直接流すテスト用実装）で 20ルート・100agent・2秒間の状態変化を流し、ホストの処理（受信→`HostEvent` 生成）が詰まらないこと（キュー滞留なし、各イベントの処理時間の上限をログ）を `#[ignore]` 付きテストで測る。画面側の2秒以内の表示（§6の暫定値）は実機確認で見る。数値目標は確定させない（正本どおり）。

---

## 2. AIバックエンド境界の拡張

### 2.1 能力宣言
`Capabilities` に `ops: Vec<OpCapability>` を追加する（既存フィールドは変えない）。

```
ParityOp = ChangeList | RevertChanges | CodeReview | ReviewToNewChat | WorkMode | Fork | Compact
         | ReferenceChat | Goal | SideChat | Skills | InstructionFiles | ToolServers | Extensions
         | CloudDelegation | Worktree | BackendStatus | SpeedTier | Personality | Memory
OpCapability { op: ParityOp, support: Support, route: OpRoute(BackendApi | AppManaged | CliHelper),
               verification: Verification(Verified | Unverified), deprecated: bool, note: Option<String> }
```
- 名前は中立（Codexの `thread`・`collaborationMode`・`serviceTier`・`plugin`・`mcp` を使わない。MCP＝ToolServers、Plugins＝Extensions）。
- AgentDock管理の操作（ChangeListのGit部分・RevertChanges・Worktree・ReferenceChat）も、バックエンドが前提の情報（変更の報告・履歴の読取り）を出せるかで `support` を決める。他AIで変更の報告がなければ RevertChanges は `Unsupported`。
- 実行時の降格: experimental の拒否・CLIのサブコマンド不在・Gitなし を検知したら `opCapabilitiesUpdated` で出し直す。

### 2.2 trait
- `AiBackend` を壊さないよう、**別trait `ParityOps`**（`AiBackend` を継承、全メソッドに既定実装＝`BackendError::Unsupported`）を `backend/parity.rs` に置く。将来のバックエンドは実装しなければ自動的に非対応になる。
- メソッド（すべて中立型。状態を変えるものは `&UserConfirmed` 必須）: `fork_chat`・`start_review`・`compact`・`get_goal`／`set_goal`／`clear_goal`・`list_work_modes`・`list_skills`・`instruction_sources`・`list_tool_servers`／`login_tool_server`／`reload_tool_servers`・`list_extensions`／`manage_extension`・`cloud_submit`／`cloud_list`／`cloud_diff`／`cloud_apply`・`backend_status`・`memory_status`／`set_memory_mode`／`reset_memory`。
- `SendRequest` に `work_mode: Option<WorkMode>`・`speed_tier: Option<String>`（チャットの現在値、DESIGN_P2 §2.6 の `AppliedSettings` にも追加）。`StartChatParams` に `ephemeral: bool`・`sandbox_override` は入れない（side は `fork_chat` の `ForkParams{through_turn, ephemeral, read_only}` で表す）。
- 新しい `BackendEvent`: `TurnChangesUpdated{turn, diff}`、`FileChangeObserved{agent, turn, item, changes: [FileChange{path, kind, move_to, diff}]}`（P4の `ArtifactObserved` と同じ検出点から出す）、`GoalUpdated{chat, goal: Option<Goal>}`、`Compacted{agent, turn}`、`ReviewResult{agent, turn, text}`、`ChatSettingsAccepted` の拡張（work_mode・speed_tier）、`ToolServersChanged`・`ToolServerLoginCompleted{name, ok}`、`SkillsChanged`、`Deprecation{message}`（警告表示のみ）。
- Codex固有の変換（`ReviewTarget`・`collaborationMode`・`serviceTier`・`McpServerConnectionStatus` 等）は `codex/parity.rs` と `codex/cli.rs`（CLI補助）に閉じ込める。

### 2.3 AgentDock管理の部品（バックエンド非依存）
- `gitops/`（新規）: Git実行の共通部品。`git` の場所は設定 `tools.git`（なければPATH）。`Command` に引数配列、シェルなし、`CREATE_NO_WINDOW`、`GIT_TERMINAL_PROMPT=0`・`GIT_OPTIONAL_LOCKS=0`、タイムアウト、出力上限16MiB。読取り系とworktree操作だけを関数として公開する（任意のgitコマンドを受け付けない）。
- CLI補助の実行部品は `codex/cli.rs`（codex.exe は `settings.executables[codex]`）。同じ制約（引数配列・シェルなし・タイムアウト・出力上限・直列化）。

### 2.4 保存形式（すべてChatKey・LocalId・中立型だけで参照）
| 追加 | 場所 | 内容 |
|---|---|---|
| `workMode`・`memoryMode`・`reviewOf`・`forkOf`・`sideSessions`・`worktree`・`pendingOps` | `chat.json`（`ChatLocalFile`） | すべて `#[serde(default)]` の追加フィールド |
| `ModelChoice.speedTier` | `chat.json` | 同上 |
| 変更の記録 | `chats\<dirId>\changes.jsonl` | `ChangeRecord`（追記型） |
| 戻す前の控え | `chats\<dirId>\revert-backup\<ms>\` | ファイル実体 |
| 圧縮前の控え | `chats\<dirId>\compactions\<ms>.json` | 表示専用の本文 |
| side相談の記録 | `chats\<dirId>\side\<id>.json` | 確定itemの短い本文 |
| worktree台帳 | ルート `worktrees.json` | `WorktreeRecord` |
| クラウド委任 | ルート `cloud-tasks.json` | `CloudTaskRecord`（タスクIDは不透明文字列） |
| 拡張管理の記録 | ルート `extension-ops.jsonl` | 前後照合の結果 |
| `tools.git`・`cloudEnvByRepo`・`worktreeRoot` | `settings.json` | 追加フィールド |

- 追加フィールドは既定値付きなので `SCHEMA_VERSION` は上げない（修正バッチが上げていればその値のまま）。新ファイルは各自 `schemaVersion: 1`。
- 使用量（P7の `get_usage`）の内訳に「変更の控え・圧縮前の控え・side相談」を追加し、チャット削除の対象（チャット領域内）に含める。worktree・クラウド記録は含めない。
- **衝突注意**: `settings.json` は修正バッチで `executables` 化される。Gitはバックエンドではないので `executables` に入れず `tools.git` に置く。`AppSettings` の構造体を同時に触るため、P3-0 は修正バッチのマージ後に着手する。

### 2.5 IPC・snapshot
- `HostSnapshot` に `pendingOps`・`sideSessions`・`worktrees`・`cloudTasks`・`opCapabilities` を追加（修正バッチの `quit` 追加の後）。Goal・ToolServers・Extensions は都度取得（snapshotに入れない）。
- `BlockedReason` に `ChatBusy`（既存なら流用）・`ExternalRunning`・`OperationUnconfirmed`・`PlanStale`・`AlreadyExists`・`GitUnavailable`・`NotARepository`・`WorktreeInUse`・`WorktreeDirty` を追加。
- 型は ts-rs 生成（`cargo test --lib export_bindings`）し、`types.ts` の再exportを足す（HANDOFF §6）。

---

## 3. 実装タスク（引継ぎ）

共通: 担当はすべて implementer。指示には「開始時に `git log -1` が master 最新か確認」「改行コードは既存のまま（`git diff --stat` が変更行数相応）」を入れる。ホスト処理は新規ファイルに置き、`host/mod.rs` は呼出し数行に留める（`host/changes.rs`・`host/thread_ops.rs`・`host/chat_prefs.rs`・`host/compose.rs`・`host/side.rs`・`host/extensions.rs`・`host/cloud.rs`・`host/worktree.rs`）。単体テストは純粋ロジック（`rules/*`）だけ。

| # | 内容（操作） | 主な対象 | 依存 | 受入条件の要点 | 単体テスト |
|---|---|---|---|---|---|
| P3-0 | 境界と共通部品: `ParityOp`・`OpCapability`・`ParityOps`（既定=Unsupported）、`parity_table.rs`、`OpAck`・`PendingOp`（送信前保存、キュー保留）、`gitops/` と `codex/cli.rs` の実行部品、保存形式の追加フィールド、snapshot・BlockedReason、`ui/commands.ts` の台帳と「同等性の確認状況」ダイアログ | `backend/{model,parity}.rs`、`codex/{parity,parity_table,cli}.rs`、`gitops/`、`store/records.rs`、`host/mod.rs`、`rules/queue.rs`、ipc・gen・types.ts | 修正バッチ | `cargo check`・`tsc` 通過／未実装の操作は全部 `Unsupported` か `Unverified` 表示で成功表示がない／`PendingOp` 未解決でキューが `OperationUnconfirmed` 保留／追加フィールドなしの旧 `chat.json` が読める | `PendingOp` によるキュー保留、`parity_table` の既定、records の serde 往復（旧ファイル互換） |
| P3-1 | #1 差分・#2 変更を戻す | `rules/{diff,revert}.rs`、`host/changes.rs`、`codex/parity.rs`（turn/diff・fileChange）、差分ビュー・戻すダイアログ | P3-0 | Codex報告とGitの出所を分けて表示／Gitなしで「非対応」／帰属不明・他チャット変更・観測外は止めて理由を表示／戻す前に控えを保存／部分成功を完了と偽らない／作業中は不可 | `split_files`、`revert::plan`（他チャットの後続記録・ハッシュ不一致・文脈不一致・追加/削除/移動）、逆適用 |
| P3-2 | #3 #4 レビュー・#6 分岐・#7 圧縮 | `host/thread_ops.rs`、`codex/parity.rs`、`rules/thread_ops.rs`（照合判定）、ダイアログ | P3-0 | detached を使わない／別チャットは thread/start＋inline、片側失敗を区別／レビュー結果は item 観測時だけ／分岐の受理不明は1件一致時だけ採用／圧縮前の控えを保存してから送る（失敗なら送らない）／受理不明で再送しない | 分岐の照合（0・1・複数件）、PendingOp の照合判定、ReviewTarget の可否（非Git） |
| P3-3 | #5 Plan・#9 Goal・#15 status・Fast・personality・memories | `host/chat_prefs.rs`、`codex/parity.rs`（collaborationMode・serviceTier・goal・memory・account）、Goal常設欄、設定・状態ダイアログ | P3-0 | 選択値と受理値の区別（settings/updated）／experimental拒否で非対応へ降格／Goal取得のためにresumeしない／personality は非推奨表示のみ／memoryリセットは確認後のみ・結果は観測値／トークンを換算しない | GoalStatus・接続状態の写像（未知値→unknown）、SettingsImpact |
| P3-4 | #8 参考指定・#10 side相談・#11 Skills・AGENTS.md | `host/{compose,side}.rs`、`codex/parity.rs`（fork ephemeral・skills・instructionSources）、`model::Attachment::Skill`、添付メニュー、sideパネル | P3-2 | 参考は入力欄へ挿入するだけ（自動送信・要約なし）／sideは read-only・一覧に出ず・キュー条件に入らず・停止対象には入る／引渡しは自動送信しない／side記録は閉じても残る／雛形は既存を上書きしない／Skillチップが skill 入力で送られる | 参考ブロックの組立て（未取得turn・フェンス）、side の停止対象集合、`build_turn_input` の Skill |
| P3-5 | #12 MCP・Plugins | `host/extensions.rs`、`codex/{parity,cli}.rs`、設定「MCP・Plugins」タブ | P3-0 | 管理APIを使わない／CLIは明示確認時のみ・直列化／前後の `config/read` 照合と不一致表示／自動rollbackなし／reloadは明示ボタン／状態を項目別に表示（§4.4）／OAuthはクリックでのみ開く・秘密を記録しない | config照合の判定、CLI出力の解析（想定外形式→原文） |
| P3-6 | #13 クラウド委任 | `host/cloud.rs`、`codex/cli.rs`、委任ダイアログ・一覧 | P3-5（cli部品）、Q2 | Submitting を保存してから実行／受理不明で再送しない／状態語を§4.1へ写像しない／apply は作業中なし＋明示確認時のみ／サブコマンドなしで非対応 | 出力解析、状態遷移（Submitting→Submitted/Rejected/Unknown） |
| P3-7 | #14 worktree | `gitops/worktree.rs`、`host/worktree.rs`、`rules/worktree.rs`、新規チャットの選択、削除ダイアログ | P3-0、Q3 | 作成前に記録／AgentDock作成分だけ削除／使用中・未コミットで止める／`--force` は2段目の確認／`branch -D` を使わない／外部worktreeは削除不可／チャット削除で消さない | 削除可否の判定、`worktree list --porcelain` の解析、ブランチ名の安全化 |
| P3-8 | #16 コマンドメニュー・#17 狭い画面 | `ui/commands.ts`、`Chrome.tsx`、`CenterPane.tsx`、`styles.css` | P3-1〜P3-4（台帳の項目が揃ってから） | 3つの入口が同じ台帳から出る／非対応・未確認・無効理由の表示／Ctrl+K／境界幅どおりのオーバーレイ／通知でパネルを開かずフォーカスを奪わない | なし（画面） |
| P3-9 | #18 同時利用・負荷 | `host/`（readセマフォ）、`live.ts`（rAFまとめ・delta合流）、`LeftPane.tsx`（ウィンドウ表示）、偽バックエンドの負荷テスト | P3-8 | 20ルート・100agent・2秒で滞留なし（ignoreテストのログ）／200件超の一覧がスクロールできる／既存表示規則を変えない | delta合流、ウィンドウ範囲の計算 |
| R3 | 設計レビュー（designer）: P3-1（ファイル書換え・帰属）、P3-4（side の停止・キュー境界）、P3-5（config.toml・CLI）、P3-7（worktree削除）の差分 → `app/REVIEW_R3.md` | — | P3-7 まで | 重大度順の指摘 | — |
| P3-10 | R3 の指摘の修正 | 指摘箇所 | R3 | 指摘の解消 | 指摘に応じて |
| S3 | 段階③の実機確認手順（NSIS）。0.3 の確認状況の表を結果で更新 | `app/LIVE_CHECK_P3.md`、`parity_table.rs` | P3-10 | — | — |

### 並行可否
- 順序の推奨: **修正バッチ → P3-0 → P3-1 ∥ P3-3 → P3-2 ∥ P3-7 → P3-4 ∥ P3-5 → P3-6 → P3-8 → P3-9 → R3 → P3-10 → S3**。
- `codex/parity.rs` を P3-1〜P3-6 が触るので、P3-0 で操作ごとの節（関数の空実装）を先に作り、各タスクは自分の節だけを埋める。それでも衝突は追記レベルなので司令塔がマージで解く。
- P3-1 と P3-3 は対象ファイルが分かれる（changes vs chat_prefs）。P3-2 と P3-7 も別（thread_ops vs worktree）。P3-4 は fork を使うので P3-2 の後。P3-6 は P3-5 の CLI部品の後。
- P3-6 は Q2、P3-7 は Q3、P3-1 は Q1 の回答が必要（回答前でも P3-0・P3-3・P3-2 は進められる）。

---

## 4. 決定事項（ユーザー判断不要。§11の設計裁量または正本から導けるもの）
1. `thread/revert` はファイル復元に使わない（schema注記）。
2. レビューの別チャットは `detached`（非推奨）を使わず thread/start＋inline。
3. 能力は「宣言」と「確認状況」の2軸。確認状況は版別のコード内の表で、実機確認で更新する。
4. Plugins の管理は CLI補助、一覧は App Server の読取り。MCPは App Server、追加・削除は CLI補助。どちらも前後照合・直列化・自動rollbackなし（正本 §3.14 #12・B03）。
5. side相談は read-only の一時fork。一覧・キュー条件に入れず、停止対象には入れる。記録は主会話の削除まで保持。
6. 参考指定は抜粋を入力欄へ挿入（要約なし）。Skill の有効・無効の切替と過去会話の mention 参照は作らない。
7. 圧縮はユーザー操作時に圧縮前の控えを保存してから送る。
8. 狭い画面の境界は 1280px／960px。コマンドメニューは Ctrl+K。
9. Goal の取得・差分表示・参考指定のために resume しない。設定変更系の操作の中でだけ ensure_live を含める。
10. personality は非推奨表示のみ。memory のリセットは明示確認つきで提供。

## 要判断（ユーザーへの質問）
- **Q1. #2「変更を戻す」の方式**
  - (1) **Codexが報告した変更の差分を逆適用＋ハッシュ照合（推奨）**: AgentDockがliveで観測したファイル変更だけを、現在の内容が観測直後と一致する場合に戻す。追加の保存は差分テキストだけで軽い。Gitがなくても動く。短所: シェルコマンドによる変更・観測していない変更（再起動中、外部）は戻せない（止めて案内）。
  - (2) **Gitリポジトリでturn開始時に基準を控える**: turn開始時に HEAD と未コミットのファイルの控えをAgentDock領域へ保存し、turn後に差のあるファイルを基準へ戻す。コマンドによる変更も戻せる。短所: Gitリポジトリだけ、控えが turn ごとに増え自動削除しない（容量）、同じフォルダで他チャットが同時に作業していると帰属不明で止まる場面が増える。
  - (3) **(1)＋(2)の併用**: Gitリポジトリでは(2)、それ以外は(1)。網羅性は最大だが実装と容量が最大。
- **Q2. #13 クラウド委任の範囲**（`codex cloud` は experimental のCLIのみ。App Serverに経路なし）
  - (1) **CLI補助で委任・状態一覧・差分表示・取込み（apply）まで（推奨）**: アプリ内で完結。短所: experimental で出力形式・環境IDの取得方法が未確認（環境IDは手入力で記憶）。取込みはローカルファイルを変える。
  - (2) **委任と状態一覧だけ。差分・取込みはWeb（ブラウザで開く）**: ローカルを変える操作を持たず安全。短所: 結果の確認・取込みがアプリ外になる。
  - (3) **委任はWebへの導線だけ（ブラウザで開く）**: 実装最小。短所: アプリ内の委任にならず、同等性としては弱い（除外の合意にはならない扱いで「Web経由」と表示）。
- **Q3. #14 worktreeの置き場所とブランチ**
  - (1) **AgentDockの領域（`%LOCALAPPDATA%\com.agentdock.app\worktrees\<リポジトリ名>-<短いID>`）、ブランチ `agentdock/<チャット名を安全化>-<短いID>`（推奨）**: 所有が明確でリポジトリの周りを汚さない。短所: プロジェクトから離れた深いパスになり、エディタで開くときに探しにくい。
  - (2) **リポジトリの隣（`<親フォルダ>\<リポジトリ名>.worktrees\<名前>`）**: 見つけやすい。短所: ユーザーのフォルダにAgentDockがフォルダを作る。
  - (3) **作成のたびに場所を選ぶ（初期値は(1)）**: 柔軟。短所: 作成の手間が増える。
  - ブランチ名の接頭辞 `agentdock/` を使うかも併せて（使わない場合はチャット名のみ）。

## 回答済み（2026-10-07、ユーザー）

- Q1: (1) 差分の逆適用＋ハッシュ照合。
- Q2: (1) CLI補助で委任・状態一覧・差分・取込みまで。
- Q3: (1) AgentDock領域＋`agentdock/`接頭辞。ただし**worktreeを指定しない新規チャットはフォルダを作らず通常の作業場所で動く**（初期値は通常。チャット作成ごとの自動作成はしない）。
