# P2 設計メモ: 保存・キュー・添付・通知・常駐・削除（段階②）

対象: 正本 §3.4・§3.6〜3.8・§3.11・§3.12・§4・§5・§11、合意（2026-10-06）。本書の内容はすべて**設計案**。前提は `DESIGN_T1.md`。

骨組み（中身は `todo!()`、`cargo check`・`tsc --noEmit` 通過済み）:

| ファイル | 内容 |
|---|---|
| `src-tauri/src/backend/local.rs` | アプリ側データ（保存状態・補足情報・添付・キュー・通知/窓設定・終了・使用量・削除）とIPC追加分（引数・`LocalBlockedReason`・`LocalHostEvent`・`local_command_names`） |
| `src-tauri/src/store/{mod,layout,atomic,records}.rs` | 保存層・配置・原子的書込み・保存ファイルの形 |
| `src-tauri/src/rules/{queue,notify,manage,window,export}.rs` | 純粋ロジック（単体テストはここだけ） |
| `src-tauri/src/attach.rs` | 添付コピー・実在確認・名前を付けて保存 |
| `src-tauri/src/win/{job,power}.rs` | Job Object・sleep/wake |
| `src/ipc/local.ts` | `local.rs` のTS側 |

前提（P0）: `host/mod.rs` の spawn は `tokio::spawn` 化済み、`App.tsx` の mock は動的import、`build.rs` 修正済み。P2はP0のマージ後に着手する。

---

## 1. 保存層（§3.12、D01・D02・D06）

### 1.1 配置
- ルートは `app.path().app_local_data_dir()` ＝ `%LOCALAPPDATA%\com.agentdock.app\`。現行 `lib.rs` は `app_data_dir()`（Roaming）なので P2 で切り替える。診断ログも `diag\` へ移す。
- レイアウトは `store/layout.rs` の doc comment のとおり。チャットごとに `chats\<dirId>\`（dirId＝AgentDockのLocalId）。thread IDをディレクトリ名に使わない（一般チャットは thread 開始前に作業領域が要る）。ChatKey→dir の対応は起動時に `chats\*\chat.json` を走査して作る。索引ファイルを持たない。
- 段階①で Roaming に作った `chats\chat-*` は既存 thread の cwd なので**移動しない**（§3.12「Codexの履歴・設定を移動しない」の趣旨）。使用量では「旧領域」として別に数える（P7）。

### 1.2 方式
- **JSON＋原子的書込み**（一時ファイル→`sync_all`→同一フォルダ内 rename 置換）。監視活動だけ追記型 JSONL（途中で切れた最終行は読込み時に無視）。
- SQLite は採らない。理由: データは小さく（チャット数×数KB）、ファイル単位で壊れても他チャットに波及しない。依存とマイグレーションを増やさない。
- 依存追加は `windows-sys`（空き容量 `GetDiskFreeSpaceExW`。P6のJob Object・電源通知と共用）だけ。
- 全ファイルに `schemaVersion`。新しい版のファイルは上書きしない。読めないファイルは `*.corrupt-<ms>` に改名して残し、警告を出す（削除しない）。

### 1.3 永続化対象
| 対象 | ファイル | 書込みの契機 |
|---|---|---|
| ピン、最近利用時刻、チャット別モデル・権限・次turnのcwd、下書き、送信前添付、一覧の見え方（アプリ側アーカイブ・一覧から外す）、削除保留、添付台帳、成果物台帳、確認済みの失敗、一覧表示用のメタデータ | `chat.json` | 変更のたび（下書きだけ500msまとめ） |
| キュー、受理不明の送信記録 | `queue.json` | 変更のたび。**送信前に Sending を保存してから**送る |
| 取得した監視活動（発見・状態変化・turn終端・確定item・鮮度変化。deltaは保存しない） | `activity.jsonl` | 追記 |
| 通知・窓・自動起動・codex.exe・既定モデル・送信キー | `settings.json` | 変更のたび |
| 窓の位置・サイズ | `windows.json` | 移動・リサイズ終了時（500msまとめ）と終了時 |

- 会話本文は保存しない（Codex履歴が正本、§3.6）。`cachedMeta` は接続前の一覧表示用で、鮮度は常に「切断」として表示し、状態の根拠にしない。
- 保存状態は `SaveStatus{scope, state}` を単位ごとに持ち `saveStatusUpdated` で出す。失敗は自動で最大3回再試行し、以後はユーザーの「再試行」（`retry_save`）を待つ。保存失敗中の単位に関わる「削除」「正常終了」は完了と表示しない。
- 空き容量: 書込み前に `必要量＋64MiB` を確認し、不足なら操作を止めて `InsufficientSpace` を返す。自動削除はしない。

### 1.4 起動時の復元
1. `Store::load_all()` → `HostData` に反映（ピン・モデル設定・下書き・添付・削除保留・キュー）。
2. キューは `restore_after_restart`: `Active`→`PausedAfterRestart`、`Sending`→`AcceptanceUnknown`。**起動で送信・再送しない**。
3. `Copying` のまま残った添付は `CopyFailed{Interrupted}`、`.partial` を消す。
4. 受理不明の送信記録は、接続後に照合を再開する（§2.4）。
5. 下書きは入力欄に戻すだけで送信しない（M40）。

## 2. キュー（§3.7、M35・M42・M44）

### 2.1 状態
- キュー全体 `QueueRun`: `Active` / `Stopped{cause}` / `PausedAfterRestart`。項目 `QueueEntryState`: `Waiting` / `Sending` / `AcceptanceUnknown` / `Sent{turn}` / `NotAccepted` / `Cancelled`。保留理由 `QueueHold` は全体に付く表示用（保存しない）。
- 既存の `model::QueueItem`・`QueueItemState`・`QueueHoldReason`・`HostEvent::QueueUpdated` は P3 で `ChatQueue`・`chatQueueUpdated` に置き換える（`format.ts` の `holdText`・`CenterPane` の Queue・mock も追随）。

### 2.2 自動送信の判断（`rules::queue::evaluate`）
順序は関数の doc comment のとおり。要点:
- 親の最新turnが **Completed と明示**され、子孫に running / waiting / initializing / unknown がなく、子孫の走査が完了していること。
- `baseline_at`（待っている親turnの開始観測時刻）以降に親・子孫の failed / interrupted があれば `Stop`。「確認済み」はキューを再開しない。
- 削除保留・切断・停止未確認・受理不明は `Hold`。
- 送信は1件ずつ。送った項目のturnが終わるまで次へ進まない。

### 2.3 鮮度の扱い（要判断、末尾A）
- 親は live 必須。子孫は「live」または「終端を明示的に確認済み（`terminal_explicit`）かつ `observed_at ≥ baseline_at` の履歴取得済み」を可とする案。厳密に全員 live を求めると、購読していない完了済みの子が1件あるだけでキューが永久に止まるため。

### 2.4 受理不明の照合（現状の1分打切りを置き換え）
- 照合は時間で打ち切らない。接続中は 5秒→10秒→30秒→60秒（以後60秒）で `reconcile_send` を繰り返す。切断中は止め、新しい監視元で接続して該当チャットが読めるようになったら再開する。
- 受理不明の記録は `queue.json` の `unresolvedSends` に保存し、再起動後も同じ扱い（`UnconfirmedSend::restore(record)` を `backend.rs` に追加。アダプターはclient_message_idだけで照合する）。
- UIの「履歴と照合」（`reconcile_send`）は同じ照合を即時に1回行うだけ。再送しない。
- `NotFound` は項目を `NotAccepted` にしてキューを `Stopped`。再送はユーザーの `retry_send`（新しいclient_message_id）だけ。

### 2.5 追加指示（`turn/steer`）
- 既存の `SendIntent::Steer`。`expectedTurnId` は `running_turn` から取る。turnが終わっていて拒否されたら `Rejected` を返し、UIは「完了後に送信へ切り替える」を出す（自動で切り替えない）。
- 受理（応答OK）と反映（そのturnに当該 clientUserMessageId の userMessage item が現れた）を区別して表示する。モデル・cwd・sandboxの変更には使わない。

### 2.6 送信時点の設定・作業フォルダ
- 項目には設定を持たせず、`begin_send` 時にチャットの現在値（モデル・effort・権限・cwd）を `AppliedSettings` として記録する。`turn/start` の `model`・`effort`・`approvalPolicy`・`sandboxPolicy`・`cwd` で渡す（schema確認済み）。
- `SendRequest` に `permission: Option<PermissionPreset>` と `cwd: Option<String>` を追加する（P3。`backend.rs`・アダプター・既存の構築箇所）。
- 設定変更コマンドは `SettingsImpact.affectedEntries` を返し、UIは「送信待ちN件にも適用されます」を表示する。
- cwd変更（M44, `set_chat_cwd`）: 作業中・停止未確認なら `ChatBusy`、送信待ちがあり未確認なら `QueueRetargetUnconfirmed{waiting}`。値は `nextCwd` に保存し、次の `turn/start` で渡す。

### 2.7 自動送信と `UserConfirmed`
- `AiBackend::send` は `UserConfirmed` を要求しない（T1）。キューの自動送信は、ユーザーが登録・再開した依頼の送信であり、監視目的の操作ではない。`ensure_live`（resume）は自動送信からは呼ばない。親がliveでなければ保留し、再開はユーザーの「キューを再開」（`ensure_live` を含む）で行う。

## 3. 添付・成果物（§3.8、M31・M32）

### 3.1 コピー
- 手順は `attach.rs` の doc comment（フォルダ拒否→空き確認→台帳にCopyingで保存→`.partial`へ書込み・sync・サイズ照合→rename→Ready）。失敗は `.partial` を消して `CopyFailed{reason}`。元ファイルは読むだけ。
- 配置は `attachments\<attId>\<安全化した元の名前>`。添付ごとに別フォルダなので、同じファイルの再添付も別コピー・別添付になり、`attachedAt` で区別する。
- クリップボード画像はWebViewの paste イベントで得たPNGを、生バイトのIPC（`add_attachment_image_bytes`、`tauri::ipc::Request` の Raw body、チャットIDと名前はヘッダー）で送る。JSON配列では送らない。
- 「選択範囲を貼り付け」（段階②の添付メニュー追加はこれだけ）: `tauri-plugin-clipboard-manager` の `readText` で文章を読み、本文中の最長のバッククォート連続より長いフェンスで囲んで入力欄のカーソル位置へ挿入する（UIのみ。ホストは関与しない）。

### 3.2 実在確認・開く・保存
- 会話を開いたとき・開く／保存の直前に `check_exists`。なければ `Missing`／`exists=false` で欠損表示。
- 開く: `tauri-plugin-opener` の open_path（明示操作のときだけ。生成完了で自動起動しない）。
- 名前を付けて保存: `tauri-plugin-dialog` の save で選ばせ、ホストは同名があれば `overwrite_confirmed` がない限り `TargetExists` で止める。UIで確認したら `overwrite_confirmed=true` で再実行。同じフォルダの一時ファイル経由で置換。
- 成果物の検出: アダプターが fileChange item の確定時に変更パスを `BackendEvent::ArtifactObserved{agent, item, path}`（P4で追加）として出す。ホストは実在確認できたものだけ `ArtifactEntry` にする。`in_chat_area` は `layout::is_inside(chat_dir, path)`。

### 3.3 バックエンドへの渡し方（§11）
- 境界の型は既存の `model::Attachment`（kind＋copyPath）。`Ready` の添付だけを `attach::to_send_attachments` で変換する。Codexの `UserInput` はアダプター内だけ。
- Codexアダプター: 画像→`localImage{path}`。その他のファイル→**本文**（本文の後ろに別の `text` 入力として「添付ファイル: 名前 — コピーのパス」を列挙）。`mention` は使わない（Codexの mention は app/plugin 参照用で、ローカルファイルを渡す意味が未確認のため。V09確認後に再検討）。`build_turn_input` の `File→Unsupported` をこの形に変え、能力宣言の `attachment_kinds` に `File` を加える。
- モデルの `input_kinds` に `Image` がなければ、画像添付の送信を `ModelLacksInput` で止める。表示できることとモデルが読めることを区別して表示する。

## 4. 通知（§3.11、D03、M38）

- 純粋ロジック `rules::notify::Aggregator`（時刻注入）。承認・質問は `push` が即時に返す。終端は同一チャットの最初のイベントから2秒の固定窓で集約し、`due(now)` で出す。ホストは `next_deadline` でタイマーを1本だけ張る。
- 通知の起点: 「このセッションで非終端を観測していたエージェントが終端になった」ときだけ（`was_nonterminal`）。初観測・起動時の読込み・再接続後の履歴補完・wake後の照合では通知しない。走査（read）で見つけた子の完了も、以前に非終端を観測していれば通知する。
- 重複防止: `(agent, turn, end)` と RequestKey を最大4096件覚える。
- 親の終端も同じ窓に入れる。中断は件数を別に出し、設定は「失敗」側に従う（正常完了ではない）。失敗が1件でもあれば種類は Failed。
- 抑制: 通常画面がフォーカスを持ち、かつ選択中チャットのときだけOS通知を出さない（`set_selected_chat` と窓のFocusedイベントで判定）。監視窓だけ・背後なら出す。一覧の印（`ChatMarks`）は通知設定に関係なく付ける。
- 本文: チャット名（`showChatName=false` なら出さない）と状態・件数だけ。会話本文・コード・ファイル内容・ツール引数・エラー詳細は入れない。
- 実装経路: `tauri-winrt-notification` を直接使う（クリックの `on_activated` を受けるため。公式通知プラグインはデスクトップでクリックを受けられない）。クリックは通常画面を表示して `navigateToChat` を出すだけ（回答・再実行しない）。AUMID はインストール時の識別子 `com.agentdock.app`。開発時の表示は受入証拠にしない。
- 「確認済み」（`acknowledge_failure`）は印を外すだけで、再実行・成功化・キュー再開をしない。

## 5. 窓・常駐（§3.11、§3.4）

| 機能 | 経路 |
|---|---|
| トレイ | tauri本体の `tray-icon` feature。左クリックで通常画面を表示、メニュー「開く／監視窓／AgentDockを終了」 |
| 重複起動防止 | `tauri-plugin-single-instance`。2つ目の起動は既存の通常画面を表示して終わる |
| 自動起動 | `tauri-plugin-autostart`（初期オフ）。起動引数 `--autostart` のときは通常画面を表示しない（窓は作るが非表示＝WebViewは動き、接続できる） |
| 閉じる | 通常画面の CloseRequested を `prevent_close` して hide（確認なし）。監視窓は閉じるとその窓だけ破棄 |
| 最前面 | 窓ごとに `set_always_on_top`。初期値オフ。設定時にフォーカスを移さない |
| 監視窓 | 別の `WebviewWindow`（label `monitor`、同じ `index.html` に `?view=monitor`、`focused(false)`）。範囲は選択中／全チャット（初期値は選択中）。承認・質問は「通常画面で開く」だけ |
| 位置・サイズ | `windows.json` に窓ごと保存。復元時に `rules::window::fit_to_monitors`（`available_monitors()` の作業領域と64px以上重ならなければ主モニター中央へ）。`tauri-plugin-window-state` は使わない（画面外補正を自前で保証するため） |
| 完全終了 | トレイ・メニューの「終了」→ `request_quit` → 作業中（未終端・停止未確認・受理不明・送信待ち）があれば `Confirming{busy}`。「作業を中断して終了」→各チャットへ停止手順→10秒で未確認なら `StopUnconfirmed`（待つ／中断を再試行／終了を取り消す＋強制終了）→全確認後 `Flushing`（保存）→保存失敗なら `SaveFailed`（正常終了と表示しない）→成功で `app.exit(0)` |
| 強制終了 | `win::job::ProcessJob`。App Server起動直後・initialize前に Job へ入れる。`KILL_ON_JOB_CLOSE` は付けない（異常終了で作業を無断停止しない）。`preview_force_kill` で同じ監視元の影響チャットを全部示し、明示確認後に `TerminateJobObject`。`active_processes()==0` を確認できたら各対象に OS実体確認の証拠を付ける。それまでは停止未確認 |
| sleep/wake | `win::power`（`PowerRegisterSuspendResumeNotification` のコールバック型）＋壁時計の飛び（予備）。復帰で live を要照合へ（状態は変えない）、`systemResumed` を出し、各ルートを read して対象集合を照合、接続が生きていれば live に戻す。復帰だけでは通知・再送・resumeをしない |
| インストーラー | `tauri.conf.json` の `bundle.active=true`、`targets=["nsis"]`、正式アイコン。実機確認はNSISで導入して行う |

追加する依存: `tauri`（features `tray-icon`）、`tauri-plugin-single-instance`、`tauri-plugin-autostart`、`tauri-plugin-dialog`、`tauri-plugin-opener`、`tauri-plugin-clipboard-manager`、`tauri-winrt-notification`、`windows-sys`（`Win32_Foundation`・`Win32_System_JobObjects`・`Win32_System_Threading`・`Win32_Storage_FileSystem`・`Win32_System_Power`）。JS側は `@tauri-apps/plugin-dialog`・`plugin-opener`・`plugin-clipboard-manager`。capabilities に各プラグインの必要権限だけを足す。

## 6. 削除・アーカイブ・エクスポート（§3.6、§3.12、M33・M41・M43・M46）

- **削除**: `preview_delete` で対象（会話・子孫数・添付・領域内の成果物・領域のサイズ）と削除しないもの（元ファイル・別保存・プロジェクト内・他チャット）を表示。`delete_chat` は `rules::manage::delete_decision`:
  - 作業中→中断と停止照合→停止を確認できなければ `DeletePending{StopUnconfirmed|OwnershipUnknown}` を保存。保留中は送信・キュー送信を止める（`QueueHold::DeletePending`、`send_message` は `Blocked`）。再起動後も保持。
  - 停止確認後も自動削除しない。`ReadyForUserRetry` にして、ユーザーが再度「削除」を選んだら実行。
  - 実行順: Codexの `thread/delete`（保存済みの子孫にも作用）→成功時だけチャット領域を削除。どちらかの部分失敗は `Partial` として保留を残す（完了と偽らない）。
  - 外部作成（`origin=external`）の会話は Codex 側を削除しない（元ファイルは参照扱い、M36）。`ListVisibility::RemovedFromList` にしてアプリの補足情報だけ消す。確認画面で `deletesBackendHistory=false` を示す。
- **アーカイブ（M43）**: `archive_chat` で即座に `ListVisibility::Archived{sync: WaitingForWorkEnd}`（通常一覧から隠す）。作業・通知・キューは継続。`rules::manage::archive_ready`（未終端なし・停止未確認なし・キュー送信待ちなし・受理不明なし）になったら `thread/archive` を送る。失敗は `Failed` で再試行可。`unarchive_chat` は未送信なら取り消すだけ、送信済みなら `thread/unarchive`。既存 `manage_chat{Archive}` は P7 でこの経路に付け替える。
- **エクスポート**: `export_markdown`。保存先は dialog の save で選び、同名は上書き確認（§3.2と同じ）。本文は `read` で取り直し、`rules::export::render_markdown` で組み立てる（添付・成果物はファイル名だけ、中身は含めない）。「監視活動履歴も含める」で `activity.jsonl`（子・孫を含む）を追加。取得できなかった部分は「未取得」と書く。
- **使用量**: `get_usage` を背景で集計（全体・チャット別、内訳＝添付・成果物・作業領域・監視活動・メタデータ、空き容量）。読めないパスは別に列挙し合計に含めない。

## 7. IPC追加

- 型・コマンド名・イベントは `backend/local.rs` と `src/ipc/local.ts` に定義済み（`local_command_names`／`LocalCommandMap`、`LocalBlockedReason`、`LocalHostEvent`）。各タスクで実装するとき `ipc.rs`／`types.ts` へ移す（`HostEvent` へ variant を移し、`live.ts` の `applyHostEvent` に分岐を足す。`BlockedReason` へ統合）。
- `HostSnapshot` に `chatLocals: ChatLocalView[]`・`queues: ChatQueue[]`・`attachments`・`artifacts`・`saveStatus: SaveStatus[]`・`settings: AppSettings`・`quit: QuitPhase` を追加（renderer再読込みで復元できるように）。モデル設定の再読込み後の復元（DEFERRED）もこれで解消する。
- コマンドとタスクの対応: P2＝`get_app_settings`・`set_app_settings`・`get_chat_locals`・`retry_save`・`set_draft`／P3＝`enqueue`・`edit_queue_entry`・`cancel_queue_entry`・`resume_queue`・`reconcile_send`・`set_chat_permission`・`set_chat_cwd`／P4＝`add_attachment_file`・`add_attachment_image_bytes`・`remove_attachment`・`open_file`・`save_file_as`／P5＝`acknowledge_failure`・`set_selected_chat`／P6＝`request_quit`・`quit_decision`・`preview_force_kill`・`force_kill`・`set_always_on_top`・`open_monitor_window`・`set_monitor_window_scope`／P7＝`preview_delete`・`delete_chat`・`archive_chat`・`unarchive_chat`・`export_markdown`・`get_usage`。
- `force_kill` は `UserConfirmed` を発行するコマンド層からだけ呼ぶ。

### 7.1 ts-rs の採否: **採用**（P1b として P2 より前に単独で行う）
- 理由: 段階②で型が約60個増え、手書き二重管理の取り違え（camelCase・タグ・null）がそのまま実行時の不具合になる。T4・T5でも同種の手直しが出ている。
- 手順（P1b）:
  1. `Cargo.toml` の `[dev-dependencies]` に `ts-rs`（最新の安定版、serde互換機能を有効）。derive は `#[cfg_attr(test, derive(ts_rs::TS))]` にして製品ビルドへ影響させない。
  2. `model.rs`・`backend.rs`（IPCに出る型だけ）・`ipc.rs`・`local.rs` に `#[cfg_attr(test, derive(ts_rs::TS))]` と `#[cfg_attr(test, ts(export, export_to = "../../src/ipc/gen/"))]`。`UnixMillis`・`u64` 等は `#[ts(type = "number")]`（bigint にしない）。
  3. `cargo test export_bindings` で `src/ipc/gen/*.ts` を生成し、`types.ts` は生成物の re-export ＋手書きの `CommandMap`・定数だけにする。`local.ts` は削除。
  4. 受入: 生成後に `tsc --noEmit` が通り、既存UIの変更が import 先の修正だけで済むこと。`rename_all_fields`・内部タグ・`Known<T>` の出力が現行 `types.ts` と同じ形であることを差分で確認。違う型があれば、その型だけ手書きに残し理由をコメントする。
  5. 生成物はコミットする。型を変えたら `cargo test export_bindings` を流す（検証担当の確認コマンドに加える）。

## 8. 実装タスク（引継ぎ）

共通: ホストの責務を分けるため、P2 で `host/` に `persist.rs`（保存の呼出し）を作り、以後のタスクは `host/queue_driver.rs`（P3）・`host/attachments.rs`（P4）・`host/notifier.rs`（P5）・`host/lifecycle.rs`（P6）・`host/manage.rs`（P7）に処理を置く。`host/mod.rs` への変更は呼出し1〜数行に留める。

| # | 内容 | 主な対象ファイル | 受入条件 | 単体テスト（純粋ロジックのみ） |
|---|---|---|---|---|
| P1b | ts-rs 導入 | Cargo.toml、model/backend/ipc/local.rs、`src/ipc/gen/`、types.ts | §7.1 の4 | なし（生成で確認） |
| P2 | 保存層と復元 | `store/*`、`host/persist.rs`、`host/state.rs`（ピン・モデル設定・下書きの反映）、`lib.rs`（`app_local_data_dir`・Store起動）、commands.rs、ipc.rs/types.ts/live.ts（chatLocal・saveStatus・settings・snapshot追加） | 再起動でピン・チャット別モデル・下書き・設定が戻る／書込みは原子的（途中終了で旧内容が残る）／壊れたJSONは退避して警告、上書きしない／空き不足で止めて案内／保存失敗が表示され再試行できる／一般チャットの作業領域が `%LOCALAPPDATA%` 配下 | `layout`（sanitize・is_inside・corrupt名）、`atomic::write_atomic`（tempdirで置換・失敗時に旧内容維持）、records の serde 往復と schemaVersion 拒否 |
| P3 | キュー・追加指示・照合の再開 | `rules/queue.rs`、`host/queue_driver.rs`、`host/mod.rs`（reconcile_loop置換・send経路）、`backend.rs`（`SendRequest` に permission・cwd、`UnconfirmedSend::restore`）、`codex/adapter.rs`（turn/start の設定・steer）、`CenterPane.tsx` Queue、`format.ts`、model.rs/types.ts の旧Queue型置換 | §2 の全項目。特に: 失敗・中断で止まり明示再開でだけ進む／再起動後は送らない／鮮度不足・状態不明で対象と理由を表示して保留／受理不明は再接続後も照合が続き再送しない／cwd変更の確認 | `evaluate` の全分岐（親完了＋子作業中、子unknown、走査未完了、baseline前の失敗は無視・後は停止、受理不明・停止未確認・削除保留・切断）、`apply_send_result`、`resume`、`restore_after_restart`、`check_cwd_change` |
| P4 | 添付・成果物 | `attach.rs`、`host/attachments.rs`、`codex/adapter.rs`（File→本文、ArtifactObserved）、`backend.rs`（`BackendEvent::ArtifactObserved`）、`host/state.rs`（match追加）、`CenterPane.tsx`（チップ・D&D・paste）、`Dialogs.tsx`（添付メニュー＋選択範囲貼り付け）、Cargo（dialog・opener・clipboard-manager）、capabilities | §3 の全項目。フォルダはコピーしない／コピー失敗・容量不足が明示され不完全なコピーは送れない／同一ファイル再添付が別添付／欠損表示／保存で無断上書きしない | `sanitize_file_name`、`kind_for`、`to_send_attachments`、`build_turn_input`（File→text）、TSの `fenceCode` は対象外（画面） |
| P5 | 通知 | `rules/notify.rs`、`host/notifier.rs`、`host/state.rs`（終端遷移の検出点）、`lib.rs`（窓フォーカス）、Cargo（tauri-winrt-notification）、設定画面の通知タブ | §4 の全項目 | Aggregator（2秒固定窓・窓は延長しない・失敗を隠さない・重複・初観測は通知しない）、`render`（抑制・名前非表示・本文に詳細を含めない・種類別オフ） |
| P6 | 窓・常駐・終了・強制終了・wake・NSIS | `win/*`、`rules/window.rs`、`rules/manage.rs`（quit・wake）、`host/lifecycle.rs`、`lib.rs`（tray・single-instance・autostart・CloseRequested・監視窓）、`codex/process.rs`（Job割当）、`tauri.conf.json`、capabilities、`Chrome.tsx`・`Dialogs.tsx`（終了・強制終了）、監視窓の入口 | §5 の全項目 | `fit_to_monitors`、`next_quit_phase`、`busy_for_quit`、`looks_like_resume`、`agents_to_mark_on_resume` |
| P7 | 削除・アーカイブ・エクスポート・使用量・設定画面 | `rules/manage.rs`（archive/delete）、`rules/export.rs`、`store`（usage・remove_chat_dir）、`host/manage.rs`、`host/mod.rs`（manage_chat付替え）、`Dialogs.tsx`（削除・エクスポート・設定の保存と容量・codex.exe「参照…」） | §6 の全項目 | `archive_ready`、`delete_decision`、`refresh_delete_pending`、`render_markdown`、`fence_for` |

### 並行可否
- 全タスクが `commands.rs`・`lib.rs`（invoke_handler）・`ipc.rs`/`types.ts`/`live.ts` に追記する。追記箇所の衝突は機械的に解ける程度なので、司令塔がマージで解消する前提とする。
- 順序の推奨: **P0 → P1b → P2（単独）→ P3 ∥ P5 → P4 ∥ P6 → P7**。
  - P3 と P4 はどちらも `codex/adapter.rs` の送信経路と `backend.rs` の `SendRequest` を変えるので並行しない。
  - P5 と P3 は `host/state.rs` に触るが箇所が別（P5は終端遷移の検出点、P3は読むだけ）。
  - P6 は `lib.rs` を大きく変えるので、`lib.rs` を触る P5 の後。P7 は P3（キュー）と P6（停止手順・終了）に依存する。

## 9. 段階①からの持ち越しで本書が扱うもの
子孫の2秒集約通知（P5）、ピン永続化・モデル設定の復元（P2）、受理不明の照合の再接続後再開（P3）、強制終了・トレイ・終了確認（P6）、codex.exe「参照…」（P7）、キュー再開バナー（P3）、削除確認・エクスポートのダイアログ配線（P7）、添付メニュー（P4、選択範囲の貼り付けのみ）、ts-rs（P1b）、アイコン差し替え（P6）。

## 要判断
- **A. キュー自動送信の鮮度条件（§2.3）**: (1) 親・子孫すべて live を必須（正本の字義どおり。購読していない完了済みの子がいると永久に保留し、ユーザーの「キューを再開」でも解けない）／(2) 親は live 必須、子孫は live または「終端を明示確認済みの履歴取得済み」を可（推奨）。
- **B. 強制終了の範囲（合意の帰結の確認）**: Job Object は App Server 単位なので、1チャットの強制終了でも同じ App Server 上の全チャットの作業が止まる。(1) そのまま、確認画面に影響する全チャットを出す（推奨）／(2) チャットごとに App Server を分ける（接続・監視の設計を大きく変える）。
