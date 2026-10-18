# 実機確認 組織実行のグラフ定義（S-G）

対象: ブランチ `feature/org-engine`（G-A0〜G-A6）。設計 `app/DESIGN_ORG_GRAPH.md`、レビュー `app/REVIEW_ORG_GRAPH.md`。実機（Windows 11・実Codex 0.162.1）で行うのはユーザー。
準備（導入・確認用リポジトリ・先に知っておくこと・報告用の場所）は `app/LIVE_CHECK_ORG.md` の 0 章と同じ。ここでは繰り返さない。
記録: 項目番号ごとに ○／×／△。×・△は「何をしたら何が出たか」を1行。

## 0. 準備（この確認で追加するもの）

1. NSIS を作り直して上書きインストールする（司令塔がビルド済みなら更新日時を確認するだけ）。
2. 定義ファイルの場所: `%LOCALAPPDATA%\com.agentdock.app\org\graphs\`。初回起動で `standard.json` と `quality-loop.json` が作られる（既存ファイルは上書きされない）。

```powershell
Get-ChildItem "$env:LOCALAPPDATA\com.agentdock.app\org\graphs"
```

3. 利用枠: 品質向上周回は最大で 24 turn（定義から計算）。依頼は小さくする。**turn 上限（開始ダイアログ）は初期 30**。

## 1. 確認項目

### 1. テンプレート1（標準）の回帰なし

操作: 開始ダイアログでテンプレート「計画して実行（標準）」のまま、`README.md の末尾に「hello」と1行足す`、作業フォルダに `ad-org\repo` で開始。

期待結果:
- 以前（`LIVE_CHECK_ORG.md` 項目1）と同じ流れ: 計画（承認カードまたは自動承認）→ Worker → Reviewer → 完了。`main` は変わらず、ブランチ `agentdock/org/...` にだけコミット。
- 画面にグラフの流れ一覧が出ても、動きは変わらない。

要報告: 以前と違う動き、止まる、`main` が変わる。

### 2. 品質向上周回（rework で直る）

操作: テンプレート「計画→実装→検証を周回（品質向上）」を選ぶ。開始ダイアログに**見積り**（最大 turn・目安トークン）が出ることを確認。依頼はわざと不足が出やすいもの: `hello.txt を作り、1行目に hello、2行目に今日の日付(YYYY-MM-DD)、3行目に空行を入れず "end" と書く。README.md にも hello.txt の説明を1行足す`。作業フォルダ `ad-org\repo`。

期待結果:
- 流れ一覧に plan → build → verify。各ノードの状態は文字＋アイコン。
- verify が rework を返したら、build が**同じブランチ**（前のコミットの続き）で直し、再び verify へ。画面に「第2周／最大3周」と判定の履歴（rework → pass など）が出る。1周目で pass なら周回は出ない（正常。その場合は依頼をさらに難しくして再試行）。
- 完了で帯「ブランチ・差分を確認して取り込んでください」。`main` は変わらない。
- 判定を読めないときは「未取得」と出る（0 や空欄にならない）。

```powershell
git -C "$env:USERPROFILE\Documents\ad-org\repo" log --oneline --all --graph
```

要報告: 周回が増えない／上限を超えて回る／verify が書込みをした／`main` が変わった／判定履歴が実態と違う。

### 3. 周回の上限（理由を書いてもう1周／この結果で受け入れる）

操作: 定義の写しで上限を1回にする。

```powershell
$d = "$env:LOCALAPPDATA\com.agentdock.app\org\graphs"
(Get-Content "$d\quality-loop.json" -Raw) -replace '"id": "quality-loop"','"id": "quality-loop-1"' -replace '"maxRounds": 3','"maxRounds": 1' | Set-Content "$d\quality-loop-1.json" -Encoding utf8
```

設定 →「グラフ定義」→「再読み込み」でテンプレート `quality-loop-1` が選べることを確認し、項目2と同じ依頼で開始。verify が 2 回 rework を返すと上限に達する（返さなければ依頼を厳しくして再試行）。

期待結果:
- 上限到達で「周回の上限」カード。ボタン「理由を書いてもう1周…」と「この結果で受け入れる…」。**自動では進まない**。
- 「理由を書いてもう1周」: 確認ブロックの後、上限が1回だけ増え build が理由つきでやり直す。理由が長すぎれば拒否のメッセージが出る（黙って切られない）。
- 別の実行で「この結果で受け入れる」: 確認ブロックの後、送信されずにワークフローが閉じる（完了）。
- どちらも確認ブロックを通らずには実行されない。

要報告: 確認なしで進む／自動でもう1周する／受け入れ後に turn が送られる／上限後も無言で回り続ける。

### 4. 完全終了 → 再起動 → 再開（周回の途中）

操作: 項目2または3の周回の途中（2周目の build 実行中など）でトレイの「終了」→ 再起動。

期待結果:
- 再起動後、状態は**自動では先へ進まず**、照合の後「要対応」または再開待ちになる。終了を failed/cancelled に変換しない。
- 「再開」を押すと周回回数が保たれたまま続く（「第N周」が戻らない）。前の報告と指摘が次の build のプロンプトに入る（役割チャットの先頭の指示に前の版が含まれる）。
- 定義ファイルを再起動前後で書き換えても、動いているワークフローは**開始時の写し**で続く（案内「定義が変わっています…」が出る）。

```powershell
Get-ChildItem "$env:LOCALAPPDATA\com.agentdock.app\org" | Sort-Object LastWriteTime | Select-Object -Last 1 | ForEach-Object { Get-ChildItem $_.FullName }
```

（`graph.json`・`roles.json`・`slots\` があること）

要報告: 再起動で勝手に送信が起きた／回数が戻った／写しがなく今の定義で動いた。

### 5. turn 上限

操作: 開始ダイアログで最大 turn を `6` にして項目2の依頼で開始。

期待結果: 周回の途中で `blocked`（turn 上限）で止まり、それ以上送らない。自動で上限を上げない。

### 6. 誤りのある定義の拒否

操作: 条件のないフィードバック辺を持つ定義を置く。

```powershell
$d = "$env:LOCALAPPDATA\com.agentdock.app\org\graphs"
(Get-Content "$d\quality-loop.json" -Raw) -replace '"id": "quality-loop"','"id": "bad-loop"' -replace ', "when": \{ "slot": "verdict", "path": "verdict", "eq": "rework" \}','' | Set-Content "$d\bad-loop.json" -Encoding utf8
```

設定 →「グラフ定義」→「再読み込み」。

期待結果: 設定の一覧と開始ダイアログに `bad-loop` の誤りの理由が出て、選べない（開始できない）。他のテンプレートは選べる。

後始末: 確認が終わったら `quality-loop-1.json`・`bad-loop.json` を削除する。

```powershell
Remove-Item "$env:LOCALAPPDATA\com.agentdock.app\org\graphs\quality-loop-1.json","$env:LOCALAPPDATA\com.agentdock.app\org\graphs\bad-loop.json"
```

### 7. 守ること（目視）

- 役割チャットに手で打ち込めない（Q9）。承認要求に AgentDock が応答していない。
- 画面を閉じても、確認のない自動マージ・自動の再送がない。
- `~/.codex/config.toml` の変更は Codex 本体の trust 追記だけ（AgentDock は書かない）。

## 2. 報告

項目ごとの ○／×／△と、×・△の1行説明。気になった表示崩れも添える。
