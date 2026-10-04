# Plugins管理：対象版APIとCLI補助経路
確認日: 2026-10-04。ソース対象ref rust-v0.160.0。公開文書は確認日現在の内容。実行なし。

## 結論
App Serverのplugin/list・read・install・uninstallは対象版に実装があるが、現在の公式文書は本番クライアントで呼ばないよう案内している。正式採用経路にはしない。一方、codex pluginのCLI補助経路は公式案内と対象版ソースの両方に根拠があり、候補として追加する。M19と合意済みのアプリ内管理要件は維持し、完成扱いにはしない。

## API存在と利用制約
[公式App Server文書](https://learn.chatgpt.com/docs/app-server)のPlugins欄は4メソッドを開発中として本番利用を控えるよう案内。対象版common.rsの899・920・1022・1027行に要求型があり、これら4つの宣言直前にはexperimental属性がない。この属性がないことは安定性や本番利用保証ではない。前回のbackgroundTerminalsのexperimental opt-inと今回の本番利用制約を混同しない。

plugins.rsではinstallがmanager.install_plugin等に進み、設定再読込・キャッシュ無効化・MCP runtime無効化・hook runtime更新を行う（1462–1538行）。uninstallもmanagerに委譲し、再読込後の実効変更を扱う（1966–1994行）。エラー種別には設定永続化やキャッシュ削除の失敗がある。単なるアプリ専用表示変更ではない。

## CLIの補助候補
[公式Developer commands](https://learn.chatgpt.com/docs/developer-commands)はcodex pluginをStableと記載し、add/list/removeとJSON出力を案内。これは現在の文書評価。対象版ソースにも以下が存在する。

|操作|タグソースの根拠|成立に残る条件|
|---|---|---|
|一覧|plugin_cmd.rs 99–115、449–495: list --json、installed/availableと状態・policy等|キャッシュ・取得失敗・対象scope・更新時期|
|追加|80–97、137–205: add --json、local/remote managerへ委譲|管理ポリシー、認証、書込み先、部分失敗、起動中threadへの反映|
|削除|118–134、645–695: remove --json、cache errorも返す|利用中会話への影響、remote/local差、削除所有範囲|
|marketplace管理|59–78: Marketplace subcommandへの委譲|個別の追加・削除・更新契約は今回未照合|

CLIはPluginInstallのApp Server要求を送るwrapperではなく、PluginsManagerを直接呼ぶ経路が確認できる。App Server管理APIを呼ばずに一覧・追加・削除を行う補助候補。ただし安定CLIという分類だけでアプリ内機能すべての同等性を確定しない。

731–753行はCODEX_HOME、既存設定、認証managerを読み込む。アプリ専用領域へ完全に閉じるとは推定しない。remote呼出のcallbackはNoneで、既存App Serverの反映完了をこのCLI応答だけで保証しない。config.tomlを無断上書きしない合意との整合は、manager内部の変更範囲を追加確認する。

CLI一覧のauthPolicyは認証方針の情報であり、実際の接続済み・callable状態と同一ではない。追加・削除とOAuth認証、MCP再読込、会話への利用可能化は別操作・別状態として照合する。

## 比較と要件への影響
|候補|利点|未解決|
|---|---|---|
|App Server管理API|同一接続で管理できる実装あり|公式文書の本番利用制約。採用しない|
|CLIをAgentDockから補助利用|公式案内と対象版実装、JSON出力あり|設定・認証・部分失敗・live反映・全希望操作の成立|
|既存Codex画面への導線|公式の管理画面を利用する候補|外部画面依存と操作数増加。アプリ内管理の代替として合意していない|

MCPやapp状態APIだけでPlugins全体管理を満たすと判断しない。CLI採用も外部画面への移譲も未決定。HTML・設定は変更しない。

## 対象版ソース
|ファイル|blob SHA|
|---|---|
|[codex-rs/app-server/src/request_processors/plugins.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/request_processors/plugins.rs)|713794c253e60209f97dfae0a0ce0fcafc408588|
|[codex-rs/app-server-protocol/src/protocol/common.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/src/protocol/common.rs)|c37c4956f1c3cbf21f293ca2aa41a6fec209bcfa|
|[codex-rs/cli/src/plugin_cmd.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/cli/src/plugin_cmd.rs)|d2824526e73cf28c7e82d2632ef66d9e4561840c|

SHAはファイル内容識別子。ユーザー環境の実動作証明ではない。

次はCLI管理の設定変更範囲とlive反映、認証・接続状態の対応を照合する。App Server最終採用は保留。実装・App Server起動・CLI実行・schema生成・実機検証・既存設定変更は行っていない。

