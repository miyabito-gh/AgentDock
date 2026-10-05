# REVIEW R2: 段階② タスク横断レビュー（P3・P6中心、P2・P5・P7との交差）

対象: `git diff 6ae7dfd c3ff603 -- app/src-tauri/src`（master c3ff603）。行番号は c3ff603 時点。コードは修正していない。

## 不具合一覧（重要度順）

### 高

**H1. 窓の移動・リサイズでtokioランタイム外から `tokio::spawn` が呼ばれ、パニックする（P6×P2）**
- 場所: `lib.rs:88` → `win/window.rs:129-134 note_bounds` → `host/lifecycle.rs:171-184 set_window_bounds` → `host/persist.rs:288-305 schedule_save`（296行の `tokio::spawn`）
- 起きること: `on_window_event` はメインスレッド（tokioランタイム外）で届く。`lib.rs:44` と `lib.rs:133` のコメントでも、ランタイム外であることは把握済み。保存先が有効なら `schedule_save` が `tokio::spawn` を呼び、「no reactor running」でパニックする。起動時に `restore_main` が位置を適用した直後や、ユーザーが窓を動かしたときに、アプリごと落ちる可能性が高い。落ちると保存の書き切り（flush）も通らない。
- 修正案: `note_bounds` は `tauri::async_runtime::spawn` で包んでから `set_window_bounds` を呼ぶ。あるいは `schedule_save` 側で `Handle::try_current()` を確認し、ランタイム外なら `tauri::async_runtime::spawn` を使う（`notifier.rs:110` と同じ扱い）。実機で、窓の移動・リサイズ・最大化を確認する。

**H2. 切断・強制終了後も `running_turn` が残り、そのチャットが送れない・キューが止まったままになる（P6×P3×状態）**
- 場所: `host/state.rs:461-475`（Disconnected 時に `running_turn` を消していない）、`state.rs:277-284`（`set_live` は追加だけで、非作業状態でも削除しない）、`host/mod.rs:625-640`、`host/queue_driver.rs:248-266`、`host/lifecycle.rs:439-467`
- 起きること: 強制終了（またはApp Serverの異常終了）→再接続→resume の後も、死んだturnが `running_turn` に残る。その結果、次の4つが起きる。
  - `send_message` の新しいturnは「実行中です」で拒否される。追加指示も死んだturnへ送られて拒否される。
  - `set_chat_cwd` は ChatBusy になる。
  - 走査ループが「実行中」と判断し続け、最大30分読み取りを続ける。
  - キューの `awaiting` は、ルートの状態が Running のまま（切断では状態を変えない）なので、`settle_awaiting` が履歴を読みに行かず、ParentWorking のまま残る。run は Active なので `resume_queue` も「すでに有効」で拒否され、ユーザーには解く手段がない（アプリ再起動まで）。
- 修正案:
  - 監視元の Disconnected で `running_turn` と `latest_turn_start` をその監視元分だけ消す。状態と鮮度は今のままにし、停止記録の証拠にもしない。
  - `set_live` で状態が作業中でなければ `running_turn` を消す。
  - `settle_awaiting` は「状態が作業中」でも鮮度が live でなければ履歴を読む。終端がなく、強制終了で OS実体確認済みなら、キューを `Stopped`（停止原因は強制終了）にして `awaiting` を外す。完了扱いにはしない。

**H3. notLoaded（Unknown）のチャットを「作業中」と数えるため、削除が永久に保留・アーカイブが同期されない・終了確認に全チャットが並ぶ（P6×P7×変換）**
- 場所: `codex/convert.rs:142`（notLoaded は常に Unknown）、`host/lifecycle.rs:66-69 work_of`（Unknown を `has_unfinished` に含める）、`host/manage.rs:304-323`、`rules/manage.rs:55-63`、`host/manage.rs:169`、`host/lifecycle.rs:441`
- 起きること: 再起動後は、ロードされていないチャットのルート・子孫はすべて Unknown になる。
  - 削除: `StopFirst` → 中断の対象がないため Err → 11.5秒待っても Unknown のまま → `Pend(StopUnconfirmed)` が保存される。`refresh_delete_pending` も Unknown を理由に `StopUnconfirmed` を保ち続けるので、削除できない。加えて削除保留が送信を止めるので、そのチャットは送信もできなくなる。
  - アーカイブ: `archive_ready` が成立しないので、`thread/archive` が永久に送られない。
  - 終了: `request_quit` で一覧に読み込んだ全チャットが「作業中」として並ぶ。
  - 強制終了: 影響チャットにも全チャットが並び、関係のないチャットにも OsProcess の停止記録が作られる（`lifecycle.rs:105-124`）。
- 修正案: T1設計の「notLoaded→unknown（履歴で終端があれば turn scope で補完）」を `work_of` 側で実装する。Unknown でも、履歴で最新turnの終端（完了・失敗・中断）を確認でき、進行中のturnがなければ「未終端なし」とする（キューの `refresh_terminal_facts` と同じ根拠を共用する）。最新turnが進行中のまま、または読めないときは、外部実行の可能性があるので `OwnershipUnknown` で保留し、理由を表示する。削除と終了の確認には、確認できた根拠を出す。

**H4. wake後に鮮度が live へ戻らず、キューが止まったままになり「キューを再開」も拒否される（P6×P3）**
- 場所: `host/lifecycle.rs:514-556`、`host/state.rs:251-261`（読み取りは HistoryOnly）、`host/queue_driver.rs:720-726`（run が Active なら AlreadyActive）、`host/mod.rs:603-608`
- 起きること: sleep のたびに、live のエージェントがすべて HistoryOnly になる。その後 live の通知を受けても、鮮度を Live に戻す経路がない（`set_live` は resume・開始だけ）。その結果、run=Active のキューは NotLive で止まり続け、ユーザーが「キューを再開」を押しても「キューはすでに有効です」で拒否される。さらに、再開済みの外部作成チャットは `external_send_locked` に掛かり、通常の送信も RunningElsewhere で止まる（もう一度再開が必要）。DESIGN_P2 §5 の「接続が生きていれば live に戻す」とも食い違っている。
- 修正案: 下の「要判断A」を参照。少なくとも次の2点は必要。
  - `resume_queue` は、run が Active でもルートが live でなければ、`ensure_live` を行って受け付ける（ユーザー操作の中だけで resume する）。
  - wake後の照合で、同じ監視元の接続が続いており、照合の後に live 通知を受けたエージェントは Live へ戻す。

### 中

**M1. 終了手順でキュー送信を保留しない。終了確認の対象一覧も古いまま使う（P6×P3）**
- 場所: `host/lifecycle.rs:292-298`（`StopAndQuit` は `request_quit` 時点の `busy` を使う）、`host/queue_driver.rs:130-138`（`gate_input` に終了中の条件がない）、`lifecycle.rs:376-411`
- 起きること:
  - Confirming の間にキューが別チャットで新しいturnを送ると、そのチャットは中断も監視もされない。それでも Stopping から Flushing へ進み、`exit` まで行く（実行中のturnを停止確認なしで終了させる）。
  - 送信待ちだけが理由で busy になっていたチャット（親が idle）は、中断の対象がないので記録が作られない。Stopping・Flushing の間に条件がそろえば、キューが送信し、そのまま終了する。
  - 正本 §3.4 の停止手順の第1段「送信保留」がない。
- 修正案: QuitPhase が Idle 以外のときは、`gate_input` で Hold（例: `QueueHold::Quitting`）にする。`StopAndQuit` で busy を取り直す。Flushing へ進む直前にも `busy_for_quit` を再評価し、新しい作業があれば Confirming へ戻す。取消し（Idle）で Hold を外す（run は変えないので、勝手に止まったり再開したりしない）。

**M2. 手動送信とキューの自動送信が同時に `turn/start` を送りうる（P3）**
- 場所: `host/mod.rs:616-658`（キューの状態を見ない）、`host/queue_driver.rs:198-233・358-389`（手動送信中であることを知らない）
- 起きること: 送信待ちがあり run=Active のチャットで、ユーザーが手動送信すると、`ensure_live`（resume）で親が Live になる。それを受けてキューの評価が `Send` を返すと、2本の `turn/start` が並行して送られる。「1件ずつ送る」という規則が崩れ、順序も逆転する。同じ依頼の二重送信ではない。
- 修正案: チャット単位の送信ロック（async Mutex）を、`dispatch_send` と `run_entry_send` で共有する。あわせて、手動送信の受理から TurnStarted を観測するまで、キュー側は ParentWorking として保留する。

**M3. キューの Sending・awaiting を「登録だけ」と見なして削除を進める（P7×P3）**
- 場所: `host/manage.rs:305-323`（`queue_pending` だけなら Proceed）、`host/lifecycle.rs:75-79`（`queue_pending` に Sending と awaiting を含む）、`host/queue_driver.rs:137`（Hold の条件が `delete_pending` だけで、`manage_rt.is_deleting` を見ない）
- 起きること: `turn/start` の送信中、または受理直後で TurnStarted を観測する前は、`has_unfinished=false` のまま `thread/delete` が実行される。合意「停止を確認できないまま削除しない」に反する。また `delete_chat` の開始から `set_delete_pending` を保存するまでの間は、キューの Hold が効かない。
- 修正案: 削除の判定では、Sending と awaiting を `has_unfinished` と同じ扱い（StopFirst）にする。「キューだけ」とみなすのは Waiting だけにする。`gate_input.delete_pending` に `manage_rt.is_deleting(chat)` を加える。

**M4. 強制終了で消滅を確認できなかったときの停止記録が、削除・送信を止めない（P6×P7）**
- 場所: `host/lifecycle.rs:107`・`110-118`（`interrupt_requested_at=None` なので NotRequested になる）、`backend/model.rs:907-911`（NotRequested は `blocks_deletion` の対象外）
- 起きること: 警告文では「停止未確認のまま扱います」と伝えるが、作られた記録は `open_stop` に掛からない。終了手順の記録としても確認待ちに入らない。削除・送信・`set_chat_cwd` の停止確認として機能しない。
- 修正案: 強制終了の要求時刻を証拠に記録する（`interrupt_requested_at` を流用するか、`kill_requested_at` を新設する）。10秒経っても確認できなければ Unconfirmed とし、削除・送信を止める側に倒す。

**M5. 監視活動（`activity.jsonl`）を書く経路がない（P2×P7）**
- 場所: `store/mod.rs:300 append_activity`（呼ぶのはテストだけ）、`host/persist.rs:338`、`host/manage.rs:493-503`
- 起きること: DESIGN_P2 §1.3 の「取得した監視活動を追記保存」がされていない。エクスポートの「監視活動履歴も含める」は常に空になる（正本 §3.12・M41）。「取得できなかった部分は未取得と書く」にもならない。DEFERRED.md にも記載がない。
- 修正案: `mutate` で確定したイベント（発見・状態変化・turn終端・確定したitem・鮮度変化）を、チャット単位で追記する経路を作る（spawn_blocking＋io_lock）。実装しない場合は DEFERRED.md に記録し、エクスポートに「未保存」と明記する。

### 低

- **L1. wake後の走査で通知が出る（P5×P6）**: `lifecycle.rs:540-555` は自分の読み取りの通知だけを捨てる。その後の `start_scan`（`mod.rs:365-377`）の読み取りで、sleep 中に終わった子が「非終端→終端」として通知される。また on_resume で読めなかったエージェントは後で通知される。DESIGN_P2 §4 の「wake後の照合では通知しない」に反する。wake からの照合が完了するまで、その対象の `nonterminal_seen` を消す（または読み取り元の印を付けて `note_end` を抑止する）。
- **L2. 取り消したキュー項目を、再送で手動送信として送れる（P3）**: `queue_driver.rs:704-717` の cancel は `rejected` から試行を消さない。`mod.rs:670-678` では、NotAccepted の項目が見つからないと `dispatch_send` で本文を送る。cancel のときに、その項目の NotAccepted 試行を `rejected` から外す。
- **L3. キュー項目の再送が保存失敗で止まると、再送権が消え、項目が Waiting に戻る（P3）**: `mod.rs:670` で `rejected` から取り出した後、`run_entry_send` が保存失敗で `abort_send` すると、`rules/queue.rs:276-284` が Waiting に戻す。同じ試行の再送はできなくなり、「キューを再開」で自動送信の対象に入る。`abort_send` で元の状態（NotAccepted）に戻し、`rejected` にも戻す。
- **L4. 送信前の保存が失敗し続けると、2秒ごとに警告が出続ける（P3×P2）**: `queue_driver.rs:405-417`。保存失敗の間は `Hold(SaveFailed)` にして、`retry_save` の成功を待つ。
- **L5. 削除保留中でも「キューを再開」で resume する（P3×P7）**: `queue_driver.rs:720-734` に `check_not_delete_pending` がない（送信は Hold されるが、停止未確認のチャットを resume する）。最初に確認を入れる。
- **L6. 手動送信は送信前に記録を保存しない（P3×P2）**: `mod.rs:682-700`。送信中に落ちると、再起動後に受理不明の記録が残らず、照合も送信の抑止も効かない（再送はユーザー操作だけなので、無条件再送にはならない）。`unresolved_sends` へ Sending 相当を先に保存する。
- **L7. §2.3: 保存形式にCodex固有の名前が入っている**: `backend/local.rs:457` の `codex_executable`（AppSettings として settings.json・UIに出る）。`host/manage.rs:30・378・384` の `BACKEND_DONE`「Codexの履歴の削除」は、表示文をそのまま chat.json の工程の印として保存・照合している。それぞれ `executables: {backend → path}`、および列挙値（例: `DeleteStep::BackendHistory`）に分け、表示文はUIで作る。

## 問題なしと確認した観点

- ロック順序: `io_lock → data`（`persist.rs:321`・`manage.rs:431-441`）、`quit → data`（`lifecycle.rs:240-260`）、`notifier.inner` は data を持たずに取る。`unresolved`・`unconfirmed`・`queue_rt.*`・`scanning` は互いに入れ子にせず、data ロックを持ったまま await する箇所もない。`mutate` → `notify_submit` は data を解放してから呼ぶ。`spawn_blocking` 内の `mutate`（`remove_area`）も、同じロックを再取得しない。デッドロックの経路は見つからない。
- Sending を保存してから送る順序: `run_entry_send` は `save_now` の Saved を確認してから送り、失敗時は送らない（保存先が使えない起動だけは例外で、起動時に警告を出す）。`begin_entry_send` のときの状態を保存するので、送信中に落ちても再起動後は AcceptanceUnknown になる。
- 受理不明: 時間で打ち切らず、切断中は止め、再接続後に再開する。照合中も `unresolved` が送信を止める。NotFound の後はユーザーの `retry_send` だけ。`retry_send` は受理不明の試行を拒否する。照合が自動と手動で二重に走ることは `Busy` で防いでいる。
- 再起動復元: `restore_after_restart`（Active→PausedAfterRestart、Sending→AcceptanceUnknown）、`Copying`→CopyFailed。削除保留は ReadyForUserRetry でも自動削除しない。アーカイブ待ちは証票がないので送らない。windows.json は位置だけ。いずれも自動で送信・削除・archive・resume をしない。
- 終了取消し: 送信済みの中断は取り消せないが、中断した turn の終端でキューは Stop になり、明示の再開まで進まない。取消しの後に古い進行taskが終了させることは、世代番号で防いでいる。
- 強制終了: `UserConfirmed` が必要、App Server単位、影響チャットを列挙する。`active_processes()==0` を確認できたときだけ OS実体の証拠を付け、時間経過で確認にはしない。`KILL_ON_JOB_CLOSE` は付けていない。
- 監視目的の resume・承認代行・停止はない（走査・照合・wake・キューの評価は読み取りだけ。`ensure_live` はユーザー操作の中だけで呼ぶ）。
- 通信断を done・failed にしない（切断では鮮度だけを変える。読めない場合は据え置き）。
- 通知: 承認・質問は即時、終端は2秒の固定窓。再起動後に一斉通知しない。
- §2.3: IPC・保存形式の識別子は ChatKey・AgentKey・LocalId で、Codexのwire型は出ていない（例外は L7）。

## ユーザー判断が必要な事項

**A. wake後に live へ戻す条件（H4）**: 正本 §3.11 は「再照合してから購読可否を判断」、DESIGN_P2 §5 は「接続が生きていれば live に戻す」としているが、実装（ebdbebc）は「戻さない」を選んでいる。
- (1) 同じ監視元の接続が続いていて、照合の読み取りが成功し、その後に live 通知を受けたエージェントは Live に戻す。あわせて `resume_queue` は run=Active でも、親が live でなければ `ensure_live` で受け付ける（**推奨**。DESIGN_P2 の記述どおりで、自動の resume はしない）
- (2) wake後は live に戻さず、`resume_queue` を run=Active でも受け付けるようにするだけ。sleep のたびにユーザーが「キューを再開」を押す必要がある
- (3) 現状のまま（キューと外部チャットの送信が止まったまま。非推奨）

H3（notLoaded の扱い）は、T1設計の「履歴で終端があれば補完」に沿って直せばよく、ユーザー判断は不要。

## 決定（2026-10-06 ユーザー合意）
- H4: (1) 接続が続き照合が成功し、その後にlive通知を受けたものだけLiveに戻す。自動resumeはしない。「キューを再開」は親がliveでなければ受け付け、ユーザー操作としての再開処理を行う。
