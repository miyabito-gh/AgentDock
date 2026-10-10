# DESIGN_P5 — 段階⑤ 一覧・本文・検索・再送・表示設定（設計）

版 0.1（2026-10-10）。要件の正本は `AgentDock_Requirements.md` 版1.3（M47〜M52、M16の詳細、受入 A124〜A134）。見た目の規則は `app/DESIGN_UI.md`（v0.3）§0・§2 に従う。段階④-UIの実機確認（UI-6）の前に、本書の全タスクを1つのバッチとして実装し、実機確認をまとめて行う。

## 0. 守ること（全タスク共通）

- CLAUDE.md の合意: resumeしない（監視・検索・表示のために再開しない）／切断・再起動・受理不明で無条件に再送しない／通信断・取得不能を done・failed に変換しない／未確認の操作を成功表示しない／`~/.codex` を変更しない。
- DESIGN_UI §0: 状態と鮮度は別の見た目、色だけで区別しない、AA コントラスト、キーボード操作とフォーカス表示、通知・更新でフォーカスを動かさない、根拠の文言（未確認・live未確認・受理済み／未受理・履歴で〜を確認・状態不明・停止未確認・受理不明・削除保留）を消さない。
- §2.1 の色: 黄（`--wait-*`）は要対応だけ。本書で増える色（検索のハイライト）は黄を使わずアクセントの淡色にする（§4.4）。
- 新しい保存項目は `AppSettings` に `#[serde(default)]` で足す（古い settings.json がそのまま読める）。ts-rs で `src/ipc/gen/` を再生成し、`ipc/types.ts` から再export、モックの既定値（`src/mock/`）も足す。
- 設定の保存は全置換の `setAppSettings` なので、連続操作で古い値に戻らないよう、App.tsx に**直列化した更新関数** `updateSettings(f: (s: AppSettings) => AppSettings)` を置く（最新値を ref に持ち、前の保存の完了を待ってから次を送る。保存結果で ref を更新）。本書の新項目はすべてこれを通す。既存の呼び出し（layout・通知等）も P5-0 で置き換える。
- 単体テストは純粋関数だけ（`node --test src/ui/xxx.test.ts`、Rust は変更した crate の該当テスト）。画面のテストは書かない。

## 1. チャット行の「…」メニュー（M47）

### UI
- 行の構造を変える（`<button>` の入れ子は不可のため）: `div.row-chat`（`role="listitem"` は付けない。現行どおり一覧はボタンの並び）の中に、選択用の `button.row-main`（現行の行の中身・`aria-current`・窓表示の `data-idx` はこちら）と、右端の `button.row-more`（`aria-label="「{チャット名}」の操作"`、`aria-haspopup="menu"`、`aria-expanded`）。
- 「…」の表示: `.row-chat:hover`・`:focus-within`・メニューを開いている間・選択中の行で見える。`@media (hover: none)` では常時。見えない間も `visibility:hidden` ではなく `opacity:0`（Tab で到達できる）。
- 開き方: 「…」押下、行の右クリック（`onContextMenu` で `preventDefault`）、`row-main` にフォーカスがあるときの `Shift+F10`・アプリケーションキー（`ContextMenu`）。どれも同じ `Popover`（`role="menu"`）を「…」ボタンに寄せて開く（マウス位置には出さない。位置計算を増やさない）。Esc・外側クリックで閉じ、フォーカスを「…」へ戻す。
- 項目（上部メニュー「チャット」と同じ順・同じ文言）: 名前を変更… ／ ピン留め・ピン留めを外す（状態で切替） ／ アーカイブ・アーカイブを解除（状態で切替） ／ Markdown にエクスポート… ／ 区切り ／ 削除…（危険の色）。無効なときは上部メニューと同じく理由を `.why` で項目内に出す（title だけにしない）。
- メニューを開く・項目を選ぶだけでは選択中のチャットを変えない・`touch_chat_used` を呼ばない。ダイアログ（名前・削除・エクスポート）は対象チャットの名前を題名に出す（選択中と違うチャットを操作していることが分かる）。

### 状態・IPC
- IPC の追加なし。App.tsx の `openDelete`・`openExport`・`archiveOp`・`togglePin`・名前変更の起点を、**対象チャットID を引数に取る形**へ変える（上部メニューは選択中のIDを渡す）。`runDelete` 後に `selRef.current === 対象` のときだけ選択を外す（現行どおり）。
- LeftPane へ `onChatAct(op: "rename"|"pin"|"archive"|"unarchive"|"export"|"delete", chatId)` を渡す。

### 受入（A132）
- 「…」・右クリック・Shift+F10 で同じメニュー。選択していない行で削除・アーカイブ・ピン・名前変更・エクスポートが、その行のチャットに対して上部メニューと同じ確認・結果表示で動く。選択中のチャットと最近利用の順が変わらない。窓表示（200件超）でも矢印キー移動が今どおり。

## 2. 作業フォルダごとの一覧表示（M48）

### 規則（純粋関数 `ui/folderGroups.ts`、テスト `folderGroups.test.ts`）
- **フォルダのキー** `folderKey(path)`: `/`→`\`、先頭の `\\?\` を外す、末尾の `\` を外す（`C:\` は残す）、ASCII の大文字小文字を同一視（小文字化）。表示用のパスは、そのグループで最近利用が最も新しいチャットの元の文字列。
- **所属**:
  - `kind === "general"` → グループ「一般チャット」（キー `general`）。
  - `cwd` が値なし（未取得・不明） → グループ「作業フォルダ不明」（キー `unknown`。ピン不可、常に最後）。
  - `cwd` が `snap.worktrees`（AgentDockが作ったworktree台帳）のどれかの `path` と同じか配下 → その `repoRoot` のグループ。行に `worktree` タグ（title にブランチ名とパス）。台帳にない外部のworktreeは、その `cwd` そのもののグループ（Gitを実行して推定しない）。
  - それ以外 → `cwd` そのもののグループ。親フォルダへのまとめはしない（推定しない）。
- **並び**: 上部の「ピン留め」欄（チャットのピン、現行どおり。ここに出たチャットはグループ内に重ねて出さない） → ピン留めしたフォルダ（ピンした順） → その他のフォルダ（中のチャットの最近利用の最大値の新しい順。最近利用は現行の `sortChats` と同じ根拠＝ユーザーの利用だけ） → 「作業フォルダ不明」。「一般チャット」も通常のフォルダと同じく並び、ピンできる。グループ内は現行の `sortChats` 順。
- **短縮表示** `shortPath(display, all)`: 末尾2階層（`C:\Users\a\Rust\AgentDock` → `Rust\AgentDock`）。ドライブ直下は `C:\foo`、ドライブそのものは `C:\`、UNC も末尾2階層。同じ短縮名が別グループにあれば、該当するものだけ3階層・4階層と延ばして区別する。全文は見出しの `title` と見出しメニューの「パスをコピー」。
- **検索中**: 一致したチャットがあるグループだけ出し、折りたたみを無視して開いて表示する（保存値は変えない）。
- **アーカイブ表示**: 従来どおり平らな一覧（グループにしない）。

### UI
- 一覧上部1行目に「表示 ▾」（`Popover`、`role="menu"` の `menuitemradio`）: 「最近使った順」「作業フォルダごと」。DESIGN_UI §4 の上部 ≤80px は維持（新規・更新と同じ行に置く）。
- 見出し `div.list-grp`（高さ固定 `GROUP_HEADER_HEIGHT = 28px`）: 開閉ボタン（`aria-expanded`、▸/▾、短縮パス、件数）、ピンの印（ピン時のみ。文字「ピン」を `aria-label` に）、**集計の印**（中のチャットの `待`・`失`・作業中の件数。折りたたみ中も出す。印の部品は行と同じ `.mark`）、「…」（フォルダをピン留め／外す・パスをコピー）。
- 窓表示: 行と見出しを平らな並びにし、2種類の固定高さの累積で窓を計算する（`listWindow.ts` に `windowRangeVar(heights, scrollTop, view)` を追加し、テスト）。矢印キーは行（`row-main`）と見出しの開閉ボタンを順に移動。

### 保存
- `AppSettings.chat_list: ChatListSettings`（`#[serde(default)]`）:
  - `view: ChatListView`（`Recent`〈既定〉／`ByFolder`）
  - `pinned_folders: Vec<String>`（`folderKey`。並び＝ピンした順。外すと除く。チャットが1件もないキーは表示しないが値は残す）
  - `collapsed_folders: Vec<String>`（`folderKey`）
- チャットのピンは従来どおり chat.json（変更なし）。フォルダのピンを AppSettings に置く理由: フォルダ単位の保存先が他になく、chat.json に置くと同じフォルダの全チャットへの書込みが要る。

### 受入（A133）
- `folderGroups.test.ts`: キーの正規化（大小・区切り・末尾・`\\?\`）、一般・不明・worktree（台帳の path 配下→repoRoot）・外部worktree、並び（ピンしたフォルダ→最近利用→不明が最後）、短縮と重複時の延長、ピン留めチャットの重複なし、検索時の展開。`listWindow` の可変高さのテスト。
- 再起動（モックは再読込）で表示の切替・フォルダのピン・折りたたみが戻る。折りたたんだ見出しに待・失・作業中の件数が出る。

## 3. Markdown の整形表示（M16 の詳細）

### 方式の比較と決定

| 観点 | A: `react-markdown`＋`remark-gfm`（採用） | B: `markdown-it`／`marked`＋`DOMPurify` | C: 自前の小さなパーサ |
|---|---|---|---|
| 安全性 | React要素を組み立てる。`innerHTML` を使わない。危険なURLの既定の除去あり。生HTMLの扱いはプラグイン数行で「文字として表示」に固定できる | HTML文字列を `dangerouslySetInnerHTML` で入れる。消毒器の設定と更新に安全性が依存 | `innerHTML` を使わなければ安全。ただし入れ子・表・リストの解釈の穴が表示崩れになる |
| 保守 | GFM（表・取消線・タスク・自動リンク・脚注）が仕様準拠。部品差し替え（a・img・code・table）が型付きで書ける | 拡張は文字列処理で、部品差し替えがReactの外 | GFMの全要素を自前で保守。逐次表示の途中状態の扱いも自前 |
| サイズ（gzip、目安） | 約45〜60KB（micromark・mdast・hast 一式） | 約30KB＋消毒器約20KB | 数KB |
| 逐次表示 | 毎回全体を再解析（間引きで対処、§3.5） | 同じ | 同じ |

決定: **A**。DESIGN_UI §0 の「ライブラリを追加しない」の唯一の例外とする（v0.3 で明記済み）。版は `react-markdown@9`・`remark-gfm@4`（React 18 と組む版。実装時に `npm view` で最新の 9.x・4.x を確認し、`package-lock.json` で固定）。追加するのはこの2つだけ（rehype 系・シンタックスハイライトは入れない）。

### 安全条件（実装で必ず守る）
1. **生HTMLは文字**: remark プラグイン `remarkHtmlAsText`（mdast の `html` ノードを同じ値の `text` ノードへ置き換える、数行）を必ず入れる。`rehype-raw` は使わない。`skipHtml` で消すのではなく、見える文字として残す（Codexが書いたものを黙って消さない）。
2. **リンク**: 部品 `a` を差し替える。`http:`・`https:` だけ押せる。押すと確認ダイアログ（§3.3）を経て、既存のコマンド `open_authorization_url`（http・https のみ開く、UIの明示クリック時だけ）で既定のブラウザへ。それ以外（`file:`・相対パス・`mailto:`・`javascript:` 等）は押せない文字＋「コピー」だけ（`title` に「このリンクは開きません（http・https以外）」）。`urlTransform` は既定（危険なスキームを空にする）のまま。
3. **画像**: 部品 `img` を差し替え、読み込まない。`[画像: {alt}]` と URL の文字（http・https ならリンクと同じ確認経路で「ブラウザで開く」を出す）。`data:` URL も読み込まない（URLは長いので先頭40文字＋「…」）。既存の添付・成果物の画像プレビュー（localImage 等）はMarkdownと別経路なので変えない。
4. **依頼（ユーザーの発言）は整形しない**: 書いたとおりの文字（`white-space: pre-wrap`）。作業の記録・承認カードも現行どおり文字。
5. エクスポート（M41）は現行どおり原文を出す（変更なし）。

### 部品
- `ui/Markdown.tsx`: `<Markdown text streaming />`。`React.memo`（`text`・`streaming` が同じなら再描画しない）。部品の差し替え: `a`→`ExtLink`、`img`→`ImageStub`、`pre`/`code`→`CodeBlock`（ブロック）／インラインはそのまま `code`、`table`→横スクロールの枠（`div.md-table` に `overflow-x:auto`）、`input`（タスクリスト）→`disabled` のチェックボックス＋読み上げ用の「済み／未」。見出しは本文内で小さめ（h1=1.25em〜h6=1em）。
- `ui/markdown/plugins.ts`: `remarkHtmlAsText`、リンクの可否の判定 `linkPolicy(url): "open" | "copyOnly"`（純粋関数）。
- `CodeBlock`: 上部の小さな帯に言語名（あれば）・「コピー」・「折り返す」（切替。既定は折り返さず横スクロール）。41行以上のブロックは高さを20行分に制限し「すべて表示（N行）」で解除（本文はDOMに全部ある＝検索の対象。§4）。
- 応答の「コピー」（現行 `onAct("stub")` で未実装）を実装: 応答の原文（Markdownのまま）をクリップボードへ。コードブロックの「コピー」はコードの原文。書込みは `@tauri-apps/plugin-clipboard-manager` の `writeText`（capability `attachments.json` に `clipboard-manager:allow-write-text` を追加、`main` 窓のみ）。結果はボタンの文字を2秒「コピーしました」に変え、`aria-live` で読み上げる。失敗は「コピーできませんでした」。

### 3.3 リンクの確認ダイアログ
- `Dialogs.tsx` に `{ type: "openLink"; url: string; text: string }`。本文: 「外部のページをブラウザで開きます」、行き先の**全文**（`mono`、折り返し）、ホスト名を太字。リンクの表示文字がURLの形で、実際の行き先のホストと違うときは警告の帯「表示の文字と実際の行き先が違います」。フッター: 副「URLをコピー」、右に「キャンセル」と主「ブラウザで開く」。「今後確認しない」は置かない。

### 3.4 本文の大きさの上限
- `FORMAT_LIMIT_CHARS = 100_000`、`FORMAT_LIMIT_LINES = 3_000` を超える応答は整形せず `pre-wrap` の文字で出し、上に1行「長いため整形せずに表示しています」＋「整形して表示」（その応答だけ。画面内の記憶）。

### 3.5 逐次表示（ストリーミング）
- 対象: 終端が未確認の turn（`TurnRecord.end === null`）の最後の応答。
- 再解析の間引き: 表示する文字列を最短120ms間隔で更新（`useThrottledText`）。turn が終端になったら即座に最終文字列で描画。
- 逐次表示中に `30_000` 文字を超えたら、完了まで整形せず文字で出す（完了時に §3.4 の上限で判定し直す）。
- 閉じていないコードブロック・表の途中: CommonMark どおり、閉じていないコードフェンスは末尾までコードとして表示する（追加処理なし）。完了時の最終文字列で確定する。途中の見た目のちらつきは許容する（間引きで頻度を下げる）。
- 完了済みの応答は `memo` で再解析しない（チャットの切替・他チャットの更新で全応答を解析し直さない）。

### UI・CSS
- styles.css に節 `/* ==== markdown ==== */` を新設（P5-2 だけが書く）。本文の文字は `--fs-body`（§6）。コードは `--mono`、`font-size: .92em`。表・引用・水平線・タスクの線と面はトークンだけで（直書き色なし）。リンクは下線＋外部の印（↗、`aria-hidden`）、`:focus-visible` を出す。

### 受入（A124〜A126）とテスト
- `markdown.test.ts`（`React.createElement`＋`react-dom/server` の `renderToStaticMarkup`。JSXを使わずに `.ts` で書く）: `<script>alert(1)</script>`・`<img src=x onerror=alert(1)>` が要素にならず文字で出る／`[a](javascript:alert(1))` に `href` が付かない／`![x](https://e/x.png)` に `<img` が出ない／表・取消線・タスクが要素になる／閉じていないフェンスが `code` になる。`linkPolicy` のテスト。
- 目視: モックの応答に Markdown の見本（全要素・長い表・200行のコード・HTML入り）を1件足して確認する（`src/mock/`）。
- `vite build` の出力サイズを着手前後で記録し、増分を報告する（目安 +200KB 未満〈非圧縮〉）。

## 4. 会話内検索（M49）

### UI
- `ui/FindBar.tsx`: 会話領域の右上に重ねる小さな帯（レイアウトを押し下げない）。入力欄、「n / N件」、前へ（↑）、次へ（↓）、閉じる（×）。0件は「見つかりません」。1000件で打ち切り「1000件以上」。
- キー: `Ctrl+F` で開く（開いていれば入力欄へフォーカスし全選択）。WebView2 の既定の検索より先に `keydown`（capture）で受け、`preventDefault`。ダイアログ・コマンドメニューの表示中と監視窓では開かない。帯の中で `Enter`＝次、`Shift+Enter`＝前、`F3`／`Shift+F3` も同じ。`Esc` で閉じ、ハイライトを消し、開く前のフォーカス（入力欄など）へ戻す。
- 検索語は入力から150msで実行。チャットを切り替えたら同じ語で切替先を検索し直す（帯は開いたまま）。

### 方式
- 対象: `.msgs` の中の依頼・応答の本文の要素（`data-find="body"` を付けた要素）だけ。作業の記録（折りたたみ）・承認カード・帯・ボタンの文字は対象外（帯に `title`「依頼と応答の本文を検索します（作業の記録は対象外）」）。表示済みの本文だけを検索し、追加の取得・再開・`thread/read` をしない。チャットの本文が部分取得のとき（`TurnRecord.complete === false` を含む）は帯に「読み込み済みの範囲を検索」と出す。
- 一致: 本文要素ごとに `TreeWalker` で文字ノードを集め、連結した文字列で探して文字ノードの位置へ戻す（太字の境目をまたぐ一致も見つける）。英字の大文字小文字だけ同一視（ASCII）。全角半角・かなの同一視はしない。純粋関数 `ui/find.ts` の `findInSegments(segments: string[], query: string, limit: number): Hit[]`（`Hit` は開始と終了の（区間番号, 位置））とテスト `find.test.ts`。
- ハイライト: CSS Custom Highlight API（`CSS.highlights` に `find-hit` と `find-cur` の `Highlight` を登録。DOM を書き換えないので React の描画と衝突しない）。色は `--find-hit`（アクセントの淡色）と `--find-cur`（アクセントの濃い面＋文字色の反転）。黄は使わない（§2.1）。API がない環境では件数と移動だけ（現在位置は `Selection` で示す）。
- 移動: 現在の一致の要素を `scrollIntoView({ block: "center" })`。高さ制限中のコードブロック（§3）の内側でも、そのスクロール枠の中で見える位置へ動く。移動は利用者の操作なので、最下部追従（`stick`）を外してよい（現行の「最新へ」が出る）。
- 本文の更新（逐次表示）: `MutationObserver` で `.msgs` の変化を見て300msごとに再検索。現在位置は「直前の現在の一致の文書上の位置以降で最初の一致」に合わせ直す（飛ばない）。

### 受入（A127）
- `find.test.ts`: 区間をまたぐ一致、大小の同一視、重なる一致を数えない（左から非重複）、上限、空語。目視: 50件の一致でハイライト・前後移動・Esc・チャット切替・逐次表示中の追従。

## 5. 編集して再送・再生成（M50）

### 操作と画面
- 依頼の吹き出し（ホバー・フォーカス時の操作。DESIGN_UI M3 と同じ出し方）: 「コピー」「編集して分岐…」。
- 応答: 「コピー」「ここから分岐」（現行）「もう一度…」。「もう一度」は各 turn の**最後の**応答にだけ出す。
- どちらも押すと `ResendDialog`（新規 `ui/ResendDialog.tsx`）を開く。無効なときもボタンは `aria-disabled`（フォーカス可）で、押すとダイアログが開いて理由だけを示す（理由を title だけにしない）。
- `ResendDialog`:
  - 題名「編集して分岐」／「もう一度（再生成）」。冒頭: 「元の会話はそのまま残ります。turn {n} の直前までを分岐した新しいチャット『{名前}（編集）』／『{名前}（再生成）』へ送ります。」。分岐の確認状況が未確認なら規則どおり「未確認」＋説明1文。
  - 本文: 依頼の文（編集は編集可能な複数行入力、再生成は読取り表示＋「文を編集する」で編集に切り替え。切り替えたら目的は「編集」になる）。注記: 「元の依頼の添付・Skill指定は引き継ぎません。必要なら新しいチャットで添付し直してください。」（常時。元に添付があったかは本文の記録から判定できないため）。
  - フッター: 副「分岐して入力欄へ（送らない）」、右に「キャンセル」と主「分岐して送信」。`Ctrl+Enter` で主。押した後は実行中表示にし、二度押しできない。
- 対象が会話の**最初の turn** のとき（分岐の終点が無い）: 分岐ではなく「新しいチャットに入れる」とする。主ボタンは「新しいチャットの入力欄へ」だけ（送信しない）。新しいチャットは元と同じ種類・作業フォルダ・モデル・権限で既存の `start_chat` で作り、`set_draft` で文を入れて選択する。

### 有効の条件（UI の純粋関数 `ui/resend.ts` の `resendAvailability(ctx)`＋テスト。ホストも同じ条件を検査し、ホストの判定を正とする）
最初に当たった理由を返す:
1. 分岐の能力が非対応・非推奨 → その理由（`availability` と同じ文言）。
2. 接続していない → 「Codex に接続していません」。
3. 削除保留 → 「削除保留中のチャットです」。
4. 外部で実行中の可能性（閲覧のみ） → 「外部で実行中か確認できないため使えません（再開の案内から確認してください）」。
5. 停止未確認 → 「停止を確認できていないため使えません」。
6. チャットの誰か（メイン・子孫）が作業中・対応待ち → 「実行中は使えません。完了するか、中断と停止の確認の後に使えます」。
7. メインの状態が不明 → 「状態を確認できないため使えません（「状態を再照合」で確認できます）」。非live でも、履歴で終端を確認できていれば使える（§4.2 と同じ考え方）。
8. 対象 turn に依頼が2件以上（実行中の追加指示を含む） → 「追加指示を含むturnは、編集・再生成できません」。編集の対象が追加指示そのものなら「追加指示は編集して送り直せません」。
9. 分岐の終点（対象の1つ前の turn）の終端が未確認（`end === null`）、または turn ID が無い → 「直前のturnの終了を確認できていません」。
10. 対象が最初の turn → 有効だが「新しいチャットに入れる」経路（上記）。

### IPC（ホスト）
```rust
/// 編集して再送・再生成（M50）。元の会話は変えない。分岐（`thread/fork`、終点 = through_turn）→ 分岐先へ下書き保存 → （send のときだけ）通常の送信経路で1回送る。
pub struct ResendAsForkArgs {
    pub chat: ChatKey,
    /// 分岐の終点。対象turnの1つ前の終端turn（最初のturnは対象外。UIは start_chat の経路を使う）。
    pub through_turn: ExternalId,
    /// 編集・再生成の対象turn。ホストは through_turn の直後がこの turn であることを履歴で照合する（UIの表示が古い場合に別の位置で分岐しない）。
    pub target_turn: ExternalId,
    pub text: String,
    pub purpose: ForkPurpose,
    /// false＝分岐して下書きに入れるだけ（送らない）。
    pub send: bool,
}
pub struct ResendAsForkResult {
    pub fork: ForkResult,                 // 既存の型。ack が Unknown なら attemptedAt で reconcile_fork（読取り）
    pub draft_saved: bool,                // 分岐先の下書きへ保存できたか
    pub send: Option<SendAttempt>,        // 送った場合だけ。既存の送信試行（acceptanceUnknown は再送しない）
}
#[serde(rename_all = "camelCase")] pub enum ForkPurpose { Fork, EditResend, Regenerate }
```
- `ForkOrigin` に `#[serde(default)] purpose: ForkPurpose`（既定 `Fork`）。中央ヘッダーのタグは「分岐」「編集から」「再生成」、title に元のチャットと turn。
- ホストの手順（`host/thread_ops.rs`。既存 `fork_chat` の中身を `fork_inner(args, purpose, name_suffix)` に分けて共用）:
  1. 上の条件 1〜9 を検査（`rules/thread_ops.rs` の純粋関数 `resend_blocker(...) -> Option<Blocker>`、テスト）。`target_turn` の照合に失敗したら `InvalidArgs`「表示が古いため、会話を読み直してからやり直してください」。
  2. 分岐。`Accepted` 以外ならここで返す（送らない。`Unknown` は `attempted_at` を返し、UIは既存の「分岐先を確認」＝`reconcile_fork` を出す）。
  3. 分岐先の名前を「{元の名前}（編集）」「（再生成）」にする（既存の `rename_best_effort`）。
  4. 分岐先の下書きに `text` を保存（保存の完了を待つ）。保存できなければ送らない（`draft_saved: false`、`send: None`）。
  5. `send` のときだけ、既存の送信経路（`send_message` と同じ内部関数。チャットの送信ロック・`clientUserMessageId`・Sending保存・受理不明の扱い）で1回送る。添付なし、意図は新しいturn。`Accepted` なら下書きを消す（既存の送信と同じ）。`Rejected`／`acceptanceUnknown` は下書きを残す。再送しない・ループしない。
- 分岐が受理不明→照合で採用できた場合: UI は採用されたチャットを選び、`set_draft` で文を入れて「分岐先を確認しました。依頼を入力欄に入れました。送信は入力欄から行ってください。」と出す（自動で送らない）。
- 送信後は分岐先を選択して表示する（ユーザーの操作の結果なので選択を移してよい）。元の会話は一覧に残り、何も送られない。

### 受入（A128〜A130）
- `rules` のテスト: 条件1〜9の各理由、`target_turn` の不一致、追加指示を含む turn。ホストのテスト（既存の偽バックエンドがあれば）: 分岐 `Unknown` で `turn/start` が呼ばれない、下書き保存失敗で送らない、送信 `acceptanceUnknown` で再送しない。`resend.test.ts`（UI 側の理由の順）。
- 目視（モック）: 依頼の編集→新しいチャットが選ばれ元が残る／無効理由がキーボードで読める。

## 6. 文字サイズ・テーマ（M51）

### 保存
`AppSettings.appearance: AppearanceSettings`（`#[serde(default)]`）:
- `theme: ThemePref`（`System`〈既定〉／`Light`／`Dark`）
- `ui_text: UiTextSize`（`Small`／`Normal`〈既定〉／`Large`）
- `body_text: BodyTextSize`（`Small`／`Normal`〈既定〉／`Large`／`XLarge`）

### 適用
- styles.css: トークンのダーク値の塊を `@media (prefers-color-scheme: dark){:root{…}}` から `:root[data-theme="dark"]{…; color-scheme: dark}` へ移す（ライトは `:root` のまま、`color-scheme: light`）。`prefers-color-scheme` は CSS から無くす。
- 文字サイズのトークン（`:root[data-ui-size]`・`:root[data-body-size]`）:

| 項目 | 小 | 標準 | 大 | 特大 |
|---|---|---|---|---|
| `--fs`（画面） | 12px | 13px | 14px | — |
| `--fs-s` | 11.5px | 12px | 13px | — |
| `--fs-xs` | 11px | 11px | 12px | — |
| `--fs-body`（会話本文） | 12px | 13px | 15px | 17px |

  `--fs-xs` は 11px 未満にしない（DESIGN_UI P13）。styles.css と TSX の inline style にある px の font-size（例 `.chead .ttl{font-size:15px}`）は `calc(var(--fs) + 2px)` などトークン基準へ置き換える。`.msgs` の本文（依頼・応答・Markdown）は `--fs-body`、作業の記録・注記は画面側のトークン。
- 一覧の行の高さ: `listWindow.ts` の固定値 `ROW_HEIGHT` を `rowHeight(uiSize)`（40/40/44px）に変え、LeftPane は設定から渡す。DESIGN_UI §4.2 の高さの目標は「標準」で判定する。
- 画面側 `ui/appearance.ts`（純粋関数 `resolveTheme(pref, osDark)` と DOM へ反映する `applyAppearance(settings)`）。`System` のときは `matchMedia('(prefers-color-scheme: dark)')` の変化を購読する。
- 起動時のちらつき防止: `main.tsx` と監視窓の入口で、描画前に `localStorage["agentdock.appearance.v1"]`（最後に反映した値の写し）を読んで `data-*` を付ける。設定（ホスト）を受け取ったら上書きし、写しも更新する。写しは表示用の控えで、正は `AppSettings`。
- Windowsのタイトルバー: `getCurrentWindow().setTheme(System なら null、他は "light"/"dark")` を主画面と監視窓で呼ぶ。capability `default.json` に `core:window:allow-set-theme` を追加。失敗しても画面の中は切り替わる（タイトルバーだけOSに従う）ので、エラーは出さない。
- 監視窓（`MonitorApp.tsx`）もスナップショットの設定で同じ `applyAppearance` を呼ぶ。

### 設定画面
- 設定の分類に「表示」を追加（`TABS` の「全般」の次）。項目: テーマ（ラジオ3つ）、画面の文字（3つ）、会話本文の文字（4つ）、一覧の並び（§2 の切替と同じ値）。変更はすぐ反映して保存（`updateSettings`）。保存に失敗したら既存の書き方でトースト、表示は戻さない（次回起動で保存値に戻ることを文に含める）。

### 受入（A134）
- `appearance.test.ts`（`resolveTheme` の4通り）。目視: 3テーマ×画面の文字3×本文4のうち、小・標準・大×ライト・ダークの主要画面で崩れ・横スクロール・読めない色がない。再起動で戻る。監視窓に効く。古い settings.json（`appearance`・`chatList` なし）が読める（Rust テスト）。

## 7. 前バッチの修正（A〜D）

### A. 状態不明の子の「履歴で再確認」（M52）
- ホスト（`host/queue_driver.rs` の `recheck_queue_state` の読取り部分を共用関数に切り出す）:
```rust
/// 状態不明の子孫の保存履歴を読むだけ（resume・送信・停止をしない）。キューの有無に依存しない。
pub struct RecheckAgentsArgs { pub chat: ChatKey, pub agents: Vec<AgentKey> }
pub struct RecheckAgentsResult { pub results: Vec<AgentRecheck> }
pub struct AgentRecheck { pub agent: AgentKey, pub outcome: RecheckOutcome }
pub enum RecheckOutcome {
    /// 履歴で終端を確認した（状態は書き換えない。表示は「履歴で〜を確認」へ）。
    TerminalFound,
    /// 読めたが終端を確認できない。状態不明のまま。
    StillUnknown,
    /// 読めなかった（理由）。状態不明のまま。
    Unreadable { message: String },
    /// 対象外（このチャットの子孫でない・メイン・作業中・live）。読まない。
    Skipped { reason: String },
}
```
  - 接続なしは `NotConnected`。対象の選別は純粋関数（`rules/` に `recheck_candidates`、テスト: メイン・live・作業中・他チャットを除く）。読取りは既存の `read_terminal_fact`（同時実行の上限は既存の read の制限に従う）。終わったら既存どおり `kick_queue()`（キューの送信条件は合意済みの規則のまま。ボタンはキューを直接送らない）。
- UI（`Dock.tsx`）: 「状態不明の子 N件」の見出しの右に `btn sm`「履歴で再確認」（title「保存履歴を読むだけです。再開・停止はしません。確認できなければ状態不明のままです」）。実行中は「確認中…」で無効。結果はトースト「履歴で確認 n件／状態不明のまま m件／読めない k件」。監視窓には置かない（本窓のみ）。
- 受入: A131。Rust: 選別のテスト、`Unreadable` で状態が変わらないこと。

### B. 使用量・控え・Gitなし
1. `UsageReport` に `#[serde(default)] webview_cache: Known<u64>`（専用領域の `EBWebView` フォルダの大きさ。読めなければ `Unknown`）。メタデータの集計から `EBWebView` を除く。合計は従来どおり専用領域全体（内訳の行が1つ増える）。設定「保存と容量」に行「画面表示のキャッシュ（WebView2）」、`?` に「画面の表示に使う一時データです。チャットのデータではありません。AgentDockは削除しません。」。削除ボタンは作らない。
2. 「控えがない」理由を1つに（`revertView.ts`）: 次の順で最初に当たったものだけを出す。①設定で控えの記録がオフ ②作業フォルダが Gitリポジトリではない（`git_info` で確認済み） ③Gitの状態を確認できていない（`git_info` 未取得・失敗） ④控えを取る前のturnだけ。入力に `baselinesEnabled` と `git: "repo" | "notRepo" | "unknown"` を足す。テスト更新。帯は1本（DESIGN_UI P7 の規則）。
3. Gitなしの無効理由: `CommandContext` に選択中チャットの Git の有無（`git_info` の結果を作業フォルダごとに画面内で記憶。チャット選択時と作業フォルダ変更時に1回読む。読取りのみ）を入れ、`availability` の `needsGit` の判定を配線する。メニュー「作業」・Ctrl+K・中央の「…」で、理由「Gitリポジトリではありません」が項目内（`.why`）と title に出る。未確認（取得前・失敗）のときは無効にしない（現行の方針「確認できているときだけ」）。

### C. 文言
1. ドックの失敗カードのボタン「確認済みに」→「確認済みにする」（`flex:none`・`white-space:nowrap` で途切れない。`aria-label`・title は今の全文のまま）。DESIGN_UI §7 D11 の短縮を更新する扱い。
2. 「原状態: notLoaded（根拠なし）」→「原状態: notLoaded（Codexが読み込んでいない状態。完了・失敗の根拠なし）」。原文のラベルと「根拠なし」の意味は残す。既知の原文に説明を添える対応表を `chatState.ts` に置き（`notLoaded` のみ。他の原文は従来どおり「（根拠なし）」）、テストを足す。

### D. 総点検（最後に1回）
- 到達性: 重要な情報（無効理由・状態の根拠・警告）が `title` だけにある箇所を `grep -n "title=" src/ui` で洗い、キーボードで読めるように（項目内の `.why`、`?` の `Popover`、`aria-describedby`、無効ボタンを `aria-disabled` でフォーカス可に）する。一覧を作り、直した／意図して title のまま（補足のみ）を verifier へ渡す。
- `:focus-visible`: 全ての操作部品（本書で増えた「…」・見出し・Markdownのリンク・コードの操作・検索帯）で見える。
- 不要CSS: styles.css のセレクタのうち `src/` から参照されないものを削除（動的なクラス名 `st ${...}` 等は残す。削除した一覧を報告）。
- ダークのコントラスト: 主要な文字色と面の組（本文・注記・状態チップ・鮮度・未確認・リンク・検索ハイライト・コード）を表にし、AA を満たさない組を直す。
- 狭い画面: 1279・959・600px でヘッダー・一覧の見出し・検索帯・ResendDialog・リンク確認・設定「表示」。
- Esc でダイアログを閉じる件は §10 要判断1。

## 8. IPC・保存の追加のまとめ

| 種類 | 追加 | 所在 |
|---|---|---|
| 設定 | `AppSettings.chat_list`（view・pinned_folders・collapsed_folders）、`AppSettings.appearance`（theme・ui_text・body_text） | `backend/local.rs` |
| 保存 | `ForkOrigin.purpose`（既定 Fork） | `backend/model.rs` |
| コマンド | `resend_as_fork`（`ResendAsForkArgs`→`ResendAsForkResult`） | `backend/ipc.rs`・`commands.rs`・`lib.rs`・`host/thread_ops.rs`・`rules/thread_ops.rs` |
| コマンド | `recheck_unknown_agents`（`RecheckAgentsArgs`→`RecheckAgentsResult`） | 同上＋`host/queue_driver.rs`・`rules/queue.rs` |
| 型 | `UsageReport.webview_cache` | `backend/local.rs`・`host/manage.rs` |
| 権限 | `clipboard-manager:allow-write-text`（main）、`core:window:allow-set-theme`（main・monitor） | `src-tauri/capabilities/` |
| 依存 | `react-markdown@9`・`remark-gfm@4` | `package.json` |

いずれも ts-rs で `src/ipc/gen/` を再生成し、`ipc/types.ts` の `Commands` 表とモック（`src/mock/`）を更新する。

## 9. タスク分解

全タスク: 担当 implementer、検証 verifier（受入条件・合意・差分の照合）、CLAUDE.md の検証ループ（修正→検証 最大3回、超えたら要判断として記録し次へ）。確認は `cd app && npx tsc --noEmit`、ホストを触るタスクは `cd app/src-tauri && cargo check` と変更モジュールのテスト、純粋関数は `node --test src/ui/xxx.test.ts`。共通の受入: DESIGN_UI §6 の①（意味の変わる文言の削除なし）④⑤（フォーカス・直書き色なし）、新しい黄の使用なし。

styles.css の節: P5-0 が `tokens` を書き換え、`markdown`（P5-2）・`find`（P5-3）・`resend`（P5-4u）の節を空で先に作る。各タスクは自分の節と、表に書いた既存の節だけを触る。

| # | 内容 | 依存 | 触るファイル | 個別の受入条件 |
|---|---|---|---|---|
| P5-0 | 基盤＋(6) 文字サイズ・テーマ: `AppSettings.chat_list`・`appearance`（serde default）と ts-rs・モック既定、`updateSettings`（直列化）と既存の設定保存の置換、`data-theme`／サイズのトークン化（px の font-size の置換）、`appearance.ts`、main.tsx・MonitorApp の適用と localStorage の写し、`setTheme` と capability、設定「表示」タブ、`rowHeight(uiSize)`、styles.css の新節の空作成 | — | `backend/local.rs`、`src/ipc/gen/*`、`ipc/types.ts`、`src/mock/*`、`App.tsx`（設定保存部）、`main.tsx`、`MonitorApp.tsx`、`ui/appearance.ts`＋test、`ui/Dialogs.tsx`（TABS と表示タブ）、`ui/listWindow.ts`、`styles.css`（tokens と font-size の置換）、`capabilities/default.json` | §6 の受入。古い settings.json が読める Rust テスト。連続で2項目を変えても両方保存される（モックで確認） |
| P5-H | ホスト一式: `resend_as_fork`（§5）、`recheck_unknown_agents`（§7A）、`UsageReport.webview_cache`（§7B1）、`ForkOrigin.purpose`、`fork_inner` への切り出し、rules の純粋関数とテスト、ts-rs・`types.ts` の `Commands` 表・モックの空実装 | — | `backend/{ipc,model,local}.rs`、`commands.rs`、`lib.rs`、`host/{thread_ops,queue_driver,manage}.rs`、`rules/{thread_ops,queue}.rs`、`src/ipc/gen/*`、`ipc/types.ts`、`src/mock/*` | §5・§7A の Rust テスト。分岐 Unknown・下書き保存失敗で送らない、送信受理不明で再送しない（偽バックエンドで確認できる範囲）。`cargo check` |
| P5-1 | (1) 行メニュー＋(2) フォルダ別表示: 行の構造変更、「…」・右クリック・Shift+F10、見出し・折りたたみ・フォルダのピン・集計の印、「表示 ▾」、`folderGroups.ts`＋test、`windowRangeVar`＋test、App.tsx の操作を対象ID引数へ | P5-0 | `ui/LeftPane.tsx`、`ui/folderGroups.ts`＋test、`ui/listWindow.ts`＋test、`App.tsx`（削除・アーカイブ・ピン・エクスポート・名前の起点と LeftPane の props）、`styles.css` の `side` 節 | §1・§2 の受入 |
| P5-2 | (3) Markdown: 依存追加、`Markdown.tsx`・`markdown/plugins.ts`・`CodeBlock`・`ExtLink`・`ImageStub`、リンク確認ダイアログ、応答とコードのコピー（stub の解消）と capability、逐次表示の間引き・上限、モックの見本 | P5-0 | `package.json`・`package-lock.json`、`ui/Markdown.tsx`（新）、`ui/markdown/plugins.ts`（新）、`ui/markdown.test.ts`（新）、`ui/CenterPane.tsx`（`Messages` の本文と応答の操作のみ）、`ui/Dialogs.tsx`（`openLink`）、`App.tsx`（`openLink` の配線のみ）、`capabilities/attachments.json`、`src/mock/*`、`styles.css` の `markdown` 節 | §3 の受入。`vite build` のサイズ増分の報告 |
| P5-3 | (4) 会話内検索: `FindBar.tsx`、`find.ts`＋test、Ctrl+F（capture）・F3・Esc、Highlight API、`data-find` の付与、MutationObserver | P5-2 | `ui/FindBar.tsx`（新）、`ui/find.ts`＋test（新）、`ui/CenterPane.tsx`（`.msgs-wrap` に帯、本文に `data-find`）、`App.tsx`（キー処理のみ）、`styles.css` の `find` 節 | §4 の受入 |
| P5-5u | (7) A〜C の画面: ドックの「履歴で再確認」、使用量のWebView2の行、控えの理由1つ、Gitなしの配線、文言2件 | P5-H、P5-0 | `ui/Dock.tsx`、`ui/chatState.ts`＋test、`ui/revertView.ts`＋test、`ui/ChangesDialog.tsx`、`ui/commands.ts`、`ui/PrefsDialogs.tsx` または容量表示の所在、`App.tsx`（git_info の記憶と再確認の呼び出しのみ）、`styles.css` の `side`・`settings` 節 | §7 A〜C の受入 |
| P5-4u | (5) 編集して再送・再生成の画面: 依頼・応答の操作ボタン、`ResendDialog`、`resend.ts`＋test、最初の turn の経路（start_chat＋set_draft）、分岐 Unknown→照合→下書き、ヘッダーの「編集から」「再生成」タグ | P5-H、P5-3 | `ui/ResendDialog.tsx`（新）、`ui/resend.ts`＋test（新）、`ui/CenterPane.tsx`（吹き出しの操作とヘッダーのタグ）、`ui/Dialogs.tsx`（種類の追加）、`App.tsx`（配線）、`styles.css` の `resend` 節 | §5 の受入 |
| R5 | 設計担当のレビュー: P5-H と P5-4u の差分（分岐→下書き→送信の順序、受理不明、送信ロック、条件の検査漏れ、resume の有無）→ `app/REVIEW_R5.md` | P5-4u | — | 不具合一覧または合格 |
| P5-7 | R5 の指摘の修正 | R5 | 指摘による | 指摘の解消 |
| P5-6 | (7) D 総点検（§7D） | P5-7 | `styles.css` 全体、`ui/*.tsx`（title・focus のみ） | §7D の一覧とコントラスト表の提出、DESIGN_UI §6 の③（3幅×2テーマのスクリーンショット）。要判断1が採用ならダイアログの Esc もここで |
| S5 | 司令塔: NSIS 再ビルドと実機確認手順書（`app/LIVE_CHECK_UI.md` に段階⑤の確認を追記し、UI-6 とまとめる） | P5-6 | — | ユーザーの確認結果 |

順序: (P5-0 ∥ P5-H) → (P5-1 ∥ P5-2) → (P5-3 ∥ P5-5u) → P5-4u → R5 → P5-7 → P5-6 → S5。

worktree 並行の可否:
- P5-0 ∥ P5-H: 可。重なりは `backend/local.rs`（別の構造体）、`src/ipc/gen/`（別ファイル）、`ipc/types.ts`・`src/mock/` の追記位置だけ。マージは P5-0 を先に、P5-H で ts-rs を再生成して衝突を解く。
- P5-1 ∥ P5-2: 可。App.tsx は P5-1 が「削除・アーカイブ・エクスポート・ピン」の区画と LeftPane の props、P5-2 が `openLink` の配線だけ。styles.css は別の節。CenterPane は P5-2 だけ。
- P5-3 ∥ P5-5u: 可。CenterPane は P5-3 だけ、Dock・設定は P5-5u だけ。App.tsx はキー処理（P5-3）と git_info・再確認（P5-5u）の別区画。
- P5-4u は CenterPane の `Messages` を P5-2・P5-3 と共有するので単独（並行しない）。P5-6 は styles.css 全体を触るので単独・最後。

## 10. 要判断（既定案で進めてよい）

1. **設定以外のダイアログを Esc で閉じるか**（挙動の変更）。既定案: **閉じる**。Esc＝そのダイアログの「閉じる／キャンセル」と同じ動作（`closeDialog` 経由。終了の確認は既存どおり「取り消す／待つ」になり、実行中の操作は止めない。危険操作の2段目では1段目へ戻るのではなく閉じる＝キャンセル）。入力中のIME変換（`isComposing`）では閉じない。P5-6 で実施。選択肢: 閉じない（現状維持）。
2. **実行中のチャットで編集・再生成を許すか**。既定案: **許さない**（§5 条件6）。分岐自体は終端turnまでなら実行中でも可能だが、送信先と元の実行が並ぶと、どちらの結果か取り違えやすく、停止確認とも絡むため。選択肢: 対象の直前turnが終端なら許す（元の実行は続く）。

## 11. CLAUDE.md への追記案（司令塔が追記）

```
### 段階⑤のタスク（2026-10-10分解。設計は `app/DESIGN_P5.md`。正本1.3 M47〜M52・M16詳細）

|#|内容|担当|依存|
|---|---|---|---|
|P5-0|基盤＋文字サイズ・テーマ（AppSettings.chatList/appearance、updateSettings直列化、data-theme、設定「表示」）|implementer|—|
|P5-H|ホスト: resend_as_fork、recheck_unknown_agents、UsageReport.webviewCache、ForkOrigin.purpose|implementer|—|
|P5-1|行の「…」メニュー＋作業フォルダごとの表示（フォルダのピン・折りたたみ）|implementer|P5-0|
|P5-2|Markdown整形（react-markdown＋remark-gfm、安全条件、コピー、リンク確認）|implementer|P5-0|
|P5-3|会話内検索（Ctrl+F、Highlight API）|implementer|P5-2|
|P5-5u|前バッチA〜Cの画面（履歴で再確認、WebView2の行、控えの理由、Gitなし配線、文言）|implementer|P5-H・P5-0|
|P5-4u|編集して再送・再生成の画面（ResendDialog）|implementer|P5-H・P5-3|
|R5|P5-H・P5-4uの差分レビュー→`app/REVIEW_R5.md`|designer|P5-4u|
|P5-7|R5の指摘の修正|implementer|R5|
|P5-6|総点検（到達性・focus-visible・不要CSS・ダークのコントラスト・狭い画面）|implementer|P5-7|
|S5|NSIS作り直しと実機確認手順（UI-6とまとめる）|司令塔|全部|

順序: (P5-0 ∥ P5-H) → (P5-1 ∥ P5-2) → (P5-3 ∥ P5-5u) → P5-4u → R5 → P5-7 → P5-6 → S5。

段階⑤で合意した判断（2026-10-10）:
- Markdownは react-markdown＋remark-gfm だけ例外として追加。生HTMLは文字で表示、リンクは確認後に http・https だけ開く、リモート画像は読み込まない、依頼は整形しない。
- 編集して再送・再生成は、対象turnの直前までの分岐へ送る（元の会話は残す）。押したときだけ1回送り、分岐・送信の受理不明は照合のみ。最初のturnは新しいチャットの入力欄へ入れるだけ。添付は引き継がない。
- テーマは OSに従う（既定）／ライト／ダーク。文字サイズは画面（3段）と本文（4段）を分ける。DESIGN_UI §0 を v0.3 で改訂。
- フォルダのピン・折りたたみ・一覧の表示はAppSettingsに保存（チャットのピンは従来どおりchat.json）。worktreeのチャットは元リポジトリの見出しへ。
```
