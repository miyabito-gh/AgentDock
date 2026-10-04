# CLI 0.160.0 購読復旧のソース照合

確認日: 2026-10-04（日本時間）  
取得: openai/codex、ref rust-v0.160.0を指定したGitHub読取り。検索は位置発見のみ。

## 結論

**ロード済みIDの再発見と本文・状態の再取得には根拠があるが、作業を変えない再購読はまだ成立確認できない。** 既存live threadへのwarm resumeは購読を戻せる経路を持つ一方、読み取り専用ではない。監視目的で無条件に呼ぶ設計は採用しない。初回生成の自動attachを確認した前回の結論は維持する。

## 確認した事実

|対象タグの処理|確認結果|判断への影響|
|---|---|---|
|thread/loaded/list|同じthread_manager内のIDをページ取得する|生成通知を取り逃したロード済みthreadを再発見する候補。所属・直接親の照合は別途必要。外部プロセスの実行一覧ではない|
|thread/read|read_thread_viewから本文・状態を返す入口。ここで購読を追加する呼出はない|再取得による表示補完の候補。イベント購読復旧の代用とはしない|
|connection_initialized|live_connectionsへ接続能力を登録する|この処理自体は既存thread全件の再購読を行わない。初期化だけで復旧済みと扱わない|
|resume_running_thread|既存threadを取得し、listenerを用意してresume応答処理を送る|既存live状態を使う経路あり。ただしロードされていなければcold resume側へ進むため、観測時点のloaded確認だけで安全を保証できない|
|warm resume応答|try_add_connection_to_threadで接続を追加し、応答後に未解決要求を再提示する|購読回復・要求再提示の根拠あり。再提示で古い要求を自動回答しない設計が必要|
|warm resumeの副作用|client情報更新、場合による履歴persist、idle lifecycleへの通知がある|設定を渡さないことだけで読み取り専用とはいえない|
|設定不一致時|購読者なし・idle等の条件でshutdownしcold resumeへ進める経路がある|監視復旧でモデル・権限等のoverrideを渡す設計は避ける。overrideなしでも他の副作用は残る|

## キューとの接続

warm resume応答末尾はemit_thread_idle_lifecycle_if_idleを呼ぶ。core側はactive turnがなく、trigger mailbox入力もない場合にlifecycle contributorへon_thread_idleを通知する。queue serviceのon_thread_idleはInterrupted以外でdispatch_if_idleへ進む。

したがって、**待機キューの条件によってはresumeが次の入力処理を促す可能性がある**。すべてのresumeが送信するという意味ではない。キューの登録状態・dispatch条件・有効化を今回網羅していないため、実際の送信発生は未確認。AgentDock側キューとCodex側キューを同一視せず、アプリ側キューを空にしただけでCodex側の副作用を防げるとは決めない。

## 根拠

行番号はタグ指定取得本文。blob SHAはファイル内容識別子でありcommit SHAではない。

|ファイル|確認箇所|blob SHA|
|---|---|---|
|[thread_processor.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/request_processors/thread_processor.rs#L2777)|2777〜2840: loaded/list・read、3578〜3601: 接続、4269〜4470: warm resume|289cb63953662eb5fba00222ec49432dd976b944|
|[thread_state.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/thread_state.rs#L368)|368〜378: 初期化済み接続の登録|8cfbc00983aa042873cdd70c4b393dcb7d18cbe5|
|[thread_lifecycle.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/request_processors/thread_lifecycle.rs#L691)|691〜716: 接続追加、780〜814: 応答・要求再提示・idle通知|b0fa49b4babcc5802a9968ea26bc230add7cba17|
|[tasks/lifecycle.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/tasks/lifecycle.rs#L59)|59〜84: idle通知の条件とcontributor呼出|afecaede5abce4db25d5054d9fb265a66b4d3014|
|[queue/service.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/ext/queue/src/service.rs#L549)|549〜565: idle時dispatch_if_idle|06aeb63b4cacccf562ee91897bafbcf962bd978c|
|[ClientRequest.ts](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/schema/typescript/ClientRequest.ts)|標準チェックイン生成要求一覧にresume・unsubscribe等があり、thread/subscribeは見当たらない|5e3fb8c9150b194b0b132935eb4a8fd6398d313d|

標準生成要求一覧に購読専用メソッドが見当たらないことだけでは、experimentalや別経路も含めた不存在の証明にはしない。

## 設計候補と未確認

1. 通知欠落・接続断時は収集状態を不明／切断へ変え、キュー自動送信を保留する。停止・完了を生成しない。
2. 同じApp Serverが生存していればloaded/listとreadで管理対象・直接親・保存本文を再照合する候補。取得可能な保存状態とライブ通知の復旧を別表示にする。常時全会話の本文取得は要求しない。
3. メタデータ読取りとlive状態取得の競合、ページ途中の増減、取得不能・未保存部分を扱う。App Serverプロセスが失われた場合を、同じプロセスへの再接続と分ける。
4. 購読だけを戻す公開経路、またはwarm resumeの副作用を確実に抑える公開条件を追加照合する。既存M11を満たせない場合は不足・補助経路・影響を具体化し、要件を勝手に緩めない。
5. 将来モックには「履歴は再取得済み／ライブ監視は未復旧／送信待ちは保留」の状態を検討する。これは表現案でありHTMLは変更していない。

V04・M09・M11・M12の復旧部分は未確認。App Server最終採用は保留。次はdispatch条件と公開購読契約の追加照合、親・子孫・ツールの停止確認を進める。

実装、App Server起動、schema生成、実機検証、設定変更、キュー操作は行っていない。
