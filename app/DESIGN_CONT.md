# DESIGN_CONT: 継続チャット最小版（G-P）設計書 v0.1

- 作成: 2026-10-18（設計担当）。対象ブランチ: `feature/org-engine`（57b241c 時点）。コードは変更していない。
- 位置づけ: `app/DESIGN_ORG_GRAPH.md` §15 U5=(b)（G-A→**G-P**→G-B縮小版→プロジェクト→窓口チャット）、U6=(a)（引継ぎ資料は決定的に組立て、追加のモデル呼び出しなし）。根拠は `app/REVIEW_FIT_GRAPH.md` 推奨2・§1(b)。
- 表記: 【事実】コード・文書で確認した。【推測】確認していない見立て（実機確認項目へ回す）。【決定】本書で設計担当が決めた（推奨案）。【要判断】§15 に列挙（実装は推奨案で進めてよい）。
- 読んだもの: `REVIEW_FIT_GRAPH.md`（冒頭〜§1）、`DESIGN_ORG_GRAPH.md` §5.1〜5.3・§15、正本 §2.3・§13.3、`store/records.rs`、`store/mod.rs`（関数一覧）、`host/mod.rs`（start_chat〜dispatch_send_as）、`host/thread_ops.rs`（resend_as_fork・fork・compact・照合）、`host/persist.rs`（一般チャットの作業領域）、`rules/compose.rs`（冒頭）、`backend/backend.rs`（TokenUsage）、`backend/local.rs`（ChatLocalView・AppSettings）、`backend/model.rs`（Chat・Capabilities）、`codex/events.rs`（tokenUsage の変換）、`ui/orgHidden.ts`、`ui/CenterPane.tsx`（関数一覧）、schema 0.162.1 の `ThreadTokenUsage.ts`・`ThreadStartParams.ts`（Instructions 欄）・`methods-*.txt`（`thread/compacted`）。
- **読んでいない**（実装担当が着手時に確認）: `host/state.rs` の Activity・TurnEnded 処理の詳細、`host/queue_driver.rs`、`host/manage.rs`（削除・アーカイブの本体）、`host/attachments.rs` の `carry_attachments`、`rules/export.rs`、UI の `ResendDialog.tsx`・`FindBar.tsx`・`LeftPane.tsx`・`Composer` の中身、`codex/convert.rs` の userMessage の本文の連結方法。

---

## 0. 結論（要点）

1. **thread＝チャット（ChatKey・chat.json・キュー・添付…）の対応は変えない**。継続は「thread の列」を chat.json に足して表す。新しい thread は新しいチャット領域を持ち、先頭（今の thread）の chat.json が `conversation.threads[]`（以前の thread の順序付きの列）を持つ。以前の thread は `supersededBy` を持ち、左一覧から隠れる。既存の全機能（送信・照合・キュー・削除）を thread 単位のまま使えるので、改修範囲が小さい。
2. **目的メモ**は会話（conversation）単位の固定文。送信のたびに毎回ではなく、**届ける必要があるとき**（その thread で未送付・版が変わった・圧縮を観測した）に、利用者の手動送信の先頭へ「資料」枠で添える。
3. **使用率**は `thread/tokenUsage/updated` の `last.totalTokens ÷ modelContextWindow`。窓が取れなければ「未取得」。推定しない。最後の値は時刻付きで保存し、再起動後は「前回の応答時点」と明示して出す。
4. **引き継ぎ**は、利用者が「引き継いで送信」を押した送信の中でだけ行う。70% 以上（値が取れたときだけ）で送信ボタンが「引き継いで送信」に変わる案内を出す（利用者が「このthreadで続ける」に戻せる）。取れないときは自動で切り替えず、メニューからの手動だけ。
5. 資料は「目的メモ＋直近 N 往復（既定3）の原文」を純関数で組み立て、長さ上限で決定的に切る。追加のモデル呼び出しなし。古い thread は残す（削除・アーカイブ・resume しない。読むだけ）。
6. 画面は1本の会話に見せる。以前の thread は「以前の会話を表示」で読み込み（読取りのみ）、薄い区切り線でつなぐ。資料の部分は折りたたみ表示。
7. タスクは C1〜C5（＋レビュー R-C、修正 C6）。順序: C1 → (C2 ∥ C4) → C3 → C5 → R-C → C6。

---

## 1. 前提（事実と推測）

### 1.1 事実
- 【事実】チャット領域は ChatKey→dirId の索引で引く（`store/mod.rs` `index: HashMap<ChatKey, LocalId>`）。chat.json（`ChatLocalFile`）は `chat: Option<ChatKey>` を1つ持つ。キュー・受理不明の記録・添付・控え・判断履歴などすべて ChatKey 単位。
- 【事実】`ChatLocalFile` の追加は `#[serde(default)]` で行い `SCHEMA_VERSION` は上げない方針（records.rs のコメント・テスト）。
- 【事実】`BackendEvent::TokenUsage { agent, turn, last_total, thread_total, context_window }` はホストの状態処理で無視されている（`host/state.rs:1107`）。org の観測（`host/org/observe.rs:158`）だけが使う。`context_window` は schema 上 `modelContextWindow: number | null`。
- 【事実】`thread/compacted` 通知と `contextCompaction` item が schema にあり、item は `ActivityKind::ContextCompaction` へ変換済み（`codex/convert.rs:414`）。
- 【事実】`resend_as_fork` が「新しい thread を作る→下書きへ保存→保存できたら `send_message` で1回だけ送る→受理不明・拒否は再送しない」を既に実装しており、同じチャットの二重実行を `ResendInflight` で止めている。引き継ぎはこの型をなぞる。
- 【事実】`start_chat` は cwd の有無で一般／開発を決め、一般チャットは新しいチャット領域の中に `workspace` を作る（`persist.rs:349`）。レビュー用の別チャットは「元の管理領域のパスは使わない」（`thread_ops.rs:182`）。
- 【事実】発話前のチャットは再起動後に再開できない場合がある（`ensure_live` の文言、`no_history`）。
- 【事実】`rules/compose.rs` に、本文中のバッククォート列より長いフェンスで囲む `fence_for`/`fenced` がある（参考指定 #8 用）。
- 【事実】ThreadStartParams には `developerInstructions` があるが TurnStartParams にはない。

### 1.2 推測（実機確認へ）
- 【推測】`last.totalTokens` は直近のモデル呼び出し1回の入力＋出力で、文脈の占有量の近似として使える（Codex の TUI の「残り」と近い値になるはず）。違えば C2 の変換を `last.inputTokens` 等へ差し替える（変換1か所に閉じる）。→ 実機 K1。
- 【推測】Codex は上限付近で自動圧縮するので、使用率は 90% 台に長く留まらない。→ 実機 K2。
- 【推測】`thread/start` だけで発話のない thread は Codex 側に残らないことがある。→ 失敗時の扱い（§6.6）を「空の先頭」前提で設計した。

---

## 2. 用語と不変条件

- **会話（conversation）**: 利用者から見た1本のチャット。1つ以上の thread（＝AgentDock のチャット領域）を順に持つ。ID は `ConversationId`（AgentDock の LocalId。バックエンドの ID ではない）。
- **先頭（head）**: 会話の今の thread。左一覧に出るのは先頭だけ。送信・キュー・圧縮・分岐など既存の操作はすべて先頭に対して行う。
- **以前の thread（segment）**: 先頭より前の thread。読取り専用で表示する。
- **資料（envelope）**: AgentDock が利用者の送信本文の先頭に付ける枠。目的メモと引き継ぎ資料を入れる。

不変条件（実装とテストで守る）:
- I1 AgentDock が自分から送るのは、利用者の手動送信（`send_message`）と「引き継いで送信」（`handoff_and_send`）の中だけ。キューの自動送信・追加指示（steer）・組織実行の送信には資料を付けない。引き継ぎはしない。
- I2 以前の thread を resume しない・送らない・アーカイブしない・削除しない（読取り `read_history` だけ）。
- I3 受理不明の送信は再送しない（既存の照合に任せる）。引き継ぎ資料の再送付は、利用者の次の手動送信の中でだけ起きる。
- I4 使用率が取れないとき、閾値による案内（自動の「引き継いで送信」への切替）をしない。
- I5 保存形式・UI 型に Codex 固有の型を入れない（ChatKey＝バックエンド種別＋内部ID、LocalId、数値、文字列だけ）。
- I6 `~/.codex` を読み書きしない（本機能は AgentDock の領域だけを書く）。

---

## 3. データモデル

### 3.1 chat.json の追加（`store/records.rs` `ChatLocalFile`）【決定】

```rust
// ── 継続チャット（DESIGN_CONT §3。既定値あり。SCHEMA_VERSION は上げない） ──
/// このチャット（thread）が会話の先頭として持つ会話の記録。None＝目的メモも引き継ぎもない普通のチャット。
/// 引き継ぎのとき、新しい先頭へ写して持たせる。以前の thread の側は引き継ぎ時点の写しを残す（変更しない）。
#[serde(default)]
pub conversation: Option<ConversationRecord>,
/// この thread が引き継がれた先。Some で、かつ引き継ぎ先のチャット領域が保存先にあるとき、左一覧から隠す。
#[serde(default)]
pub superseded_by: Option<Superseded>,
/// 直近に観測した文脈の使用量（ルートのエージェントの値だけ）。表示用で、判断の根拠は「いつの値か」と一緒に出す。
#[serde(default)]
pub context_usage: Option<ContextUsageSample>,
```

```rust
/// 会話の記録。将来 AgentDockプロジェクト（G-C）ができたら `purpose` をプロジェクトのゴール・メモへ移し、
/// ここは `PurposeSource::Project` の参照に置き換える（DESIGN_ORG_GRAPH §5.3）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRecord {
    /// 会話のID（最初に記録を作ったときに採番。引き継いでも変えない）。
    pub id: LocalId,
    /// 目的メモ。None＝未設定。
    #[serde(default)]
    pub purpose: Option<PurposeNote>,
    /// 以前の thread の列（古い順）。先頭（このチャット自身）は含めない。
    #[serde(default)]
    pub threads: Vec<ThreadSegment>,
    /// この thread へ引き継いだときの資料。最初の thread では None。
    #[serde(default)]
    pub seed: Option<HandoffSeed>,
    /// 目的メモをこの thread へ届けた記録（受理を確認できた送信に含めた版）。thread が替われば新しい先頭では None から始まる。
    #[serde(default)]
    pub purpose_delivered_rev: Option<u32>,
    /// 届けた後に、この thread で文脈の圧縮を観測したか（真なら次の手動送信で目的メモを再び添える）。
    #[serde(default)]
    pub compacted_since_delivery: bool,
}

/// 目的メモ（利用者が書く固定文）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurposeNote {
    pub text: String,
    /// 保存のたびに1増やす（届けたかの判定と、同時編集の検出に使う）。
    pub revision: u32,
    pub updated_at: UnixMillis,
    /// 由来。最小版は常に Chat。G-C で Project { … } を足す。
    #[serde(default)]
    pub source: PurposeSource,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PurposeSource { #[default] Chat }

/// 以前の thread 1本。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSegment {
    pub chat: ChatKey,
    /// 引き継いだ時刻（この thread が先頭でなくなった時刻）。
    pub handed_off_at: UnixMillis,
    /// 引き継ぎ時点の表示名（一覧から隠れた後の区切り線に出す）。
    #[serde(default)]
    pub title: Option<String>,
}

/// 引き継がれた側に付ける印。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Superseded {
    pub by: ChatKey,
    pub at: UnixMillis,
}

/// 新しい先頭へ入れた引き継ぎ資料。再起動後も同じ資料で送り直せるよう、組み立てた本文をそのまま持つ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffSeed {
    /// 引き継ぎ元。
    pub from: ChatKey,
    /// 組み立てた資料の本文（目的メモは含めない。目的メモは送るときの版を別に入れる）。
    pub material: String,
    pub rounds_included: u32,
    pub truncated: bool,
    pub created_at: UnixMillis,
    pub state: SeedState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SeedState {
    /// まだ受理を確認していない（次の手動送信に資料を付ける）。受理不明もここ（照合か履歴で確定する）。
    Pending,
    /// 資料を含む送信の受理を確認した、または履歴で資料つきの依頼を観測した。
    Delivered,
}

/// 文脈の使用量の観測値。推定値は持たない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsageSample {
    /// 直近のモデル呼び出しのトークン数（`last.totalTokens`）。
    pub used: u64,
    /// 文脈の上限。バックエンドが示さなければ None（割合は出さない）。
    pub window: Option<u64>,
    pub observed_at: UnixMillis,
    pub turn: Option<ExternalId>,
    /// この値の後で文脈の圧縮を観測した（次の値が来るまで割合を出さない）。
    #[serde(default)]
    pub stale_after_compaction: bool,
}
```

- 後方互換: 3項目とも `#[serde(default)]`。旧ファイルは None で読める（records.rs の既存テスト `chat_local_without_stage3_fields_still_reads` と同じ形のテストを足す）。
- `conversation` を先頭だけでなく以前の thread にも写しのまま残すのは、先頭のチャット領域が失われた（削除・破損）ときに以前の thread が自然に先頭へ戻れるようにするため（§6.7）。

### 3.2 設定（`AppSettings`）【決定】

```rust
/// 継続チャット（DESIGN_CONT）。古い settings.json は既定値で読む。
#[serde(default)]
pub continuation: ContinuationSettings,

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuationSettings {
    /// この割合（%）以上の使用率を観測したら「次の送信で引き継ぐ」を案内する。None＝案内しない。既定 Some(70)。範囲 50〜95。
    pub suggest_at_percent: Option<u8>,
    /// 引き継ぎ資料に入れる直近の往復数。既定 3。範囲 1〜10。
    pub rounds: u8,
}
```

### 3.3 能力（`Capabilities`）【決定】

```rust
/// 文脈の使用量（使用トークン数・上限）の通知。Unsupported なら使用率の表示と閾値の案内を出さない（「このAIでは非対応」）。
#[serde(default = "support_unknown")]
pub context_usage: Support,
```
Codex アダプターは `Supported` を宣言する。引き継ぎ自体は「新しいチャットの開始＋送信」だけで成り立つので別の能力は設けない。

### 3.4 UI へ出す型（`backend/local.rs`、ts-rs 生成）【決定】

```rust
/// ChatLocalView に追加。
#[serde(default)] pub conversation: Option<ConversationView>,
#[serde(default)] pub superseded_by: Option<ChatKey>,
#[serde(default)] pub context_usage: ContextUsageView,

pub struct ConversationView {
    pub id: LocalId,
    pub purpose: Option<PurposeNoteView>,      // text・revision・updatedAt
    pub threads: Vec<ThreadSegmentView>,       // chat・handedOffAt・title
    pub seed: Option<SeedView>,                // from・roundsIncluded・truncated・chars・state
    /// 次の手動送信で目的メモを添えるか（`rules::continuity::purpose_due` の結果）。
    pub purpose_due: bool,
}

/// 文脈の使用率の表示。未取得・非対応・前回値を値で区別する。推定はない。
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ContextUsageView {
    /// この接続でまだ値が届いていない（保存値もない）。
    #[default] NotFetched,
    /// バックエンドが能力として示していない。
    Unsupported,
    /// 使用量は届いたが上限が示されない。割合は出さない。
    WindowUnknown { used: u64, observed_at: UnixMillis },
    /// 割合を出せる値。`live=false` は保存値（前回の応答時点、接続後に未更新）。
    Known { used: u64, window: u64, percent: u8, observed_at: UnixMillis, live: bool },
    /// 圧縮を観測した後で、次の値をまだ受けていない。
    AfterCompaction { observed_at: UnixMillis },
}
```
`percent` はホストが `min(used*100/window, 255)` の整数で出す（100 を超えても切り詰めない値を表示側で「100%超」と出す）。

---

## 4. 文脈の使用率

- 取り込み【決定】: `BackendEvent::TokenUsage` のうち `agent` がチャットのルート（`agent_key_of(chat)`）のものだけを使う。子孫の値は使わない。`last_total` が None なら捨てる（0 で代用しない）。`context_window` が None または 0 なら `window=None`。
- 保存: メモリの値を即時に `ContextUsageUpdated` で通知し、chat.json へは `update_local(emit=false, delay=2s)` でまとめて書く（turn 中に何度も来るため）。
- 圧縮の観測: ルートの `ContextCompaction` の item が完了したとき（または `thread/compacted` を正規化したとき）に `stale_after_compaction=true`・`compacted_since_delivery=true`。【未読】`thread/compacted` は `events.rs:69` で一覧にあるが正規化先は未確認。item 経由だけで足りるなら通知は増やさない（C2 で確認）。
- 表示の閾値【決定】: `<suggest_at_percent`＝通常色、`≥suggest_at_percent`＝注意色＋案内（§7.3）、`≥90`＝強い注意色（文言「まもなく Codex が自動で圧縮します」【推測に基づく文言。実機 K2 で確認】）。`live=false` の値では案内を出さない（古い値で判断しない）。

---

## 5. 目的メモ

### 5.1 編集
- 場所: ヘッダーの「…」メニュー「目的メモ…」と、入力欄の下の小さな「目的メモ」表示（設定済みなら先頭1行を出す。押すと編集）。ダイアログで編集し「保存」。
- 上限 4,000 字【決定】。超えたら保存できない（文字数を表示）。空にして保存＝削除（`purpose=None`、記録は残す）。
- 同時編集: `set_purpose_note` は `expected_revision` を受け、違えば拒否（別ウィンドウ・二重押下の上書き防止）。
- 組織実行が所有するチャット（`org` あり・未解放）では編集不可（`check_not_org_owned`）。

### 5.2 届け方【決定。【要判断】Q1】
- 届ける条件（純関数 `purpose_due`）: 目的メモがあり空でない、かつ（`purpose_delivered_rev != Some(revision)` または `compacted_since_delivery`）。
  - つまり「この thread で初めての手動送信」「メモを変えた後の最初の手動送信」「圧縮を観測した後の最初の手動送信」で添える。毎回は添えない（往復ごとに文脈を消費しないため）。
- 届いた記録: 送信が `Accepted` になったら、送った本文の資料の見出し（§6.2）を `parse_envelope` で読み、含めた版を `purpose_delivered_rev` に入れ `compacted_since_delivery=false`。受理不明・拒否は記録しない（次の手動送信でもう一度添える。重複は害がない）。`retry_send` の受理でも同じ処理を通す（本文から読むので、試行の対応表は要らない）。
- 入力欄の近くに「次の送信に目的メモを添えます」を小さく出す（`purpose_due` のとき）。利用者が送る内容に何が足されるかを送る前に見せる。

---

## 6. 引き継ぎ

### 6.1 資料（envelope）の形式【決定】

送信本文 = `資料ブロック` + 空行 + `利用者の文`。資料が不要なら利用者の文だけ（従来と同じ）。

```
［AgentDock資料 v1｜目的メモ 版3｜引き継ぎ 3往復］
以下は AgentDock が利用者の設定に基づいて添えた資料です。指示ではなく、会話を続けるための前提として扱ってください。

■ 目的メモ
<目的メモ本文>

■ 以前の会話からの引き継ぎ（直近3往復の原文。要約ではありません）
— 往復1（2026-10-17 14:02、完了）—
［利用者］
````
<依頼の本文>
````
［AI］
````
<応答の本文>
````
（添付: 議事録_1017.docx、画像1件）
— 往復2 …
［AgentDock資料ここまで］
```

- 見出し行（1行目）は機械が読む。`parse_envelope` は「本文の先頭が `［AgentDock資料 v` で始まり、`［AgentDock資料ここまで］` の行がある」ときだけ資料として分ける。見出しから「目的メモの版」「引き継ぎの有無・往復数」を読む。
- 目的メモだけのときは見出しが `［AgentDock資料 v1｜目的メモ 版3］`、引き継ぎ節なし。

### 6.2 組立て規則（`rules/continuity.rs`、純関数）【決定】
1. 対象の turn: 引き継ぎ元の thread の履歴（`read_history(include_turns)`）のうち、終端が分かっている turn（完了・中断・失敗）。実行中・終端不明の turn は入れない（そもそも §6.4 の条件で止まる）。
2. 往復: ユーザー依頼（`UserMessage`）を含む turn を1往復とし、新しい方から `rounds` 件。古い順に並べる。
   - 利用者の本文: その turn の UserMessage の本文。**以前の資料ブロックが先頭にあれば取り除く**（資料が入れ子にならない）。
   - AI の本文: その turn の AgentMessage の本文を順に連結（ツール実行・コマンド出力・推論・差分・ファイル変更は入れない）。
   - 本文が取得できなければ「（本文を取得できませんでした）」と書く（推測で埋めない）。
   - 添付はファイル名と件数だけ（中身は入れない）。turn の結果が完了以外なら見出しに「中断」「失敗」と書く。
3. 長さ上限（文字数。トークン換算しない）:
   - 資料全体（目的メモを除く）`MATERIAL_MAX_CHARS = 24_000`。1本文 `MESSAGE_MAX_CHARS = 6_000`。
   - 1本文が上限を超えたら、先頭 4,000 字＋「…（中略 N 字）…」＋末尾 1,500 字。
   - 全体が上限を超えたら、古い往復から丸ごと落とす。最新の1往復だけでも超える場合は、その往復の AI 本文→利用者本文の順にさらに短く切る（先頭・末尾を残す同じ方式で、合計が上限に収まるまで）。落とした・切ったら `truncated=true`、見出しに「（一部省略）」。
   - 0 往復（依頼が1つもない）なら引き継ぎ節を「以前の会話に依頼はありません」とする（引き継ぎ自体は可能）。
4. 無害化:
   - 埋め込む全本文（目的メモ・依頼・応答）と、利用者が今回書いた文の中の `［AgentDock資料` を `[AgentDock資料`（全角括弧→半角）に置き換える。利用者の文が資料の見出しに化けない／資料の終端を偽装できない。
   - 依頼・応答の本文は `rules::compose::fence_for` と同じ規則のフェンスで囲む（本文中のコードフェンスで枠が壊れない）。
   - 改行は `\n` に統一し、`\t` `\n` 以外の制御文字を除く。
5. 決定性: 同じ履歴・同じ設定なら同じ文字列（時刻は turn の時刻を使い、現在時刻は入れない）。テストで固定する。

### 6.3 状態の流れ

```
[先頭 P・利用者が「引き継いで送信」]
  └ C0 前提確認（§6.4）→ 不可なら理由を返して何もしない
  └ C1 P の履歴を読む（読取りのみ）→ 失敗なら中止（何も作らない）
  └ C2 資料を組み立てる（純関数）
  └ C3 新しい thread N を開始（P と同じ作業場所・モデル・権限・作業モード）
        失敗（拒否）→ 中止。P は変えない。入力欄の文は残る
        受理不明（応答なし）→ 中止。「新しい thread を作れたか確認できません」。再試行は利用者の操作で
  └ C4 N の chat.json を保存して待つ: hosted・設定の写し・conversation（id・purpose 写し・threads=P.threads+[P]・seed=Pending）・下書き=利用者の文
        保存できない → N は空のまま一覧に出す（ただの新しいチャット）。P は変えない。送らない
  └ C5 P の chat.json を保存して待つ: supersededBy=N。P の下書きを空に
        保存できない → 送らない。N を先頭として出す（修復は §6.7 の起動時照合が行う）
  └ C6 添付を P→N へコピー（resend_as_fork の carry_attachments）。失敗 → 送らず「添付を引き継げませんでした」。下書きは N に残る
  └ C7 N へ send_message（通常の送信経路で1回だけ）。資料は send_message の中で付ける（seed=Pending だから）
        Accepted → seed=Delivered、purpose_delivered_rev 更新、N の下書きを空に
        Rejected → seed は Pending のまま。N の入力欄に文が残り、「再送」か編集して送信（どちらも資料つき）
        AcceptanceUnknown → 既存の照合へ。照合で受理と分かれば Delivered、受理なしと分かれば Pending のまま
```

- 先頭が替わる時点は **C5（P に supersededBy を書いた時点）**。送信の成否は N 上の普通の送信として扱う（送信の失敗で P に戻さない。P へ戻すと「送ったか分からない資料」を二重に持つため）。
- 送信は `send_message` を使うので、送信ロック・送信前記録・受理不明の照合・再起動後の照合がそのまま効く（I3）。

### 6.4 前提条件（`handoff_blocker`、純関数＋ホストの材料集め）【決定】
引き継ぎ元 P について、次のどれかなら止めて理由を返す（UI はボタンを無効にし理由を出す）:
- 未接続。P が先頭でない（`superseded_by` あり。古い画面からの二重押下）。
- 組織実行が所有（未解放）。削除中・削除保留。worktree に結び付いている（`worktree` あり。**最小版の対象外**【決定】）。
- ルートが実行中・入力待ち（承認・質問を含む）・初期化中、または `running_turn` あり。停止未確認の記録あり。
- 受理不明の送信・作成直後の最初の送信が未確定・結果が未確認の操作（`pending_ops`）あり。作業フォルダを書き換える操作の実行中。
- キューに送信待ちの項目がある（「取消すか、送り終えてから引き継いでください」）。
- 子孫に実行中・入力待ちがある（承認待ちを含む）。
- ルートの状態が不明で、待機の確認が取れていない（`unknown_confirmed_idle` でない）。外部で実行中の可能性（`external_send_locked`）。
- 次は止めずに注記する: 子孫の状態が不明（live でも終端確認済みでもない）→「子 N 件の状態は未確認のまま引き継ぎます」（2026-10-12 のキュー合意と同じ考え方）。

### 6.5 二重実行の防止【決定】
- ホスト: 引き継ぎ元 P ごとの印（`ResendInflight` と同じ形の `HandoffInflight`）。さらに処理の全体で **P の送信ロック**（`queue_rt.send_lock(P)`）を保持し、P への手動送信・キュー送信と交わらないようにする。N への送信は N のロックなので、ロックの順序は常に P→N（逆順はない）でデッドロックしない。
- 古い画面からの再実行: C0 で `superseded_by` を見て拒否。
- UI: 押下後は結果が出るまで入力欄とボタンを無効にする。

### 6.6 失敗・切断・再起動
- C3 の前に切断 → C0 で止まる。C3 の応答待ちで切断 → 受理不明として中止（N が Codex 側にできたかもしれないが、AgentDock は記録しない。【推測】発話のない thread は残らないことがある。残っても空の会話で害はない）。
- C4〜C6 の途中でアプリが落ちた → 起動時の照合（§6.7）で形を戻す。送信はしていない。
- C7 の受理不明 → 既存の照合（再起動後も継続）。
- **空の先頭の再試行**: N の seed が Pending で、N が「Codex に履歴がない（`no_history`）」ため送れない場合、N で「引き継ぎをやり直す」を出す。やり直しは N を引き継ぎ元として C3〜C7 を行い、資料は **N の seed.material をそのまま使う**（P を読み直さない）。新しい N2 の `threads` は N の `threads` と同じ（空の N は列に入れない）。N には `supersededBy=N2` を付けて隠す（削除しない）。

### 6.7 起動時の照合（読込み後、純関数 `resolve_chains`）【決定】
入力は読み込んだ全 chat.json の `(ChatKey, conversation, superseded_by)`。出力は「隠すチャット」と「修復の書込み」。
- 隠す: `superseded_by.by` のチャット領域が保存先にある（読み込めた）チャット。引き継ぎ先が無い・読めない場合は隠さない（以前の thread が先頭として戻る。会話の記録は引き継ぎ時点の写し）。
- 修復: 先頭 H の `threads` に P があり、P の `superseded_by` が None → P に `supersededBy=H` を書く（C5 の前に落ちたとき）。
- 競合: 2つの先頭が同じ P を列に持つ → P の `superseded_by` が指す方を正とし、もう一方はそのまま別の会話として出す（自動で統合しない）。警告を1回出す。
- 修復の書込みは保存の直列化に乗せる。読めないファイル（破損・新しい版）は既存の扱い（触らない）。

---

## 7. 画面

### 7.1 目的メモ
- ヘッダー「…」→「目的メモ…」。入力欄の下の補助行に「目的メモ: <先頭1行>」（未設定なら出さない。「…」から設定）。
- 編集ダイアログ: 複数行入力、文字数（上限 4,000）、「この会話の送信に資料として添えます（この thread で最初の送信・変更後・圧縮後）」の説明、保存／取消。Esc で閉じる（IME 変換中は閉じない、既存の合意）。

### 7.2 使用率
- 位置: 入力欄の下の補助行（モデルの表示の隣）に小さなチップ「文脈 62%」。ヘッダーには出さない（ヘッダー1行の方針）。
- 表示: `Known live`＝「文脈 62%」／`Known 保存値`＝「文脈 62%（前回）」淡色／`WindowUnknown`＝「文脈 未取得」／`NotFetched`＝「文脈 未取得」／`AfterCompaction`＝「文脈 圧縮後・未取得」／`Unsupported`＝「文脈 非対応」。ツールチップに「使用 12,345 / 上限 200,000 トークン、14:02 の応答時点」。
- 色: 閾値未満＝通常、閾値以上＝注意、90% 以上＝強い注意。

### 7.3 引き継ぎの案内と確認
- 閾値以上（`Known live` のときだけ）: 入力欄の上に1行の案内「文脈が 72% です。次の送信で新しい thread へ引き継ぎます（目的メモ＋直近3往復）［資料を見る］［このthreadで続ける］」。送信ボタンの文言が「引き継いで送信」になる。
  - 「このthreadで続ける」を押すと、その thread では閾値の案内を出さない（メモリだけ。再起動や次の値の 90% 到達で再び出す【決定】）。
- 手動: ヘッダー「…」→「新しい thread へ引き継ぐ…」で同じ案内を出す（使用率が未取得でも可）。案内の［×］で解除。
- 送信キー（Enter／Ctrl+Enter）も「引き継いで送信」として扱う（案内が見えている状態で押すので、別の確認ダイアログは出さない【決定】）。
- 「資料を見る」: `preview_handoff` の結果（資料の本文・文字数・往復数・省略の有無・止める理由・注記）を読み取り専用で表示。
- 止める理由があるときは案内に理由を出し、送信ボタンは通常の「送信」に戻す（引き継がずに送るか、待つかは利用者が選ぶ）。
- 引き継ぎ後: 画面は N（新しい先頭）を開いたまま、区切り線の下に送信中の依頼が出る。結果の文言は既存の送信の表示に任せる。

### 7.4 つながった表示
- 先頭の会話の上端に「以前の会話（thread 2本）［表示］」。押すと直前の thread から1本ずつ `read_segment_history` で読み、上へ足す（読取りのみ。resume しない）。読めなければ「この thread の履歴を読めませんでした（再試行）」。
- thread の境目に薄い区切り線「── ここから新しい thread（2026-10-18 10:02 に引き継ぎ）──」。
- 以前の thread の発言は読取り専用: 操作は「コピー」と「選択範囲を貼り付け」だけ（編集して再送・再生成・分岐・確認済み等は出さない）。
- 資料つきの依頼: 依頼の吹き出しは利用者の文だけを出し、上に折りたたみの行「資料: 目的メモ（版3）・引き継ぎ 3往復（12,345字）▸」。開くと資料を等幅で表示（Markdown 整形しない）。
- 判定は UI 側の純関数 `splitEnvelope(text)`（ホストの `parse_envelope` と同じ規則。両方にテストを置く）。

### 7.5 検索・コピー・エクスポートとの整合【決定】
- 会話内検索（Ctrl+F）は表示中の DOM が対象（既存）。折りたたんだ資料・未読込みの以前の thread は対象外。検索バーに「以前の会話は［表示］で読み込むと検索できます」を、未読込みの thread があるときだけ出す。
- コピー: 依頼の「コピー」は利用者の文だけ。資料は開いた枠の「コピー」で別に。
- Markdown エクスポート: 先頭の thread だけを出し（既存どおり）、冒頭に「以前の thread が N 本あります（AgentDock の表示上の会話）」、資料つきの依頼は利用者の文＋「（資料: 目的メモ版3・引き継ぎ3往復を添付）」の1行。【未読】`rules/export.rs` の形に合わせる。
- 編集して再送（ResendDialog）: 元の依頼の初期値は利用者の文だけ（資料を除く）。

---

## 8. IPC（`backend/ipc.rs`・`commands.rs`・`ipc/client.ts`・`types.ts` 再export）

| コマンド | 引数 | 結果 | 備考 |
|---|---|---|---|
| `set_purpose_note` | `{ chat, text, expectedRevision: number \| null }` | `ConversationView` | 4,000字超・org 所有・版の不一致は拒否。空文字＝削除 |
| `preview_handoff` | `ChatArgs` | `HandoffPreview` | 読取りのみ。止める理由があっても資料は返す（読めた範囲） |
| `handoff_and_send` | `{ chat, text, attachments: LocalId[] }` | `HandoffResult` | §6.3。文が空で添付もなければ拒否 |
| `read_segment_history` | `{ head, segment }` | `AgentHistory` | `segment` が `head` の threads にあるときだけ。読取りのみ、状態へ反映しない |

```rust
pub struct HandoffPreview {
    pub blockers: Vec<HandoffBlock>,      // { kind, message }
    pub notes: Vec<String>,               // 止めない注記（子の状態未確認など）
    pub envelope: String,                 // 送る資料ブロック（目的メモ込み。利用者の文は含めない）
    pub chars: u32,
    pub rounds_included: u32,
    pub rounds_available: u32,
    pub truncated: bool,
}

#[serde(tag = "stage", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum HandoffResult {
    /// 新しい thread を作らずに止まった（前提・読取り・開始の拒否／受理不明）。P は変わっていない。
    NotStarted { message: String, start_unknown: bool },
    /// 新しい thread は先頭になったが、送っていない（保存・添付の失敗。下書きは N にある）。
    CreatedNotSent { chat: Chat, message: String },
    /// 送信した（結果は send の状態。受理不明は既存の照合へ）。
    Sent { chat: Chat, send: SendAttempt },
}
```
- 通知: `HostEvent::ContextUsageUpdated { chat, usage: ContextUsageView }`。会話の記録・`superseded_by` の変化は既存の `ChatLocalUpdated` で運ぶ。
- 送信の資料付けは `send_message` の中で行う（新しい IPC は要らない）。対象は `SendIntent::NewTurn` かつ `steer_entry` なし。

---

## 9. 既存機能との相互作用

| 機能 | 扱い【決定】 |
|---|---|
| 圧縮（#7、手動） | 先頭に対してだけ。受理・観測で `compacted_since_delivery=true`、使用率は `AfterCompaction`。以前の thread には出さない |
| Codex の自動圧縮 | item（ContextCompaction）を観測したら上と同じ |
| 分岐・編集して再送（fork） | 先頭からの分岐は別の会話。分岐先へ目的メモを写す（新しい `ConversationRecord`、id 新規、threads 空、seed なし、delivered なし）→分岐先の最初の手動送信で目的メモが添わる |
| 再送（retry_send） | 拒否された送信の本文（資料込み）をそのまま送る。受理で `parse_envelope` により届いた記録を更新 |
| キュー（§3.7） | キューの自動送信に資料を付けない（I1）。目的メモが届けるべき状態なら、キューの入力欄に「目的メモは次の手動送信で添えます」。キューに項目があるうちは引き継ぎ不可 |
| 追加指示（steer） | 資料を付けない。引き継ぎ不可（実行中だから） |
| ピン | 引き継ぎで N へ写す。以前の thread のピンは隠れている間は意味を持たない（値は変えない） |
| アーカイブ | 先頭だけが対象（既存の動作）。以前の thread は変えない（I2） |
| 削除 | 先頭だけが対象。削除が完了すると引き継ぎ先が無くなるので、直前の thread が会話の先頭として一覧に戻る（§6.7。会話の記録は引き継ぎ時点の写し）。削除の確認ダイアログに「以前の thread が N 本あります。削除するのは今の thread だけで、直前の thread が一覧に戻ります」と出す |
| 一般チャットの作業領域 | N は **P と同じ作業領域のパスを作業フォルダとして使う**（ファイルの続きが見えるように）。種類は一般のまま。N のチャット領域に作業領域は作らない。N を削除しても P の作業領域は消えない（N の領域だけ消す）。【未読】一般／開発の種類の判定の仕方と、作業領域の削除の範囲を C3 で確認し、P の領域を消す経路がないことをテストで固定する |
| 開発チャットの作業フォルダ | P の作業フォルダの並び（受理済みの記録、なければ cwd）で N を開始。強い確認が要るフォルダを含むときは引き継げない（理由を出す。最小版で確認画面は足さない） |
| モデル・権限・作業モード | `ForkInherit` と同じ規則で P→N へ写す（元より広い権限で走らせない） |
| 名前 | N へ P の表示名を付ける（`rename_best_effort`）。会話の名前変更は先頭だけ |
| 監視・ドック・通知 | 以前の thread の監視は続く（既存のまま）。通知の名前に「（以前のthread）」を付ける。左一覧から隠れていても通知・ドックからは開ける（開くと読取り専用の表示ではなく通常のチャット画面。送信は止めない：外部 CLI から続いた場合を含め、利用者の判断） |
| 組織実行 | 役割チャットでは目的メモ・引き継ぎとも不可 |
| side 相談・レビュー用チャット | 先頭に対しては普段どおり。以前の thread からは開かない |
| 変更の確認（Git 基準の控え） | thread ごとのまま。以前の thread の控えは以前の thread の画面で見る（最小版でつなげない） |

---

## 10. エッジケース

| 場面 | 扱い |
|---|---|
| 使用量が届かない・上限が null | 「未取得」。閾値の案内は出さない。手動の引き継ぎは可 |
| 再起動直後 | 保存値を「（前回）」で出す。案内は出さない（live の値が来てから） |
| モデル変更で上限が変わる | 次の値で更新（保存値は当時の上限のまま表示） |
| 実行中・承認待ち・質問待ち | 引き継ぎ不可（理由表示）。案内は出たまま、送信は追加指示として扱われる既存の動作に従う（資料は付けない） |
| 子孫が動いている | 不可。子孫の状態が不明なだけなら注記して可 |
| 切断中 | 不可（「接続後に引き継げます」）。使用率は保存値（前回）表示 |
| 引き継ぎ中に切断 | §6.6 |
| 受理不明の後に再起動 | 既存の照合が N で続く。seed は Pending のまま。照合で受理なら Delivered |
| 目的メモの編集中に送信 | 送信は保存済みの版で資料を作る（編集中の未保存文は使わない） |
| 目的メモを空にした | 以後は添えない。過去の送信の資料は残る（履歴は変えない） |
| 以前の thread が Codex から消えた | 表示で「読めません」。会話の列は変えない |
| 利用者が資料の見出しと同じ文字を入力 | 無害化で `[AgentDock資料` に変わる（§6.2-4） |
| 外部（CLI）で以前の thread に続きが書かれた | 隠れたまま。通知・ドックから開ける。列は変えない |
| 2つの画面で同時に押す | §6.5（ホストの印と送信ロック、`superseded_by` の確認） |

---

## 11. 受入条件（全体）

- A1 旧 chat.json（追加項目なし）と旧 settings.json が読める。追加項目の往復（保存→読込み）が一致する。
- A2 使用率は、ルートの値で上限が取れるときだけ割合を出す。上限 null・未着・非対応・圧縮後・保存値をそれぞれ別の表示にする。0 や推定値で埋めない。
- A3 目的メモは「この thread で最初の手動送信」「変更後」「圧縮観測後」の手動送信にだけ添わる。キュー送信・追加指示には添わらない。受理を確認した送信だけで「届いた」になる。
- A4 「引き継いで送信」で、P を resume・送信・アーカイブ・削除せずに、新しい thread N が作られ、N に資料＋利用者の文が1回だけ送られる。拒否・受理不明で自動再送しない。
- A5 資料は同じ入力で同じ文字列になり、上限（24,000 字・1本文 6,000 字）を超えない。入れ子の資料・見出しの偽装が起きない。
- A6 §6.4 の条件のどれかで引き継ぎが止まり、理由が出る。子孫の状態不明だけなら注記して進む。
- A7 二重押下・二つの画面から押しても N は1つだけ（2本目は「すでに引き継ぎ済み」または「実行中」で止まる）。
- A8 途中で落ちても、再起動後に P と N が両方先頭として出ることはない（修復）。N の領域が無くなれば P が一覧に戻る。
- A9 画面で1本の会話として読め、区切り線が出る。以前の thread は読取りのみ。資料は折りたたまれ、コピー・検索・エクスポート・編集して再送は利用者の文を基準にする。
- A10 `~/.codex` を読み書きしない。保存形式・UI 型に Codex 固有の型がない。

---

## 12. タスク分解（実装担当向け）

共通の確認コマンド（PowerShell ではなく、実装担当の Bash で）:
- `cd app/src-tauri && cargo check`
- `cd app/src-tauri && cargo test --lib <変更したモジュール>`（例 `rules::continuity`、`store::records`）
- ts-rs の再生成: `cd app/src-tauri && cargo test --lib export_bindings`（新しい型は `app/src/ipc/types.ts` に再exportを手で足す）
- `cd app && npx tsc --noEmit`
- 全テストは流さない。画面のテストは書かない。

各タスクの指示に入れる正本の抜粋（共通）:
> 正本 §2.3: 「UI・保存・監視・キュー・通知・停止照合はバックエンド共通の正規化モデルで扱い、thread・turn等のCodex固有の型をUIや保存形式へ直接持ち込まない」「各バックエンドは能力一覧を宣言し、UIは非対応の操作を隠すか『このAIでは非対応』と表示する。非対応を成功表示しない」
> 正本 §13.3: 「切断・再起動・受理不明で無条件に再送しない。監視目的で resume しない。監視のためにプロンプト追加・承認代行・停止をしない」「通信断・取得不能を done・failed に変換しない。未確認の操作を成功表示しない」
> 本書 §2 の不変条件 I1〜I6。

### C1 データモデルと純粋ロジック（implementer）— 依存なし
- 対象: `store/records.rs`（§3.1 の型と3項目、`ChatLocalFile::new`）、`backend/local.rs`（§3.2 設定、§3.4 View 型）、`backend/model.rs`（§3.3 能力）、Codex アダプターの能力宣言（`context_usage: Supported`）、新規 `rules/continuity.rs`（`rules/mod.rs` へ登録）。
- `rules/continuity.rs` の純関数: `build_material(turns, rounds) -> Material`、`build_envelope(purpose: Option<&PurposeNote>, seed: Option<&HandoffSeed>) -> Option<String>`、`compose_send_text(envelope, user_text) -> String`（無害化込み）、`parse_envelope(text) -> Option<ParsedEnvelope { purpose_rev, handoff_rounds, body_range, user_text }>`、`strip_envelope(text) -> &str`、`sanitize(text)`、`purpose_due(rec) -> bool`、`usage_view(sample, cap, live) -> ContextUsageView`、`handoff_blocker(ctx: HandoffCtx) -> Result<Vec<String> /*notes*/, HandoffBlock>`、`resolve_chains(entries) -> ChainResolution { hidden, repairs, conflicts }`。
- 受入: A1・A5 の単体テスト（長さ上限の各分岐、0往復、入れ子の除去、見出しの偽装、フェンス、決定性）、`purpose_due` の4条件、`usage_view` の全分岐（null・0・100%超・圧縮後・保存値）、`handoff_blocker` の各条件と「子孫不明は注記」、`resolve_chains` の修復・競合・引き継ぎ先なし。ts-rs 生成と `types.ts` 再export、`tsc --noEmit` 通過。

### C2 ホスト: 使用率と目的メモ（implementer）— 依存 C1
- 対象: `host/state.rs`（`TokenUsage` を無視している箇所 1107 行付近でルートの値を取り込む。ContextCompaction の観測）、`host/persist.rs`（`local_view` へ View を足す）、`host/mod.rs` の `send_message`（資料の付加と、受理時の届いた記録）、`retry_send`・照合での受理（`parse_envelope` による記録）、新規 `host/continuity.rs`（`set_purpose_note`）、`commands.rs`・`lib.rs`・`ipc.rs`・`ipc/client.ts`。
- 受入: A2・A3。`send_message` で資料を付けるのは NewTurn・steer_entry なし・利用者の送信だけ（キュー・steer・org は通らない）ことをコードとテスト（純関数側）で確認。`HostEvent::ContextUsageUpdated` の通知。chat.json への書込みは 2 秒まとめ。`cargo check`・`cargo test --lib rules::continuity`・`tsc --noEmit`。
- 注意: `thread/compacted` の正規化の有無を確認し、無ければ item の観測だけで足りるか判断して報告（新しい BackendEvent を足すなら能力・テストも）。

### C3 ホスト: 引き継ぎ・つながった履歴・起動時の照合（implementer）— 依存 C2
- 対象: `host/continuity.rs`（`preview_handoff`・`handoff_and_send`・`read_segment_history`・`HandoffInflight`）、`host/thread_ops.rs` の `carry_attachments`・`ForkInherit`・`rename_best_effort` の再利用、`host/persist.rs`（一般チャットの作業領域の引継ぎ）、`host/lifecycle.rs` か読込み処理（`resolve_chains` の適用と修復書込み）、`host/manage.rs`（削除確認の文言用に以前の thread の本数を返す。削除の挙動は変えない）、分岐時の目的メモの写し（`adopt_forked`）。
- 手順は §6.3 の C0〜C7 どおり。保存は `save_now` で書き終えるのを待ってから次へ進む（resend_as_fork と同じ）。P の送信ロック → N の送信の順。
- 受入: A4・A6・A7・A8・A10。P に対して `resume`・`send`・`manage_chat` を呼ばない（コードで確認）。一般チャットで N を削除しても P の作業領域が消えないこと（テスト）。空の先頭のやり直し（§6.6）。`cargo check`・関係モジュールのテスト・`tsc --noEmit`。

### C4 UI: 目的メモ・使用率・引き継ぎの案内（implementer）— 依存 C2（C3 の IPC は型だけ先に使ってよい）
- 対象: `ui/CenterPane.tsx`（`Composer` の補助行・案内行・送信ボタンの文言・送信キー）、ヘッダーの「…」メニュー（`menuModel.ts`・`commands.ts`）、新規 `ui/ContinuityDialogs.tsx`（目的メモの編集、資料のプレビュー）、設定画面（`PrefsDialogs.tsx` に「継続チャット」：案内の閾値・往復数）、`ipc/live.ts` の `ContextUsageUpdated` 取り込み。
- 受入: §7.1〜7.3。未取得・非対応・前回値・圧縮後の表示を区別。閾値の案内は `Known live` のときだけ。「このthreadで続ける」で解除。止める理由の表示。Esc・IME の既存規則。ダーク・狭い画面の崩れなし。`tsc --noEmit`。

### C5 UI: つながった表示と既存機能の整合（implementer）— 依存 C3・C4
- 対象: `ui/CenterPane.tsx`（`Messages`・`UserBubble`：以前の thread の読込み、区切り線、読取り専用、資料の折りたたみ）、新規 `ui/continuity.ts`（`splitEnvelope`・`isHiddenPredecessor` の純関数＋テスト）、`ui/LeftPane.tsx`（以前の thread を隠す。`orgHidden.ts` と同じ置き方）、`ui/FindBar.tsx`（未読込みの注記）、コピー、`ResendDialog.tsx`（初期値から資料を除く）、`rules/export.rs`（§7.5）、削除確認ダイアログの文言。
- 受入: A9。`splitEnvelope` とホストの `parse_envelope` に同じテストケースを置く。`tsc --noEmit`、`cargo test --lib rules::export`（変更したなら）。

### R-C レビュー（designer）— 依存 C3（C5 と並行可）
- C2・C3 の差分。観点: 自動送信・自動 resume がないか（I1・I2）、受理不明の扱い、ロック順、保存の書込み順と修復、`send_message` の資料付けの範囲。結果は `app/REVIEW_CONT.md`。

### C6 R-C の指摘の修正（implementer）— 依存 R-C

順序: C1 → (C2 ∥ C4) → C3 → C5 ∥ R-C → C6 → 実機確認（ユーザー）。C4 は C2 の IPC が揃うまでモック値で画面を作ってよい（`app/src/mock/` の動的 import の規則に従う）。

---

## 13. 実機確認（ユーザー）

| ID | 手順 | 期待 |
|---|---|---|
| K1 | 一般チャットで数往復し、入力欄下の「文脈 %」と Codex（VS Code 拡張か CLI の `/status`）の残量を比べる | おおむね一致（大きく違えば C2 の変換を見直す） |
| K2 | 長い資料を貼って往復を重ね、70%・90% の表示と、Codex の自動圧縮の発生、圧縮後の表示を見る | 70% で案内、圧縮後は「圧縮後・未取得」→次の応答で下がった値 |
| K3 | 目的メモを書いて送信→次の送信→メモを変えて送信 | 1回目と3回目だけ資料が付く（依頼の上に「資料」行） |
| K4 | 「引き継いで送信」 | 新しい thread で応答が返る。上に区切り線。左一覧は1行のまま。前の会話を［表示］で読める |
| K5 | 引き継ぎの直後に応答の内容を確認 | 目的メモの様式と直近の話題を踏まえた返事になる（品質の確認。悪ければ往復数・上限を調整） |
| K6 | 一般チャットで AI にファイルを作らせてから引き継ぎ、続けてそのファイルを編集させる | 同じ作業領域のファイルが見える |
| K7 | 承認待ち・実行中・キューあり・子エージェント作業中で引き継ぎを試す | 止まり理由が出る |
| K8 | 引き継ぎの送信中に Codex を落とす（タスクマネージャ等）→再接続・再起動 | 自動で再送しない。照合後に状態が確定する。一覧に同じ会話が2行出ない |
| K9 | 先頭の thread を削除 | 直前の thread が一覧に戻る |
| K10 | Ctrl+F・コピー・エクスポート・編集して再送 | 利用者の文が基準。資料は開いたときだけ |

---

## 14. 規模とリスク

- 規模: C1 中、C2 中、C3 大（引き継ぎ手順と照合）、C4 中、C5 中。
- リスク: (1) 一般チャットの作業領域の共有（C3。削除の範囲を誤るとファイルを消す。テスト必須）。(2) `last.totalTokens` の意味（K1）。(3) 資料つき依頼の Codex 側の扱い（長い依頼の上限、表示）【推測】。(4) 以前の thread を隠したときの通知・ドックからの到達。

---

## 15. 要判断（重大なものだけ。実装は推奨案で進める）

- **Q1 目的メモを添える頻度**（§5.2）
  - (a) 推奨: この thread で最初の手動送信・変更後・圧縮観測後だけ。往復ごとの文脈の消費を抑える。圧縮で薄れた後は自動的に再送付される。
  - (b) 手動送信のたび毎回。確実に効くが、4,000 字のメモなら往復ごとに最大約 2〜4 千トークンを使い、引き継ぎが早まる。
  - (c) Codex の `developerInstructions`（thread 開始・再開時の指示）として渡す。毎回の本文に出ないが、Codex 固有の経路で、途中の変更には resume が要り（送信目的の resume は既存経路にあるが）、反映の確認方法が未実測。最小版では採らない。
- **Q2 閾値で送信ボタンを「引き継いで送信」に切り替えること**（§7.3）
  - (a) 推奨: 70% 以上（live の値のときだけ）で案内を出し既定で切り替える。利用者は送信前に案内を見て「このthreadで続ける」で戻せる（§5.2 の「次の送信で引き継ぎます」に合わせた）。
  - (b) 案内だけ出し、切り替えは利用者が「引き継ぐ」を押したときだけ。より保守的だが「意識しない」体験から遠い。
- **Q3 worktree に結び付いたチャットを最小版の対象外にすること**（§6.4）
  - (a) 推奨: 対象外（理由を表示）。worktree 台帳がチャット1つに結び付く前提のため、付け替えの設計が要る。業務チャット（主目的）には影響しない。
  - (b) 対象にする（台帳の結び付けを N へ付け替える。C3 が大きくなる）。
