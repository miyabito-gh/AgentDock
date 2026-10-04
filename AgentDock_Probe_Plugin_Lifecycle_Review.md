# AgentDock Plugins反映・再信頼・部分失敗の限定検証

更新日: 2026-10-04。要件v0.48、第50節の技術証拠。最終採用・製品受入を意味しない。

codex-cli 0.160.0、試行ごとの専用CODEX_HOME/work、ローカルモデル・MCP・自作pluginのみで実施。既存設定は変更前後のハッシュ一致を確認。既存認証の変更・コピー、クラウド推論、本体実装、HTML変更は行っていない。

## 測定結果

| 条件 | 観測 | 採用判断への意味 |
|---|---|---|
| App Server起動中にCLIでplugin追加 | global Skill一覧と会話別MCP状態一覧に現れる。追加前の会話では明示Skill本文・モデル向けMCP能力なし、新規会話ではあり | 一覧表示と実利用を分ける |
| MCP reload直後と1500ms後 | 上記の既存／新規会話の差が維持 | reload成功だけで会話全能力の反映完了としない |
| 信頼済みHookの呼出し先スクリプトだけ変更 | 定義ハッシュは同じ、変更後の自作スクリプトが実行された | 定義ハッシュによる信頼をスクリプト内容の完全性保証としない |
| インストールキャッシュ内Hook定義を変更 | 稼働中は新規会話・MCP reload後1500msでも旧ハッシュ・旧定義が継続。再起動後は新ハッシュ、modifiedとなり抑止 | 今回の変更経路は稼働中再読込みが未成立。再起動で再評価を確認 |
| 新定義を確認し専用設定に正確なハッシュを信頼登録して再起動 | Hook実行が再成立 | 自動再信頼は行わず、定義の確認と信頼状態を分ける |
| 専用configを一時read-onlyにしてCLI add | exit 1、設定は不変だがcache配置あり。CLI list installedは空 | エラーを原状復帰と扱わない |
| 同条件のCLI remove | exit 1、設定のplugin項目が残りcacheは削除。CLI list installedは空 | 一覧だけで削除完了を確定しない |

partial failureは専用configの属性を必ず復元。add失敗後の明示的なfixture再追加は成功した。異なる失敗点や同時設定変更までの保証ではない。Hook変更はローカルキャッシュ編集であり、marketplaceバージョン更新の証拠ではない。Skill本文の注入とMCP能力の広告を測り、全MCPツール実行・Apps/OAuth・非同期Hook・全能力を保証しない。

## 証拠と試行の扱い

- live追加／Hook再信頼: `probes/app-server/runs/2026-10-04T10-36-18-033Z/result.json`。同じ試行内の別測定2件。
- 部分失敗: `probes/app-server/runs/2026-10-04T10-33-01-637Z/result.json`。
- 初回live追加 `2026-10-04T10-32-42-440Z`、初回Hook `2026-10-04T10-33-01-089Z` も保持。待ち時間を追加した選択試行と区別。
- 集約 `probes/app-server/batch-result.json` は18測定を16選択試行から統合。一度の全suite合格や製品合格ではない。不成立・未完了試行も残す。

現在公開の[Hooks文書](https://learn.chatgpt.com/docs/hooks)は定義変更後の再信頼とplugin導入だけでは信頼しないことを説明する。[App Server文書](https://learn.chatgpt.com/docs/app-server)とともに参照し、現在文書を固定版0.160.0の全動作保証に読み替えない。

## 責任境界と次の条件

CLI補助は追加・削除等の実行経路候補。AgentDockは要求・終了コード・設定状態・実体状態・会話への反映・Hook信頼を別々に記録し、食い違いは要照合として保持する案を比較する。自動rollbackや再信頼を前提にしない。操作中の関連会話への適用範囲を確定できるまで、全会話への反映完了とは表示しない。

18操作・D01〜D07は維持。App Server第一候補、stdio優先評価、Tauri 2第一UI検証候補も継続し、選定確定にはしない。残る条件は同時設定変更、marketplace更新・削除後の既存会話、Apps/OAuth、Hook他イベント、30分idle／Windows sleepと安全な購読、停止対象の所有・実終了照合。今回確認した3測定でこれらを完了扱いしない。
