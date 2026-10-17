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

## 結果の渡し方

標準出力の最後の行に保存先が出る。`probes/app-server/runs/org-probe-<日時>.json`。中身は件数と主要フィールドだけで、メールやトークンは含まない（モデルの短い返答の冒頭のみ含む）。

最新のファイルの場所を表示する。

```powershell
Get-ChildItem probes/app-server/runs/org-probe-*.json | Sort-Object LastWriteTime | Select-Object -Last 1 -ExpandProperty FullName
```

そのファイルの中身（またはパス）を司令塔へ渡す。
