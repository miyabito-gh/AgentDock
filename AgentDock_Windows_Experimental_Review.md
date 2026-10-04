# CLI 0.160.0 Windows終了範囲とexperimental条件
確認日: 2026-10-04。対象ref rust-v0.160.0。検索は位置発見のみ、根拠はタグ指定取得。

## 結論
Windowsの管理プロセス終了経路はあるが、全OS子孫停止の保証としては不足。experimentalは接続単位で明示する条件を確認したが、安定性・サポート保証とは別。App Server最終採用は保留。

## Windowsの終了範囲
- pipe.rs 174–205: Job Objectを作り、spawn後にprocessを登録する。登録前に生成された子孫がJobに入らない競合をソースコメントが明示。Job作成・登録失敗時は単一PID終了へfallbackする。
- pipe.rs 47–104: WindowsのInterruptはkillへ進む。Jobならjob.terminate、fallbackならTerminateProcess。OS子孫全件の列挙・確認ではない。
- pipe.rs 271–281: 親の正常終了後はjob.preserve_descendantsを呼ぶ。
- win/job.rs 215–252: preserve_descendantsは明示的Job終了とkill-on-closeを無効化する。以後terminateはOkを返して終了処理を省略する。保持と終了の競合は先にlockを取った処理が決める。
- pty.rs 79–110、133–137: Windows PTYはConPtySystem。signal Interruptはこのwrapperではunsupported。killはChildKillerへ委譲する。pipeのInterruptと同一の挙動ではない。
- win/mod.rs 115–118: WinChildKillerはjob.terminateへ進む。同ファイルの正常終了待ちもpreserve_descendantsを呼ぶ（132–144行）。
- process.rs 221–226: request_terminateはkiller.killのエラーを破棄する。terminateはこれを呼んでI/O補助taskをabortする。前回のLocal terminate_confirmedがこの戻り値なしの経路へ進むため、上位の成功だけではOS終了の成否を確定できない。
- Jobに登録されたプロセス群へ終了APIを呼ぶ根拠はある。ただし管理外・登録前・保持済み子孫の終了、実環境の終了完了確認は未確認。JobObjectにsuspended spawn用helperが存在しても、pipeのspawn経路へ適用されるとは推定しない。

## experimental利用条件
- initialize_processor.rs 82–83、134–142: initializeのcapabilities.experimental_apiを接続sessionへ保存する。
- message_processor.rs 975–979: 要求にexperimental_reasonがあり、接続でexperimentalが有効でなければinvalid_requestとして拒否する。
- backgroundTerminalsの3メソッドは前回確認のprotocol/common.rsでexperimental。単にメソッド名を送れば使えるとは判断しない。
- 接続初期化の指定は設計候補であり、共有config.tomlを書き換える要件ではない。今回は接続も設定変更もしていない。
- initialize_processor.rs 75–80付近のコメントには、複数クライアントの異なる能力指定による共有threadの動作差が未解決として記載される。コメントは実装済みの競合防止策の証明ではない。
- 同ファイル105–115のuser verificationはoriginとclient名等の別条件を持つ。experimental有効化で全Desktop専用機能が開放されるとは推定しない。AgentDockが別製品のclient名を偽装する案は採用しない。
- experimental opt-inは利用ゲートの条件。安定API化、製品利用のサポート、利用資格、Windows実動作の保証ではない。

## 採用への影響
既存M要件・10秒後停止未確認・子孫不明時キュー保留を維持。ターン中断、管理API終了結果、OS子孫停止を別々に記録する。今回の情報で全対象停止を成立済みにしない。追加公開経路または不足への対処と要件への影響を具体化する必要がある。

## 根拠
|タグ指定ファイル|blob SHA|
|---|---|
|[codex-rs/utils/pty/src/process.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/utils/pty/src/process.rs)|5593450b89d0578d59f3ab74cb5cfcf625c908f6|
|[codex-rs/utils/pty/src/pipe.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/utils/pty/src/pipe.rs)|802dfa5280101e310cf3a49a6f67f429c8dde679|
|[codex-rs/utils/pty/src/pty.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/utils/pty/src/pty.rs)|035fad8841761f95eee4cd2db0365ea75f39bd0d|
|[codex-rs/utils/pty/src/win/job.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/utils/pty/src/win/job.rs)|f963c6f55fd5786e0249340ac147e69c577ae4b1|
|[codex-rs/utils/pty/src/win/mod.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/utils/pty/src/win/mod.rs)|05063cf66a69f9b0855e54a537ce7d53947e5424|
|[codex-rs/app-server/src/message_processor.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/message_processor.rs)|9e9a90437b3fddde282a0d05216907d77fa5a745|
|[codex-rs/app-server/src/request_processors/initialize_processor.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/request_processors/initialize_processor.rs)|d68a29df9f364ed0836904847bae3927db1ae339|
SHAはファイル内容識別子。実動作検証ではない。

次はPlugins管理の対象版契約と代替公開経路を照合し、要件を満たす経路があるかを判定する。実装・App Server起動・schema生成・実機検証・既存設定変更・HTML変更なし。

