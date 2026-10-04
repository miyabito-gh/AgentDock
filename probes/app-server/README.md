# AgentDock 小規模App Server検証

ユーザーの2026-10-04の承認に基づく独立検証モジュール。製品実装・モックとは分離する。

## 実行

Node.js 24.10.0、外部パッケージ不要。対象版以外は停止する。

```powershell
node probes/app-server/probe.mjs 'C:\Users\wmasa\.vscode\extensions\openai.chatgpt-26.930.31730-win32-x64\bin\windows-x86_64\codex.exe'
```

VS Code拡張の配布物からexeを直接起動する。VS Codeアプリを起動・利用せず、製品の実行時依存決定でもない。PATHのcodexは今回0.159.0-alpha.12.1だった。旧26.917.62051の同梱exeは0.155.0-alpha.16.3。新26.930.31730の同梱exeは0.160.0。既存のユーザー申告を訂正するのではなく、今回検証した実ファイルを特定したもの。

## 隔離

- 毎回runs/<日時>/codex-homeとworkを新規作成。CODEX_HOMEは子プロセスだけ指定。
- 認証保存はfile指定。既存auth.jsonをコピーせず、既存アカウントへログインしない。
- モデルは127.0.0.1のResponses模擬サーバー。gpt-5.4は送信形式確認用のモデル識別子で、実モデル推論ではない。
- MCPは付属のローカルfixtureのみ。設定変更は検証専用config.tomlだけ。
- serverからの承認・動的ツール要求は拒否。模擬モデルはコマンド・ファイル変更・子agentを要求しない。
- 原本config.tomlの前後hashを比較。これは原本設定の不変確認であり、既存環境全ファイルの完全監査ではない。
- WebSocket試験は127.0.0.1のみの一時listener、認証なし。実験用であり製品transportやauth方式の選定ではない。
- owned client/serverはfinallyで閉じる。Windows子孫プロセスすべての終了保証を検証したものではない。
- schema生成・Plugins install/uninstall・クラウド推論・製品実装は行わない。

## 出力

latest-result.jsonが最新結果。各runs/<日時>/result.jsonは前の試行を保持する。要求本文・認証headerは記録せず、モデル識別子・tier・ツール名のみ記録する。実会話履歴の代わりに検証専用履歴が生成される。自動削除しない。

## 判定範囲

passは各assertionの条件成立だけを意味する。最後の接続断の状態は観測値として保存し、suiteのpassを接続耐久性保証にしない。
最終13ケースと原本設定不変チェックの計14件がpass。AgentDock全体、Plugins無効化、子孫停止、キュー付き復旧の合格ではない。
結果解釈は ../../AgentDock_Probe_Review.md を参照。


## 追加検証（正本v0.44）

追加モジュールは extended-probe.mjs。対象exeを第1引数、任意の第2引数に測定名をカンマ区切りで指定すると、該当手順だけを実行する。省略時は6手順。元のprobe.mjsとlatest-result.jsonは前回14チェックの記録として保持する。

追加手順は実local fixture pluginのCLI追加・会話単位無効化・再有効化・CLI削除＋reload、保存キューcold resume、TCP全切断12条件、warmキューと通常WS close、親子孫生成と個別中断、管理実行interrupt／terminate／clean。追加パッケージ・schema生成・実クラウド/authは使わない。

管理実行のみread-only＋on-requestで正確なfixture command/cwdに一致する単発承認を返す。fixtureはPID出力と45秒待機のみで、永続execpolicyを追加しない。その他のserver要求は拒否。local plugin管理・設定・履歴の書込みは各runのCODEX_HOME内だけ。孫用max_depth=2も専用config限定。

latest-extended-result.jsonは最後のfocused runであり全6手順の証拠ではない。採用した6測定の統合はbatch-result.json、生成はsummarize-batch.mjs。5試行からの選択で、1回の全suite合格ではない。trialsに過去の不成立を保持。partial-result.jsonとdisconnect-matrix.jsonは途中の観測を残す。08:37:28の外部停止介入は当runのintervention.jsonを参照。

TCP破棄と通常close handshakeを区別する。close応答未確認を全接続断成立と扱わない。passは手順完了であり、親再帰停止・キュー安全性・全plugin能力・OS全子孫停止の合格ではない。詳しくは../../AgentDock_Probe_Batch_Review.md。

## idle・所有・Plugins更新の追加測定

以後の測定追加により、第2引数省略時は旧6手順だけではない。**測定名を明示してfocused runを行う。** `idle_31min_read_recovery` は明示選択時だけ31分の実時間待機を行い、無指定では実行しない。サーバーのgrace timerを短縮せず、観測中にRPCやresumeを行わない。実Windows sleepは起こさない。

追加名は `plugin_concurrent_cli_config`、`plugin_cli_api_config_version_conflict`、`plugin_version_refresh_remove_existing`、`plugin_git_marketplace_upgrade_loopback`、`managed_owner_scope_reconciliation`、`read_subscription_and_explicit_continue`、`read_only_active_partial_polling`。各測定は新しい試行ディレクトリで実施する。

Git marketplace測定は試行内だけにsource/bare fixture repositoryを作り、localhost HTTPから実fetch・upgradeを行う。本体ワークスペースをGit化せず、実cloud marketplace・認証情報は使わない。local marketplaceへのupgrade拒否は別に記録する。Hookの信頼は専用fixtureの正確なkey/hashだけ。

所有照合は今回fixtureのPIDだけを出力するCIM読取りを含む。サンドボックスのアクセス拒否は取得成功にせず、必要な昇格許可を得て再実行した。初回と補強前のSkill測定は `evidence-assessment.json` を併記して保存し、元resultを改変しない。

Skill本文の全要求内boolは履歴残存も含む。更新／削除の新規注入判定はinput位置・opaque hash・token出現数を用い、fresh会話と比較する。要求本文・認証headerを保存する方式へ拡張していない。read-only部分本文測定の保存データは自作fixture本文のみ。

詳しくは [追加検証レビュー](../../AgentDock_Idle_Stop_Plugin_Followup_Review.md)。source-followup/manifest.jsonは追加した対象タグソースの取得記録。batch-resultは選択試行の統合であり、全suite・製品受入ではない。

## 実認証スモーク（B1、2026-10-05許可）

`live-smoke.mjs` は共有既定CODEX_HOME（A3）の既存ChatGPTログインで、stable APIのみ・ephemeral・read-onlyの一般チャット1ターンを実推論する。account/read、rateLimits、model/list、thread/start、turn/start（effort=low、固定トークンの返答確認）、thread/readを記録。メール・トークン・認証header・自由文本文は保存せず、config.tomlの前後hashとauth.jsonの変化有無を記録する。結果は `runs/<日時>-live/result.json` と `latest-live-result.json`。利用枠を少量消費する。

```powershell
node probes/app-server/live-smoke.mjs 'C:\Users\wmasa\AppData\Local\Programs\OpenAI\Codex\bin\codex.exe'
```

## 生成schema

`schema/0.160.0/` に `generate-json-schema`／`generate-ts` の通常版と `--experimental` 版を保存。`methods-*.txt` はClientRequest／ServerRequest／ServerNotificationのメソッド一覧。
