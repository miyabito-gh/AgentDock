# P-ORG-4c 実機確認の手順書（sandbox健全性チェック・trust照合）

根拠: `app/DESIGN_ORG_P4.md` §5.1（sandbox）、§6（起動オプション）、§7.1（共通親フォルダ案 V1〜V3）。コードの実行はユーザーが行う。

## 実行前の注意

- どれも turn を送らない（Codex の利用枠を使わない）。`thread/start` と読取り・`command/exec`（`cmd.exe /d /c cd`、readOnly・ネットワークなし）だけ。
- `~/.codex` の `config.toml`・`auth.json` はプローブが書き換えることはない。ただし Codex 自身が `thread/start` のときに `config.toml` へ trust を追記することがある（それが B の確認対象）。
- 作業はリポジトリ直下から行う。`probes/app-server/org-probe.mjs`（P-ORG-1 のプローブに4cのモードを足したもの）を使う。
- 結果の JSON は `probes/app-server/runs/org-probe-<時刻>.json` に出る。コンソール表示と合わせて報告する。

## A. sandbox 健全性チェック（`sandboxcheck`）

目的: ①回避オプションなしで `thread/start` 後の `command/exec` が失敗する（`os error 32` の競合が再現する）か、②回避オプションありで成功するか、を見る。

### A1. 回避なし

```powershell
node probes/app-server/org-probe.mjs sandboxcheck
```

### A2. 回避あり（`features.plugins=false` と `mcp_servers.node_repl.enabled=false` を起動引数で付ける。`cua_repl` は付けない）

```powershell
node probes/app-server/org-probe.mjs sandboxcheck --no-plugins
```

### A3. 任意: thread 単位の上書き（回避なしの起動のまま、`thread/start` の `config` で同じ設定を渡す）

```powershell
node probes/app-server/org-probe.mjs sandboxcheck --thread-override
```

### 期待結果と要報告

|実行|見る欄|期待|要報告|
|---|---|---|---|
|A1|`readiness`、`exec.exitCode`、`exec.stderrHead`、`stderrMentionsSetupRefreshErrors`|競合が再現するなら `passed: false`（`exec.error` か `exitCode` が 0 以外、`stderrMentionsSetupRefreshErrors: true`）|`passed: true`（再現しない）。その場合は実行時の検出（`setup refresh had errors`）を主にし、開始前チェックは補助に位置付け直すため、判断を仰ぐ|
|A2|同上|`readiness: "ready"`、`exec.exitCode: 0`、`passed: true`|`passed: false`、起動自体の失敗（`cua_repl` などの未定義名で App Server が落ちていないか。`stderrTail` を見る）|
|A3|同上|A1 が失敗で A3 が成功なら、回避を thread 単位に狭められる|A1 が失敗で A3 も失敗（thread 単位では効かない）、`thread/start` が `config` を拒否した（エラー文）|

- `readiness` が `notConfigured`／`updateRequired` のときは `exec` は実行されない（`skipped`）。その値を報告する（`setupStart` は呼ばないので、Codex 側で設定する）。
- 実行が止まった・応答がない場合は「未確認」。再実行して同じなら報告。

## B. trust 追記の確認（`trust`）

目的: 共通親フォルダ案の要否を決める（§7.1）。`thread/start` の前後で `config.toml` の `[projects.*]` 見出しが増えるかを見る。見出しの一覧は全件は出さず、増減した分と件数だけを出す。

### 準備

- Codex で一度開いて trust 済みになっている Git リポジトリを1つ決める（パスを `<repo>` とする）。未 trust だとプローブは何もせず止まる。
- 念のため `config.toml` を控えておく（任意）。

```powershell
Copy-Item "$env:USERPROFILE\.codex\config.toml" "$env:USERPROFILE\config.toml.before-org4c"
```

### B1. V1 と V2

```powershell
node probes/app-server/org-probe.mjs trust --repo "<repo>"
```

- V1: `<repo>` の git worktree を `%LOCALAPPDATA%\com.agentdock.app\worktrees\agentdock-orgprobe-wt-<時刻>` に作って cwd にする。
- V2: 同じ親の下の Git でない普通のフォルダ（`agentdock-orgprobe-plain-<時刻>`）を cwd にする。
- どちらも終了時に worktree／フォルダを削除し、`thread/archive` を試みる。

|見る欄|期待|読み方|
|---|---|---|
|V1 の `addedHeadings`|空|空なら Codex は worktree を元リポジトリとして扱っている。共通親フォルダ案は不要。「Planner の cwd が未 trust のときだけ1件増える」で確定|
|V1 の `addedHeadings`|1件以上|共通親フォルダ案を試す余地がある。B2（V3）へ|
|V2 の `addedHeadings`|比較用（増えるのが普通）|増えた場合の見出しが `agentdock-orgprobe-plain-` を含むことの確認|

要報告: `thread` が開始できない（`threadStarted: false`・`error`）、`removedHeadings` がある（見出しが減った）、`archived` が失敗（`thread/archive` の失敗文。作った thread の整理に影響するため）。

### B2. V3（B1 の V1 で増えたときだけ）

1. 自分で `config.toml` を控えたうえで、次の2行を末尾に手で追加する（AgentDock・プローブは書かない。パスは自分の LOCALAPPDATA に置き換える）。

   ```
   [projects.'C:\Users\<あなた>\AppData\Local\com.agentdock.app\worktrees']
   trust_level = "trusted"
   ```

2. V1 だけを新しい worktree でもう一度実行する。

```powershell
node probes/app-server/org-probe.mjs trust-v1 --repo "<repo>"
```

|見る欄|読み方|
|---|---|
|V1 の `addedHeadings` が空|共通親フォルダ案は有効。AgentDock は書かないので、採るなら「利用者が手で1回追加する」案内になる（改めて報告）|
|V1 の `addedHeadings` が増える|共通親フォルダ案は無効|

### 後始末

- この確認で増えた見出しのうち、名前に `agentdock-orgprobe-` を含むものだけは手で消してよい（許可済み）。それ以外の見出しや他の設定は触らない。V3 で手で足した2行は、案を採らないなら消してよい。
- 控えを取った場合: 不要になったら削除する。

```powershell
Remove-Item "$env:USERPROFILE\config.toml.before-org4c"
```

## C. AgentDock 側の確認（任意・ビルド後）

- 設定 →「Codex」に「sandbox の競合を回避する（プラグインを無効にして Codex を起動）」がある。既定はオフ。
- オンにして保存しても接続中の App Server は再起動されず、「AgentDock の再起動後に有効になります」が表示される。再起動後に接続し直すと、表示が消える。
- オンの間は computer-use などのプラグインが使えなくなる（説明文のとおり）。確認が済んだらオフに戻す。
