# App Server採用判断の一括確認

v0.47最新: [復旧・停止競合](AgentDock_Probe_Concurrency_Review.md)、正本第49節。累計15測定／14選択試行。取得中の子孫再走査・中断の履歴照合・AgentDock側キュー管理の必要性を追加。最終採用は保留。

v0.46最新: [未知子孫・再起動・停止照合](AgentDock_Probe_Discovery_Review.md)と[責任境界・18操作・取得方式・UI比較](AgentDock_Responsibility_Technology_Review.md)を追加。正本第48節、累計12測定／11選択試行。以下は過去の照合記録として保持。App Server第一候補・最終採用保留。

最新の実測追記は[AgentDock_Probe_Recovery_Review.md](AgentDock_Probe_Recovery_Review.md)と正本v0.45・第47節。以前の追加検証も履歴として維持。本文のソース注記と未検証状態は当時の記録。fixture MCP、明示Skill、信頼済みUserPromptSubmit Hookの会話単位filterを確認したが、全Plugins能力成立には広げない。内部キューは失敗後・子稼働中にも進むためAgentDock側の送信判断が必要。第一候補維持・最終採用保留。
確認日: 2026-10-04（日本時間）。正本v0.42。
対象ソース: openai/codex、rust-v0.160.0。検索はファイル位置発見のみ。公開文書は確認日現在の内容であり、対象版の保証と分ける。

## 結論
**App Serverはローカル実行・対話・監視の第一候補を維持するが、単独で全要件を満たす方式として最終採用する根拠は不足。App Server＋限定CLI補助＋AgentDock側管理を比較候補とする。採用決定ではない。**

新たな実装・実機確認は行わず、Plugins、認証・反映、side、Goal、Fast/personality/memories、クラウド、worktree、復元をまとめて照合した。監視復旧・停止は前回までの対象版根拠を統合して評価した。

## 採用に残る主要条件
|条件|確認済み|未成立・未確認|設計候補|
|---|---|---|---|
|B01 安全な監視復旧|新規子孫の直接親IDと自動attach、ロード済み再発見|通知欠落後・再接続後の作業を変えない購読復旧。warm resumeに副作用|履歴再取得とlive復旧を分け、不明時キュー保留。購読専用契約または副作用抑止を追加確認|
|B02 全対象停止|ターン終了通知、experimental管理プロセス終了API|親中断の子孫再帰伝播、Windows全OS子孫停止保証|各ターン・管理実行を個別照合。受付・ターン終了・OS停止を分け、10秒後停止未確認|
|B03 Plugins管理|公式Stable CLIの一覧・追加・削除とJSON出力|設定変更範囲と同時変更競合、認証、部分失敗、live反映完了|明示管理操作に限定したCLI補助、変更scope表示、保存・接続・利用可能化の状態分離|

B01〜B03は今回の調査で優先度が高い横断条件であり、他のV01〜V10が合格済みという意味ではない。クラウド・安全なファイル復元・参考会話・添付等の残る条件も維持する。

## 1. Plugins設定・部分失敗
manager.rs 2245–2301: ローカルplugin installはmaterialize/store installを先に行い、その後set_user_plugin_enabledを呼ぶ。
2322–2365: uninstallはstore uninstall後にclear_user_pluginを呼ぶ。
plugin_edit.rs 50–74: CODEX_HOME/config.tomlを読み、対象plugin項目を編集し、変更時にatomic writeする。

したがってローカルCLI補助でもユーザー設定へ永続化する。アプリ専用領域だけで完結する操作ではない。store操作とconfig書込みは順次処理なので、後段エラーで前段が完了している可能性がある。全体の自動rollbackは今回確認していない。atomic writeがあっても他プロセスとの読取り・編集・書込み競合を解決する保証ではない。

既存合意は「設定を無断上書きしない」。明示Plugins管理に伴う限定変更の候補として扱い、常時・起動時・監視復旧時の設定変更へ広げない。scope分離や専用CODEX_HOME採用は、認証・履歴・既存plugin利用への影響があるため未決定。remote変更の全保存範囲は未網羅。

## 2. 認証・接続・live反映
- common.rsにmcpServer/oauth/login、mcpServerStatus/list、config/mcpServer/reload、app/installed等の公開入口がある。
- mcp_processor.rs 85–92からmcp_refresh.rs 9–29へ進み、各loaded threadの設定を再取得してrefresh_mcp_configを呼ぶ。
- これはMCP更新の経路であり、Plugins全体のSkills・Hooks・Appsがすべて反映済みになる証明ではない。各threadへの適用は別処理。MCP状態取得とOAuth完了を照合する候補。
- skills_watcher.rs 158–167付近はskills cacheを無効化しSkillsChanged通知を出す。この通知だけでMCP/Hooks更新済みとはしない。
- apps_processor/installed.rs 35–64: threadId指定ならthread実効configを使う。enabled/callableはruntime_enabledとの組合せ（185–192行）。force_refreshの失敗はエラーを返し、既存snapshotが残る場合がある（150–171行）。非refresh時はcacheを使う。表示を常に最新確認済みにしない。
- plugin authPolicyは認証方針であり接続済み状態ではない。plugin installed、enabled、OAuth完了、server起動、tool callable、更新済みを別に扱う。
- ThreadSettings.ts 14–17とthread.rsの応答型にはdisabledPluginIdsが保存値であり、まだplugin capabilitiesをfilterしないという注記がある。会話単位の無効化が効くと受理値だけから判定しない。

CLI補助→再読込→各接続状態確認という候補には根拠が増えたが、既存API制約を避けつつ全plugin能力を安全に反映する経路は確認完了ではない。

## 3. side会話
tui/app/side.rs 609–621: side用configはephemeral=trueと専用developer instructionsを設定。
app_server_session.rs 877–895・934–981: sideはThreadForkParamsを作りthread/forkへ送る。
side.rs 597–607に境界メッセージ、656–664に主履歴をモデルの参考に残しつつ画面をside境界から表示する処理がある。

単に新規threadを作るだけではないが、**一時fork＋相談境界＋アプリ側表示**という組合せ候補に対象版根拠がある。主会話への明示移送、終了時の保持・廃棄、権限・Goal引継ぎ、副作用の抑止は追加確認。ephemeralをAgentDock側保存禁止へ勝手に読み替えない。要件の保存範囲を維持する。

## 4. Goal・Plan・分岐・圧縮・Skills
common.rsにthread/goal/set・get・clear、review/start、thread/fork、thread/compact/start、skills/list、collaborationMode/listがある。ThreadGoalSetParamsにはobjective/status/tokenBudgetがある。入口存在の確認であり、目標解除・自動継続・Plan中の操作抑止・実行への移行・保存の受入成立は未確認。

thread/forkにはephemeralやdeveloperInstructions、限定ターン分岐がある。runtimeWorkspaceRootsやdeferGoalContinuation等はexperimental。forkとspawn親子関係は別として保持する。過去会話の参考指定はforkやresumeで代替済みとはしない。

## 5. Fast・personality・memories
|機能|対象版根拠|判断|
|---|---|---|
|Fast|Model.tsのserviceTiers/defaultServiceTier、TurnStartParams.tsのserviceTier/serviceTierForTurn|カタログ対応の候補。固定tier・固定資格を仮定しない。thread既定とturn限定を区別|
|personality|Model.ts、ThreadSettings.ts、TurnStartParams.tsに非推奨注記。openai_models.rs 958でsupports_personality=false。SDK資料95–111付近でも旧値の扱いを説明|旧friendly/pragmaticを切替可能な有効設定として表示しない。モデル指示による応答調整案は別機能であり同等性合意は未取得|
|memories|common.rs 692–709: thread/memoryMode/set、memory/status、memory/resetはexperimental|入口あり。読取り・生成・停止・保存・削除のscopeと資格は未確認。今回reset等は実行なし|

現在の[公式App Server文書](https://learn.chatgpt.com/docs/app-server)のpersonality例・supportsPersonality説明と、対象タグの非推奨注記には差がある。最新ページの例で対象版の動作を上書きしない。独立アプリのなぎ人格とCodex側の廃止済み選択値を混同しない。

## 6. クラウド・worktree・変更復元
|機能|今回の根拠|採用への制約|
|---|---|---|
|クラウド|cloud-tasks/cli.rs 16–26にexec/status/list/apply/diff。現在公式Developer commandsはcodex cloudをExperimentalと分類|ローカルApp Serverだけで成立としない。CLI補助候補はあるが環境選択、資格、停止、結果反映競合、ローカル親子監視への対応は未確認|
|worktree|worktree/lib.rs 62以降はGit worktree作成、285以降は管理対象検証と削除。現在公式文書もGit repo必須|内部Rust managerの存在を独立App Serverの公開作成APIと同一視しない。AgentDock担当のGit管理候補。保存・未コミット・ignoredファイル・所有権・削除失敗を設計する必要|
|会話履歴の復元|ThreadRevertParams.ts 5–8で保存履歴のみ変更、ローカルファイルは戻さないと明記|thread/revertを安全なファイルUndoとして使わない。元の変更・他会話変更を保護するファイル別の復元設計が別途必要|

worktree managerのremoveは現在checkout内からの削除やignored local filesを拒否する。既存Desktopと同等の退避・archiveが実装されている証明ではない。今回workspaceをGit化したりworktreeを作ったりしていない。

## 7. 合意済み18操作の現時点整理
「入口根拠」は受入成功ではない。「アプリ担当候補」は方式採用ではない。
|操作|現時点の整理|残る確認|
|---|---|---|
|1 差分画面|アプリ担当候補＋差分イベント|変更所有範囲・ファイル実在|
|2 変更を戻す|thread/revertはファイル復元ではない|安全なファイル復元設計|
|3–4 レビューと保存先|review/start入口＋fork候補|実行対象・保存・別会話対応|
|5 Plan|collaborationMode等入口根拠|計画中制約・移行契約|
|6–7 分岐・圧縮|fork/compact入口根拠|設定添付引継ぎ・保存履歴|
|8 過去会話参考指定|独立要件として維持|参照内容と公開入力契約|
|9 Goal|set/get/clear入口根拠|継続・停止・解除・保存|
|10 side|一時fork＋境界＋表示の対象版根拠|主会話への明示移送・保持・権限|
|11 Skills/AGENTS.md|skills/list等入口、アプリ確認導線候補|階層適用・initの既存保護|
|12 MCP/Plugins|MCP公開経路＋Plugins CLI補助候補|設定・認証・部分失敗・反映|
|13 クラウド|experimental CLI補助候補|資格・停止・成果反映|
|14 worktree|アプリGit管理候補|所有・未コミット・退避・削除|
|15 status等|Fast候補、memory実験API、personality差分あり|対象資格・実効値・非推奨の代替|
|16–18 操作導線・狭幅・同時利用|AgentDock側要件|モック整合・負荷受入|

18項目の除外・延期・再選択は行わない。

## 8. 次のまとまった作業と判断
細目ごとに報告せず、次は以下を一括で具体化する。
1. B01〜B03の対応候補について、API/CLI/AgentDock担当、設定・保存scope、失敗時状態を一覧化。
2. クラウド・worktree・復元・personality等について、代替による利用上の違いを具体化。
3. 文書で解ける不足と、将来の実機確認でしか判断できない条件を分離し、必要なユーザー判断をまとめた比較案を作る。

今回の候補構成は「ローカルApp Serverで対話・活動を取得、限定CLIでPlugins/Cloudを補助、AgentDockで保存・キュー・状態・Git管理・UIを担う」。B01/B02の不足が解決した証明ではなく、全機能の最終採用は保留。実装許可は別途必要であり、今回の続行依頼に含めない。

## 根拠一覧
現在の公式文書: [App Server](https://learn.chatgpt.com/docs/app-server)、[Developer commands](https://learn.chatgpt.com/docs/developer-commands)、[Worktrees](https://learn.chatgpt.com/docs/environments/git-worktrees)、[Memories](https://learn.chatgpt.com/docs/customization/memories)。

前回までの対象版根拠: AgentDock_Subagent_Source_Review.md、AgentDock_Subscription_Recovery_Review.md、AgentDock_Stop_Queue_Source_Review.md、AgentDock_Descendant_Stop_Review.md、AgentDock_Windows_Experimental_Review.md、AgentDock_Plugins_Review.md。

今回取得の対象タグファイル:
|ファイル|blob SHA|
|---|---|
|[codex-rs/core-plugins/src/manager.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core-plugins/src/manager.rs)|e5614b9f55bc9cd4083bf9f077d7559b5eb42171|
|[codex-rs/config/src/plugin_edit.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/config/src/plugin_edit.rs)|63795cc6c6338da47f2a9a6729714491b40b2435|
|[codex-rs/app-server/src/skills_watcher.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/skills_watcher.rs)|da3d1f7076b53a7e6eeb289a6f8242c1ac437219|
|[codex-rs/app-server/src/mcp_refresh.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/mcp_refresh.rs)|b654ad6a2851ffe95eb7e276404392a58d6231d0|
|[codex-rs/app-server/src/request_processors/mcp_processor.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/request_processors/mcp_processor.rs)|120e8918632d200b076a3e3c4b7a05c6091bcc43|
|[codex-rs/app-server/src/request_processors/apps_processor/installed.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/request_processors/apps_processor/installed.rs)|00f4f86c49cce86e699fbd9c2dc689d07e65d76f|
|[codex-rs/tui/src/app/side.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/tui/src/app/side.rs)|f4006ba36ae59f8f14a8a27f147b87b69f6b52ef|
|[codex-rs/tui/src/app_server_session.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/tui/src/app_server_session.rs)|f45e2b12640bfa78aaa8bf8c3338e47a69025a2b|
|[codex-rs/app-server-protocol/src/protocol/v2/thread.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/src/protocol/v2/thread.rs)|2e166d5506b10996e2da286feb6082d6b8711340|
|[codex-rs/app-server-protocol/src/protocol/common.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/src/protocol/common.rs)|c37c4956f1c3cbf21f293ca2aa41a6fec209bcfa|
|[codex-rs/cloud-tasks/src/cli.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/cloud-tasks/src/cli.rs)|e2c3a24442277d64483c1f85f827fb3ab7bd5156|
|[codex-rs/worktree/src/lib.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/worktree/src/lib.rs)|c52bb559ff54c0414cf61857970f96d1efe83312|
|[codex-rs/app-server-protocol/schema/typescript/v2/ThreadSettings.ts](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/schema/typescript/v2/ThreadSettings.ts)|90818f634e58693f612dd574109fdb10c5f70778|
|[codex-rs/app-server-protocol/schema/typescript/v2/Model.ts](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/schema/typescript/v2/Model.ts)|6a1c065eebe87be433fa362a901c7eeb35301b39|
|[codex-rs/app-server-protocol/schema/typescript/v2/TurnStartParams.ts](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/schema/typescript/v2/TurnStartParams.ts)|b5f532191679abd32ae22b2574d7169d64b00613|
|[codex-rs/app-server-protocol/schema/typescript/v2/ThreadGoalSetParams.ts](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/schema/typescript/v2/ThreadGoalSetParams.ts)|b92720c1787244f13ad854cc718179c82009800b|
|[codex-rs/app-server-protocol/schema/typescript/v2/ThreadRevertParams.ts](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/schema/typescript/v2/ThreadRevertParams.ts)|1090a5c33004b0c11f01381991ffa33e0590cb57|
|[codex-rs/protocol/src/openai_models.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/protocol/src/openai_models.rs)|e5f02ac9e86d31bc6f27e3624fd64b4a8fa3e881|
|[sdk/python/docs/api-reference.md](https://github.com/openai/codex/blob/rust-v0.160.0/sdk/python/docs/api-reference.md)|9fd42b5e7d68d763a96466a6bfc098d33100e1f4|

blob SHAはファイル内容識別子。main検索結果は対象版根拠に採用しない。memoryの標準生成型とcore/session/config_refresh.rsの取得失敗は不存在証明にしない。生成型はリポジトリにチェックインされた公開資料の読取りであり、schema生成はしていない。

実装・App Server起動・CLI実行・schema生成・実機検証・認証操作・設定変更・HTML変更は行っていない。

v0.48最新追加: AgentDock_Probe_Plugin_Lifecycle_Review.md・正本第50節。起動中追加の一覧と会話実能力の差、Hook定義とスクリプト内容の信頼境界、CLI設定書込み失敗時のcache／設定不一致を実測。集約batch-result.jsonは18測定／16選択試行。全suite・製品受入ではない。既存の歴史記述と今回の限定隔離実測を区別する。
