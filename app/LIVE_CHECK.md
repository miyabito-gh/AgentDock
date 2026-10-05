# 段階① 実機確認の手順（ユーザー実施）

実際のCodex App Server・実認証・実推論を使うため、ユーザーが行う。利用枠を少量消費する。`~/.codex`は共有され、AgentDockは変更しない（確認4で前後hashを見る）。

## 0. 準備

```powershell
cd C:\Users\wmasa\Documents\Rust\AgentDock_claude\app
npm install
npm run tauri dev
```

初回はRustのビルドに数分かかる。起動しない・窓が白いときは、エラー全文をClaudeへ貼る。

## 1. B1 実認証スモーク（任意・先に1回）

```powershell
cd C:\Users\wmasa\Documents\Rust\AgentDock_claude
node probes/app-server/live-smoke.mjs 'C:\Users\wmasa\AppData\Local\Programs\OpenAI\Codex\bin\codex.exe'
```

`probes/app-server/latest-live-result.json` ができる。成否だけClaudeへ伝える。

## 2. 確認項目（画面で。結果は○／×／気づきで一括報告）

|#|操作|期待|
|---|---|---|
|1|起動する|既定パスのcodex.exeへ自動接続。版（0.160.0）・会話一覧・モデル一覧が出る。未導入・版違いなら警告表示（接続エラー状態。done/failedにはならない）|
|2|左下のモード|「実接続」が既定。「モック」に切り替えると仮データのシナリオが出る|
|3|新規チャット（作業フォルダのフルパス・モデル・最初の依頼）|新しい会話ができ、返答が逐次表示される。初期状態はidle→実行中→完了|
|4|`~/.codex/config.toml`・`auth.json`|操作の前後でhash・更新日時が変わらない（`Get-FileHash`で確認）|
|5|既存の保存済み会話を開く|履歴が出る。開くだけではresumeされない（実行中表示にならない）|
|6|保存済み会話へ送信|ユーザー送信時のみresumeして送られる|
|7|承認が出る依頼（ファイル書込みなど）|承認カードがホスト通知で即時表示され、許可・拒否で更新される|
|8|質問が出る依頼|質問カードが出て回答できる|
|9|実行中に中断|「中断を受け付けた」表示。10秒で停止の証拠がなければ「停止未確認」案内（停止確定とは表示されない）|
|10|サブエージェントを使う依頼（例「3つの子エージェントに別々に調べさせて」）|ドックに子・孫が出る。完了・失敗通知が2秒窓で集約されるかは段階②。ここでは表示のみ|
|11|送信直後にネットワーク切断／codexプロセスを終了（タスクマネージャ）|状態はdone/failedにならず、鮮度が切断になる。受理不明なら再送ボタンなし、「履歴と照合」のみ|
|12|アプリを閉じる|codexプロセスが残らない（タスクマネージャで確認）。固まらない|
|13|開発時にもう一度起動（StrictMode二重マウント）|接続が二重にならない|

## 3. 実機で特に見てほしい点（レビューより）

- `thread/read`（includeTurns）で空のチャットがエラーにならないか。turnsの並び順、`startedAt`がnullになるか、`itemsView`が`full`か（`summary`だと受理不明の照合が永久に確定しない）。
- 権限拒否`{"permissions":{},"scope":"turn"}`が受理されるか。
- `thread/closed`が出る条件（購読中でも来るか）。`systemError`が出る契機。
- `ancestorThreadId`（experimental）で孫まで返るか。
- 100agent規模は段階①の範囲外（B01）。

## 4. 報告のしかた

項目番号ごとに○／×／気づきを書いて、×はエラー文または画面のスクリーンショットを添える。Claudeが原因を切り分けて修正する。
