// 仮データ。ipc/types.ts の型に適合させる。T5で実IPCに差し替えるまでの表示確認用。
import type {
  Agent, AgentKey, AgentState, AgentStatus, AgentView, Attachment, Capabilities, Chat, ChatKey,
  ChatModelSettings, Evidence, EvidenceSource, Freshness, HostSnapshot, Known, ModelInfo,
  PendingRequest, QueueItem, SaveState, SourceInfo, StopRecord, TurnRecord, WaitInfo,
} from "../ipc/types";

const NOW = Date.now();
const ago = (s: number) => NOW - s * 1000;

const ck = (id: string): ChatKey => ({ backend: "codex", id });
const ak = (id: string): AgentKey => ({ backend: "codex", id });
const val = <T,>(value: T): Known<T> => ({ kind: "value", value, basis: "direct" });
const nf = <T,>(): Known<T> => ({ kind: "notFetched" });
const ev = (source: EvidenceSource, sAgo: number, rawLabel: string | null = null): Evidence =>
  ({ source, rawLabel, sourceTime: ago(sAgo), observedAt: ago(sAgo) });

export const SOURCE = "src-1";

const caps: Capabilities = {
  descendantMonitoring: "supported", descendantSearch: "experimental", historyReadWithoutResume: "supported",
  resume: "supported", listLoaded: "supported", approvalKinds: [{ kind: "commandApproval" }, { kind: "fileChangeApproval" }, { kind: "userInput" }],
  steer: "supported", interrupt: "supported", modelSelection: "supported", effortSelection: "supported",
  rename: "supported", archive: "supported", delete: "unknown", externalHistory: "supported",
  attachmentKinds: ["image", "file"], managedExecControl: "experimental",
};

const source = (connection: SourceInfo["connection"], version?: SourceInfo["version"]): SourceInfo => ({
  source: SOURCE, backend: "codex", pid: val(12840), startedAt: ago(3600),
  version: version ?? { kind: "match", version: "0.160.0" }, capabilities: caps, connection,
});

function chat(id: string, name: string, over: Partial<Chat> = {}): Chat {
  return {
    key: ck(id), kind: "development", cwd: val(`C:\\Users\\wmasa\\Documents\\Rust\\${name}`), name: val(name), preview: nf(),
    pinned: false, archived: val(false), origin: "appManaged", draft: null, createdAt: val(ago(86400)), lastUsedAt: ago(60), ...over,
  };
}

function status(state: AgentState, label: string, sAgo: number, over: Partial<AgentStatus> = {}): AgentStatus {
  return { state, raw: { label }, scope: "agent", turn: null, wait: null, evidence: ev("liveEvent", sAgo), ...over };
}

function agent(id: string, chatId: string, parent: string | null | "?", name: string | null, role: string | null, task: string | null): Agent {
  return {
    key: ak(id), chat: ck(chatId),
    parent: parent === null ? { kind: "root" } : parent === "?" ? { kind: "unknown" } : { kind: "explicit", parent: ak(parent) },
    forkedFrom: val(null), displayName: name ? val(name) : nf(), role: role ? val(role) : nf(), assignment: task ? val(task) : nf(),
    latestTurn: "t1",
  };
}

function view(a: Agent, s: AgentStatus, act: string | null, freshness: Freshness = "live"): AgentView {
  return {
    agent: a, status: s, freshness,
    currentActivity: act === null ? null : {
      key: { agent: a.key, turnId: "t1", itemId: `i-${a.key.id}` }, kind: { kind: "agentMessage" }, phase: "inProgress",
      summary: val(act), evidence: s.evidence,
    },
  };
}

const waitApproval = (id: string): WaitInfo => ({
  reason: { kind: "approval", request: { backend: "codex", source: SOURCE, requestId: id } }, started: ev("liveEvent", 30),
});

export interface Bundle {
  snapshot: HostSnapshot;
  /** key: ChatKey.id */
  turns: Record<string, TurnRecord[]>;
  attachments: Attachment[];
  modelSettings: Record<string, ChatModelSettings>;
  save: SaveState | null;
  /** 一覧外で作られた会話のラベル（origin=external時の表示用） */
  externalLabel: Record<string, string>;
}

function turns(chatId: string, entries: Array<[string, string]>, end: TurnRecord["end"] = null): TurnRecord[] {
  return [{
    key: { agent: ak(`${chatId}0`), turnId: "t1" }, end, startedAt: val(ago(1800)), completedAt: end ? val(ago(100)) : nf(),
    complete: end !== null,
    entries: entries.map(([k, t], i) => ({
      key: { agent: ak(`${chatId}0`), turnId: "t1", itemId: `m${i}` },
      kind: k === "u" ? { kind: "userMessage" } : k === "a" ? { kind: "agentMessage" } : { kind: "other", raw: k },
      text: val(t), phase: "completed" as const,
    })),
  }];
}

const settings = (m: string, e: string): ChatModelSettings => ({
  selected: { model: m, effort: e }, accepted: val({ model: m, effort: e }), effective: val({ model: m, effort: e }), applies: "nextTurn",
});

export const MODELS: ModelInfo[] = [
  { id: "gpt-6.1-sol", displayName: "gpt-6.1-sol", description: null, efforts: [{ id: "低", description: null }, { id: "中", description: null }, { id: "高", description: null }], defaultEffort: "中", isDefault: true, hidden: false, inputKinds: ["image", "file"] },
  { id: "gpt-6.1-mini", displayName: "gpt-6.1-mini", description: null, efforts: [{ id: "低", description: null }, { id: "中", description: null }], defaultEffort: "低", isDefault: false, hidden: false, inputKinds: ["file"] },
];

function base(): Bundle {
  const A = (id: string) => `a${id}`;
  const a0 = agent(A("0"), "a", null, "メイン", "main", "不具合調査の統括");
  const a1 = agent(A("1"), "a", A("0"), "Explorer", "explorer", "再送経路の洗い出し");
  const a2 = agent(A("2"), "a", A("0"), "Worker", "worker", "受理確認後に再送するよう修正");
  const a3 = agent(A("3"), "a", A("2"), "Tester", "tester", "二重送信の再現テスト");
  const a4 = agent(A("4"), "a", A("0"), "reviewer-2", null, null);
  const a5 = agent(A("5"), "a", "?", null, null, null);
  const c0 = agent("c0", "c", null, "メイン", "main", "依存更新");
  const b0 = agent("b0", "b", null, "メイン", "main", "文章整理");
  const e0 = agent("e0", "e", null, "メイン", "main", "外部で作成");

  const queue: QueueItem[] = [{
    id: "q1", chat: ck("a"), text: "修正後に、変更点を箇条書きでまとめて", attachments: [], order: 1,
    state: { kind: "waiting" }, attempts: [], settings: null,
  }];

  return {
    snapshot: {
      seq: 1, sources: [source({ kind: "connected" })],
      chats: [
        chat("a", "通信モジュールの不具合調査", { pinned: true }),
        chat("c", "依存ライブラリの更新", { lastUsedAt: ago(120) }),
        chat("b", "要件定義の文章整理", { kind: "general", cwd: { kind: "missing" }, draft: "第3章の言い回しを", lastUsedAt: ago(1800) }),
        chat("d", "週報の下書き", { kind: "general", cwd: { kind: "missing" }, archived: val(true), lastUsedAt: ago(7200) }),
        chat("e", "ビルド設定の見直し", { origin: "external", lastUsedAt: ago(5400) }),
      ],
      agents: [
        view(a0, status("running", "active", 5), "サブエージェントの結果を待っています"),
        view(a1, status("done", "completed", 170, { scope: "turn" }), "再送箇所を3件報告"),
        view(a2, status("running", "active", 10), "src/reconnect.rs を編集中"),
        view(a3, status("running", "active", 17), "cargo test reconnect を準備"),
        view(a4, status("idle", "idle", 180), null),
        view(a5, status("unknown", "notLoaded", 100), null, "needsReconcile"),
        view(c0, status("running", "active", 70), "Cargo.toml を確認"),
        view(b0, status("done", "completed", 2000, { scope: "turn" }), "要件_整理版.md を作成"),
        view(e0, status("unknown", "notLoaded", 5400, { evidence: ev("historyRead", 5400) }), null, "historyOnly"),
      ],
      requests: [], stops: [], queue, monitorScope: { kind: "selectedChat", chat: ck("a") },
    },
    turns: {
      a: turns("a", [
        ["u", "再接続したときに同じ依頼が二重に送られる不具合を調べて。ログも添付する。"],
        ["a", "ログを確認しました。再接続直後に未確認の送信が再送されています。調査をサブエージェントに分けて進めます。\n\nreconnect() -> resend_pending()  // 受理確認前に再送"],
        ["subAgent", "Explorer を生成: 再送経路の洗い出し"],
        ["subAgent", "Worker を生成: 修正案の実装"],
        ["subAgent", "Worker が Tester を生成: 再現テスト"],
      ]),
      c: turns("c", [["u", "依存ライブラリを最新にして、ビルドが通るか確認して。"], ["a", "tokio を 1.x 系の最新に上げます。メジャー更新が必要な crate が1件あります。"]]),
      b: turns("b", [["u", "この要件の文章を読みやすく整えて。"], ["a", "整えた版を作りました。成果物として保存しています。"]], "completed"),
      d: turns("d", [["u", "今週の作業から週報の下書きを作って。"]]),
      e: turns("e", [["u", "release ビルドの設定を見直して。"], ["a", "（CLIで作成された保存履歴）profile.release の設定を提案しました。"]]),
    },
    attachments: [
      { id: "f1", kind: "image", originalPath: "C:\\tmp\\エラー画面.png", copyPath: "x", attachedAt: ago(600), exists: val(true), ownerChat: ck("a") },
      { id: "f2", kind: "file", originalPath: "C:\\logs\\server.log", copyPath: "x", attachedAt: ago(540), exists: val(true), ownerChat: ck("a") },
    ],
    modelSettings: { a: settings("gpt-6.1-sol", "中"), c: settings("gpt-6.1-sol", "低") },
    save: null,
    externalLabel: { e: "CLI" },
  };
}

export const SCENARIOS: Array<[string, string]> = [
  ["normal", "通常（実行中・子孫あり）"],
  ["requests", "承認と質問が別チャットで同時"],
  ["stale", "接続断の後・live未復旧"],
  ["stop", "中断後10秒・停止未確認"],
  ["queue", "キュー保留と受理不明"],
  ["childFail", "子の失敗あり（親は継続）"],
  ["launch", "起動失敗・版の警告"],
  ["storage", "保存失敗"],
  ["empty", "初回起動（チャットなし）"],
];

const T = (id: string) => ({ backend: "codex" as const, source: SOURCE, requestId: id });
const patch = (b: Bundle, id: string, f: (v: AgentView) => void) => {
  const v = b.snapshot.agents.find((x) => x.agent.key.id === id);
  if (v) f(v);
};

export function makeBundle(name: string): Bundle {
  const b = base();
  const s = b.snapshot;
  if (name === "requests") {
    patch(b, "a3", (v) => { v.status = status("waiting", "active[waitingOnApproval]", 30, { wait: waitApproval("r1") }); v.currentActivity = null; });
    patch(b, "c0", (v) => { v.status = status("waiting", "active[waitingOnUserInput]", 20, { wait: { reason: { kind: "userInput", request: T("r2") }, started: ev("liveEvent", 20) } }); });
    const nonDeny = (id: string, label: string, effect: "allow" | "deny", scope: "once" | "session" | "unknown") => ({ id, label, effect, scope, description: null });
    const unk = <X,>(): Known<X> => ({ kind: "notFetched" });
    const r1: PendingRequest = {
      key: T("r1"), chat: ck("a"), agent: ak("a3"), turn: "t1", item: null, kind: { kind: "commandApproval" },
      detail: { summary: "コマンドの実行を許可しますか", reason: unk(), command: val("cargo test reconnect -- --nocapture"), cwd: val("C:\\Users\\wmasa\\Documents\\Rust\\Sample"), files: unk(), extraPermissions: unk(), url: unk(), questions: [] },
      options: [nonDeny("o1", "1回だけ許可", "allow", "once"), nonDeny("o2", "このセッション中は許可", "allow", "session"), nonDeny("o3", "拒否", "deny", "once")],
      state: { kind: "pending" }, receivedAt: ago(30),
    };
    const qopts = ["メジャー更新して修正も行う", "マイナー更新だけにする", "今回は見送る"].map((l, i) => nonDeny(`q${i}`, l, "allow", "unknown"));
    const r2: PendingRequest = {
      key: T("r2"), chat: ck("c"), agent: ak("c0"), turn: "t1", item: null, kind: { kind: "userInput" },
      detail: { summary: "質問に回答してください", reason: unk(), command: unk(), cwd: unk(), files: unk(), extraPermissions: unk(), url: unk(),
        questions: [{ id: "qq", header: null, text: "tokio をメジャー更新すると API 変更が必要です。どうしますか？", options: qopts, allowsFreeText: false, secret: false }] },
      options: qopts, state: { kind: "pending" }, receivedAt: ago(20),
    };
    s.requests = [r1, r2];
  }
  if (name === "stale") {
    s.agents.forEach((v) => { if (v.status.state === "running") v.freshness = "historyOnly"; });
    s.queue[0].state = { kind: "held", reason: { kind: "notLive", freshness: "historyOnly" } };
  }
  if (name === "stop") {
    patch(b, "a0", (v) => { v.status = status("interrupted", "interrupted", 8, { scope: "turn" }); v.currentActivity = null; });
    const rec = (t: number | null, tend: boolean, ex: boolean): StopRecord["targets"][number]["evidence"] =>
      ({ interruptRequestedAt: t === null ? null : ago(t), turnEndConfirmed: tend ? { end: "interrupted", at: ago(8) } : null, managedExecEndedAt: ex ? ago(5) : null, osGoneConfirmedAt: null });
    s.stops = [{
      id: "s1", chat: ck("a"), startedAt: ago(12),
      targets: [
        { target: { kind: "turn", turn: { agent: ak("a0"), turnId: "t1" } }, ownership: "confirmed", evidence: rec(12, true, false), summary: "turnEndConfirmed" },
        { target: { kind: "turn", turn: { agent: ak("a2"), turnId: "t1" } }, ownership: "confirmed", evidence: rec(12, false, false), summary: "unconfirmed" },
        { target: { kind: "managedExec", agent: ak("a3"), execId: "cargo test" }, ownership: "confirmed", evidence: rec(12, false, false), summary: "unconfirmed" },
        { target: { kind: "osProcess", pid: 7788, createdAt: { kind: "notFetched" }, executable: { kind: "notFetched" } }, ownership: "unknown", evidence: rec(null, false, false), summary: "ownershipUnknown" },
      ],
    }];
    s.queue[0].state = { kind: "held", reason: { kind: "stopUnconfirmed" } };
  }
  if (name === "queue") {
    const mk = (id: string, text: string, state: QueueItem["state"], order: number): QueueItem =>
      ({ id, chat: ck("a"), text, attachments: [], order, state, attempts: [], settings: null });
    s.queue = [
      mk("q1", "修正後に、変更点を箇条書きでまとめて", { kind: "held", reason: { kind: "descendantsNotFinished" } }, 1),
      mk("q2", "テストが通ったらコミットメッセージ案も", { kind: "waiting" }, 2),
      mk("q3", "README の該当箇所も直して", { kind: "acceptanceUnknown" }, 3),
    ];
    s.queue[2].attempts = [{ attemptId: "at1", clientMessageId: "cm1", at: ago(40), state: { kind: "acceptanceUnknown", since: ago(40) } }];
  }
  if (name === "childFail") {
    patch(b, "a3", (v) => { v.status = status("failed", "failed", 20, { scope: "turn" }); v.currentActivity = null; });
    s.queue[0].state = { kind: "held", reason: { kind: "other", message: "子孫の失敗でキューを止めています" } };
  }
  if (name === "launch") {
    s.sources = [source({ kind: "launchFailed", reason: "notFound", message: "指定のパスに codex.exe がありません: C:\\Tools\\codex\\codex.exe" }, { kind: "mismatch", expected: "0.160.0", actual: "0.161.0" })];
    s.agents.forEach((v) => {
      v.freshness = "disconnected";
      if (v.status.state === "running" || v.status.state === "waiting") v.status = status("unknown", "disconnected", 10); // 通信断をdone/failedにしない
    });
  }
  if (name === "storage") {
    b.save = { kind: "saveFailed", message: "ディスクへの書込みに失敗しました", savedPart: "会話本文 12件、添付 2件", unsavedPart: "監視活動ログ 2件" };
  }
  if (name === "empty") {
    s.chats = []; s.agents = []; s.queue = []; s.monitorScope = { kind: "selectedChat", chat: null };
    b.turns = {}; b.attachments = []; b.modelSettings = {};
  }
  return b;
}
