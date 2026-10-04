# AgentDock 引継ぎ

更新日: 2026-10-04（日本時間）
移行先: C:\Users\wmasa\Documents\Rust\AgentDock
フェーズ: 要件定義・UIモック。製品実装未着手。承認済みの隔離App Server小規模検証を実施済み。

**正本切替（2026-10-05）: [AgentDock_Requirements.md](AgentDock_Requirements.md) 1.0をユーザーが承認し正本とした。Agent_Monitor_Requirements.md（v0.50まで）は経緯・証拠記録。次は新モック（同書§10が入力、ファイルはAgentDock_Mock.html）。**

統合版草案の経緯: 1.0-draft2。draft2で将来の他AI接続（Claude Code、チャットごとに選択、初期はCodexのみ実装しバックエンド境界で備える）を§2.3に追加。v0.50までの有効な要件を機能領域ごとに再構成し、schema（安定／experimental）の根拠と新モックへの入力を追加。ユーザー承認後に正本へ切替え、本書群は経緯・証拠記録として保持する。承認までは下記v0.50が正本。

最新v0.50（2026-10-05、Claude Code引継ぎ）: 正本第52節。実装前方針A1〜A7に回答。A2 codex.exeは設定でパス指定、A3 既存CODEX_HOME共有（config.toml変更は明示操作時のみ）、A4 Tauri 2＋React＋TypeScript、A5 段階①ホスト・会話・履歴・三領域・子孫監視→②トレイ・通知・キュー・添付→③18操作、A6 未解決制約下で着手、A7 既存モックは修正せず要件修正後に新モックを再作成してから実装。A1は個人利用（公式変更履歴2026-09-05「app-serverはexperimental・本番ワークロード非サポート」を互換性リスクとして受容）。B1〜B4許可済み: schema生成済み（probes/app-server/schema/0.160.0）、git init済み、単体版codex.exeは検証exeと同一ハッシュ。B1実認証スモーク（live-smoke.mjs）は自動許可判定で拒否され未実行、ユーザー実行待ち。次は要件修正→新モック。

v0.49時点: [AgentDock_Idle_Stop_Plugin_Followup_Review.md](AgentDock_Idle_Stop_Plugin_Followup_Review.md)・正本第51節。31分の無購読idle後はloaded空／read notLoadedで完了履歴回収、モデル要求増加なし。readはturn/item購読や進行中部分本文を復旧しない条件を確認。会話別管理IDとCIMで所有・親子孫各3PIDの実終了を限定照合。Plugins同時設定操作、stale API版拒否、loopback Git marketplace更新、削除後の会話別MCPと過去Skill本文を別照合。累計26測定／24選択試行、全suite・製品受入ではない。実Windows sleepは未実施、powercfg読取りでS3／休止利用可。次は実sleep／安全な購読、任意停止所有、更新途中失敗と未確認能力。最終採用保留、既存設定／認証・本体・HTML・クラウド推論変更なし。

v0.48時点: [AgentDock_Probe_Plugin_Lifecycle_Review.md](AgentDock_Probe_Plugin_Lifecycle_Review.md)・正本第50節。起動中追加は一覧と既存会話の実能力が不一致、MCP reload後1500msでも差が維持。Hookスクリプトのみ変更は同じ定義ハッシュで実行、cache内定義変更は今回の条件では再起動後にmodifiedとなり抑止、確認した新定義ハッシュの再信頼後に再成立。CLI add/removeの設定書込み失敗でcache／設定不一致が残り、list installedだけでは残存設定を検出できない。累計18測定／16選択試行、全suite・製品受入ではない。次はidle/sleepと安全な購読、停止対象の所有・実終了照合、Plugins同時設定変更・marketplace更新・削除後の既存会話反映。最終採用保留、既存設定／認証・本体・HTML・クラウド推論変更なし。

v0.47時点: [AgentDock_Probe_Concurrency_Review.md](AgentDock_Probe_Concurrency_Review.md)・正本第49節。ページ間生成した孫は最初の走査で欠落、先頭再走査で回収。無接続中に生成した未知子の中断は4件とも3秒応答timeoutだが履歴interrupted、個別resumeしたキューなし比較2件は応答・通知成立。内部原因は未確定。親停止中に生成した孫と子は継続。120秒TCP断はactiveターンとキュー保持、切断前idleの別会話の内部キューは断中に自律開始。読取り自体の要求増加なし。初回stream idle timeout混在は不成立として保持。15測定／14選択試行、製品合格ではない。

v0.46追加: [AgentDock_Probe_Discovery_Review.md](AgentDock_Probe_Discovery_Review.md)・正本第48節。生成結果を受け取らずTCP断した未知子・孫をルートIDから祖先検索・ページ取得・readで再発見。実断15122ms／914ms。専用サーバー再起動後も保存結果を取得、loaded一覧空・子孫notLoaded、探索でモデル要求増加なし。別子2件の再接続後中断はreadでinterruptedを照合、resumeなしでは完了通知なし。初期試行の20秒interrupt timeoutは未解決として保持。累計12測定／11選択試行、全suite・製品合格ではない。

責任境界・18操作・取得方式・UI比較は[AgentDock_Responsibility_Technology_Review.md](AgentDock_Responsibility_Technology_Review.md)。App Server＋限定CLI補助＋AgentDock管理を具体化、stdio優先評価、Tauri 2を第一UI検証候補に推奨。最終採用・UI選定・製品実装は未決定。次は走査中生成・ページ増減、長時間idle/sleep・購読副作用、停止競合・timeout、Plugins live追加・Hook再信頼・部分失敗をまとめて照合。既存設定・認証の変更／コピー、実クラウド推論、本体実装・HTML変更へ広げない。合意済み18操作・D01〜D07を再質問せず、承認済み隔離検証は再確認不要。

## 正本と読む順序

1. Agent_Monitor_Handoff.md（本書）
2. Agent_Monitor_Requirements.md（正本v0.49、全文を読む。より新しい版があれば優先）
3. Agent_Monitor_Mock_Review.md（過去のレビュー記録）
4. Agent_Monitor_Mock.html（最新操作モック、表示名AgentDock）
5. AgentDock_Probe_Recovery_Review.md
6. AgentDock_App_Server_Assessment.md
7. AgentDock_Responsibility_Technology_Review.md
8. AgentDock_Probe_Concurrency_Review.md
9. AgentDock_Probe_Plugin_Lifecycle_Review.md
10. AgentDock_Idle_Stop_Plugin_Followup_Review.md

正式名称はAgentDock（エージェントドック）、旧仮称Agent Monitor。既存資料名は参照継続のため保持。元チャット全履歴は不要。資料がなければ推測せず報告する。

## 確定事項・優先関係

- 対象環境はユーザー申告で確認済み: Windows直接起動（PowerShell）、Codex CLI 0.160.0、VS Code Codex拡張26.917.62051。版や起動方法を再質問しない。
- Windows独立Codexクライアント。アプリ内起動・チャット・承認・質問・中断・履歴と動的な親子孫監視。VS Codeは実行時依存にしない。
- ローカルApp Serverを第一候補として採用方向。必要機能充足を確認して最終判断する。採用確定・対象版動作保証ではない。
- 第32節A82以降・チェックシート18項目全A、D01〜D07（第34節）は回答済み。古い引継ぎの自動起動トレイ質問も合意済み。未回答として再質問しない。
- 既存の設定・履歴を無断変更しない。保持期限なし・容量不足で自動削除しない。停止未確認で削除しない。キューを再起動等で無条件再送しない。
- LocalAppData内に専用領域、全体とチャット別容量、子孫通知2秒固定集約、承認・質問は即時。中断10秒で停止未確認案内（停止確定ではない）。保存失敗は保存済みと未保存を分ける。
- 正式名称は第36節。最新決定は第32〜36節が古い未決記述に優先。
- 技術証拠の最新は第51節・AgentDock_Idle_Stop_Plugin_Followup_Review.md。第47〜50節の限定証拠も維持。次は実Windows sleepと安全な購読、任意構成／PID再利用／遅延生成の所有照合、Plugins更新途中失敗・暗黙Skill・進行中ツール・非同期Hook。過去の未検証状態は当時の履歴として扱う。

## 最新UIモック

- 上部はチャット／作業／表示／設定／ヘルプ。メニューと項目にアイコンは不要（文字のみ）。低頻度操作はメニュー経由。
- 左に履歴、中央に会話・入力、右に状態・作業内容・親子線を持つ監視カード。
- 小さいボタンで密度を統一。主要操作と状態は線画アイコン、文字ラベル併用。
- 公式Codex IDE・ChatGPTアプリ資料、frontend-design指針、Linear・Vercel公式デザインを参照。対象拡張実画面を直接観察したわけではない。
- sol（gpt-6-sol）のレビューとChrome 1440／1100／390px表示・操作を検証。重大不具合なし。添付文脈のキュー保持と読み上げ名を修正済み。
- 最新版はその後にメニューアイコン除去・AgentDock表示名を反映。レビュー記録は履歴であり、最新画面はHTML。
- モックの添付は名前だけ、データは再読込みでリセット、モデル等は例。OSトレイ・最前面・Codex実停止等は表示例。製品実装やApp Server検証の証拠ではない。
- 完成画面の最終承認は未取得。

## 未確認と次の作業

1. 適用AGENTS.mdと作業ツリーを確認し、正本を全文読む。
2. 最新モックと要件の整合を確認。特に終了済みの表示規則: 正本は選択会話監視で完了も表示、現在モックは終了済みチェックで切替。モックで要件を上書きせず、修正方針を整理する。
3. 第13・30・31・33・34節の未解決公開経路を文書で照合し、対象版事実と最新文書情報を区別。Plugins管理の開発中制約、子孫詳細購読、停止確認、side・クラウド・worktree・Fast・memories・差分安全復元等を保持する。
4. 古い未決文章の整合を整理し、要件正本を継続更新。決定済み事項を再質問せず、必要な確認だけ行う。既に全項目選択式での一括回答を希望した経緯がある。
5. UI技術は未選定。RustフォルダにあることだけでTauri採用と推定しない。

ユーザーの直前依頼はApp Serverの最終採用と対象版機能成立の確認。公式文書の再照合を開始し、AgentDock_App_Server_Assessment.mdへ確認台帳を作成した。第37節にサポート範囲等の追加根拠を記録した。対象版固定ソースは第38・39節の範囲で照合済み。ユーザー環境のschema生成と実動作は未確認。新たな未回答質問はない。実装・App Server起動・schema生成・実機検証・既存設定変更への許可には読み替えない。

引継ぎ後のモック静的照合はAgentDock_Continuation_Review.md。HTMLは変更していない。次は子孫購読・実停止・Pluginsの公開経路と対象版契約を優先して文書照合する。

最新追加: rust-v0.160.0指定の公開ソースを取得し、AgentDock_Subagent_Source_Review.mdに記録。直接親IDと新規生成後の自動購読に対象タグ根拠あり。生成通知Lagged時の再同期省略、best-effort attach、再接続回復が残る。第38節を参照。対象版固定ソースは一部取得済みだが、ユーザー環境のschema生成と実動作は未確認。次は欠落・再接続回復と実停止のソース照合。

v0.37追加: AgentDock_Subscription_Recovery_Review.mdと第39節に復旧照合を記録。loaded/list・readで再発見／再取得する候補あり。warm resumeは接続購読を戻すがidle lifecycleからqueue dispatchへつながる等の副作用があるため、監視目的で無条件使用しない。安全な再購読は未確認。次はdispatch条件・購読専用契約と実停止の照合。実装・実機検証は未着手。

v0.38追加: AgentDock_Stop_Queue_Source_Review.mdと正本第40節に記録。通常中断の成功応答は自然完了時にも返るため、終了状態との照合が必要。子孫・外部プロセス全体停止は未確認。保存キューはcoreのidle開始へ進むがAgentDock条件との一致は未確認。次はabort/hooksの子孫伝播とbackground terminal停止、core idle開始条件。実装・実機検証は未着手。

v0.39追加: AgentDock_Descendant_Stop_Review.mdと正本第41節に記録。通常task.abortとInterrupt hooksから子孫再帰停止の根拠は得られず、全経路不存在とも断定しない。backgroundTerminalsのlist/terminate/cleanは対象タグにあるがexperimental。管理IDとOS PID、管理API終了結果と全OS子孫停止を区別。次はWindows終了範囲・experimental利用条件とPlugins等の採用判定整理。実装・実機検証は未着手。

v0.40追加: AgentDock_Windows_Experimental_Review.mdと正本第42節。Windows Job登録競合・fallback・正常終了後の子孫保持・killエラー破棄を確認し、上位成功だけで全OS実停止を確定しない。experimentalはinitialize接続能力指定が必要だが安定性保証とは別。次はPlugins管理の対象版契約・代替公開経路。採用保留、実装・実機検証未着手。

v0.41追加: AgentDock_Plugins_Review.mdと正本第43節。Plugins管理APIの本番利用制約を維持。codex plugin add/list/removeのJSON付きCLI補助経路は公式案内・対象版に根拠あり。採用未決定で設定・認証・部分失敗・live反映を追加照合。次はmanagerの変更範囲と反映・接続状態の契約。CLI実行も実装・実機検証も未着手。

v0.42追加: AgentDock_Adoption_Batch_Review.mdと正本第44節に18項目をまとめた採用照合を記録。最新の進め方はユーザー指定の「ある程度まとめて確認して報告」。過去の一項目ずつの次作業案より優先する。App Server単独の最終採用は保留し、限定CLI補助とAgentDock側管理を比較候補へ追加。Plugins CLIの共有config変更・部分失敗、disabledPluginIdsの能力filter未実施、personality非推奨と非対応モデル情報、sideの指示と強制制限の差、履歴revertと安全なファイル復元の差を記録。次は担当範囲・代替方式の影響・文書確認と将来の実機確認条件をまとめて比較する。実装・起動・CLI実行・実機検証・設定変更は未着手。新規チャットは必要時のみで、まだ作成していない。

v0.43追加: ユーザーが小規模検証モジュール作成と隔離App Server実行を明示承認。AgentDock_Probe_Review.mdと正本第45節を最新とする。probes/app-server/probe.mjsとmcp-fixture.mjsで0.160.0実物を専用CODEX_HOME・ローカルモデル/MCP応答で確認。最終14チェック成立。ただし最後の接続断でloaded一覧に残らない試行もあり、原因未確定。単一ターン中断・MCP無効化反映・限定購読復旧だけ前進。子孫/OS全停止・キュー付き復旧・実Plugins能力filter・実クラウド/authは未検証。本体・HTML・既存設定変更なし。過去の「実機未着手」は履歴。次は切断キュー・全接続断の差・子孫停止・実plugin反映をまとめて検証する。


v0.44追加: AgentDock_Probe_Batch_Review.md・正本第46節・probes/app-server/batch-result.jsonを最新とする。6測定手順を5試行から選択、全suite一度の合格ではない。TCP断12条件は保持、通常WS closeは2秒応答未確認、前回非保持は未解決。保存キューcold resumeで待機依頼開始、active／中断後warmは短時間内保留。親中断後に子孫継続、個別中断後statusを確認。turn interrupt後のyield管理実行は生存、個別terminate／cleanではfixture PID終了を確認。実plugin MCPは会話単位無効化で消失・再有効化で再出現・CLI削除＋reload後消失。過去の能力filter未実施注記と実測差を併記、全Skills/Hooks/Apps・OS全子孫・クラウド/authは未確認。既存config hash一致、本体・HTML・既存認証変更なし。次は親子孫の切断復旧・生成競合、キュー境界、OS子孫停止、pluginのSkills/Hooksを一括検証。前回承認を引き継ぎ、再確認不要。

v0.45追加: 最新はAgentDock_Probe_Recovery_Review.md・正本第47節。既知IDの親子孫をTCP断後readで照合し、切断中に終わった孫のcompletedを回収。個別resumeはキューなしで要求増加なし、親・子の個別中断通知を再購読側で確認。内部キューはfailed後・子稼働中にも次を開始し、AgentDock側送信判断が必要。限定Windows親子孫3PIDはterminate／clean後500msで消失。実pluginの明示Skill本文と信頼済みUserPromptSubmit Hookは会話無効化で実行／注入を抑止。正確なfixture Hook key/hashのみ専用configで信頼、bypassなし。10測定を9試行から統合、全体受入合格ではない。未知子孫再発見・生成競合・長時間断／再起動・停止完了照合・他Plugin能力・部分失敗／同時変更・技術選定が残る。本体・HTML・既存設定／認証変更なし。前回承認を継続、合意済み事項を再質問しない。


