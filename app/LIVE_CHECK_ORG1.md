# P-ORG-1 実測の手順書（AI組織制御エンジン）

目的: `app/DESIGN_ORG.md` §5.2 の P-ORG-1 の4項目を実機で測る。コードの実行はユーザーが行う。

## 実行前の注意

- Codexの利用枠を少し使う（全項目で10turn前後、どれも数十トークン級の短い依頼）。
- ChatGPTログイン済みであること。`~/.codex` の設定・認証は読むだけで書き換えない（実行後に前後のハッシュ照合結果が出る）。
- 承認要求などが来ても応答せず、記録して中断する。中断した項目は「未確認」として扱う。
- 項目2は一時フォルダに使い捨てのGitリポジトリを作り、終了時に削除する。`git` が使えること。
- 作業はリポジトリ直下から行う。

## 実行

全項目を一度に実行する。

```powershell
node probes/app-server/org-probe.mjs all
```

項目ごとに実行する場合は `1`〜`4` を指定する。

```powershell
node probes/app-server/org-probe.mjs 1
```

```powershell
node probes/app-server/org-probe.mjs 2
```

```powershell
node probes/app-server/org-probe.mjs 3
```

```powershell
node probes/app-server/org-probe.mjs 4
```

codex.exe の場所が違う場合は第2引数で指定する。

```powershell
node probes/app-server/org-probe.mjs all "C:\path\to\codex.exe"
```

## 期待結果と要報告

|項目|期待結果|こうなったら要報告|
|---|---|---|
|1 outputSchema|`conforming prompt` で `jsonParse: true`、`answerIsInteger`／`unitIsString` が true|`jsonParse: false`、`error` あり、`agentMessageItems` が2以上、スキーマ無視（conflicts側が `hello` のまま通る／拒否される／エラーになる、いずれも要報告）。次のturnにスキーマが残る（`next turn without outputSchema` が JSON）かどうかも結果として知りたい|
|2 sandbox+worktree|`worktreeCreated: true`、thread作成成功、`replyMentionsFileContent: true`、`writeTestFileExistsOnDisk: false`、`worktreeStatusPorcelain` が空|書込みが成功した（ファイルが存在／statusが空でない）、thread作成が失敗、`serverRequests` に何か記録された|
|3 並列3thread|3つとも `status: completed`、`ownMarker: true`、`otherMarkers: 0`、`overlapped: true`（wallMsが各turn時間の合計より短い）|直列化されている（`overlapped: false`）、取り違え、エラー、`unconfirmed`|
|4 tokenUsage|`updatedCount` が1以上。`total` が turn をまたいで増え、`last` がそのturn分になる（`interpretation.totalEqualsPreviousPlusLast: true`）|通知が来ない（`updatedCount: 0`）、粒度が違う（turn内で複数回、`total` が増えない等）|

共通: 結果の `configUnchanged` と `authUnchanged` が両方 `true` であること。`false` なら必ず報告。`unconfirmed`（120〜180秒で完了を確認できなかった）は成功でも失敗でもなく「未確認」。再実行して同じなら報告。

## 項目2b: 項目2の失敗原因の切り分け（追加）

前回、項目2のコマンドが全て `exitCode: -1`（起動できず）だった。原因が「worktreeをcwdにしたこと」か「read-only sandbox自体」かを切り分ける。利用枠は3 turn。`all` には含まれないので個別に実行する。

```powershell
node probes/app-server/org-probe.mjs 2b
```

同じ使い捨てリポジトリで、次の3ケースを順に1 turnずつ実行する（hello.txtを読み、ファイルを1つ作る依頼）。承認要求が来ても応答せず記録して中断する。

|ケース|cwd|sandbox|
|---|---|---|
|a|通常フォルダ（worktreeでない）|read-only|
|b|worktree|read-only（項目2の再現）|
|c|worktree|workspace-write（approvalPolicy=never）|

見る欄は各ケースの `commandExecutions`（`status`／`exitCode`／`outputHead`）、`replyHead`、`replyMentionsFileContent`、`writeFileExists`。

### 何が分かれば良いか

|結果|読み方|
|---|---|
|a・b・c とも `exitCode: -1`|read-only／worktreeに関係なくこの環境でコマンドが起動していない（sandbox起動失敗など）。`outputHead` のエラー文と `stderrTail` を報告。AgentDockの通常チャットで同じ依頼が動くかも確認したい|
|a だけ成功（読取り成功、`replyMentionsFileContent: true`）|worktree固有の問題（`.git` がファイルであること等でread-only sandboxが起動できない可能性）|
|a・b 成功、c 失敗|workspace-writeだけ問題。Worker役の前提に影響|
|a・b 失敗、c 成功|read-only sandboxだけが問題。Reviewer等はworkspace-write＋指示で読取り専用にする案を検討|
|a・b・c 成功|項目2は一時的な失敗だった可能性。b の再現性を見て再実行|

共通の確認: aとbで `writeFileExists: false`（書込み拒否）、cで書込みが成功するか、`cwdStatusPorcelain`、`serverRequests`（承認要求の有無）。

## 結果の渡し方

標準出力の最後の行に保存先が出る。`probes/app-server/runs/org-probe-<日時>.json`。中身は件数と主要フィールドだけで、メールやトークンは含まない（モデルの短い返答の冒頭のみ含む）。

最新のファイルの場所を表示する。

```powershell
Get-ChildItem probes/app-server/runs/org-probe-*.json | Sort-Object LastWriteTime | Select-Object -Last 1 -ExpandProperty FullName
```

そのファイルの中身（またはパス）を司令塔へ渡す。

## 診断: node_repl MCPを起動しない場合

node_repl MCPを起動しないとsandboxが動くか（`node_repl.exe` のACL更新で os error 32 になる疑いの切り分け）を見る。`~/.codex/config.toml` は変更せず、起動引数 `-c` だけで上書きする。結果JSONの `launchOverrides` に記録される。

```powershell
node probes/app-server/org-probe.mjs 2b --c mcp_servers.node_repl.enabled=false
```

効かない場合の代替: 環境変数で渡す（`$env:ORGPROBE_CODEX_ARGS='mcp_servers.node_repl.enabled=false'`）。それでもMCPが起動する場合は `mcpServer/startupStatus/updated` の通知を確認し、`--c features.<name>=false` 等を別途検討する。

診断：プラグインを全部無効にするとsandboxが動くか。動けば原因のプラグインを絞る。`--no-plugins` は `config.toml` を読むだけで、`[plugins."<id>"]` 全部と `node_repl` を `-c` で無効化する（`--c` と併用可。渡した引数は結果JSONの `launchOverrides` に記録）。

```powershell
node probes/app-server/org-probe.mjs 2b --no-plugins
```
