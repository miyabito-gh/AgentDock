# 小規模検証の一括結果

追加検証の最新は[AgentDock_Probe_Batch_Review.md](AgentDock_Probe_Batch_Review.md)と正本v0.44・第46節。本書の14チェックと不成立試行は前回の記録として維持する。

日付: 2026-10-04。正本v0.43。ユーザーが「検証モジュールの作成と隔離したApp Serverでの実行」を承認した後に実施。

## 結論

**0.160.0の実App Serverで基本対話・単一ターン中断・MCP反映・限定条件の購読復旧を確認できた。最終試行14チェック成立。ただしApp Server最終採用は保留。**

実App Serverと模擬モデルを分けて評価する。Windows上の本物のApp Serverを起動したが、モデル応答はローカル固定fixture。クラウドモデル・利用資格・実アカウント認証の成立確認ではない。

## 最終試行

結果: probes/app-server/runs/2026-10-04T08-23-53-601Z/result.json。
コード: probes/app-server/probe.mjs、mcp-fixture.mjs。Node.js 24.10.0、追加依存なし。
実行ファイル: C:\Users\wmasa\.vscode\extensions\openai.chatgpt-26.930.31730-win32-x64\bin\windows-x86_64\codex.exe。--versionでcodex-cli 0.160.0を確認し、モジュールも版不一致なら停止する。

|範囲|実際の結果|成立判断の限界|
|---|---|---|
|接続初期化|未初期化要求はNot initialized、initialize正常応答、専用codexHomeを確認|一般クライアントとして接続。userAgentは配布バイナリ側がCodex Desktopと報告するが、clientInfoはagentdock_probe|
|会話・応答|thread/start、ローカルResponses要求、turn/completedのcompleted状態|実モデル推論ではない|
|単一ターン中断|保持中の模擬ストリームへinterrupt、空応答とinterrupted終了通知を照合。最終92ms|親子孫再帰停止・外部OSプロセス停止・一般的な時間保証ではない|
|履歴とロード|App Server再起動後readで2件以上のターンを取得。loaded/listは空で、モデル要求増加なし|readは購読復旧ではない|
|idle resume|ロード復旧とdisabledPluginIds保存を確認。モデル要求増加なし|キューなし条件。キュー付きの副作用を否定しない|
|live購読復旧|別接続を維持して観測接続を交換。read後は設定変更通知なし、resume後は通知あり、中断完了も取得。モデル要求重複なし|単一会話・キューなし・loopback experimental WebSocket|
|全接続切断の追加観測|最終試行では500ms後もloaded threadが残り、履歴ターンinProgress・runtime activeを確認|後述の不成立試行もある。長時間断・競合・再現性は未確認|
|MCP接続|ローカルstdio MCPのconnected、probe_pingを取得。authStatusはunsupported|OAuth成立の証拠ではない|
|MCP無効化反映|専用config変更→reload→次ターン。モデル要求からMCP namespace消失、状態disabled・tools空を確認|Plugins全体のSkills/Hooks/Appsや会話単位plugin無効化は別|
|Plugins保存値|settings/updateは空応答、settings/updated通知に指定ID。再起動resumeでも保存値を確認|架空IDの保存のみ。実pluginの機能filterは未検証|
|backgroundTerminals|何も起動していない会話で一覧が空|実プロセスのlist/terminate/clean成立は未確認|
|experimentalゲート|能力指定なしの接続でbackgroundTerminals/listを拒否|安定性・本番サポート保証とは別|
|原本設定|前後SHA-256一致|既存環境全ファイルの完全監査ではない|

## 修正と不成立試行

- 初回は検証コードがsandboxをreadOnlyと送信して拒否された。対象exeが要求するread-onlyへ修正。対象版API不成立とはしない。
- 次はsettings/updateの空応答に設定値を期待していた検証側の誤りを修正。settings/updated通知とresume保存値で確認する。
- MCPツールを単一tool名で照合したが、実要求にはmcp__agentdock_fixtureのnamespaceとして載るため検証側を修正。MCP無効化後はnamespace消失とstatusを両方確認。
- **08:20:46 UTC開始試行では、最後の接続を切った後のloaded/listに対象会話が残らず、live再接続チェックが不成立だった。**原因は未切り分け。修正後は他接続維持のケースを独立確認し、最後の接続断も500ms待つ追加観測として記録した。最終試行で残ったことをもって、前の結果を取り消したり安全な再接続が保証されたとしない。
- 試行結果はruns以下に保持する。finalの14passは最終条件のみの数字であり、過去全試行の成功率ではない。

## 採用判断への反映と次の範囲

B01の入口と限定条件は実確認へ進んだが、最後の接続断の差、欠落・長時間断・保存キュー・親子孫の復旧が残る。B02は単一ターンだけ前進。B03はMCPの反映だけ前進し、Plugins管理CLIの部分失敗・認証・全能力更新は残る。

次の小規模検証は、キューを含む切断・再接続と全接続断の差の切り分け、親子孫と管理実行の停止、実fixture pluginの能力無効化をまとまった範囲で扱う。モデルfixtureが生成するtool要求は検証用作業に限定し、一般タスクの自律実行へ広げない。クラウド認証・本体実装・既存設定変更は今回の承認に含めない。

公式プロトコル参考: [App Server](https://learn.chatgpt.com/docs/app-server)、[設定参照](https://learn.chatgpt.com/docs/config-file/config-reference)。本資料の成立判断は現在文書の例ではなく実測JSONを優先。
