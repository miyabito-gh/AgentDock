# 引き継ぎ（2026-10-07、段階③実装完了・実機確認は未実施）

**追記（段階③）**: 実機確認は2026-10-09にほぼ完了（結果は `app/LIVE_CHECK_P3.md` 末尾。未実施は6-1・6-3・5-3・8-2）。修正バッチ・P3-0〜P3-10・R3まで完了しmasterにコミット済み（設計 `app/DESIGN_P3.md`、レビュー `app/REVIEW_R3.md`）。実機確認は `app/LIVE_CHECK_P3.md`（段階②の未確認分と合わせてNSIS版で）。R3の低L1（side分岐通知の競合）・L2（取込みで変わったファイルの見落とし）・L5（削除ファイル本文とdiffの誤判別）は見送り。確認状況は `codex/parity_table.rs` に版別で持つ（0.160.0は実機確認済みの操作がVerified。未確認はクラウド委任・Plugins・memories・personalityと、同時作業時の戻す(B-7)）。 2026-10-10: #1・#2 はGit基準の控えへ作り直し、実機確認○（同時作業時の扱いB-7のみ未確認、結果は LIVE_CHECK_P3.md 末尾）。`parity_table.rs` は0.160.0の15操作+RevertChangesを確認済みに更新済み。以下は段階②時点の記述。

新しいセッションは、CLAUDE.md（毎ターン読込み）→ 本書 → `app/DEFERRED.md` の「段階②の結果」以降の順で読む。

## 1. 現状

- 段階②のタスク P0〜P8・R2 は完了・検証合格・master にコミット済み。設計は `app/DESIGN_P2.md`、横断レビューは `app/REVIEW_R2.md`。
- 実機確認（`app/LIVE_CHECK_P2.md`、NSIS版）は途中。ユーザーの希望で、**残りの実機確認は後でまとめて行い、実装を先に進める**（2026-10-07）。
- 実機確認で出た不具合は4回に分けて修正済み（下表）。

|回|主な内容|
|---|---|
|1|notLoadedの子でキューが永久保留（子の最新turn未取得と履歴終端の照合）、保留の出口「状態を再確認／確認して今すぐ送る」（ユーザー合意）、ピン・名前変更の接続、モデル受理（`thread/settings/updated`・`thread/resume`応答）、一覧の更新、`ChatLocalFile.hosted` の永続化|
|2|確認済みにできる起動警告（`StartupWarning.acknowledgeable`）、一覧に無いhostedチャットの補完、更新結果の表示、再起動後の帯、子カードの経路チップ|
|3|作業フォルダが `chats\<dirId>\workspace` 配下の一般チャットはアプリ管理、「履歴なし」の削除（no rollout found→Codex側は対象なしで完了）、削除保留チャットを一覧に残す|
|4|一覧の並び（ピン→最近利用→作成時刻、ユーザー操作でだけ更新、`touch_chat_used`）、削除保留の印、`thread/resume` に `excludeTurns: true`（deprecationNotice対策）|

## 2. 実機確認でユーザーが○にしたもの

A1（下書き復元）・A2・A3（警告の確認済み・履歴なしの表示・更新）・B1（notLoadedの子でも自動送信）・★（アプリで作成した会話が再起動後もアプリ管理）・モデルの受理済み表示・子カードの経路・履歴なしの削除。

## 3. 後でまとめて行う実機確認（ユーザー）

- 修正4回目の確認: 一覧の並び（新規が上、ピンは上部、背景活動で動かない）、削除保留の印と送信不可の表示、送信時に「full history」の警告が出ないこと。
- `LIVE_CHECK_P2.md` の B2〜B8、C1〜C7、D1〜D7、E1〜E10、F1〜F6。
- 了承済み: chat.json を失った**開発チャット**（作業フォルダを自分で指定したもの）と段階①のRoaming領域の会話は外部扱いのまま。

## 4. 次の作業（実装を先に進める）

1. **段階②持ち越しの修正バッチ**（`app/DEFERRED.md` の「段階②で持ち越した項目」から。優先度順の提案）
   - R2-L6: 手動送信も送信前に記録（Sending相当）を保存する（送信中の異常終了で受理不明の照合が効かない）
   - 成果物「開く」: プロジェクト外・実行形式（.exe/.bat/.ps1等）は警告か確認を挟む
   - `HostSnapshot` に終了手順の状態（quit）を含める（UI再読込み・トレイ「終了」でダイアログが出ない）
   - 画像貼付けの生バイトIPCに受け側の上限
   - R2-L7: 保存形式のCodex固有名（`AppSettings.codex_executable`、削除工程の印）をバックエンド中立に（保存形式の移行。段階③の前に）
   - wake誤検出（壁時計の飛び）、作業中アーカイブの再起動後の扱い、起動直後の終了確認の未確認チャット は様子見でよい
2. **段階③（18操作、正本§3.14）の分解**: CLAUDE.md の方式どおり。設計判断は designer（Opus）。分解後に CLAUDE.md の段階②の表の下へ段階③の表を追記する。要件にない判断は選択式でまとめて1回ユーザーに確認。
3. 最後に NSIS を作り直し、実機確認の手順（上の3＋段階③分）をまとめて渡す。

## 5. 実機で分かった事実（段階①の分に追加）

- `thread/start` で開始した会話も source は vscode。アプリ管理の根拠はAgentDock側の記録（`hosted`）と作業領域の位置だけ。
- 発話のない thread は `thread/list` に出ず、`thread/delete` は `no rollout found for thread id …` で拒否される。
- `thread/list` 由来の子は最新turnを持たない（notLoaded）。終端は履歴で確認する。
- `thread/resume` 応答に model・reasoningEffort が入り、受理済み表示に使える。`excludeTurns` なしの resume には deprecationNotice（全履歴展開は非推奨）が来る。
- 子の `agent_nickname`（例 Plato）はCodexが付ける呼び名。spawn時の名前（例 luna）は経路 `/root/luna` に出る。

## 6. 運用上の注意（このセッションで分かったこと）

- `agentdock-*` サブエージェントは登録済みで呼べる。
- `isolation: "worktree"` のworktreeが古いコミットから始まることがある。指示に「開始時に `git log -1` を確認し、master最新でなければ `git reset --hard <hash>`」を入れる。並行タスクは後から master を取り込ませてからマージする。
- 改行コード: `host/mod.rs`・`codex/*` などCRLFのファイルがある。編集でCRLF⇔LFが変わると全行差分・全行衝突になる。指示と検証に「改行コードは既存のまま（`git diff --stat` が変更行数相応か）」を入れる。
- ts-rs の生成は `cd app/src-tauri && cargo test --lib export_bindings`。新しい型は `app/src/ipc/types.ts` の再exportを手で足す。
- `package.json` の依存が増えたら本体で `npm install`（`package-lock.json` はgit管理外）。
- NSISビルド: `cd app && npm run tauri build` → `app/src-tauri/target/release/bundle/nsis/AgentDock_0.0.1_x64-setup.exe`（約4分）。
- 利用制限で止まったサブエージェントは、同じIDへ SendMessage で再開できる（未コミット変更は残る）。完了通知だけ来て報告本文が届かないことがあるので、その時は差分を直接確認する。
