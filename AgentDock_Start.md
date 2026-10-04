# AgentDock 開始指示

このプロジェクトの要件定義とUIモック検討を引き継いでください。

最新状況（2026-10-04）: 正本v0.49・第51節、AgentDock_Idle_Stop_Plugin_Followup_Review.md、probes/app-server/batch-result.jsonを優先する。承認済み隔離検証は続行可能。製品実装・HTML・既存設定変更・実クラウド認証や推論への拡大は行わない。下記の古い実機未着手指示は第45〜46節で更新済み。

最初にプロジェクトのAGENTS.md等の適用指示と作業ツリーを確認し、次の資料を全文読んでください。

1. Agent_Monitor_Handoff.md
2. Agent_Monitor_Requirements.md（正本v0.49、全文）
3. AgentDock_Probe_Recovery_Review.md
4. AgentDock_App_Server_Assessment.md
5. AgentDock_Responsibility_Technology_Review.md
6. AgentDock_Probe_Concurrency_Review.md
7. AgentDock_Probe_Plugin_Lifecycle_Review.md
8. AgentDock_Idle_Stop_Plugin_Followup_Review.md

Agent_Monitor_Mock.htmlは操作モックです。正式名称はAgentDock。既存ファイル名は参照継続のため残しています。

第32〜36節の最新決定が古い未決記述に優先します。決定済み事項は再質問せず、事実・推測・未確認・合意済みを区別してください。まずモックと正本の整合を確認し、未解決事項と次の文書作業を提示してください。実装・App Server起動・schema生成・実機検証・既存Codex設定変更はまだ行わないでください。

2026-10-04更新: 上の未着手指示の後、ユーザーが小規模検証モジュール作成と隔離App Server実行を明示承認した。最新正本v0.43・第45節とAgentDock_Probe_Review.mdを参照。検証用の起動・専用設定・専用履歴は許可された範囲。本体実装・クラウド認証・既存Codex設定変更・HTML変更の許可には広げない。報告は細目ごとではなくある程度まとめる。

2026-10-04最新更新: 正本v0.45・第47節とAgentDock_Probe_Recovery_Review.mdを優先。キュー境界、既知親子孫復旧、限定OS子孫、明示Skills／信頼済みHookの実測を追加済み。承認済み隔離検証は継続可能。製品実装・HTML・既存設定／認証変更は別段階。

