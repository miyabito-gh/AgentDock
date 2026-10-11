# DESIGN_WORKFOLDERS — 作業フォルダの追加と複数フォルダの確認（HANDOFF 39・28・19）

版: 0.1（2026-10-11、設計担当）。コードは未変更。実測（§4）が終わるまで実装しない項目に【実測後】を付ける。

## 0. 前提（schema 0.160.0 で確認した事実）

すべて **experimental**（`ts-stable/v2` には型だけあり、`ThreadStartParams`・`TurnStartParams`・`Thread` の該当項目は無い）。

| 項目 | 形 | 意味（schemaのdoc） |
|---|---|---|
| `thread/start.runtimeWorkspaceRoots` | `AbsolutePathBuf[] \| null` | スレッドの作業ルートを**置き換える**（絶対パス必須） |
| `turn/start.runtimeWorkspaceRoots` | 同上 | このturnと以後のturnで置き換える（sticky） |
| `thread/start.environments` / `turn/start.environments` | `{environmentId, cwd, runtimeWorkspaceRoots?}[]` | 先頭がプライマリ。省略＝既定（sticky）、**空配列＝環境アクセスを無効化** |
| `ThreadStartResponse`・`ThreadResumeResponse`・`ThreadForkResponse` `.runtimeWorkspaceRoots` | `AbsolutePathBuf[]`（必須） | `:workspace_roots` の実体 |
| `Thread.environments` | `ThreadEnvironment[] \| null` | 読み込み済みスレッドだけ。`null`＝未読込みまたは非公開、`[]`＝環境なし。接続状態ではない |
| `environment/info`・`environment/status` | `environmentId` 必須 | 既定環境の一覧取得メソッドは**無い**（`environment/add` はリモートexec-server用） |

rollout の `session_meta.runtime_workspace_roots` と同名であることから、CodexのUIの「複数フォルダ」は `runtimeWorkspaceRoots` 側だと推定（**推定**、§4 M1で確認）。

**設計の主方針**: 送るのは `runtimeWorkspaceRoots`（`cwd`＝プライマリと併送）。`environments` は送らない（`environmentId` の取得元が無く、空配列の誤送信で環境アクセスを止める危険がある）。`Thread.environments` は**読取り専用の確認材料**として使う。→ HANDOFF 39(b)の「`turn/start.environments` で渡す」から変更。ユーザー確認の論点Q1。

## (a) 確認表示

入口: ヘッダー「…」→「作業フォルダ」（既存のM44の項目を「作業フォルダ（N）」に）。ポップオーバーで以下を出す。高さを取らない（会話領域に帯を出さない）。

```
現在（根拠: 直接取得 12:03）
  ● プライマリ  C:\…\DocTrail
    追加        C:\…\shared-lib
次の送信から（未送信）
  ● プライマリ  C:\…\DocTrail
    追加        C:\…\shared-lib, D:\data
プロジェクト「アプリ開発」(推定) : 一致 2/3（不足: C:\…\docs）
[変更…]
```

値の区別（`Known` で表す。文言は固定）:

| 状態 | 出し方 |
|---|---|
| 読み込み済みで `Thread.environments` か `runtimeWorkspaceRoots` を取得 | 「直接取得」＋時刻 |
| AgentDockが送って**受理を確認**した指定のみ（いまは未読込み） | 「AgentDockの送信記録（受理済み・時刻）」。バックエンドの現在値とは断定しない |
| 未読込み・記録なし | 「未確認（このチャットは読み込まれていません）」。**読むためにresumeしない** |
| 受理不明の送信に含まれた指定 | 「未確認（受理を確認できていません）」 |
| experimental無効・拒否 | 「非対応（理由）」。プライマリ（cwd）だけ従来どおり表示 |
| 外部作成チャット | 上と同じ規則。rolloutは読まない |

プロジェクト一致（19）は既存の推定ロジック（cwd一致・projectId優先）に、roots の包含（何個一致・不足）を足すだけ。推定の印を消さない。

## (b) 操作と送信時の受け渡し

### 新規チャット
- 作業フォルダ欄の下に「フォルダを追加」。並びは上から、先頭＝プライマリ（「プライマリ」ラベル、ドラッグ不可の代わりに「↑↓」ボタン＋キーボード操作）。
- 「プロジェクトで新しいチャット」から作るときは roots 全部を既定で並べる（Q5）。外せる。
- `thread/start` に `cwd`＝先頭、`runtimeWorkspaceRoots`＝**全件（先頭を含む）**【実測後: 先頭を含めるべきか M3】。
- worktree指定（P3-7）と併用するとき、プライマリは worktree の場所。追加フォルダはそのまま（worktreeにしない）。

### 既存チャット（M44 の拡張）
- 「次の送信から使う作業フォルダを変更」を「作業フォルダを変更」ダイアログにし、プライマリ変更・追加・削除・並べ替えを1画面で行い、「次の送信から使う」で確定。
- 条件は M44 と同じ: 実行中・停止未確認なら不可（`CwdChangeCheck::Busy`）、送信待ちがあれば件数を出して確認（`NeedsQueueConfirmation`）。
- 確定値は `ChatLocal.next_roots`（§f）に保存。送信まで何も送らない。

### 送信
- `SendMode::NewTurn` のときだけ付ける。追加指示（Steer）には付けない（§3.7）。
- `next_roots` があれば `cwd`＝先頭、`runtimeWorkspaceRoots`＝全件。無ければ両方とも付けない（サーバーのstickyに任せる）。
- 受理を確認したら `apply_roots_after_accept`: `roots_record` を更新し、`next_roots` が送った値と**同じときだけ**外す（`apply_cwd_after_accept` と同じ規則）。
- 受理不明: `next_roots` を残し、表示は「未確認」。既存の照合（`reconcile_send`）に任せ、**無条件に再送しない**。照合で受理なしが確定したら、ユーザーの再送操作のときだけ同じ指定で送る。
- 切断: 状態を done/failed にしない（既存規則）。指定も「未確認」のまま。
- キューの自動送信は送信時点の `next_roots` を使う（モデル・権限と同じ扱い）。
- 分岐・編集して再送（fork）: `thread/fork` にも `runtimeWorkspaceRoots` があるが、既定では付けない（サーバー側の継承に任せる）【実測後 M6】。継承されないと分かったら、分岐元の直接取得値か送信記録を、分岐ダイアログに表示したうえで付ける。

## (c) 安全

追加・変更は「Codexの読取り・編集の範囲が広がる」操作として扱う。

1. **確認ダイアログ**（追加・プライマリ変更のたび。削除・並べ替えだけなら不要）: 追加するパスを全文表示（省略しない）、「このチャットの次の送信から、Codexがこのフォルダを検索・読取り・編集できるようになります」「権限: <現在の権限>」「『変更を戻す』の対象はプライマリだけです（追加フォルダの変更は戻せません）」。
2. **除外（追加不可・理由を出す）**: UNC（`\\server\…`、`\\?\UNC\…`）、ネットワークドライブ（`GetDriveTypeW`＝`DRIVE_REMOTE`）、存在しない・ディレクトリでない、相対パス、重複（`\\?\`除去・区切り統一・大文字小文字無視で正規化して比較）、`%WINDIR%` 以下、`%ProgramFiles%`・`%ProgramFiles(x86)%` 以下、AgentDockのデータ領域（`%LOCALAPPDATA%\com.agentdock.app`。ただし worktrees 配下は可）、`~/.codex` 以下（Q3）。
3. **強い確認**（パスをもう一度入力ではなく、チェックボックス「このフォルダ全体を許可することを理解しました」＋既定ボタンを「やめる」にする）: ドライブ直下、ユーザーホーム直下、`%APPDATA%`・`%LOCALAPPDATA%` 直下、`%ProgramData%` 以下、他のrootの親（入れ子で範囲が実質広がる）、プライマリ以外にgitリポジトリでないホーム外の大きなフォルダ（判定しない。対象外）。
4. ホストで再検査: UIの確認結果を信用せず、`set_next_workspace_roots` 内で同じ判定を再実行し、強い確認対象に `confirmed_strong` が無ければ拒否（UIを経由しない呼出しでも広がらない）。送信直前にも存在・UNCを再確認し、消えていたら送らずに理由を出す（勝手に外して送らない）。
5. 上限: 8件（プライマリ含む）。根拠は表示と確認の負担。Q4で変更可。
6. 状態の根拠: (a)の表のとおり「受理済み／未確認／非対応」を出す。受理済みは「AgentDockが送って受理された」であり、Codexが実際にそのフォルダを使えたことの確認ではない（直接取得で一致したときだけ「反映を確認」）。
7. `config.toml`・`~/.codex` を書かない。rollout を読まない。監視目的の resume・thread/read のための resume をしない。

## (d) 実測で確かめる項目と手順

turn/start の実送信が要る項目（M2〜M6）は利用枠を使うので、**ユーザーの承認後**に最小の依頼（「pwdを表示して」程度）で行う。プローブは `probes/app-server/workfolders-probe.mjs`（新規）として作り、テスト用の空フォルダ2つ（`%TEMP%\ad-wf-a`・`ad-wf-b`）だけを使う。

| # | 確かめること | 手順 | 結果で決まること |
|---|---|---|---|
| M1 | `experimentalApi:true` で `thread/start{cwd:A, runtimeWorkspaceRoots:[A,B]}` が受理されるか。応答の `runtimeWorkspaceRoots` と `thread.environments`（`environmentId` の値、`runtimeWorkspaceRoots` に B が入るか） | turnなし。thread/start のみ | 主方針（runtimeWorkspaceRoots）の可否。`environmentId` の取得元＝`thread/start` 応答の `thread.environments[0].environmentId`（既定環境） |
| M2 | 読み込み済みスレッドの `thread/read`（includeTurns:false）で `environments` が non-null か | M1のスレッドで | (a)の「直接取得」の経路 |
| M3 | `runtimeWorkspaceRoots:[B]`（cwd=Aを含めない）を送ると A が消えるか／自動で残るか | thread/start を2通り | 送るリストに先頭を含める規則 |
| M4 | `turn/start{runtimeWorkspaceRoots:[A,B,C]}` 後、次の `thread/read` に反映されるか（sticky）。次のturnで省略しても残るか | 最小の依頼1回＋read | 「次の送信から」の意味と、省略時の扱い |
| M5 | `sandboxPolicy: workspaceWrite, writableRoots:[]` のとき、Bに書けるか（「Bに a.txt を作って」）。承認要求が来るか、その `cwd`／`environmentId` | 最小の依頼1回。書けたら削除 | 追加フォルダを書込み可にするため `writableRoots` にも入れる必要があるか（Q2） |
| M6 | `thread/fork` が roots を継承するか | M4のスレッドを fork（turnなし） | (b)の分岐の扱い |
| M7 | experimental無効・UNC・存在しないパスでの拒否の形（code・message） | thread/start のみ | 非対応判定（`is_experimental_rejection` に足す文言）と、パス拒否の表示 |
| M8 | `instructionSources`（応答）に追加フォルダの AGENTS.md が入るか | Bに AGENTS.md を置いて thread/start | 「AGENTS.mdはプライマリ基準」の説明文の正否 |

`environmentId` は保存しない（サーバーが付ける内部ID）。表示・照合にも使わない。

## (e) experimental非対応時

- `Capabilities` に `workspace_roots: Support` を追加（`projects` と同じく `#[serde(default = "support_unknown")]`）。Codexアダプターは `enable_experimental` が偽なら `Experimental`、真なら `Supported`。
- 実行時に `is_experimental_rejection` か `-32601` で拒否されたら、そのセッションの能力を `Unsupported` に落とし（`HostEvent::CapabilitiesChanged` 相当で通知）、送信は**指定を外して自動で送り直さない**。その送信は「拒否（複数フォルダは非対応）」として失敗表示し、ユーザーが「プライマリだけで送る」を押したときだけ1回送る。
- `Unsupported`/`Experimental` のとき: 「フォルダを追加」と並べ替えを無効化し、ツールチップに理由。プライマリの変更（M44）は従来の `cwd` だけで動く。既存の `next_roots` に追加分があれば「非対応のため追加フォルダは送られません」を変更ダイアログに出す（送信時に黙って捨てない＝送信前に確認を求める）。

## (f) 境界の内側に置く型・IPC・保存形式

### 共通モデル（`backend/model.rs`、ts-rs出力）— Codexの語（environment、runtimeWorkspaceRoots）を使わない

```rust
/// チャットが使う作業フォルダの並び。先頭がプライマリ。パスはOSの絶対パス（`\\?\`を外した表示形）。
pub struct WorkFolders { pub folders: Vec<String> }

/// 作業フォルダの値の出どころ。
pub enum WorkFoldersBasis { Direct, AgentDockAcceptedSend, }

/// 確認表示用（get_work_folders の戻り値）。
pub struct WorkFoldersView {
    pub current: Known<WorkFolders>,          // Direct のみ。未読込み=NotFetched、非対応=Unsupported
    pub last_accepted: Option<AcceptedFolders>,// AgentDockの送信記録（受理済み）
    pub unconfirmed: Option<WorkFolders>,      // 受理不明の送信に含まれた指定
    pub next: Option<WorkFolders>,             // 次の送信から使う（未送信）
    pub project_match: Option<ProjectFolderMatch>, // 推定。印つき
    pub support: Support,
}
pub struct AcceptedFolders { pub folders: WorkFolders, pub accepted_at: UnixMillis }
pub struct ProjectFolderMatch { pub project_name: String, pub matched: u32, pub total: u32, pub missing: Vec<String>, pub inferred: bool }

/// フォルダ1件の検査結果（純粋関数＋OS問い合わせの薄い層。rules/work_folders.rs）。
pub enum FolderCheck {
    Ok { normalized: String },
    NeedsStrongConfirm { normalized: String, reasons: Vec<StrongReason> },
    Rejected { reason: FolderReject },
}
pub enum StrongReason { DriveRoot, HomeRoot, AppDataRoot, ProgramData, ContainsOtherFolder }
pub enum FolderReject { Unc, NetworkDrive, NotAbsolute, NotFound, NotDirectory, Duplicate, SystemFolder, AppDataOfAgentDock, CodexHome, TooMany }
```

`BackendChat` 等の既存型は変えない。`SendRequest` に `work_folders: Option<WorkFolders>`（Steerでは常にNone）、`StartChatParams` に `extra_folders: Vec<String>` を足す。Codexアダプターだけが `cwd`＋`runtimeWorkspaceRoots` へ変換し、応答の `runtimeWorkspaceRoots`／`Thread.environments`（先頭環境の `cwd`＋`runtimeWorkspaceRoots`）を `WorkFolders` に戻す。

`AiBackend` に読取り1つ: `async fn read_work_folders(&self, chat: &ChatKey) -> BackendResult<Known<WorkFolders>>`。**読み込み済み（`list_loaded` で確認）のときだけ** `thread/read` し、未読込みなら通信せず `NotFetched`。

### IPC（`backend/ipc.rs`、`{args}` 形にそろえる＝HANDOFF 15の教訓）

| コマンド | args | 戻り |
|---|---|---|
| `get_work_folders` | `{chat}` | `WorkFoldersView` |
| `check_work_folder` | `{chat?, path, current: Vec<String>}` | `FolderCheck` |
| `set_next_work_folders` | `{chat, folders: Vec<String>, confirmedStrong: Vec<String>, queueRetargetConfirmed: bool}` | `SettingsImpact`（既存） |
| `start_chat`（既存） | `StartChatArgs.extraFolders: Vec<String>`、`confirmedStrong` 追加 | 既存 |

`set_chat_cwd` は残し、内部で `set_next_work_folders([cwd] ＋ 既存の追加分)` に委譲（UIは段階的に移行）。

### 保存（`ChatLocal`／`store/records.rs`、すべて `#[serde(default)]`）

```rust
pub next_folders: Option<Vec<String>>,      // 次の送信から。None＝変更なし（next_cwd と併存、next_folders 優先）
pub accepted_folders: Option<AcceptedFoldersRecord>, // { folders, accepted_at }
```

- 既存の `next_cwd` は読み込み時に `next_folders` へ写さない（後方互換のため両方読む。`next_folders` が None なら `next_cwd` を従来どおり使う）。
- `environmentId`・Codexの型名は保存しない。保存の空き確認（`precheck_space`）は M44 と同じ。
- fork（thread_ops.rs の設定引継ぎ）では `next_folders` を `next_cwd` と同じ規則で引き継ぐ。`accepted_folders` は引き継がない（別スレッドの受理記録ではないため）。

## (g) 実装タスク分割案

| # | 内容 | 担当 | 依存 |
|---|---|---|---|
| WF-0 | 実測プローブ作成（`workfolders-probe.mjs`）→ M1・M2・M3・M6・M7・M8（turnなし）を実行。M4・M5はユーザー承認後 | implementer＋ユーザー | — |
| WF-1 | 共通型・`rules/work_folders.rs`（正規化・UNC・重複・システム判定・強い確認の純粋ロジック＋単体テスト）・`Capabilities.workspace_roots` | implementer | — |
| WF-2 | Codexアダプター: start/turn への変換、応答・readからの逆変換、拒否→Unsupported、`read_work_folders` | implementer | WF-0（M1・M3・M7）・WF-1 |
| WF-3 | ホスト: 保存項目、`set_next_work_folders`（busy・キュー確認・再検査）、送信時付与、受理後の反映、受理不明の扱い、IPC・ts-rs | implementer | WF-2 |
| WF-4 | 画面: 新規チャットの「フォルダを追加」、作業フォルダ変更ダイアログ（並べ替え・確認・強い確認）、ヘッダー「…」の確認ポップオーバー、プロジェクト一致 | implementer | WF-3 |
| WF-R | WF-2・WF-3 の差分レビュー（受理不明・stickyの取り違え、Steerへの混入、空配列送信） | designer | WF-3 |
| WF-S | 実機確認手順（M4・M5の再確認を含む） | 司令塔 | WF-4 |

WF-1 と WF-0 は並行可。WF-0 の M5 結果で Q2 を確定してから WF-2 の `sandboxPolicy.writableRoots` を決める。

## 要判断（ユーザーへの選択式確認）

- **Q1 送る経路**: (A) `runtimeWorkspaceRoots`＋`cwd` で送り、`environments` は読取りだけ【推奨。IDの取得元が不要、空配列の事故がない】／(B) HANDOFF案どおり `turn/start.environments`（`environmentId` を読み込み済みスレッドから取る。未読込みチャットでは送れない）。
- **Q2 追加フォルダへの書込み**: (A) 権限が「作業フォルダ内で書込み」のとき追加フォルダも書込み可（必要なら `writableRoots` にも入れる。M5で判断）【推奨。CodexのUIの「検索・読取り・編集用」と同じ】／(B) 追加フォルダは読取りのみ（`writableRoots` に入れず、M5で書けてしまうなら機能自体を止める）。
- **Q3 システム系フォルダ**: (A) Windows・Program Files・`~/.codex`・AgentDockのデータ領域は追加不可、ドライブ直下・ホーム直下・AppData直下・ProgramData は強い確認【推奨】／(B) すべて強い確認で許可／(C) すべて追加不可。
- **Q4 未読込みチャットの表示**: (A) 「AgentDockの送信記録（受理済み）」を出し、直接取得とは区別【推奨】／(B) 「未確認」だけ出す。
- **Q5 プロジェクトから新規作成**: (A) プロジェクトのフォルダを全部既定で並べる（外せる）【推奨】／(B) 先頭フォルダだけ（追加は手動）。上限8件の可否もここで（超えるプロジェクトは先頭8件＋案内）。


## 決定（2026-10-11 ユーザー）

Q1=runtimeWorkspaceRoots＋cwd／Q2=権限に従う（M5実測で確定）／Q3=一部追加不可（Windows・Program Files・~/.codex・AgentDockデータ領域は不可、ドライブ直下・ホーム直下は強い確認）／Q4=送信記録を直接取得と区別して表示／Q5=プロジェクトの全フォルダを既定で並べる・上限8件。実装は次のバッチ（WF-0〜）。
