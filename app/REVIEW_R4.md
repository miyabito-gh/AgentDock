# REVIEW_R4: Git基準の「変更を戻す」（P3B-0〜4）の差分レビュー

対象: 58bf39e・87f052b（P3B-0〜2、補助）、8e7e5fe（P3B-3 控えの取得）、bb75379（P3B-4 戻す処理の作り替え・観測の廃止）。照合先: `app/DESIGN_P3B.md`（§3〜§4、回答済み Q1・Q2）、CLAUDE.md の合意。
件数: **高 3／中 4／低 6**。UI（`app/src/ui`）は対象外。

確認して問題がなかったもの（指摘なし）:
- ロックの順序。送信 = send_lock→作業キュー→リポジトリ(読)、戻す = revert_lock→FolderOpGuard→作業キュー→リポジトリ(書)、一覧・計画 = 作業キュー→リポジトリ(読)、チャット削除 = 削除印→作業キュー。循環はない。書込みロック中の C は `baseline_capture_locked` で取るので、自分で取ったロックを待つことはない。
- 強制不可の条件（GitBusy・HEAD/ブランチ変更・Skipped・フォルダ・作業フォルダ外・戻し済み・作業中）に強制は効かない（`admit` で Blocked は `failed`、計画時 Blocked→実行時 NeedsOverride は `stale`）。
- 控えの保存に失敗したら作業ツリーは変えない。成功したファイルだけを記録する。自動削除はない。`~/.codex` は変えない。受理不明のときは再送しない。
- 観測コードの削除漏れはない（`FileChangeObserved`・`TurnChangesUpdated`・`ChangeLine`・`append_change`・`backendReported` の参照はなし。`CHANGES_FILE` は使用量とテストだけ）。旧 `settings.json` は `#[serde(default)]` で読める。

---

## 高

### H1. 強制したファイルの再照合が「実行時の再計画」との比較になっており、確認の後に変わった内容を上書きする（M1違反）
- 場所: `src-tauri/src/rules/baseline.rs:513-523`（`admit` が `f.write`＝再計画の `WriteOp` を実行に使う）、`src-tauri/src/host/changes.rs:1106-1107`・`1279`
- 症状: `expect`（書込み直前に照合する sha256）を、書込みロック内の再計画で読み直した値から取っている。強制の対象（NeedsOverride）は、プレビューの後にファイルが変わっても理由が同じ（`ChangedAfter`／`EndUnknown`／`ConcurrentChange` のまま）なら受理される。その結果、ユーザーが2段目の確認で見ていない新しい内容が、照合を通って上書きされる。§4.4-5 の「計画時の `expect` と一致するか（強制分も同じ）」が強制分では効いていない（Revertible は C が変われば `ChangedAfter` が増えて `stale` になるので、この問題は起きない）。
- 再現: turn の後に a.txt を手で編集する → プレビューで a.txt が「要確認」になる → エディタでさらに a.txt を保存する → 2段目の確認で強制実行 → 2回目の編集も消える（控えには残る）。
- 修正案: `admit` で、実行に使う `WriteOp` の `expect`（と `kind`・`target`）を**保存した計画（old）のもの**にする。または `old.write.expect != fresh.write.expect` なら、そのファイルを `stale`（または `failed`「計画後に変更されたため」）にする。単体テストに「強制ファイルが計画後に変わった→書かない」を `admit` の段で追加する。

### H2. 送信前の総上限（10秒）が前の区間の E 取得を途中で打ち切ると、前の区間が開いたまま残り、次の turn の変更が前の turn の変更として「戻せる」になる
- 場所: `src-tauri/src/host/baseline.rs:432-438`（外側の `timeout`）、`448-452`（`EndNow` → `baseline_end_inner` を同じ上限の中で実行）、`615-631`（後で poll が E を取る）
- 症状: `PrevAction::EndNow` の E 取得中に総上限を超えると、内側の future が捨てられ、`open` は前の区間（`root_ended=true`、`end_in_flight=false`）のまま残る。タイムアウト側の処理は新しい区間を `Failed{Timeout}` にするだけで、前の区間は閉じない。送信が進むと、`baseline_poll` は次の turn が終わって静かになった時点で前の区間の E を取る。この E には次の turn の変更がすべて含まれる。次の turn には区間がないので、「turn1 の変更」として一覧に出て、`Revertible`（`includesTurns` も空）と判定される。E の取得と B の取得が同じ10秒を分け合うので、大きいリポジトリで終端の直後（2秒の poll より前）に送信すると起きる。
- 修正案: タイムアウト側で、`open` が自分の区間でなければ `open=None`（前の区間は終了未確認のまま）にする。あわせて `EndNow` の E は総上限の外で取る（または E 取得の打切りを `Failed{End}` として記録して閉じる）。「打ち切られた E を後から取らない」の単体テストを足す。

### H3. 同時作業の記録が「Bを取れた瞬間」と「Eの瞬間」の2点だけで、区間の途中で始まって終わった作業を取りこぼし、他の会話の変更を「戻せる」と判定する
- 場所: `src-tauri/src/host/baseline.rs:487`（B 成功時だけ）、`639`（E 時）、`595-612`（`baseline_observe` は TurnStarted を見ない）、`489-493`（B 失敗時は記録しない）
- 症状: §3.5 は「B を取るとき・**区間が開いている間に**」同じリポジトリの作業を `Concurrent` として記録する設計になっている。しかし実装は両端で一度ずつ見るだけで、次の作業を拾えない。
  - (a) 外部の会話（`ExternalRunning`）が自分の区間の途中で始まり、途中で終わった場合。
  - (b) 同じリポジトリの別チャットの送信で、B が失敗した場合（時間切れ・設定で無効・記録の失敗）。相手側でも `baseline_record_concurrency` を呼ばないので、どちらにも記録が残らない。
  - (c) 区間を作らない turn（外部から resume した会話で VS Code から送られた turn など）が途中で始まって終わった場合。

  これらの変更は自分の B→E の差に入るので、O2・O3 にも掛からず `Revertible` になり、2段目の確認なしで相手の作業を消す。
- 再現: チャットA（同じリポジトリ）で長い turn を走らせる → その間に外部（VS Code）で同じスレッドに短い turn を送って終える → A の終了後に「変更を戻す」→ 外部の turn が変えたファイルも `Revertible` になる。
- 修正案: `baseline_observe` で `TurnStarted`（どのチャットでも。外部も含む）を受けたら、そのチャットの作業フォルダと `paths_overlap` する、開いている区間すべてに `Concurrent` を追記する（相手の区間がなくても自分側には書く）。B が失敗したときも、作業フォルダ（またはリポジトリルート）で重なる開いた区間へ `Concurrent` を書く。

---

## 中

### M1. 「戻す」がE未取得の区間や、turn不明のまま動いている会話を止めない。戻した書込みが他の区間のEに入り、帰属を誤らせる
- 場所: `src-tauri/src/host/changes.rs:1006-1030`（`revert_blocker`）
- 症状:
  - (a) `busy` は `running_turn`・停止未確認・PendingOp しか見ない。状態が Running／Waiting なのに turn が分からない会話を通してしまう（`send_message` の `active` 判定、`chat_quiescent` の `is_working` と食い違う）。
  - (b) 開いている区間（終端は観測したが、E は poll 待ちで最大2秒）を止めない。自分のチャットなら、E が戻した後の状態で取られ、その turn の差分が消えたように見える。同じリポジトリの別チャットの区間なら、相手の E に「こちらが戻した書込み」が相手の変更として入る（同時作業の記録もない）。後で相手の turn を戻すと、こちらが戻したはずの変更が確認なしで元に戻る。
- 修正案: `revert_blocker` に、このチャットと同じリポジトリの他チャットで「`is_working(status.state)`」「`baseline_rt` の開いた区間がある」ものを止める条件として加える。または戻す実行時に、開いている区間すべてへ `Concurrent{External{label:"AgentDockの戻す操作"}}` 相当を書く。前者のほうが単純。

### M2. 戻す実行中の手動送信が待たない。10秒で B を諦め、戻す処理の書込み中に turn が始まる
- 場所: `src-tauri/src/host/baseline.rs:432-433`（総上限がキュー待ち・ロック待ちも数える）、`517-519`（コメント「待ち時間は数えない」と実装が合っていない）、`src-tauri/src/host/mod.rs:832-834`、`src-tauri/src/host/changes.rs:1084`（`FolderOpGuard` が止めるのはキューの自動送信だけ）
- 症状: §3.4「送信は戻すの実行が終わるまで待つ」に反する。同じチャットはキュー待ち、同じリポジトリの別チャットは読取りロック待ちのまま10秒を超え、`Failed{Timeout}` で送信が進む。`cat-file --filters`（LFS の smudge、上限120秒）が多いと戻す処理はすぐ10秒を超える。turn は控えなしで、戻す処理が書いている最中のファイルを編集し始める（ファイルごとの再照合の窓は狭いが、ゼロではない）。
- 修正案: 送信の入口（`send_message`／`dispatch_send` の前）で、`folder_op_overlaps` に当たれば「戻す処理の実行中です」と拒否する。キューの自動送信と同じ扱いにする。あるいはリポジトリロックの待ちを総上限の外にして、本当に待つ。

### M3. `EndIsNextBase` の解決先が「次に残った同じリポジトリの区間」になり、無関係の期間の変更を turn に帰属させる
- 場所: `src-tauri/src/host/baseline.rs:454-457`・`472-474`（リポジトリが違っても carry する）、`src-tauri/src/host/changes.rs:138`（Abandoned を除いた後で解決）、`573`、`850`・`1104`（リポジトリで絞った後で解決）、`src-tauri/src/rules/baseline.rs:325`
- 症状: E の代わりにした「次の B」が使えなくなると、さらに後の区間の B を E として使ってしまう。使えなくなるのは次の場合。
  - (a) その送信が受理なしで確定した（Abandoned で区間ごと除かれる）。
  - (b) 作業フォルダの変更で次の B が別のリポジトリで取られた。carry 自体がおかしく、別リポジトリの B を E としている。
  - (c) 子孫が「承認待ち」（`Waiting`）のまま長く止まっていて、その間にユーザーが手で編集したあと次を送った。設計上の前提「途切れず作業中」が崩れる。

  いずれも B_k→E_k の差に別の変更が入るが、要確認にならない。
- 修正案: carry は `prev.repo_root == 新しい root` のときだけ行う。`build_segments`／`end_point` は、`NextBase` を**記録上の直後の区間**（Abandoned でも、別リポジトリでも）で解決し、使えなければ `EndUnknown` として扱う。(c) は、子孫に `Waiting` が含まれていたら carry せず終了未確認にする（または carry した区間に常に `EndUnknown` 相当の要確認を付ける）。

### M4. 大文字小文字だけが違うパスの組（Windows）で、戻すと最後にファイルが消える
- 場所: `src-tauri/src/rules/baseline.rs:426-437`、`src-tauri/src/host/changes.rs:1142-1161`・`1272-1288`
- 症状: turn が `A.txt` を `a.txt` に改名した（内容は同じ）。計画は `A.txt`＝Restore、`a.txt`＝RemoveAdded になる。NTFS では2つは同じ実体なので、どちらの `expect` も同じ sha になる。パス順に Restore（同じ内容を書く）→ RemoveAdded（sha が一致して削除）の順で実行され、ファイルがなくなる。結果は両方とも「成功」と表示される（控えには残る）。
- 修正案: 計画で、`norm_path` が同じになる対象パスが2つ以上あれば、両方を `Blocked`（理由:「大文字小文字だけが違う名前の変更は戻せません」）にする。

---

## 低

### L1. E の記録を `chats` の std::Mutex を持ったまま行っている（ファイルI/O・fsync・`mutate`）
- 場所: `src-tauri/src/host/baseline.rs:643-647`
- 症状: ロックの順序が `chats → io_lock → data` に固定される。将来 `mutate` の中や emitter、io_lock を持つ処理から `baseline_rt` に触れるとデッドロックする。非同期ランタイムのスレッドも fsync の間ふさがる。
- 修正案: ロックの中では世代番号（または `open` の seg）の確認と「書込み中」の印付けだけにし、追記はロックの外で行う。E の作業は作業キューの中なので、切断との競合は印で判定できる。

### L2. 取り除く `GIT_*` 環境変数に漏れがある
- 場所: `src-tauri/src/gitops/mod.rs:24-44`
- 症状: `GIT_ICASE_PATHSPECS`／`GIT_GLOB_PATHSPECS`／`GIT_NOGLOB_PATHSPECS` があると、`ls-tree` の一致が変わる（大文字小文字違いのエントリが返って Absent と誤判定する）。`GIT_ATTR_SOURCE` は `cat-file --filters` の改行・フィルター変換を変える。`GIT_CONFIG_GLOBAL`／`GIT_CONFIG_SYSTEM`／`GIT_CONFIG_NOSYSTEM`、`GIT_DIFF_OPTS`、`GIT_INDEX_VERSION` も、別の設定を読ませてしまう。
- 修正案: これらを `INHERITED_GIT_VARS` に加える。`ls-tree` 側は `--literal-pathspecs` があるので、ICASE だけでも外す。

### L3. 控えの取得がユーザーの `.git/objects` の更新時刻を変える（内容は変えない）
- 場所: `src-tauri/src/gitops/snapshot.rs`（`hash-object -w`・`write-tree`）
- 症状: git は、書こうとしたオブジェクトが alternates（リポジトリ側）に既にあると、新しく書かずにそのファイル（または pack）の mtime を更新する（freshen）。設計の「objects に一切書かない」とは厳密には違い、gc の prune の判断に影響する。結合テストはファイル一覧しか比べていないので見つからない。
- 修正案: 実害は小さいので、DESIGN_P3B §1.1 の安全性の説明に「既存オブジェクトの mtime 更新は起こりうる」と書き足す。テストは一覧の比較のままでよい。

### L4. `segments.jsonl` の最終行が途中で切れていると、次の追記行と1行に混ざって読めなくなる
- 場所: `src-tauri/src/store/atomic.rs:60-72`、`src-tauri/src/store/mod.rs:406-411`
- 症状: 異常終了で最終行が欠けると、次に追記した `Concurrent`／`Reverted`／`Abandoned` も一緒に捨てられる。`Concurrent` を失うと判定が安全でない側（`Revertible`）に倒れる。
- 修正案: 追記の前に、ファイルの末尾が `\n` でなければ改行を補ってから書く。

### L5. 戻す前の控えで、全ファイルの内容を同時にメモリへ読み、io_guard を持ったまま書く
- 場所: `src-tauri/src/host/changes.rs:1218-1267`
- 症状: 最大 64MiB×件数 を同時に持つ。大きいファイルを多数戻すと、メモリ不足で落ちる（書込み前なので作業ツリーは無事）。io_guard を持つ間は、他の保存がすべて止まる。
- 修正案: 1ファイルずつ「読む→控えへ書く」の順にする（空き確認はサイズの合計を `metadata` で先に出す）。

### L6. 一覧・プレビューがチャットの作業キューを長く持ち、その間の送信の B が時間切れになる
- 場所: `src-tauri/src/host/changes.rs:844-845`・`957-958`・`1040-1041`
- 症状: 差分ダイアログを開いたまま（`ChangesUpdated` で再取得中）送信すると、C の取得と全体 diff（最大30秒）の間 B がキューを待ち、`Failed{Timeout}` になる。その turn は戻す対象から外れる。
- 修正案: 読取り専用の一覧・差分はキューを取らず、リポジトリの読取りロックだけで行う。または M2 と合わせて、B の総上限からキュー待ちを除く。
