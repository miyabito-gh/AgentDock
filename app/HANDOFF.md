# 現在の引き継ぎ

更新: 2026-10-24。通常機能改善と新方式のソース実装・関連検証・独立レビューを保存済み。実Codexによる受入は未実施。README → AGENTS → 本書 → MIGRATION_SUBAGENTSの順に読む。

最新追記（2026-10-25）: master/origin/masterは09b00e3。開発はfeature/visual-editor。設計5733b16、初期基盤887b639、probe fc15894/dfce3bdを保存。実機V0は通信隔離に不合格。以下の古い保存地点・未検証記録は当時の状態。詳細は末尾を参照する。

## 保存地点
- 旧版はfbe67ca。masterとorigin/masterへ保存済み。タグbaseline/org-engine-2026-10-24もpush済み。旧フォルダと既存worktreeを保持。
- 作業場所はAgentDock_next、ブランチfeature/subagent-migration。新ブランチは未push。
- bdcd23f: 共通指示。40f3ae4: 基準版依存lock。0a136a1: 通常機能U1〜U5。3ac8b9f: 新方式。d7a4c28: 検証記録。

## 検証
- 通常改善: Rust8件、画面build（tsc含む）成功。Luna静的確認で回帰指摘なし。
- 新方式: cargo test --manifest-path app/src-tauri/Cargo.toml --lib subagent --locked は16件成功。型8件・host安全性4件・既存関連4件を含む。一時Gitで検証。
- Solの初回レビューで受理ガードの期限失効/照合Accepted未記録と、取り込み先の別チャットとの競合を検出。Sol利用上限後はLunaが限定修正。
- 受理turnの終端までガードを維持し、取り込み先と重なる全チャットの送信ロック・未確定操作・停止を検査。初回送信・review/compactもガードへ参加。通常キューの15秒方針は維持。
- 追加回帰2件、最終cargo check、最終画面build成功。別Lunaの独立再レビューで追加必須修正なし。
- 詳細はNORMAL_IMPROVEMENTS.md、SUBAGENT_IMPLEMENTATION.md、SUBAGENT_UI.md。GUI/外部アプリ/実Codexの受入は未実施。旧orgは有効でL1退役は未着手。

## 残りの受入と環境
- 専用ローカルrepoで子生成、追加指示、承認/質問、親子停止、再起動後照合、成果取り込みを確認する。Astraは使わない。
- 実Codex利用量・会話作成の許可は明示回答未確認。旧アプリの作業停止・終了を確認してからバックアップし、新旧同時書込みを避ける。
- Codex観測版0.163.0、TARGET_VERSIONは0.162.1。未検証で警告を消していない。
- 設定・認証・通常履歴は継承。~/.codexをコピー/変更していない。実アプリ保存領域への書込み試験は未実施。
- 旧exe/NSISは.local-baseline/にハッシュ付き保存。基準ソース一致は未検証。
- .git書込み許可指定後も通常実行のindex.lockが拒否された。必要時は昇格経路を使用し、制限解除を断定しない。

## 配布物
- NSIS x64を作成済み。終了コード0。詳細と実行コマンドはINSTALLER_BUILD.md。インストーラー実行は利用者が行う。
- app/で npm.cmd run tauri -- build --bundles nsis。通常実行はesbuildのアクセス拒否で失敗し、昇格実行で再試行。
- 前回のPowerShell→Python標準入力で文字コード変換が起き、日本語記録の一部が「?」になったため修復。アプリソースは変更していない。

## 2026-10-24 実機スモーク試験で検出した終了照合の修正
- 対象: feature/subagent-migration、基点9b95bd5。state.rsとqueue_driver.rsの未コミット修正。
- 専用repo agentdocktestで、Lunaの子readme_readerが正常終了し、親がREADMEへ指定行を追加、7c1df4fでコミット。元repoはe7b0874のままclean。アプリの取り込み判定が子孫の終端未確認として阻害し、取り込みは未実行。
- Liveの最新turnをmetadataの欠損/旧値から保護し、TurnEndedでも矛盾しないturnを保持。最新turn・status・running情報と矛盾する旧終端を判定に使わない。終端証拠が欠ける非作業中のLive子も履歴読取りで再確認する。resume/送信/停止はしない。保存schemaとUIは変更なし。
- Sol実装、Luna検証、別Solの独立レビュー。必須レビュー指摘なし。host::state 30件、host::queue_driver 18件、subagent 21件、cargo check --locked、npm type-check、npm build成功。4件の回帰テストを追加。既存ts-rs/import混在/bundle容量の警告あり。
- 既存のdesktop/windows schema変更は保持。テストで生成されたIPC型はGit差分上の内容変更なし。
- 更新用NSISを生成済み（release 8分4秒、終了コード0）。稼働中のインストール版は変更していない。修正版での実機取り込み成功は未確認。アプリ終了後に更新し、既存依頼を再読込して取り込み確認を再開する。依頼の再送や手動マージは不要。
- 既知の限定条件: 同一起動中、終端キャッシュc1と欠損latest/status.turn=c2が矛盾すると安全に保留したまま再読しない可能性。通常の新AgentStatus経路ではlatestを補完し、再起動でキャッシュが空になるため今回の更新後復旧は妨げない。
- 修正版NSIS: app/src-tauri/target/release/bundle/nsis/AgentDock_0.0.1_x64-setup.exe、7027472 bytes、2026-10-24 22:32:53 JST。SHA-256: 7F2C443DE4FF9F63302CE23AAD10E04F1A66BE334873C100DCD125F41AF76CFE。インストール/再起動未実施。

## 2026-10-24 再起動後の親終了履歴の取り込み判定
- 更新後の実機で子の阻害は解消したが、親HistoryOnly/Unknownの「親の最新turnの完了・鮮度を確認できません」が残った。前回修正は親の履歴再読を対象から漏らしていた。
- 読取り専用queue_gate_inputでは親も既存上限・バックオフ付きで履歴再確認する。通常のキュー送信経路は子のみのまま。キャッシュとstatus.turnの不一致も再読対象にした。
- queue_driver.rsとsubagents.rsを追加変更、回帰3件追加。実Hostで親読取試行と実gate_blockersを確認（成功履歴はキャッシュ注入であり、実通信とGit準備のE2Eではない）。新turn/失敗/中断/不完全履歴/読取失敗は拒否を維持。Sol実装・別Solレビューで必須修正なし。
- Luna検証: queue_driver 21件、subagent 24件、cargo check --locked成功。type-checkも成功したがRustのみの変更のため今後この種の再実行は省く。
- 利用者方針: 変更に必要なテストだけ実施し、成功済み証拠を再利用する。未変更範囲のテスト/型チェック/画面buildを繰り返さず、追加実行は失敗・追加変更・具体的な未解決懸念がある場合に限定。テストとレビューの分担は最小構成にする。
- 今回は既存distを再利用し、TEMPの設定でbeforeBuildCommandを空にしてdebug NSISを生成する。通常の配布設定は変更していない。稼働中アプリと保存データは未変更、元repoへの取り込みは未実行。
- debug NSIS生成成功（dev compile 1分47秒、終了0）。成果物: C:\Users\wmasa\Documents\Rust\AgentDock_next\app\src-tauri\target\debug\bundle\nsis\AgentDock_0.0.1_x64-setup.exe、SHA-256: 3688D332362474DD57A98045F65697A36BA7FAA5BAD15A4DB572AC64014CB4CB。インストールと実機取り込みは未実施。

## 2026-10-24 22:47 実機取り込みの確認結果
- 利用者がdebug修正版へ更新後、最初のpreviewは走査中として阻害。ログで対象rootのcomplete=trueを確認し、再読込とpreview再実行で解消。親子はHistoryOnly/Unknownの表示を維持しつつ履歴正常終了の証拠で判定を通過した。
- 利用者が確認チェック後に取り込みを1回実行。画面「取り込み適用を確認しました」、imports.jsonlのend outcome applied、元repo HEAD 7b062570ddcedde85cb84455634aa68d462ae31aを確認。
- 元repo READMEは見出し＋AgentDock動作確認の2行、git statusは## mainのみでclean。成果コミット7c1df4fも履歴に含まれる。
- 子による読取り→親の専用worktree編集/コミット→元repo取り込みの最終合格条件3点は満たした。ただし途中で2件の修正と更新/再起動を挟んだため、初回から無中断の正常系成功とは区別する。
- 次の別試験は追加指示、停止、再起動復旧。今回の付随観察として再起動後の既存依頼復元と取り込みは確認できたが、独立した復旧試験の合格とは扱わない。
- UI残課題: previewの走査待ちから自動再評価されず手動再読込が必要。取り込み成功後も適用ボタンが有効に見える。新方式の取り込みコミット文言が「組織実行」のまま。今回未修正。
## 2026-10-24 追加指示と再取り込みの実機確認
- 同じ親01a16dc0-4676-79c1-8265-f5bfe579be7dへ完了後の追加指示を利用者が送信。新しい子を作らず、専用worktree READMEへAgentDock追加指示確認を1行追加し61f04adでコミット。取り込み前の元repoは7b06257でclean・追加行なしを確認。
- 同じ依頼のpreviewはREADME +1/-0、成果61f04ad、取込先7b06257で阻害なし。利用者が取り込みを実行し、台帳applied・元repo009f4d00957cc318e98009a6b3a6ba36f17a8ea6を確認。
- 元READMEは見出しと2つの確認行が各1回、git statusは## mainのみ。完了済み親への追加指示と同じ依頼の再取り込みは合格。実行中steer・停止試験はまだ未実施。
## 2026-10-24 親子中断の実機確認
- 同じ親へ停止試験を送り、新Luna子Meitner /root/sleep_probe（01a16e30-898e-7423-ac7c-92c874e36c6d）がStart-Sleep 120秒を実行、親はwait。利用者が入力欄の中断を1回押した。選択肢は表示されず親子とも中断表示。
- activity.jsonlで親turn 01a16e30-7a34-7952-a0de-335a45d42797が22:54:26、子turn 01a16e30-8b6f-7d60-bdc9-4101a918a941が22:54:27にinterrupted終了したことを確認。正常完了や要求のみとは区別できる。
- 元repoは009f4d0でclean、専用worktreeもclean。親子turnの中断確認は合格。OS上の外部プロセス全停止までを保証する試験ではない。再起動後の停止状態復元と二重送信抑止は次の独立試験。
## 2026-10-24 停止後の再起動復旧確認
- 利用者が通常終了/起動後に既存親を開いた。親は状態不明・履歴取得済み/live未復旧、子2件は履歴確認（完了1/中断1）。履歴とlive状態を混同していない。
- 保存queueはpausedAfterRestart、entries空、awaitingなし、unresolvedSends空、pendingOps空。活動記録のuserMessageは初回/追加指示/停止試験の計3turnのままで、再起動後の追加送信・新turnは観測されない。対象の子孫走査はfound=2 complete=true。
- 元repoはclean。停止済みの依頼を再起動後に履歴として復元し、勝手に再送しない試験は合格。実行中にアプリ終了した場合/受理不明送信/切断故障は未試験。
- 入力欄の受理未確認はCenterPaneのaccepted model設定表示であり、送信受理不明の表示ではない。履歴だけ開いた状態では設定のlive受理未確認が正しい。
## 2026-10-24 実行中追加指示の実機確認
- 利用者が親単独の待機試験を送信し、実行中にCtrl+Shift+Enterで追加指示を送信。両userMessageは同じturn 01a16e35-6bcc-7b41-9095-57bba0af3682に記録。queue entries空・unresolvedSends空で完了後の別送信ではない。
- 同turn最終応答「実行中追加指示を受信」、23:01:26のcompleted終了をログで確認。元repoと専用worktreeはclean。
- 実行中steer受信・反映・正常終了は合格。活動ログでは待機コマンドの完了項目を確認していないため、PowerShellの120秒待機自体の実行保証とは区別する。今回必要な判定は実行中追加指示の同turn受理。
## 2026-10-24 実行中からの安全な終了・再起動
- 親単独でStart-Sleep 180秒のinProgress表示を確認。利用者のメニュー終了で「作業を中断して終了」の確認が出るため、通常経路では中断なし終了ではなく安全終了の試験に変更した。強制終了は未実施。
- 対象turn 01a16e3b-b694-7832-8719-2e9444f3b0baは23:08:20にinterrupted終了、その後disconnected。再起動後の履歴で待機コマンドfailed/終了コード-1を表示。正常完了とは扱っていない。
- queueはpausedAfterRestart、entries/awaiting/unresolvedSends空。対象活動記録に再起動後の追加userMessageや新turnはない。元repo clean。
- 実行中からの終了確認→中断→アプリ終了→再起動後の履歴復元・二重送信抑止は合格。通常経路での確認範囲は終了。異常終了/切断/受理不明の故障試験は未実施。
## 2026-10-24 UI情報整理
- 利用者承認により左ペインの検索は常設1本、作業フォルダ/親状態/アーカイブは絞り込みへ集約。適用条件を閉じてもチップと全解除を表示。クイック新規・並べ方は小メニューへ移設。
- サブエージェント依頼は「新しい依頼」と「依頼一覧」に分離。主操作は親チャットを開く/取り込み内容を確認。作業場所・開始段階・復旧候補・コミット・履歴は詳細表示へ。親の名前欠損も「親」で明示、readyを作業完了と表示しない。元フォルダ（取り込み先）の意味を明記。
- 取り込み確認は変更ファイル/増減/成果パス/取込先を表示し、確認チェックを維持。apply開始時に確認内容を消費、適用結果はpreview外へ保持。拒否や通信失敗時も同一確認の再利用を防止。台帳Beginなしを確認できた拒否は再previewへ復帰、unknown/partialは照合が済むまで禁止。
- 走査中だけ3秒ごと最大20回の非重複・読取り専用更新。状態変更や走査解消で古いpreview/確認チェックを破棄。新規依頼や送信/再開/取り込みは自動実行しない。
- 「受理未確認」バッジを「モデル未確認」へ。共通取り込みコミットの見出しを中立な「AgentDock: 成果を取り込み」へ。
- 同時実行: subagent_startのoperation lockは開始処理中だけで、その後の複数依頼実行を1件に制限しない。依頼ごと別worktree。Codex側の子数/資源上限は別。複数実依頼の負荷試験は利用量節約のため未実施。
- Sol worker実装・Sol reviewerレビュー指摘（拒否後verifyからの復帰）を反映。Luna testerがRust既存コミット文言テスト1件成功。worker/tester/reviewerが利用上限で停止後、親が残りのUI確認/ビルドを引継ぎ、最終fixを再確認。
- npm build(tsc含む)成功。通常実行はesbuildの親ディレクトリアクセス拒否、同一コマンドの権限付き再実行で成功。既知import混在/bundle容量警告のみ。debug NSISは既存dist再利用・beforeBuildCommand省略、dev compile 40.65秒・終了0。広範Rust回帰/追加Codex依頼は実行せず。
- 実コンポーネント+固定データのブラウザQA: 左条件開閉/表示/解除、タブ間draft保持、通常適用は1回かつpreview消費、開始前拒否→結果照合→新確認へ復帰、既存unknownは照合後も禁止、960px固定枠で配置確認。IPCはstubで実保存データ未使用。fixture欠損のassignment/blockersを補正して確認。実アプリUI更新後の実通信確認は未実施。
- 一時Viteサーバーは対象PID/コマンドを照合して停止、4つのQAファイルはTEMPへ退避。既存の生成型/schema変更・既存Rust終了照合fixは保持。ソースは未コミット、push/インストールなし。
- UI改善版installer: C:\Users\wmasa\Documents\Rust\AgentDock_next\app\src-tauri\target\debug\bundle\nsis\AgentDock_0.0.1_x64-setup.exe / 7634847 bytes / SHA-256 575FF38FA56BF9281841DFE5DDF0F4ACDC4420F289D5A86F0E24A86300655B1B / 2026-10-24T23:38:08。QA控え: C:\Users\wmasa\AppData\Local\Temp\agentdock-ui-qa-20261024-235017。

## 2026-10-25 クイック新規の常設復帰とリリース準備
- 利用者指定でクイック新規を左ペイン上部の元のアイコンボタンへ戻し、メニュー内の重複を削除。他のUI整理は維持。
- 簡易確認として差分確認とnpm build(tsc含む)成功。変更は配置復帰のみのため広範テストは反復せず、前節の関連Rust/実機/固定データ確認を再利用。画面buildは既知のesbuildアクセス制限を権限付き再実行で解消。
- 利用者がpushとリリースビルドを明示承認。feature/subagent-migrationをoriginへpushし、確認済みdistを再利用してdebug指定なしのNSISを生成する。実機へのインストールはしない。生成物のハッシュと対象HEADは配布物のbuild.jsonに記録する。

## 2026-10-25 Visual Editorの設計案
- 対象: feature/subagent-migration / 調査基点09b00e3。利用者の仕様整理01〜05に基づき、[DESIGN_VISUAL_EDITOR.md](DESIGN_VISUAL_EDITOR.md)を新規作成。設計のみ、アプリ実装・保存schema変更なし。
- 06は「確認したルート内の関連HTML/CSS/JSを自動検出」を推奨。専用編集用コピー→採用で元へ反映、最新の未採用編集単位を戻す方式は追加提案であり未合意。元仕様の確定事項と区別した。
- 現行App/CenterPane・通常送信/受理照合・AiBackend・Git基準の変更確認/復元・フォルダ操作ガード・Tauri設定を確認。プレビュー隔離、DOMとソース、停止照合、差分/部分反映/復旧、受入条件を設計。既存Git復元を今回専用の復元保証とは扱わない。
- 次: 追加提案3点の決定とV0技術検証。専用WebView中央配置、frameの通信/遷移遮断と独自IPC拒否、相対module等、Windowsの並行変更保護は未検証。実Codex・実WebView・ビルド試験は今回実行せず。文書整合確認とgit diff --checkは成功（既存生成ファイル等のLF/CRLF警告あり）。
- 今回起動した常駐プロセスなし。設計書と本追記は未コミット。調査開始時からの生成IPC型・desktop/windows schemaの未コミット変更は保持。commit/push・実機データ操作は行っていない。

## 2026-10-25 master反映とVisual Editor着手
- 利用者の明示指示でmasterをfeature/subagent-migrationの09b00e3へ早送りし、origin/masterへpush。git ls-remoteでリモートの同一hashを確認。旧master fbe67caは既存baselineタグと履歴に保持。force pushなし。
- 続いてfeature/visual-editorを09b00e3から作成。設計書とその引き継ぎは新ブランチへ持ち越した。生成型/schemaの変更表示39件はhash-object --no-filtersとindex blobの比較ですべてバイト一致（内容差分0件）。破棄操作なし。
- 対象: Visual EditorのV0技術調査と、範囲・編集用コピー・B/E/C競合判定の独立Rust基盤。受入: 元を変えずコピーできる、範囲外/特殊パスを拒否、追加/変更/削除や他作業の競合を検出、上限とバイト保持。安全性未確認のプレビュー/採用を製品IPCから有効化しない。
- Luna Explorerが専用WebViewと通信/IPC遮断を調査、Sol Workerが新規Rustモジュールを実装。最終確認はLuna Tester、独立レビューはSolを予定。親は統合と文書を担当。必要な確認は関連Rust試験とcargo check --locked。UI/既存送信を変えない段階では画面build/実Codex試験を反復しない。
- 調査開始時点でAgentDock本体・cargo/rustc・関連開発サーバーは観測されず。Codexアプリ自体のプロセスは稼働。実アプリ保存データ・~/.codexには触れない。新ブランチのpushはまだ行わない。
## 2026-10-25 Visual Editor初期基盤とV0実測
- 対象feature/visual-editor。887b639は明示範囲のsnapshot・分離コピー・読取り専用競合判定、fc15894/dfce3bdはopt-in WebView2 probeと起動修正・実測。新ブランチは未push。master/origin/masterは09b00e3のまま。
- Sol WorkerがRust基盤、別Sol Workerがprobeを実装。Luna Explorerが依存ソース/IPC/起動を調査、Luna Testerが最終CLIと実reportを照合、別Sol Reviewerが独立レビュー。パス・並行操作・未信頼WebViewを含むため高リスクとして分担。Astra不使用。helperの区切り比較・reparse検査の2指摘を修正後、実験コードの追加必須修正0件。製品隔離は承認していない。
- `cargo test --manifest-path app/src-tauri/Cargo.toml --lib visual_edit --locked` は7/7成功。最初の通常実行はjunction fixture作成Access deniedで6/7だったため、同じコマンドを権限付きで再確認して成功。製品assertion失敗と環境拒否を区別した。
- 最終 `cargo check --manifest-path app/src-tauri/Cargo.toml --locked` はbuild.rs修正後も成功（39.04秒）。probeの `cargo build --manifest-path app/src-tauri/Cargo.toml --example visual_preview_probe --features visual-preview-probe --locked --offline` は最終7.65秒で成功。既存ts-rs属性・incremental hard-link fallback警告あり。
- 初回GUI起動の承認待ちが中断されたが、利用者の再承認後に親が起動。TaskDialogIndirectのloaderエラーをComputer Useで確認して閉じ、native終了-1073741511を取得。Cargo exampleへmanifestが適用されないことが原因で、probe feature＋Windows MSVC＋examplesだけにCommon-Controls v6 manifestを追加。実PEも照合した。
- 承認を再利用する固定helper `app/verification/visual-editor/Run-NativeProbe.ps1` を追加。Inspectは読取り、Runは既存probeを拒否して固定exe/引数で非表示起動し、GUID別ログと自身PIDの25秒待機を使う。自動停止なし。reparseは拒否するが事前検査はatomicではなく、ランチャーを並行実行しない。
- 実測NativeOnlyは11:15:22〜11:15:30 JST、Layeredは11:19:35〜11:19:42 JST、いずれもnative終了2。CSP併用で要求拒否を観測したが、禁止先の別loopbackポートへTCP接続が届いた。Layeredの150ms読取りは0 bytes/HTTP行なし。原因は未確定でpreconnectを断定しない。**V0通信隔離は不合格**。
- fixtureの相対CSS/module/SVGと親DOM到達拒否を観測。iframeにはTauri globalsが見えた。製品IPC・中央UI追従・全通信手段は未検証。詳細/実report ID/EXE hashはverification/visual-editor/V0_NATIVE_PROBE.md。製品UI/AI接続/採用writer/undo writer/台帳/loaderは未実装で公開しない。
- 次: 専用host/OS側を含む通信境界の補強を設計し、V0を再評価する。未確認のbrowser flagで成功扱いにせず、Windows設定/firewallや~/.codexを勝手に変更しない。UI未変更なのでnpm type-check/buildは反復せず、実Codex呼出しも行っていない。
- 承認設定を読取り調査: 現在のprojectはtrusted、default_permissionsはrelease-projects（workspace継承＋明示root/.git書込み）。GUI/ネットワーク/システム照会まで無制限にする設定ではない。config.toml変更なし。担当間の実行セッションIDは共有できず、以後は実行担当が終了結果を回収してから渡す。
- probeは各試験後にhelper processes=[]で終了確認。常駐プロセスの追加なし。通常AgentDock保存領域・認証・設定への書込みなし。生成型/schemaの既存変更表示39件は最終raw hash比較ですべてindexとバイト一致を確認。内容の破棄やステージはせず保持。試験profile/report/logはignoredなtarget配下へ保持。

## 2026-10-25 通信経路の分離とOffline比較

- 対象feature/visual-editor、基点84ba0d7。利用者の続行指示で、既存5攻撃を別canaryへ分離。read前にTCP acceptを専用上限付き記録へ残し、通常reportはstop/join後にsnapshotする。watchdog終了5に末尾accept完全収録の保証はない。実Runtime版も取得。
- Luna調査、Sol実装、親が承認済み固定helperで逐次GUI検証、Luna Testerがreportを独立照合、別Solがレビュー。Astra不使用。権限付きGUIを担当間で重複実行せず、すべて親が終了結果を取得。
- NativeOnly/Layeredの両方でnested-frame宛てTCP1件/HTTP0bytes。redirect宛てのacceptはなし。内部機構や以前の共有canaryの原因まで断定しない。
- 続いてOffline条件を追加。全資源・302をメモリ応答にし、CDP2コマンドの完了後だけNavigate。設定失敗/白紙/資源不足/通信fallbackで成功にしない。CDPはOS境界ではない。
- Offlineも設定・全資源の表示は成功したが、nested-frame TCP1件/0bytesと予約originへのTCP accept5件を観測し、native終了2。旧イベント名にfallback-attemptとあるがHTTPは読まず、実HTTP fallbackは確認していない。**V0隔離は不合格のまま。** 最終ソースではTCP acceptの名前に訂正、offlineValid項目を追加。終了条件は不変。
- focused example buildは16.20秒（経路分離）、8.36秒（Offline）、6.79秒（命名/出力項目訂正）で終了0。最終diffcheck/PowerShell構文検査成功。通常lib/Cargo/build.rs/UIは未変更のため、前回成功の基盤Rust7件/通常checkと画面buildを反復せず。実Codex試験は行っていない。
- 実測report・時刻・試験hash/最終build hashはverification/visual-editor/V0_NATIVE_PROBE.mdに分離して記録。命名/出力項目だけの変更後はGUI反復なし。Solは診断コード保存可、製品有効化不可と判断。既存icon/Chrome_WidgetWin警告は残る。
- 次の検証計画はverification/visual-editor/NETWORK_BOUNDARY.md。独立preview hostをOSのネットワーク境界に置く方式を候補とするが、起動・全子プロセス・中央UIは未実証。profile/ACL/firewall/パッケージ登録などOS設定はまだ変更していない。独立profileだけで通信隔離とせず、JavaScript無効化等で仕様を勝手に縮小しない。
- 3回のGUI後はいずれもhelperのprocesses=[]で終了確認。通常AgentDock領域・~/.codexは未変更。targetの実測ログ/profileは保持。新ブランチは未push、master/origin/masterは09b00e3のまま。既存生成型/schemaの変更表示は引き続き保持する。
## 2026-10-25 利便性を優先した通信許可と製品統合着手

- 利用者が「プレビューの通信は許可し、アプリ権限・ファイル範囲は守る」方針を明示承認。厳格なTCP遮断/OS通信隔離を初期必須条件から外す。過去の不合格記録は保持するが、現在の阻害条件にしない。要件正本/設計§15を更新し、旧NETWORK_BOUNDARYを参考扱いへ変更。
- 対象feature/visual-editor、基点cc69010。今回の実装対象は実際の中央preview＋要素選択＋既存チャットへの情報挿入。AI自動編集・採用・復元の完成とは区別する。挿入だけで送信しない。
- Luna Explorerが既存Tauri/IPC/画面を調査。Sol Backend/UIが担当ファイルを分けて実装。Luna Testerが最終検証、別Solが独立レビュー。親は方針/統合/文書を担当。Astra不使用。
- 受入: root内相対HTML/CSS/module/画像表示とHTTP(S)依存利用、選択/複数/親/Esc、選択情報挿入時の既存draft保持、閉じる/overlay時のnative child退避、previewから全製品command/plugin/任意ファイルの拒否。過去のTCP到達自体を失敗条件にしない。
- 確認予定: 関連Rust純粋ロジック試験・cargo check --locked、appでnpm type-check/build。必要な実WebView検証は専用identifier/profile/固定fixtureで行い、通常アプリ領域や認証へ書かない。実Codex送信・採用はまだ試験しない。