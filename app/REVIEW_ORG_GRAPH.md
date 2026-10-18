# REVIEW_ORG_GRAPH — グラフ定義（G-A）実装の差分レビュー（R-G）

- 対象: `git log 0d7cd7d..HEAD`（550cfba G-A0、1d4433c G-A1、19651fc G-A3、f007d99 G-A2、fcac3a0 G-A4、90386c0 G-A5）
- 設計: `app/DESIGN_ORG_GRAPH.md`（§7・§9.1・§10・§12・§13）
- 実行したテスト: `cargo test --manifest-path app/org-engine/Cargo.toml`（145件すべて合格）、`cargo test --manifest-path app/src-tauri/Cargo.toml --lib host::org`（84件すべて合格）
- 再現確認: org-engine を作業用フォルダへ複製し、テストを足して実行した（リポジトリは変更していない）。H1・M2・L1 は実際に再現した。

## 要約表

### G-A6 で直すべき（高・中）

|#|重大度|箇所|内容|
|---|---|---|---|
|H1|高|`org-engine/src/graphrun.rs:643-663`（`types.rs` `seen_seq`）|入れ子の周回で、内側の周回が上限（ask／fail）に達したのに、下流の待機中ノードが ready になって turn を送る（**再現済み**）|
|M1|中|`graphrun.rs:470-489`、`OrgPane.tsx:490`|ask 上限で S を reviewing に残す判断は状態表とは整合する。ただし「この結果で受け入れる」がなく、T を取り消すと S が reviewing のまま残り、ワークフローが完了しない。設計 §7.3 の記述とも違う|
|M2|中|`graphrun.rs:539-544, 702-709`、`persist.rs:664`、`driver.rs:278`|gate／router が running の間に落ちると、再起動後に要照合のまま動かなくなる（**再現済み**）。gate は判断を記録済みなので、もう一度選ぶこともできない|
|M3|中|`core.rs:380`、`host/org/mod.rs:585-608`|開始時の写しが使えないまま復元すると、graph なしのグラフモードになる。出力の解析が「形式違反」になり、形式修正 turn を送ったうえで failed{成果物不正} に変わる|
|M4|中|`host/org/ports.rs:243, 279`、`mod.rs` 復元部|再起動後の承認方針（untrusted／onRequest）が、開始時の写しではなく今の役割ファイルから取られる。sandbox は写し、承認方針は今のファイルという混在になる|
|M5|中|`graph/validate.rs:338-386`（V8）、`graphrun.rs:243`|V8 が役割の `workplace` を見ていない。detachedAtCommit の役割を target なしで使う定義や、ownWorktree で readOnly の役割を使う定義が通り、利用者の作業フォルダを cwd にして動く（設計 §8.1 違反）|
|M6|中|`graphrun.rs:199-245, 250-261`|同じ周回の本体に書込みノードが2つあると、2周目の後ろの書込み役が前の書込み役の新しいコミットを取り込まない（最初の起点のまま）。成果物のブランチから修正が黙って落ちる|
|M7|中|`graphrun.rs:538`、`host/org/ipc.rs:41`|gate の理由（note）を切り詰めずにスロット値へ入れている。長い理由を書くとスロットの書込みに失敗して `persist_failed` になり、ワークフロー全体が止まる。資料欄では 4,000 字の上限も効かない|
|M8|中|`org-engine/src/prompt.rs:11-14`|`quote()` が USER NOTE の区切りを無害化していない。モデルが出力したスロット値に `-----BEGIN USER NOTE-----` を入れると、次の役割から見て「利用者の要望」に見せかけられる|

### 持ち越し（低・G-B 以降）

|#|箇所|内容|
|---|---|---|
|L1|`validate.rs:286`（V4）|自己ループ（S→S のフィードバック）が検証を通るが、必ず「戻り先が差し戻せる状態ではありません」で止まる（**再現済み**）|
|L2|`graphrun.rs:470-489`|上限到達の分岐で戻り先の状態を確認していない（ask で戻り先が reviewing 以外なら、何も止めずに S だけが reviewing で残る）。fail では本体の途中のノードが reviewing のまま残る|
|L3|`host/org/graphs.rs:265-270`|`roles.json` の `widened`・`confirmed` を検証せずに使っている（指紋が覆うのは def だけ）|
|L4|`validate.rs`|検証の小さな穴: gate の `shows` のスロットがあるか、条件のスロットが自分か祖先の出力か、本体に agent が1つもない周回、`in` が全値を含む条件|
|L5|`core.rs:588-595` ＋ `graphrun.rs:577-586`|出力のないグラフのタスクを手動で完了にすると、辺の選び直しで無条件の辺を進み、次のノードが「未取得」の資料で動き出す|
|L6|`host/org/mod.rs`（開始）|`standard.json` を編集しても無視され、組込みの経路で動く。開始ダイアログには、その編集内容が出る|
|L7|設計 §7.5 末尾|テンプレート1の成果物要約をスロットへ保存する項目が未実装|
|L8|`graphrun.rs:494-506`|feedback の途中で落ちたとき、照合後の再処理で数え過ぎることがある（設計どおり安全側。記録のみ）|

---

## 1. 合意違反の確認（観点1）

|合意|結果|
|---|---|
|受理不明で再送しない|変更なし。`send_turn` だけが送り、`graph_complete_slot`・`settle_node` は `Confirmed` のときだけ動く（`graphrun.rs:347, 398`）。テスト `no_edge_is_chosen_for_an_unconfirmed_task` で確認|
|自動 resume しない／承認要求に応答しない|新しい経路に resume・承認応答はない。gate は `UserGate`（`UserConfirmed`）だけで決まる|
|自動マージしない|`add_branch_worktree_from_commit` は `agentdock/org/` のブランチを新しく作るだけ。merge・push・reset はない|
|終了・通信断を cancelled/failed/completed にしない|新しい経路では守られている。ただし M3（写しが使えないときに failed{成果物不正} へ変わる）は例外|
|`~/.codex` を書かない|`graphs.rs`・`store/org.rs` が書くのは `%LOCALAPPDATA%` の配下だけ|
|利用者テキストの区切り|`quote_note` の区切り・4,000 字・注記は実装されている。穴は M7（gate の note が切られない）と M8（資料の側から利用者の要望を装える）|
|周回×役割の上限で止まる|V11 と turn 上限は効いている。ただし H1 で、上限到達後に下流が余分な turn を使う（turn 上限の内側には収まる）|

## 2. 並行・永続化（観点2）

### 2.1 順序とクラッシュ点

|落ちた位置|復元後|判定|
|---|---|---|
|PutSlot の後、SlotWritten の前|版は進んでいない。次の書込みが同じ版を上書きする|問題なし|
|SlotWritten の後、状態変更の前（agent）|running → 要照合。照合後に再処理され、新しい版を書く|問題なし（重複した版が残るだけ）|
|前向きの辺: 状態変更の後、ActivationAdded または EdgeTaken の前|`edge_pending` → パス A が保存したスロットから選び直す。既にあるタスクは再利用する|問題なし（テストあり）|
|feedback: EdgeTaken または FeedbackCounted の後、状態変更の前|S は running → 照合後に再処理されて、もう一度数える|数え過ぎるだけ（L8）。数え足りなくなることはない|
|feedback: T が rework になった後、ready になる前|監査で `blocked{要対応}` にする|止まるが、利用者が戻せる手段がない（再試行は送信済みのため不可）。G-A6 で、監査の代わりに rework→ready を決定的に行ってよい（再起動後は Halted{Restart} で、再開まで送られない）|
|gate／router が running の間|**要照合のまま動かない**|M2|
|ask の上限: T を blocked にした後、EdgeTaken の前|S が running なら照合後に再処理、reviewing ならパス A が EdgeTaken だけを記録する|問題なし|

### 2.2 ask の上限で verify を reviewing に残す判断（M1）

- 状態表（`state.rs` ALLOWED）には Completed→Rework がない。「もう1周」で S を Rework に戻すには S が Reviewing でなければならないので、この判断は状態表と整合する。
- 問題は次のとおり。
  1. 設計 §7.3・§7.4 の「S は completed」「この結果で受け入れる」と実装が違う。画面のカード（`OrgPane.tsx:490`）は「もう1周か、取り消すか」だけを出している。
  2. T を取り消すと（`user_cancel`）、S は reviewing のまま誰にも動かされない。`graph_done` は false のままで、`WorkflowCompleted` に届かない。
- 修正案: `UserAcceptRound { task }`（`UserConfirmed`）を足す。T: Blocked(RoundLimit)→Running→Completed（Cause::User）。S: Reviewing→Completed。S から `EdgeTaken{edge: None}` を記録し、`take_forward` と同じように本体を閉じる。設計 §7.3 は「S は reviewing（もう1周に備える）、受け入れたら completed」に書き換える。

## 3. 指摘の詳細

### H1（高）内側の周回が上限に達した後、下流の待機ノードが動き出す

- 箇所: `graphrun.rs:643-663`（パス B）。`types.rs` の `seen_seq()` は `max(last_output_seq, edge_seq)`。
- 原因: パス B は「親 D が Reviewing／Completed で、`D.seen_seq > M.seen_seq`」なら M を ready にする。上限到達（ask・fail）の分岐でも `EdgeTaken{to: None}` が記録され、D の `edge_seq` が進む。そのため、D が合格していなくても条件を満たしてしまう。
- 再現条件: plan→build→test（S1、build へ戻る feedback、上限1、ask）→review（S2、plan へ戻る feedback）。
  1. review が replan を返す。plan・build・test・review が rework になる（review は待機）。
  2. その後、test が rework を2回返す。
  3. build は blocked{周回上限}、test は reviewing になる。**ここで review-1 が Running になり、turn を1回送る**（期待は Rework のまま）。
- 実行したテスト（作業用の複製に追加したもの）:

```rust
sim.complete("plan-1", SPEC); sim.complete("build-1", REPORT);
sim.complete("test-1", PASS); sim.complete("review-1", REPLAN);
sim.complete("plan-1", SPEC); sim.complete("build-1", REPORT);
sim.complete("test-1", REWORK); sim.complete("build-1", REPORT);
sim.complete("test-1", REWORK);
// build-1 = Blocked(RoundLimit), review-1 = Running（送信 1→2）
```

- 影響: 上限で止まるはずの流れが、下流で余分に turn を使う。review が replan／pass を返すと、周回上限のカードが2枚になったり、外側の本体を閉じようとしたりする。gate が待機しているときは、同じ経路で判断カードが早く開く。
- 修正案: 「D の最新の EdgeTaken が M へ向かう前向きの辺である」ことを条件にする。たとえば `Task` に `edge_to: Option<TaskId>` を足し、`EdgeTaken` の apply で記録する。パス B は `d.edge_to == Some(m) && d.edge_seq > m.seen_seq` にする。上のテストを回帰テストとして残す。

### M2（中）gate／router が running のまま落ちると、再起動後に動かない

- 箇所: `user_gate`（`graphrun.rs:535-544`。GateDecided→Running→PutSlot/SlotWritten→settle の順）、`run_router`（`702-709`）。`classify_after_restart`（`persist.rs:664`）は running を要照合にし、免除するのは reviewing の `is_idle_reviewing` だけ。照合（`driver.rs:278`）はチャットのないタスクを飛ばす。
- 再現条件: gate で「ok」を選んだ直後、`StatusChanged{confirm-1→Running}` の後で落ちる。復元して再開すると `Running / NeedsReconcile(Restart)` のまま止まる（再現済み）。GateDecided が記録済みなので `user_gate` も受け付けない。
- 修正案: 送信をしないタスク（`node.is_some() && last_attempt.is_none()`）は、running でも再分類しない。監査で、gate は SlotWritten があれば settle、なければ AwaitingApproval に戻す（Running→Blocked は状態表で許されている）。router は settle をやり直す（純関数なので同じ結果になる）。

### M3（中）写しが使えないまま復元すると、形式違反から failed に変わる

- 箇所: `core.rs:380`（`restored_with` は set_graph の失敗を捨てる）、`mod.rs:585-608`（写しが使えないとき graph=None で起動する）。
- 再現条件: `graph.json`／`roles.json` が欠けているか指紋が違う状態で、running のタスクを照合し、利用者が再開して turn の終端が届く。`parse_output`→`graph_parse_output` が Err（「グラフ定義がありません」）を返し、`parse_failed` が形式修正 turn を送る。2回目は failed{成果物不正} になる。
- 修正案: `is_graph_mode() && graph.is_none()` のときは、新しい `HaltReason::DefinitionUnavailable` で止める。`user_resume` はこれを解除しない。parse は形式修正へ進めず、`blocked{要対応}` にする。

### M4（中）再起動後の承認方針が開始時の写しと違う

- 箇所: `ports.rs:279` は `self.org_roles_report()` を見る。復元では `mod.rs` がここに今の役割ファイルを入れ、エンジンには写しを渡している。
- 再現条件: untrusted の役割でワークフローを開始する。役割ファイルを onRequest に戻して再起動し、その後に役割チャットを作る（新しいノードが動くとき）。
- 修正案: 承認方針を `RoleCfg` から `RoleChatSpec.approval` として渡し、ホストはそれを使う。写しの値が唯一の出どころになる。

### M5（中）V8 が役割の作業場所を見ていない

- 箇所: `validate.rs:338-386`。`RoleInfo` は `writes` しか持たない。
- 再現条件: `verifier`（detachedAtCommit）を target なしで使う定義が検証を通り、`graph_launch` の `(None, None)` で `begin_chat(id, repo)` に進む。読取り専用だが、検証対象を固定していない。
- 修正案: `RoleInfo { writes, workplace }` にし、設計 §8.1 の規則を V8 に足す。detachedAtCommit なら target が必須。ownWorktree なら書込み役で、capture が必須。sharedReadOnly なら target も capture もなし。

### M6（中）周回の中の2つ目の書込み役が古い起点のまま

- 箇所: `graph_launch`（has_chat のときは `continue_launch` だけ）、`writer_base`（作成時に1回だけ見る）。
- 再現条件: plan→A（書込み）→B（書込み）→verify。verify→A の feedback で、2周目の A のコミット A2 が B の worktree（A1 が起点）に入らない。verify は B のコミットを見るので、A2 を確かめていない。
- 修正案（G-A では簡単なほう）: 同じ周回の本体の中で、書込み役の下流に別の書込み役を置く定義を V7／V8 で誤りにする。G-B 以降で取り込み（rebase／merge）を設計する。

### M7（中）gate の理由が切り詰められていない

- 箇所: `graphrun.rs:538` の `json!({ "choice", "note": note.unwrap_or_default() })`。`add_note` は 4,000 字で切るが、スロット値には元の全文が入る。
- 再現条件: 256KB を超える理由を書く。`FileOrgStore::put_slot` が失敗し、`persist_failed` で以後の記録と副作用がすべて止まる。それより短くても、feedback の資料（`graph_ready_turn` の feedback 欄）に 4,000 字を超える全文が入る。
- 修正案: スロット値の note を `NOTE_MAX_CHARS` で切る（`add_note` と同じ値）。IPC（`clean_note`）でも上限を確かめて拒否する。

### M8（中）資料の側から利用者の要望を装える

- 箇所: `prompt.rs:11-14` の `quote()` が置き換えるのは MATERIAL の区切りだけ。
- 再現条件: verify の findings（モデルの出力）に `-----BEGIN USER NOTE-----` と `-----END USER NOTE-----` で囲んだ文を入れる。次の build の turn では、MATERIAL の中にその文が並び、「利用者からの要望」の区切りと同じ文字列になる。
- 修正案: `quote()` でも `-----BEGIN USER NOTE`／`-----END USER NOTE` を `quote_note` と同じ規則で無害化する。テストを足す。

### L1〜L8（持ち越し）

- L1: V4 で `ed.from == ed.to` を誤りにする（または、S が running のままでも戻せるようにする）。
- L2: ask・fail のとき、戻り先が Reviewing でなければ `edge_unresolved` にする。fail のときは本体の途中の reviewing を閉じる扱い（failed の後の表示）を決める。
- L3: `load_copies` で `load_role(def, stem, Some(copy.permission_fingerprint))` を通し、`widened`・`confirmed` を計算し直す。
- L4: V12 で `shows` のスロットを確かめる。V9 で、条件のスロットが「自分か祖先の output」であることを確かめる。V4 で、本体に agent が1つ以上あることを確かめる。
- L5: グラフのタスクを手動で完了にするときは `EdgeTaken{edge: None}` を同時に記録し、自動で進めない（続きは利用者の操作にする）。
- L6: 開始ダイアログで、`standard` は組込みの経路で動き、ファイルの編集は反映されないことを明記する。
- L7: 設計 §7.5 末尾のテンプレート1のスロット保存は G-B 以降へ送る。
- L8: 設計どおり（安全側）。記録のみ。

## 4. グラフ検証の抜け（観点3）

- 静的な上限（V11）: 前向きの部分が木（V5）で、周回はフィードバックの辺でしか起きない（V3）。回数はワークフロー全体で数え、戻さない。このため、活動数は `ノード数 + Σ本体×maxRounds` 以下になる。router だけの周回も maxRounds で止まり、drive は32回までで打ち切る。**静的に上限を計算できない定義は見つからなかった。**
- 条件なしのフィードバック（V4）は誤りになる。`in` が全値を含む実質無条件のもの（L4）は残るが、上限内に収まる。
- 読取り役への capture は V8 で誤りになる。穴は workplace と target の整合（M5）、自己ループ（L1）。

## 5. FromCommit の worktree（観点4）

- ブランチ名は `org_branch_name(wf, task)`、つまり `agentdock/org/<wf>/<task>`。`add_branch_worktree_from_commit` が接頭辞、置き場所の内側、既存のパス、実体の親フォルダを確かめる。既存のブランチは git が拒否する。所有記録は作成の前に保存する。利用者のブランチ・他人の worktree には触れない。テストで確認した。
- 起点のコミットはエンジンの `Committed` 記録の値で、`resolve_commit` で解決できなければ作らない。問題は見つからなかった（起点が古くなる M6 は別の問題）。

## 6. 旧 v1 の復元と写しの指紋（観点5）

- v1 の行（`GraphBound` なし）は `standard` として復元する（テスト `legacy_v1_events_restore_as_standard`）。G-A0 の切出しも見比べた。`committed`・`launch`・`ready_turn`・`parse_output`・`complete_verdict`・`dep_ok` は、グラフ以外の経路で旧コードと同じ。変わったのは、再計画の理由（note）を渡すようになった点だけで、これは意図した追加。回帰は見つからなかった。
- 写しは `GraphBound.fingerprint`（グラフ）と `roles_fingerprint`（役割の def）で照合し、合わなければ使わない。照合そのものは正しい。穴は M3（使えなかったときの動き）、M4（承認方針の出どころ）、L3（`widened`・`confirmed` を検証していない）。

## 7. G-A5（UI）の安全面

- 状態を変える `org_gate_decide`・`org_extra_round`・`org_replan` は、どれも `UserConfirmed::from_user_command()` を通る。画面にも確認ダイアログがある。ホストが事前に確かめ、エンジンも同じ確認をもう一度する。`org_open_graph_file` は `.json` の素のファイル名だけを受け、グラフ定義のフォルダ内に限る。`org_reload_graphs` は動いているワークフローがあれば拒否する。確認を通らない状態変更は見つからなかった。
