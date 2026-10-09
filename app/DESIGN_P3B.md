# P3B 設計メモ: #2「変更を戻す」をGit基準の控えへ置き換える

対象: 正本 §3.14 #1・#2、合意（2026-10-07 #2）、§3.14.1、§13.5、守ること（§13.3）。前提は `DESIGN_P3.md` §1 #1・#2・§2、`REVIEW_R3.md` H2・M1・M2・M3・L6。本書の内容はすべて**設計案**。
ユーザー決定:
- 2026-10-10: 戻す方式を「Codex報告の差分の逆適用＋ハッシュ照合」から「Git基準の控え」へ**置き換える**（併用しない）。
- 2026-10-10 Q1＝(2): 帰属を判別できないファイルは、止めて理由を出したうえで、**2段目の確認で「別の変更も含めて控えの内容に戻す」をファイル単位で選べる**（§4.3・§4.4）。**合意（2026-10-07 #2「推定で戻さない」）の変更**。
- 2026-10-10 Q2＝(2): 「Codex報告」タブを**廃止**し、差分表示は「控え基準」と「Git（HEAD比較）」の2タブ。**#1 の合意（出所「Codex報告」の表示）の変更**。

背景（事実・実測）: Windows版Codexはファイル編集をPowerShellコマンドで行うことが多く、`fileChange` として観測されないため、現行方式では戻せず「Codex報告」も空になる（LIVE 1-1〜1-3、§13.5）。

**この設計で変わること（要約）**
- ユーザーの送信でturnを始める直前に、作業フォルダのGitリポジトリの状態をAgentDock専用の**オブジェクト置き場**（チャット領域内）へ控える（基準 B）。turnとその子孫が終わったら同じ方法で控える（終了 E）。
- 戻す: 「このturn以降の変更」を、ファイルごとに B の内容へ書き戻す。現在の内容が「このチャットの変更だけ」と言えるファイルはそのまま戻せる。言えないファイル（別の変更の混在・終了未確認・同時作業）は止めて理由を出し、ユーザーが2段目の確認で個別に選んだときだけ戻す。HEAD・ブランチの変更後、Git操作の途中、対象外の種類は常に止める。
- ユーザーの作業ツリー・インデックス・stash・ブランチ・refs・リポジトリの `objects` には一切書かない。
- 現行の差分逆適用と `fileChange`／turn差分の観測・記録（`changes.jsonl`）は**廃止**。旧ファイルは消さずに残す（読まない）。

---

## 1. 基準の取り方（決定）

### 1.1 方式の比較と結論
| 候補 | 判定 | 理由 |
|---|---|---|
| A. `git stash create` 相当（一時インデックス） | 不採用 | 未追跡ファイルを含められない（`-u` は `stash push` 専用で作業ツリーを変える）。作られたコミットがどこからも参照されず gc 対象。オブジェクトをリポジトリの `objects` に書く |
| B. `git add -A`＋`write-tree`（一時インデックス、未追跡込み） | 不採用 | 全ファイルを毎回ハッシュし巨大リポジトリで遅い。`add` は clean フィルター（autocrlf・`.gitattributes` の eol・LFS）を通すため、戻すと元のバイト列にならない。オブジェクトをリポジトリへ書く |
| C. HEADとの差分パッチ＋未追跡本体の保存 | 不採用 | パッチはCRLF・バイナリ・巨大ファイルで再適用が不安定（現行方式と同じ弱点）。比較のたびに自前の差分計算が要る |
| **D. HEADの木＋「HEADと違うファイルだけ生バイトで追加」の木を、AgentDock専用のオブジェクト置き場に作る** | **採用** | 下記 |

**採用方式 D（「上書き木」）**
1. `git --no-optional-locks status --porcelain=v2 -z --branch --untracked-files=all --no-renames --ignore-submodules=none`（読取りのみ、索引を書かない）で、HEAD のコミットID・ブランチ名と、HEAD と違うパス（索引の変更 X・作業ツリーの変更 Y・未追跡）を得る。無視ファイル（.gitignore 対象）は出ない。
2. 違うパスだけ、Rust 側で `symlink_metadata` を見て分類（§1.3）し、通常ファイルは `git hash-object -w --no-filters --stdin-paths` で**生バイトのまま**ハッシュして専用置き場へ書く。削除されたパスは「無し」。
3. 一時インデックス（`GIT_INDEX_FILE`＝AgentDock領域の一時ファイル）に `git read-tree <HEAD>`（HEADがなければ `--empty`）→ `git update-index --index-info`（標準入力で `mode oid\tpath`、削除は mode 0）→ `git write-tree` で木IDを得る。一時インデックスはAgentDock自身の一時ファイルなので、作業後に消す（ユーザーデータではない）。
4. 結果を「控え（Snapshot）」として記録する: `{head, branch, tree, raw: 生バイトで入れたパスの集合, skipped: 控えなかったパスと理由}`。

**なぜ安全か**: すべての git 実行で `GIT_OBJECT_DIRECTORY=<チャット領域>\baselines\objects`、`GIT_ALTERNATE_OBJECT_DIRECTORIES=<リポジトリの共通objects>`、`GIT_INDEX_FILE=<一時ファイル>` を渡す。新しいオブジェクトは専用置き場にだけ書かれ、リポジトリの objects・索引・refs・stash・ブランチ・reflog は変わらない（結合テストで照合する §9）。`gc --auto` を起こすコマンドは使わない。

**既知の限界（REVIEW_R4 低L3）**: git は、書こうとしたオブジェクトが alternates（リポジトリ側）に既にあると、書かずにそのファイル（または pack）の更新時刻（mtime）を更新する（freshen）。そのため、ユーザーの `.git/objects` の mtime が変わりうる。内容は書き換えない。gc の prune の判断に影響しうる程度で、実害は小さい。結合テストはファイル一覧の比較のみで、mtime は比べない。

**なぜ速いか**: HEAD と同じファイル（大半）はハッシュせず、HEAD の木のエントリをそのまま使う。木オブジェクトもHEADと同じ部分木はリポジトリ側に既にあるので書かない。費用は「status 1回＋違うファイルのハッシュ」で、巨大リポジトリでも未コミット変更の量に比例する。

**「HEAD形」と「生形」**: 木の各エントリは、HEADから来たもの（HEAD形＝クリーン形。作業ファイルは smudge 後の内容）か、生バイトで入れたもの（生形）かのどちらか。`raw` 集合で区別する。
- 比較は `(oid, 形)` の組で行い、形が違えば内容が同じでも「違う」とみなす（安全側。誤って「同じ」とは言わない）。
- 書き戻しは、生形なら `git cat-file blob <oid>`、HEAD形なら `git cat-file --filters --path=<path> <oid>`（checkout と同じ eol 変換・smudge・LFS を適用したバイト列）で取り出して、AgentDockが原子的に書く。git に作業ツリーを書かせない（`checkout` 系は使わない）。

### 1.2 Windows・巨大リポジトリ・各種ファイルの扱い（決定）
| 対象 | 扱い |
|---|---|
| CRLF・autocrlf・`.gitattributes` | 生形は `--no-filters` で元のバイト列そのもの。HEAD形は `cat-file --filters` で checkout と同じ変換。改行の違いだけの変化は status が正規化するため見えない場合がある（既知の限界として注記） |
| LFS | 違うファイルは生バイト（実体）で控える（上限内）。HEAD形の書き戻しは LFS の smudge を通す（ローカルにない実体の取得に失敗したらそのファイルは失敗。他は続行） |
| 大きいファイル | 1ファイル 64MiB 超は控えない（`skipped: TooLarge`）。1回の控えの生バイト合計 512MiB を超えた分も控えない（`skipped: Budget`）。控えないパスは戻せない（強制も不可） |
| シンボリックリンク・ジャンクション | 辿らずに `skipped: Link`。戻せない（強制も不可） |
| サブモジュール | 中身に入らない。status の submodule 行は `skipped: Submodule`。HEAD の gitlink はそのまま。戻せない（強制も不可） |
| 無視ファイル | 控えない（status に出ない）。turnが作った・変えた無視ファイルは一覧に出ず戻せない（注記に明記） |
| sparse-checkout（skip-worktree）・assume-unchanged | 作業ツリーに無い・status に出ないパスは B と E で同じ扱いになり差分に出ない。assume-unchanged のファイルの変更は検出できない（既知の限界） |
| 初回コミット前（HEADなし） | `read-tree --empty` から作る。HEAD比較は「どちらもなし」で一致 |
| Git worktree（#14） | リポジトリのルートは worktree のトップ、alternates は `git rev-parse --path-format=absolute --git-common-dir` の `objects`（Git 2.31 以上） |
| `safe.directory` 等で git が拒否 | 控えは失敗（理由をそのまま記録・表示）。送信は止めない（§3.2） |
| SHA-256 形式のリポジトリ | oid は不透明な文字列として扱い、Rust 側で git の oid を計算しない（照合用ハッシュは自前の sha256、§4.2） |

### 1.3 違うパスの分類（純粋ロジック `rules::baseline::classify`）
status の各行と `symlink_metadata` の結果から: `Hash`（通常ファイル、上限内）／`Gone`（無い＝削除）／`Skip{Link|Submodule|TooLarge|Budget|Unreadable}`。mode は status の作業ツリー側 mode（`mW`）を使い、通常ファイル以外の mode は `Skip`。

---

## 2. 控えの保存場所・形式（決定）

```
chats\<dirId>\
  baselines\
    objects\            専用のgitオブジェクト置き場（loose。内容アドレスなので同じ内容は1つ）
    segments.jsonl      区間（turn）ごとの控えの記録と「戻し」の記録（追記。最終行の欠けは無視）
    tmp\                一時インデックス（作業ごとに作って消す。AgentDock自身の一時ファイル）
  revert-backup\<ms>\   戻す直前の現在の内容（現行と同じ形式: 実体＋manifest.json）
  changes.jsonl         旧方式の観測記録。新規には書かず、読まない。消さない（チャット削除で一緒に消える）
```
- チャットごとに置き場を分ける（同じリポジトリを使う別チャットとは共有しない）。チャット削除で領域ごと消える既存の削除工程に乗る（`remove_tree` は読み取り専用の loose オブジェクトも解除して消す）。
- **自動削除しない**。容量不足でも消さない。古い控えを間引かない。控えは「このチャットの控えを削除」（ユーザーの明示操作、§7.1）とチャット削除でだけ消える。
- 書込み前に空き確認（違うファイルの合計サイズ＋1MiB）。足りなければ控えは失敗として記録し、送信は止めない（§3.2）。
- 使用量: `UsageBreakdown` に `baselines`（`baselines\` 配下）と `revertBackups`（`revert-backup\` 配下）を追加（`#[serde(default)]`）。`metadata` から差し引いて二重計上しない。DESIGN_P3 §2.4 の「変更の控え」の計上漏れもここで解消する。旧 `changes.jsonl`・圧縮前の控え・side 記録は現状どおり metadata。

**`segments.jsonl` の行（中立型・Codex固有の型なし。oid・ブランチ名は不透明な文字列）**
```
BaselineLine =
  | Started  { seg: LocalId, at, attempt: LocalId, repo: RepoRef{root, objectsDir}, cwd, base: Snapshot }
  | Bound    { seg, turn: ExternalId }                     受理でturnと対応付け
  | Ended    { seg, at, end: Snapshot | EndIsNextBase }    終了の控え（§3.3）
  | Abandoned{ seg, reason }                               送信が受理なしで確定（区間から外す）
  | Failed   { seg?, at, attempt, phase: Base|End, reason: BaselineFailure }   控えられなかった
  | Concurrent { seg, with: ChatKey | External{label} }    同じリポジトリでの同時作業（§3.5）
  | Reverted { at, fromSeg, items: [RevertedPath{path, restored: EntryRef|Absent, backupFile, forced: bool, overridden: Vec<RevertBlockCode>}], failed: [path, reason], backupDir }
Snapshot = { head: Option<String>, branch: Option<String>, tree: String, raw: Vec<String>, skipped: Vec<{path, reason}>, tookMs }
EntryRef = { oid: String, form: Head|Raw }
BaselineFailure = NotARepository | GitUnavailable | WorkFolderUnknown | Timeout | InsufficientSpace{required, available} | GitBusy | Disabled | Git{message}
```
- `schemaVersion: 1` を各行に持つ（既存の追記型と同じ）。パスはリポジトリ相対（`/` 区切り、git の表記のまま）。IPC へは絶対パスに直して出す。
- `forced`・`overridden` は、2段目の確認で強制したファイルと、そのとき無視した理由（監査・表示用）。

---

## 3. turnとの対応（決定）

### 3.1 区間
- 1区間＝AgentDockが送ったユーザー送信1件（`SendMode::NewTurn`）で始まるturnと、その子孫の作業。基準 B は**送信の直前**、終了 E は**そのturnの終端を観測し、チャット内に作業中のエージェントがなくなったとき**。
- 追加指示（steer）は走っている区間に含まれる（新しい区間を作らない）。レビュー・圧縮・side相談（read-only）は区間を作らない（それらで変わったファイルは次の区間との連続性検査で「区間の間の変更」になる）。
- 外部（VS Code等）が始めたturnは区間にならない（そのファイルは連続性検査で「区間の間の変更」になる）。

### 3.2 B（基準）を取る時点
- `dispatch_send`（host/mod.rs）と `run_entry_send`（queue_driver.rs）の、**送信前保存の後・`backend.send` の前**に `take_base(chat, attempt, cwd)` を呼ぶ。turn開始通知を待ってから取ると、Codexの編集が先に始まっている可能性があるため、通知では取らない。
- 結果に応じて: 成功→`Started`、Gitリポジトリでない・作業フォルダ不明→`Failed{phase: Base}`（警告なし。ダイアログで理由表示）、Git失敗・時間切れ・空き不足→`Failed`＋警告「このturnの変更は控えられていません（理由）。『変更を戻す』の対象になりません」（同じ理由の警告はチャットごとに1回）。**控えの失敗では送信を止めない**（送信が本来の機能で、戻すは補助のため）。
- 時間の上限は 10 秒（status・ハッシュ・木作成の合計）。超えたら打ち切って `Failed{Timeout}`。設定 `baselines.enabled`（既定 true）で無効にでき、無効なら `Failed{Disabled}`（警告なし）。
- 受理後に `Bound{turn}`。受理なしが確定したら `Abandoned`。受理不明は照合で受理・turnが分かった時点で `Bound`、受理なしが確定したら `Abandoned`、分からないままなら区間は turn 不明のまま（個別には選べないが、区間の並びには入る）。

### 3.3 E（終了）を取る時点
- そのチャットの**対応付けたturnの終端**をliveで観測し、かつそのチャットのエージェントに `running_turn` が残っていないとき、E を取る（観測の処理は待たない。チャットごとの控え作業キューに積む）。子孫がまだ動いていれば、最後の子孫の終端まで待つ。
- 子孫が動いたまま次の送信が来たら、次の B をそのまま前の区間の E とする（`EndIsNextBase`）。チャットが途切れず作業中だった区間なので、間に「アイドル中の外部変更」は入らない。
- 切断・再起動・sleep 等で終端を観測できなかった区間は `Ended` がない＝**終了未確認**。後から履歴で終端を確認しても E は作らない（いつの状態か分からないため）。終了未確認の区間を含む範囲は「要確認」（強制可、§4.3）。
- E を早く取りすぎても安全側（後の変更は「turn後の変更」として要確認になる）。遅く取ると外部の変更をturnのものと誤認するので、終端観測後すぐ取る。

### 3.4 チャットごとの控え作業キューとロック
- B・E・現在状態（§4.2の C）・控えの削除は、チャットごとに1本のキューで直列に行う（次の送信の B は、前の区間の E の完了を待つ）。
- リポジトリごとの `RwLock`（正規化したリポジトリルートがキー）: 控え（B・E・C）は読取り側、戻すの実行は書込み側。送信は戻すの実行が終わるまで待つ（数秒）。ロック順は「send_lock → リポジトリロック」「revert_lock → リポジトリロック」のみで、戻すは send_lock を取らない（循環なし）。

### 3.5 同じフォルダの複数チャット・子エージェント・外部
- 子エージェントの変更は親チャットの区間に含まれる（チャット＝親＋子孫）。子孫の作業フォルダが別リポジトリなら、その変更は控えの対象外（注記）。
- **同時作業の記録**: B を取るとき・区間が開いている間に、同じリポジトリ（他チャットの作業フォルダがリポジトリルートと `paths_overlap`）で作業中のAgentDockの別チャットがあれば、双方の区間に `Concurrent{with}` を書く。外部実行中（`ExternalRunning`）の会話が同じ場所にあれば `Concurrent{External}`。
- 判定（§4.3）: 同時作業のあった区間で変わったパスは、相手の重なった区間でも変わっていれば（相手の E がなければ「変わった可能性あり」）`ConcurrentChange`（要確認）。相手が `External` なら、その区間で変わったパスはすべて `ConcurrentChange`。
- AgentDock外のエディタ・ツールがturnの最中に変えたものは原理的に区別できない（既知の限界。ダイアログに常に注記する）。
- 既存規則との整合: 戻すの実行中は `FolderOpGuard` を**リポジトリルート**で取る（重なるチャットのキュー自動送信を保留。既存の `folder_op_overlaps` がそのまま効く）。`revert_blocker` の「同じ作業フォルダの別チャットが作業中」も、作業フォルダではなくリポジトリルートとの `paths_overlap` で判定する（M3 の方針を維持・拡張）。`revert_blocker` は強制の有無にかかわらず適用する（作業中は強制でも戻さない）。

---

## 4. 戻す対象の選び方と判定（決定）

### 4.1 単位
- 選択は現行と同じ: 「このturn以降」（`turn`）または「このチャットの控えのある変更すべて」（＝最初の区間から）、さらにファイルの選択。
- 1ファイルを戻すと、そのファイルは**選んだ区間の B の内容**になる。より後の区間の同じファイルの変更も一緒に戻る（`includesTurns` で表示）。途中のturnだけを打ち消す3方向マージはしない。
- 対象パス＝選んだ区間 k からチャットの最新区間 n までのどこかで変わったパス（各区間の B と E の `(oid,形)` の差、`git diff-tree -r -z --no-renames --name-only` ＋ `raw` 集合の差）。E のない区間は B と次の B（最新なら C）の差で代える。移動は「削除＋追加」の2パスとして出す。
- 強制はファイル単位（§4.3 の「要確認」のファイルを、ユーザーが2段目の確認で1件ずつ選ぶ）。

### 4.2 現在の状態 C
- 計画のたびに、§1.1 と同じ手順で現在の状態 C を作る（専用置き場に書くだけで、ファイル・リポジトリは変えない）。さらに対象パスごとに、Rust でファイルを直接読んだ sha256（`expect`。無ければ「無し」）を取り、書込み直前の再照合に使う（強制のファイルも同じ）。

### 4.3 判定（純粋ロジック `rules::baseline::plan`）
対象パス p ごとに判定する。入力は区間の記録と、各スナップショットにおける p の `EntryState = Present(EntryRef) | Absent | Skipped(reason)`（ホストが `git ls-tree` で引いて渡す）。
判定は2段階: まず「止める（強制不可）」を上から調べ、当たれば `Blocked`。当たらなければ「要確認（強制可）」の条件を**すべて**集め、1つ以上あれば `NeedsOverride{reasons}`、なければ `Revertible`。

**止める（強制不可）**
| # | 条件 | `RevertBlockCode` |
|---|---|---|
| S0 | リポジトリが rebase・merge・cherry-pick 等の途中（`--git-path` の `rebase-merge`・`rebase-apply`・`MERGE_HEAD`・`CHERRY_PICK_HEAD`・`REVERT_HEAD` の有無） | `GitBusy`（全体） |
| S1 | Gitリポジトリでない・Gitを実行できない・範囲に控えのある区間がない | `NotARepository`／`GitUnavailable`／`NoBaseline` |
| S2 | p が作業フォルダ（チャットの cwd）の外、または解決後にリポジトリ外（`confine_to`） | `OutsideWorkFolder`／`PathOutside` |
| S3 | 区間 k〜n のどれかの B・E、または C で p が `Skipped`（リンク・サブモジュール・64MiB超・合計上限・読めない） | `NotSnapshotted` |
| S4 | 区間 k〜n のどこかで `B.head ≠ E.head`、`E_j.head ≠ B_{j+1}.head`、最後の控え（E_n、なければ最後の B）と C の head が違う、またはブランチ名が変わった | `HeadMoved`（コミット・ブランチ切替・reset・rebase 後） |
| S5 | `C(p)` が、最新の控え以後のAgentDockの `Reverted` の `restored` と一致（§4.5） | `AlreadyReverted` |

**要確認（強制可。2段目の確認でファイル単位に選べる）**
| # | 条件 | `RevertBlockCode` |
|---|---|---|
| O1 | 区間 k〜n のどれかが終了未確認（E なし） | `EndUnknown` |
| O2 | 区間の間で `E_j(p) ≠ B_{j+1}(p)`（その間にAgentDockの `Reverted` があり、`B_{j+1}(p)` がその書き戻し内容と一致すれば連続とみなす） | `ChangedBetween` |
| O3 | `C(p) ≠ E_n(p)`（E_n がなければ O1 のみ） | `ChangedAfter` |
| O4 | §3.5 の同時作業で帰属を判別できない | `ConcurrentChange` |

- 書き戻し内容の取り出し（`cat-file`）の失敗は実行時に `ReadFailed`（そのファイルは失敗、他は続行）。
- 1つのファイルが止まっても他は判定を続ける。計画は5分有効・メモリのみ（現行の `PLAN_TTL_MS`・`MAX_PLANS` を流用）。
- 書き戻し先は O1〜O4 があっても常に「選んだ区間 k の B の内容」（推定で中間の状態を作らない）。

### 4.4 実行（M1・M2・L6・強制・「何も変えない」）
1. `revert_lock` → `FolderOpGuard(リポジトリルート)` → `revert_blocker`（作業中・停止未確認・受理不明・PendingOp・削除保留・外部実行中・同じリポジトリの別チャットの作業中で止める）→ リポジトリの書込みロック。
2. 書込みロックの中で再計画（C を取り直す）。保存した計画と違えば `PlanStale`（現行どおり）。`paths` の各ファイルは新しい計画で `Revertible`、`forced` の各ファイルは `NeedsOverride` で、**理由の集合が計画時と同じ**であること（増えていれば `PlanStale`）。`Blocked` のファイルは強制指定があっても書かない（`InvalidArgs` ではなく `failed` に理由）。
3. 全対象の作業フォルダ内検査（`check_inside`）。1つでも外なら何もしない（現行どおり）。
4. **控え**: 書き換える全ファイル（強制分を含む）の現在の内容を `revert-backup\<ms>\` へ（空き確認つき）。失敗・空き不足なら**何も変えない**（現行どおり）。io_guard はこの控えと記録の追記の間だけ持つ（L6）。
5. ファイルごと: 書き戻す内容を取り出す（失敗→そのファイルは失敗、他は続行）→ **書込み直前に現在の内容の sha256 が計画時の `expect` と一致するか再照合**（不一致→書かずに「計画後に変更されたため書き換えませんでした」、M1。強制分も同じ）→ 原子的に書く／削除する（削除で空になったフォルダは消さない）。
6. **成功したファイルだけ** `Reverted.items` に入れて追記する（M2。強制分は `forced: true`・`overridden` 付き）。失敗は `failed` に理由つき。記録の追記に失敗したら警告「戻した記録を保存できませんでした。ファイルは戻っています（控えは保存済み）」。
7. 結果 `RevertResult{reverted, forced, failed, backupDir}`。`failed` が空でなければUIは「一部のみ」で完了と書かない（現行どおり）。

### 4.5 「戻し済み」の扱い（H2 の後継）
- S5: `C(p)` が、最新の控え以後の `Reverted` の `restored`（`EntryRef` が同じ、または Absent どうし）と一致するなら `AlreadyReverted`（一覧では「戻し済み」）。記録は「どの区間から・どのパスを・何に戻したか」を持つので、他チャット・他区間の変更を消化済みにしない（H2 の誤表示が構造上起きない）。
- 戻した後にさらに外部で変えられたら O3（turn後の変更、要確認）になる。

---

## 5. 戻せない・要確認の表示（決定。理由はすべて画面に出す）
| 状況 | 区分 | 表示（要旨） |
|---|---|---|
| Gitリポジトリでない | 止める | 「作業フォルダがGitリポジトリではないため、変更の控えを取っていません。戻せません」 |
| Gitを実行できない | 止める | 「Gitを実行できません（未導入、または設定のGitの場所が違います）」。能力は実行時に降格（§8） |
| 控えの失敗（時間切れ・空き不足・git のエラー・無効設定） | 止める | 「このturnの開始時に控えを取れませんでした（理由）」 |
| コミット・ブランチ切替・reset・rebase の後 | 止める | 「turnの後にGitのHEADが変わったため、控えへは戻せません。Gitの操作（git revert 等）で戻してください」 |
| rebase・merge 等の途中 | 止める | 「Gitの操作の途中のため戻せません。完了または中止してから再確認してください」 |
| 控えていないパス | 止める | 「控えの対象外です（シンボリックリンク／サブモジュール／64MiB超／.gitignore 対象）」 |
| 作業フォルダの外 | 止める | 「作業フォルダの外のファイルは戻しません」 |
| 控え導入前のturn | 止める | 「このturnは控えを取る前のものです」（区間がない） |
| 終了未確認 | 要確認 | 「turnの終了時の控えがありません（切断・再起動など）。turn後の別の変更と区別できません」 |
| turnの後・間に別の変更 | 要確認 | 「turnの後（間）に、別の会話・エディタ等による変更があります」 |
| 同時作業 | 要確認 | 「同じリポジトリで同時に作業していた別の会話も、このファイルを変更した可能性があります」 |

---

## 6. 現行方式・観測の扱い（決定）
- **廃止（コードから削除）**:
  - 戻す: `rules/revert.rs` の計画・逆適用（`plan`・`plan_confined`・`is_consumed`・`RevertMark`）、`rules/diff.rs` の逆適用用の補助（削除本文の判別など。L5 は対象がなくなる）、`host/changes.rs` の `ChangeLine::Reverted` 書込み・`RecordId` による消化。
  - 観測: `BackendEvent::FileChangeObserved`・`TurnChangesUpdated` と `FileChange` 型、`codex/events.rs` のそれらへの写像（`turn/diff/updated` は「意図的に写像しない既知の通知」として空を返す。`fileChange` item は活動表示の写像だけ残す）、`host/changes.rs` の `start_changes_observer`・`observe_changes`・`record_changes`・`observe_post_state`・`load_changes`・`group_records`・`reported_files`、`host/state.rs` の turn差分の保持（`turn_diffs_of_chat` 等）、`store` の `append_change`・`read_all_changes`、`backend/changes.rs` の `ChangeRecord`・`ChangeLine`・`PostState`・`RecordId`、`ChangeSource::BackendReported`。
  - 「目安」として縮小して残す案は採らない: 控え基準の差分が同じ役目（どのファイルを触ったか）をコマンド編集も含めて果たすため、二重の出所を作らない（Q2 の趣旨）。
- **残す**: `ArtifactObserved`（成果物、P4。同じ検出点だが別イベントなので影響なし）。`fileChange` item のチャット内の活動表示（会話の表示であり、差分ダイアログの出所ではない）。`norm_path`・`sha256_hex`・`check_inside`・`confine_to`（汎用部品として `rules/paths.rs` へ移す）。`split_files`・`count_for`・`parse_status_v2`（Git・控え基準の一覧で使用）。
- **旧記録**: `changes.jsonl` は新規に書かず、どこからも読まない（したがって読込みの互換問題は生じない）。ファイルは消さず、使用量では metadata に含まれ、チャット削除で一緒に消える。旧 `revert-backup\` も消さず `revertBackups` に計上。旧方式の時代のturnは区間がないので「控えを取る前のturn」と表示する。
- DESIGN_P3 §1 #1・#2・§2.2（新イベント）・§2.4（`changes.jsonl`）の該当箇所、正本の合意は P3B-6 で司令塔が更新する（§10）。

---

## 7. IPC・UI（決定）

### 7.1 IPC（既存コマンドを再利用）
- `ChangeSource = Baseline | Git`（`BackendReported` を削除）。表示名は「turnの変更（控え基準）」「Git上の現在の差分（HEAD比較）」。
- `get_change_list{chat, scope, source}`（`source` を必須で追加）: `Baseline` は `Turn{turn}`＝その区間の B→E（終了未確認・作業中なら B→C で、`notes` に「途中の状態」と明記）、`Chat`＝最初の区間の B→最新の控え（なければ C）。`ChangedFile.turn`＝そのパスを変えた最後の区間のturn、`reverted`＝S5 の戻し済み。`Git` は `scope` を無視して現行の作業ツリー差分。`ChangeScope::WorkingTree` は削除（`source: Git` で表す）。
- `get_file_diff{chat, path, source, turn?}`: `Baseline` は `git diff --no-ext-diff --no-textconv --no-color <B> <E|C> -- <path>`（専用置き場＋alternates の環境で、読取りのみ）。生形とHEAD形の混在で改行だけの差が出うるため表示専用と注記。`Git` は現行どおり。
- `preview_revert{chat, turn?, paths?}`: 現行の引数のまま。`RevertVerdict = Revertible{summary} | NeedsOverride{summary, reasons: [{code, message}]} | Blocked{code, message}`。
- `revert_changes{chat, planId, paths, forced}`: `forced: Vec<String>`（2段目の確認で選んだ要確認ファイル。既定は空）を追加。`RevertResult` に `forced: Vec<String>`（強制で戻したファイル）を追加。
- `RevertBlockCode` を置き換える: `GitBusy`・`NotARepository`・`GitUnavailable`・`NoBaseline`・`OutsideWorkFolder`・`PathOutside`・`NotSnapshotted`・`HeadMoved`・`AlreadyReverted`・`EndUnknown`・`ChangedBetween`・`ChangedAfter`・`ConcurrentChange`・`ReadFailed`。旧コードは削除（IPCのみで、`segments.jsonl` 以外に保存されないため）。
- 追加: `get_baseline_status{chat}` → `[SegmentStatus{turn?, startedAt, state: Ready | Running | EndUnknown | Failed{reason} | Abandoned, concurrent: bool}]`（「戻す範囲」の選択肢と理由表示）。`delete_baselines{chat}`（`UserConfirmed` 必須。作業中でないときだけ。チャットの控え作業キューで直列に実行し `baselines\` を消す）。イベントは既存の `changesUpdated` を、控えの記録（Started・Ended・Failed・Reverted）の追記時に出す。
- 型は ts-rs 生成（`cargo test --lib export_bindings`）→ `types.ts` の再export。`live.ts`・`App.tsx`・`client.ts` の `backendReported` 参照を置換。

### 7.2 UI（`ChangesDialog.tsx`）
- 差分ダイアログのタブは2つ: 「turnの変更（控え基準）」（既定）／「Git上の現在の差分（HEAD比較）」。「変更を戻す…」ボタンは控え基準のタブに置く。控え基準タブのturn選択は `get_baseline_status` から作る。旧 `NOTE_REPORTED` の注記は削除し、控え基準タブの注記「turnの開始時と終了時にAgentDockがGitで控えた内容の差です。.gitignore 対象・サブモジュール・作業フォルダ外は含みません。turnの最中に別のツールで変えた内容も含まれます」に置換。
- 戻すダイアログの説明文（置換）: 「turnの開始時にAgentDockがGitで控えた内容へ、ファイルを書き戻します。現在の内容がこのチャットのturnによる変更だけのファイルはそのまま戻せます。turnの後や間に別の変更がある・終了時の控えがない・別の会話と同時に作業していたファイルは、理由を表示して止めます。内容を確認したうえで、ファイルごとに『別の変更も含めて戻す』を選べます。コミットなどでHEADが変わった後のファイルは戻せません。戻す前の内容は、このチャット用の領域に控えとして保存します（自動では削除しません）。」
- 一覧: `Revertible` はチェック（初期オン）。`NeedsOverride` は「要確認」として理由を並べ、チェック「別の変更も含めて控えの内容に戻す」（**初期オフ**）。`Blocked` は理由のみ（チェックなし）。
- 確認: 1段目「選んだN件を戻す…」。強制のファイルが1件以上あれば2段目（`alertdialog`）で、強制ファイルの一覧と理由、「これらのファイルでは、別の会話・エディタによる変更や時期の分からない変更も消えます。書き換える前の内容は控えに保存します」を出し、「別の変更も含めて戻す」で実行。2段目でやめれば何も変えない。
- 結果: 現行の表示（一部のみ／件数／控えの場所）に「強制で戻したファイル」の行を追加。
- 設定画面: `baselines.enabled`（「turnの開始時に変更の控えを取る（Gitリポジトリのみ）」既定オン）。使用量に「変更の控え」「戻す前の控え」の行と、チャットごとの「控えを削除…」（確認:「このチャットのturnは、以後『変更を戻す』の対象になりません。N MB」）。

---

## 8. 能力宣言と確認状況（決定）
- `RevertChanges`: `route = AppManaged`、`support = Supported`（条件はturnの開始・終端をバックエンドが通知できること。ファイル変更の報告には依存しない。他AIでも turn イベントがあれば対応）。`note`: 「Gitリポジトリのみ」。
- `ChangeList`: `route = AppManaged` に変更（出所が控え基準とGitだけになり、バックエンドの報告を使わないため）。`support = Supported`、`note`:「Gitリポジトリのみ」。
- Gitを実行できないと分かったら、既存の `opCapabilitiesUpdated` で両操作の `note` に理由を出す（チャットごとの「リポジトリでない」は能力ではなく `Blocked{NotARepository}`／一覧の `NotSupported` で返す）。
- 確認状況は `parity_table.rs` で両方 `Unverified`（方式を変えたため。実機確認で司令塔が更新）。

---

## 9. テスト方針
**単体（純粋ロジック、`rules/baseline.rs`）**
- `classify`: status v2 `-z` 行（通常・未追跡・削除・submodule・mode 120000）とメタデータから `Hash/Gone/Skip`、上限 64MiB・合計 512MiB。
- `plan`: 変更なし→`Revertible`／turn後の変更→`NeedsOverride[ChangedAfter]`／区間の間の変更→`[ChangedBetween]`／終了未確認→`[EndUnknown]`／同時作業（相手も変更・相手のEなし・External）→`[ConcurrentChange]`／複数理由の同時収集／AgentDockの戻しを挟んだ連続／戻し済み→`Blocked`／HEAD移動（区間内・区間後・ブランチ名だけ変化）→`Blocked`（O 条件があっても `Blocked` が優先）／`GitBusy`／Skipped／作業フォルダ外／`EndIsNextBase`／`includesTurns`／形違い同oidを「違う」と判定。
- 強制の受付判定（実行時の再計画との照合）: `forced` が `NeedsOverride` で理由が同じなら実行、理由が増えた→`PlanStale`、`Blocked` になった→`failed`、`Revertible` に変わった→そのまま実行。
- `segments.jsonl` の serde 往復（`forced`・`overridden` を含む）。旧 `settings.json`（`baselines` なし）・旧使用量が読めること。旧 `changes.jsonl` があっても起動・使用量・チャット削除が動くこと。

**結合（一時リポジトリ、`app/src-tauri/tests/baseline_git.rs`。git がなければスキップ）**
- `core.autocrlf=true` のCRLFファイル・`.gitattributes eol=crlf`・未追跡・削除・無視ファイル・初回コミット前で、B→（ファイル変更）→E→戻す の結果がバイト単位で B と一致。
- 前後で、`.git/index` のバイト列・`git for-each-ref`・`git stash list`・`git reflog`・`.git/objects` 配下のファイル一覧が**変わらない**こと。
- コミット後は `HeadMoved`（強制指定しても書かれない）、`rebase-merge` を置くと `GitBusy`。
- E の後にファイルを書き換え→`NeedsOverride`。強制なしでは書かれず、強制ありでは B と一致し、控えに書換え前の内容が残る。
- 書込み直前の再照合（計画後にファイルを書き換えて実行→そのファイルは書かれない、強制分も同じ。他は戻る）、控え保存の失敗（控え先を読み取り専用に）で何も変わらない。
- 画面のテストは書かない（方針どおり）。テストは変更したモジュールに限って流す。

---

## 10. 実装タスク
共通: 担当はすべて implementer。開始時に `git log -1` が master 最新か確認。改行コードは既存のまま。ホストの新処理は `host/baseline.rs` に置き、`host/mod.rs`・`queue_driver.rs` は呼出し数行に留める。

| # | 内容 | 主な対象 | 依存 | 受入条件の要点 | 単体テスト |
|---|---|---|---|---|---|
| P3B-0 | 実行部品の拡張: `exec::RunSpec` に標準入力（`stdin: Option<&[u8]>`）、`gitops` に控え用の操作（`StatusSnapshot`・`HashObjectsRaw{write}`・`ReadTree{head?}`・`UpdateIndexInfo`・`WriteTree`・`LsTree{tree, paths}`・`DiffTreeNames{a,b}`・`DiffTrees{a,b,path}`・`CatFileBlob`・`CatFileFiltered{path,oid}`・`GitPaths`（toplevel・common-dir・進行中の操作））と、環境の組 `SnapshotEnv{objects, alternates, index}`。gitops の冒頭docに「控え用の操作はAgentDock領域にだけ書く」を追記 | `exec.rs`、`gitops/mod.rs`、`gitops/snapshot.rs`（新規） | — | `cargo check` 通過／控え用の操作は必ず `SnapshotEnv` を要求し、無い呼出しは型で書けない／パスは `--` の後・標準入力で渡す／既存の操作の引数は変わらない | 各操作の引数配列（`--no-filters`・`--no-optional-locks`・`--no-renames`・`--` の位置） |
| P3B-1 | 中立型と保存: `backend/baseline.rs`（`BaselineLine`・`Snapshot`・`EntryRef`・`BaselineFailure`・`SegmentStatus`）、`RevertBlockCode`・`RevertVerdict`（`NeedsOverride`）・`RevertChangesArgs.forced`・`RevertResult.forced` の置換、`ChangeSource = Baseline｜Git`・`get_change_list` の `source`、`store` の `baselines\` 配置（`append_baseline`・`read_baselines(chat)`・`baseline_objects_dir`・`delete_baselines`）、`UsageBreakdown.baselines/revertBackups`、`settings.baselines.enabled`、ts-rs 再生成 | `backend/{baseline,changes,local}.rs`、`store/{layout,mod,records}.rs`、`src/ipc/gen`・`types.ts` | — | 旧 `settings.json`・旧使用量が読める／削除工程が `baselines\`（読み取り専用ファイル含む）を消せる／自動削除の経路がない | serde 往復・旧ファイル互換・使用量の内訳が二重計上しない |
| P3B-2 | 判定の純粋ロジック `rules/baseline.rs`（`classify`・`plan`（止める／要確認の2段）・区間の並べ方・同時作業の判定・戻し済み・強制の受付判定）。旧 `rules/revert.rs` の計画・逆適用を削除し、汎用部品を `rules/paths.rs` へ移す | `rules/{baseline,paths,diff}.rs` | P3B-1 | §4.3 の表どおり（止める条件が要確認より優先、要確認は全理由を集める）／推定で `Revertible` にしない／形違いを「違う」とする／強制不可の条件に強制が効かない | §9 単体の `plan`・`classify`・強制の受付判定 全ケース |
| P3B-3 | 控えの取得: `host/baseline.rs`（チャットごとの控え作業キュー、リポジトリごとの RwLock、`take_base`（送信2箇所から）、受理・受理なし・受理不明の照合結果での `Bound`/`Abandoned`、終端観測での E・`EndIsNextBase`、同時作業の記録、空き確認・10秒上限・警告の1回化、`get_baseline_status`・`delete_baselines`） | `host/baseline.rs`、`host/mod.rs`（dispatch_send）、`host/queue_driver.rs`（run_entry_send・照合）、`host/state.rs`（終端の通知点）、`commands.rs` | P3B-0・P3B-1 | B は `backend.send` の前／控え失敗で送信を止めない／steer・レビュー・圧縮・side で区間を作らない／終端未観測は E なし／ユーザーの索引・refs・stash・objects を書かない／ロック順が §3.4 どおり | 区間の状態遷移（Started→Bound→Ended／Abandoned／EndIsNextBase） |
| P3B-4 | 戻すの作り替えと観測の廃止: `host/changes.rs` の `preview_revert`・`revert_changes` を控え基準に（C の取得、`plan` 呼出し、§4.4 の手順＝強制の照合・再照合・成功分のみ記録、`FolderOpGuard` と `revert_blocker` をリポジトリルート基準に、io_guard は控えと追記の間だけ）。`get_change_list`/`get_file_diff` を `Baseline｜Git` に。§6 の観測の削除（`FileChangeObserved`・`TurnChangesUpdated`・`FileChange`・`changes.jsonl` の読み書き・turn差分の保持、`codex/events.rs` の写像とそのテストの更新、`turn/diff/updated` は空を返す） | `host/{changes,state,mod}.rs`、`backend/{changes,backend}.rs`、`codex/events.rs`、`store/mod.rs`、`codex/parity.rs`（`ChangeList` の route） | P3B-2・P3B-3 | 控え保存失敗で何も変えない／書込み直前の再照合（強制分も）／強制は `NeedsOverride` で理由が同じファイルだけ／部分成功を完了と偽らない／他チャット作業中・PendingOp・受理不明・外部実行中・削除保留で止める（強制でも）／`changes.jsonl` を読み書きしない／`ArtifactObserved` と `fileChange` の活動表示は変わらない | `codex/events.rs` の既存テストの更新のみ（判定は P3B-2 に集約） |
| P3B-5 | UI: 2タブ化（Codex報告の削除）・注記・戻すダイアログの文言・要確認の表示と初期オフのチェック・2段目の確認・結果の「強制で戻した」行・新しい理由ラベル・設定のオンオフ・使用量の行・「控えを削除…」。`live.ts`・`App.tsx`・`client.ts` の `backendReported` 参照の置換 | `ui/ChangesDialog.tsx`、`Dialogs.tsx`、設定・使用量の画面、`ui/commands.ts`（無効理由）、`ipc/{client,live}.ts`、`App.tsx` | P3B-1（型）、P3B-4 | `tsc --noEmit` 通過／タブは2つ／戻すボタンは控え基準タブ／要確認は初期オフで2段目の確認を経ないと送られない／取れない値を0や空で代用しない | なし（画面） |
| P3B-6 | 結合テスト（§9 結合）と、司令塔による文書の更新: 正本 §3.14 #1（出所を「控え基準」「Git」に変更、Codex報告の廃止＝合意変更）、#2・合意（2026-10-07 #2 → 2026-10-10: Git基準の控え、帰属不明はファイル単位の2段目の確認で強制可＝合意変更）、§3.14.1（観測に基づく限界・L5 の削除、控え方式の限界の追加）、§13.5（合意済みとして移す）、版更新。DESIGN_P3 #1・#2・§2.2・§2.4 に「P3Bで置換」注記。`parity_table.rs` の注記 | `src-tauri/tests/baseline_git.rs`、文書 | P3B-5 | 一時リポジトリでバイト一致・リポジトリ無変更・HEAD移動（強制不可）・GitBusy・要確認と強制・再照合・控え失敗 | — |
| R4 | 設計担当のレビュー: P3B-3・P3B-4 の差分（送信前の控えと送信の順序、ロック順・デッドロック、終端の後着、切断時、控え失敗で送信を止めない／戻すで何も変えない、強制の範囲が §4.3 の要確認だけか、リポジトリを汚さない、合意違反） | — | P3B-6 | 重大度順の指摘 | — |

順序: P3B-0 ∥ P3B-1 → P3B-2 → P3B-3 → P3B-4 → P3B-5 → P3B-6 → R4 →（指摘の修正）→ 実機確認。

**実機確認（ユーザー、NSIS版）で見ること**: PowerShellで編集させたturnの差分（控え基準タブ）と戻す／戻した後の再確認で「戻し済み」／手で編集後は「要確認」になり、強制なしでは戻らず、2段目の確認で戻せて控えが残る／コミット後は強制も出ず止まる／非Gitフォルダの理由表示／CRLFのファイルがバイト一致で戻る（`git diff` が空）／同じリポジトリの2チャット同時作業で「要確認」／再起動をまたいだturnが「終了未確認」／送信の遅れが体感できる程度か（大きいリポジトリで）。

---

## 11. 決定事項
1. 基準は「HEADの木＋HEADと違うファイルの生バイト」を専用オブジェクト置き場に作る方式（§1.1 D）。ユーザーのリポジトリには書かない。
2. 控えはチャット領域内、チャットごと。自動削除なし。削除はチャット削除と「控えを削除」（明示操作）だけ。
3. B は送信の直前、E は終端観測＋チャット内の作業なし。終端を観測できない区間は終了未確認として扱い、推定で補わない。
4. 控えの失敗で送信を止めない（警告は理由ごとにチャット1回）。設定でオフにできる。
5. 戻す単位は「選んだturn以降」×ファイル。書き戻し先は常に選んだ区間の B。途中のturnだけの打消し（3方向マージ）はしない。
6. 帰属を判別できないファイル（終了未確認・区間の間／後の変更・同時作業）は止めて理由を出し、ファイル単位の2段目の確認でだけ強制できる（ユーザー決定 Q1＝(2)）。書換え前の内容は必ず控える。強制でも、HEAD・ブランチ変更後、Git操作の途中、対象外の種類（リンク・サブモジュール・64MiB超・無視ファイル）、作業フォルダ外、作業中は戻さない。
7. 差分表示は「控え基準」と「Git（HEAD比較）」の2タブ。Codex報告と `fileChange`／turn差分の観測・記録は廃止（ユーザー決定 Q2＝(2)）。
8. 現行の逆適用方式は廃止。旧 `changes.jsonl`・旧 `revert-backup` は消さず、読まない（使用量には計上）。
9. 書込み直前の再照合（M1）、成功分のみ記録（M2）、io_guard は控えと記録の間だけ（L6）、同じ場所の判定はリポジトリルート基準（M3）を引き継ぐ。H2 は区間・パス単位の戻し記録で構造的に解消。

## 回答済み（2026-10-10、ユーザー）
- Q1: (2) 止めたうえで、2段目の確認で「別の変更も含めて控えの内容に戻す」をファイル単位で選べる。強制不可の条件は §4.3「止める」。合意（2026-10-07 #2）の変更として正本 §3.14 #2・§13.5 を更新する。
- Q2: (2) 「Codex報告」タブを廃止し、控え基準とGitの2タブ。#1 の合意の変更として正本 §3.14 #1 を更新する。観測（`FileChangeObserved` 等）は縮小せず廃止（§6）。
