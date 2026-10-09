// AgentDock UI デザイン比較モックの生成（A/B/C）。実行: node app/design/gen-mocks.mjs → 同じフォルダに mock-{a,b,c}.html を書く。
import fs from "node:fs";
import { fileURLToPath } from "node:url";
const OUT = fileURLToPath(new URL(".", import.meta.url));
fs.mkdirSync(OUT, { recursive: true });

const I = (n) => `<svg class="i" aria-hidden="true"><use href="#i-${n}"/></svg>`;
const st = (c, t, extra = "") => `<span class="st ${c} ${extra}"><i></i>${t}</span>`;

const SPRITE = String.raw`<svg width="0" height="0" style="position:absolute">
<symbol id="i-list" viewBox="0 0 24 24"><path d="M4 6h16M4 12h16M4 18h16"/></symbol>
<symbol id="i-dock" viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M15 4v16"/></symbol>
<symbol id="i-plus" viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></symbol>
<symbol id="i-refresh" viewBox="0 0 24 24"><path d="M20 12a8 8 0 1 1-2.6-5.9M20 4v5h-5"/></symbol>
<symbol id="i-search" viewBox="0 0 24 24"><circle cx="11" cy="11" r="6"/><path d="M20 20l-4.5-4.5"/></symbol>
<symbol id="i-clip" viewBox="0 0 24 24"><path d="M20 11.5l-8.2 8.2a5 5 0 0 1-7-7l8.2-8.2a3.3 3.3 0 0 1 4.7 4.7l-8.2 8.2a1.6 1.6 0 0 1-2.3-2.3l7.6-7.6"/></symbol>
<symbol id="i-send" viewBox="0 0 24 24"><path d="M12 19V5M6 11l6-6 6 6"/></symbol>
<symbol id="i-stop" viewBox="0 0 24 24"><rect x="6.5" y="6.5" width="11" height="11" rx="1.5"/></symbol>
<symbol id="i-folder" viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></symbol>
<symbol id="i-chat" viewBox="0 0 24 24"><path d="M4 5h16v11H9l-5 4z"/></symbol>
<symbol id="i-check" viewBox="0 0 24 24"><path d="M5 12.5l4.5 4.5L19 7.5"/></symbol>
<symbol id="i-x" viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></symbol>
<symbol id="i-copy" viewBox="0 0 24 24"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/></symbol>
<symbol id="i-branch" viewBox="0 0 24 24"><circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="7" r="2"/><path d="M6 7v10M18 9c0 5-8 3-12 8"/></symbol>
<symbol id="i-alert" viewBox="0 0 24 24"><path d="M12 4l9 16H3z"/><path d="M12 10v4M12 17v.5"/></symbol>
<symbol id="i-info" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7.5v.5"/></symbol>
<symbol id="i-archive" viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="4" rx="1"/><path d="M5 8v11h14V8M10 12h4"/></symbol>
<symbol id="i-lock" viewBox="0 0 24 24"><rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/></symbol>
<symbol id="i-min" viewBox="0 0 24 24"><path d="M7 12h10"/></symbol>
<symbol id="i-max" viewBox="0 0 24 24"><rect x="7" y="7" width="10" height="10" rx="1"/></symbol>
</svg>`;

// ---------- 左: チャット一覧（14.png・21.png の内容） ----------
const rows = [
  ["同時1", "dev", st("done", "完了"), "sel"],
  ["同時2", "dev", st("run", "実行中")],
  ["同時3", "dev", st("done", "完了")],
  ["a.txt の内容を HELLO に、…", "dev", st("done", "完了"), "tent"],
  ["c.txtをつくって1行書いて", "dev", st("done", "完了"), "tent"],
  ["（名前未確認）", "dev", st("idle", "待機（idle）"), "tent"],
  ["Review the current co…", "dev", st("unk", "状態不明")],
  ["Review the current co…", "dev", st("unk", "状態不明"), "tent"],
  ["apply patch ツールを使っ…", "dev", st("unk", "状態不明"), "tent"],
  ["test", "gen", st("unk", "状態不明"), "tent"],
  ["選択肢付きの質問をする", "ext", st("unk", "状態不明")],
  ["aa", "gen", st("unk", "状態不明"), "tent", `<span class="mark pend" title="停止を確認できるまで削除を保留しています">削除保留</span>`],
];
const kind = (k) => (k === "dev" ? I("folder") : k === "gen" ? `<span class="kind">一般</span>` : `<span class="kind">外部</span>`);
const LEFT = String.raw`
<aside class="left" aria-label="チャット一覧">
  <div class="left-top">
    <div class="row1">
      <button class="btn primary">${I("plus")}新しいチャット</button>
      <button class="btn" title="一覧を更新">${I("refresh")}更新</button>
      <button class="btn subtle only-l" data-close="l" aria-label="一覧を閉じる">${I("x")}</button>
    </div>
    <div class="upd">一覧を更新しました（Codex から50件、続きあり）01:23:33</div>
    <label class="search">${I("search")}<input type="search" placeholder="名前で検索" aria-label="名前で検索"></label>
    <label class="chk"><input type="checkbox">アーカイブも含める</label>
  </div>
  <div class="list">
    <div class="list-h">最近使った順</div>
    ${rows.map(([n, k, s, f = "", m = ""]) => `<button class="row ${f.includes("sel") ? "sel" : ""}" ${f.includes("sel") ? 'aria-current="true"' : ""}>
      <span class="l1"><span class="nm ${f.includes("tent") ? "tent" : ""}" ${f.includes("tent") ? 'title="確認済みの名前ではありません（最初の依頼から仮表示）"' : ""}>${n}</span>${m}</span>
      <span class="sub">${kind(k)}${s}</span></button>`).join("\n")}
  </div>
  <div class="left-foot">${I("archive")}アーカイブ（0）</div>
</aside>`;

// ---------- 中央 ----------
const CENTER = String.raw`
<main class="center">
  <header class="chead">
    <button class="btn subtle only-l" data-toggle="l" aria-label="チャット一覧を開く">${I("list")}</button>
    <div class="tt">
      <div class="l1"><h1>同時1</h1>${st("done", "完了")}<span class="sep" aria-hidden="true"></span><span class="fr live" title="収集の鮮度。エージェントの状態とは別"><i></i>受信中</span></div>
      <div class="l2"><span class="tag ai" title="この会話のAI">Codex</span><span class="path" title="作業フォルダ: C:\Users\wmasa\agentdock-p3-test">C:\Users\wmasa\agentdock-p3-test</span><button class="link">フォルダ変更</button></div>
    </div>
    <div class="ops" role="group" aria-label="主要な操作">
      <button class="hop" title="未確認の操作です（結果は観測した事実だけを表示します）">差分<span class="unv">未確認</span></button>
      <button class="hop" title="未確認の操作です（結果は観測した事実だけを表示します）">戻す<span class="unv">未確認</span></button>
      <button class="hop">レビュー</button>
      <button class="hop">分岐</button>
      <button class="hop">Goal</button>
      <button class="hop more" title="すべての操作を検索（Ctrl+K）">操作…<kbd>Ctrl+K</kbd></button>
    </div>
    <button class="btn subtle only-r" data-toggle="r" aria-label="エージェントのドックを開く">${I("dock")}</button>
  </header>
  <div class="msgs">
    <div class="msg user"><div class="who">あなた</div><div class="bubble">3 つのサブエージェントを起動して、それぞれ別の天気予報を調べて</div></div>
    <div class="msg ai"><div class="who">Codex</div><div class="mbody"><p>3つのサブエージェントに、東京・大阪・札幌の天気予報をそれぞれ調べてもらっているよ。結果がそろったらまとめて伝えるね。</p></div>
      <div class="macts"><button class="btn subtle">${I("copy")}コピー</button><button class="btn subtle">${I("branch")}ここから分岐</button></div>
      <details class="work"><summary>作業の記録（7件）</summary><div>collab: spawn_agent ×3／wait_agent ×3／close_agent ×1</div></details>
    </div>
    <div class="msg user"><div class="who">あなた</div><div class="bubble">こんにちわ</div></div>
    <div class="msg ai"><div class="who">Codex</div><div class="mbody"><p>こんにちは。今日は何を一緒にやろうか？</p></div>
      <div class="macts"><button class="btn subtle">${I("copy")}コピー</button><button class="btn subtle">${I("branch")}ここから分岐</button></div>
    </div>
  </div>
  <div class="composer">
    <div class="shell">
      <textarea aria-label="メッセージ" placeholder="メッセージを入力" rows="2"></textarea>
      <div class="ctrl">
        <button class="btn subtle icon" aria-label="ファイル・画像・参考情報を追加">${I("clip")}</button>
        <select class="pick" aria-label="モデル"><option>GPT-6-Luna</option><option>GPT-6.1-Sol</option></select>
        <select class="pick" aria-label="推論の強さ"><option>medium</option><option>low</option><option>high</option></select>
        <select class="pick" aria-label="速度"><option>速度: 指定なし（変更しない）</option><option>速度: 標準（default）</option><option>Fast</option></select>
        <select class="pick" aria-label="計画／実行"><option>計画／実行: 指定なし</option><option>Plan</option><option>Default</option></select>
        <select class="pick" aria-label="権限"><option>作業領域内の書込み＋必要時に確認（初期値）</option></select>
        <span class="grow"></span>
        <button class="btn primary send" aria-label="送信">${I("send")}</button>
      </div>
    </div>
    <div class="hint"><span>Ctrl＋Enter で送信、Enter で改行</span><span class="acc">受理済み: <b>gpt-6-luna／medium</b></span></div>
  </div>
</main>`;

// ---------- 右: ドック ----------
const card = (o) => `<div class="card ${o.c}${o.stale ? " stale" : ""}">
  <div class="c1"><b class="nm" ${o.nmTitle ? `title="${o.nmTitle}"` : ""}>${o.nm}</b><span class="rel">${o.rel}</span>${o.st}</div>
  ${o.path ? `<div class="c2"><span class="path" title="Codexが返したエージェントの経路（呼び名とは別）">${o.path}</span></div>` : ""}
  ${o.l2 ? `<div class="c2 em">${o.l2}</div>` : ""}
  <div class="c2">${o.role}</div>
  <div class="c3">${o.act}</div>
  <div class="c4"><span>${o.t}</span><span>${o.src}</span>${o.fr ?? ""}</div>
  ${o.acts ? `<div class="acts">${o.acts}</div>` : ""}
</div>`;
const childRole = `<span class="kvp">役割: 未提供（Codexが返していません）</span><span class="kvp">担当: 未確認</span>`;
const DOCK = String.raw`
<aside class="right" aria-label="エージェントのドック">
  <div class="dock-h">
    <div class="t"><b>エージェントのドック</b><button class="btn subtle only-r" data-close="r" aria-label="ドックを閉じる">${I("x")}</button></div>
    <span class="seg" role="group" aria-label="表示範囲"><button aria-pressed="true">選択中のチャット</button><button aria-pressed="false">全チャット</button><button aria-pressed="false">実行中</button></span>
    <label class="chk"><input type="checkbox">親のみ表示（子・孫を隠す）</label>
    <div class="dock-note">完了したエージェントも表示しています</div>
  </div>
  <div class="dock-s">
    <ul class="tree"><li>
      ${card({ c: "done", nm: "同時1", rel: "メイン", st: st("done", "完了"), role: "役割: メイン（会話の本体）", act: "活動なし", t: "07:11:57", src: "直接取得（ライブ）" })}
      <ul class="tree">
        <li>${card({ c: "done", nm: "McClintock", nmTitle: "Codexが付けた呼び名です（役割ではありません）", rel: "子", path: "/root/weather_tokyo", st: st("done", "完了"), role: childRole, act: "活動なし", t: "07:12:07", src: "直接取得（ライブ）" })}</li>
        <li>${card({ c: "done", nm: "Kepler", nmTitle: "Codexが付けた呼び名です（役割ではありません）", rel: "子", path: "/root/weather_osaka", st: st("done", "完了"), role: childRole, act: "活動なし", t: "07:12:08", src: "直接取得（ライブ）" })}</li>
        <li>${card({ c: "done", nm: "Turing", nmTitle: "Codexが付けた呼び名です（役割ではありません）", rel: "子", path: "/root/weather_sapporo", st: st("done", "完了"), role: childRole, act: "活動なし", t: "07:12:13", src: "直接取得（ライブ）" })}</li>
      </ul>
    </li></ul>
    <details class="grp"><summary title="履歴の読取りだけで終端を確認した子です（live確認ではありません）">履歴のみで確認した子 4件（完了4）</summary>
      <ul class="tree flat"><li>${card({ c: "done", stale: true, nm: "（呼び名なし）", rel: "子", st: st("done", "履歴で完了を確認", "hist"), l2: "live未確認・取得23:14:39（履歴の読取りで確認）", role: childRole, act: "原状態: notLoaded（根拠なし）", t: "23:14:39", src: "保存履歴", fr: `<span class="stale-t">状態は走査で更新（約3秒間隔）</span>`, acts: `<button class="btn sm">${I("check")}確認済みにして隠す</button>` })}</li></ul>
    </details>
    <details class="grp"><summary>状態不明の子 1件</summary>
      <ul class="tree flat"><li>${card({ c: "unk", stale: true, nm: "（呼び名なし）", rel: "子", st: st("unk", "状態不明"), role: childRole, act: "原状態: notLoaded（根拠なし）", t: "23:14:39", src: "保存履歴", fr: `<span class="stale-t">状態は走査で更新（約3秒間隔）</span>`, acts: `<button class="btn sm">${I("check")}確認済みにして隠す</button>` })}</li></ul>
    </details>
  </div>
  <div class="dock-foot">状態は Codex から取得した情報です。取得できない項目は「未確認」と表示します。</div>
</aside>`;

// ---------- 変更を戻す（33.png・38.png・41.png の内容） ----------
const REVERT = String.raw`
<div class="scrim" id="d-revert" data-screen-of="revert"><section class="dlg" role="dialog" aria-modal="true" aria-labelledby="h-revert">
  <header><h2 id="h-revert">変更を戻す</h2><button class="btn subtle icon" data-close-dlg aria-label="閉じる">${I("x")}</button></header>
  <div class="bd">
    <p class="lead"><b>シェルコマンドで a.txt の内容を HELLO にして</b> の変更を、turnの開始時の控えへ戻します。</p>
    <p class="verify"><span class="unv">未確認</span><span>この操作は実際のCodexで動作を確認していません。表示は受け取った報告と、読み取ったファイルの内容だけに基づきます。</span></p>
    <div class="about">turnの開始時にAgentDockがGitで控えた内容へ、ファイルを書き戻します。現在の内容がこのチャットのturnによる変更だけのファイルはそのまま戻せます。turnの後や間に別の変更がある・終了時の控えがない・別の会話と同時に作業していたファイルは、理由を表示して止めます。内容を確認したうえで、ファイルごとに「別の変更も含めて戻す」を選べます。コミットなどでHEADが変わった後のファイルは戻せません。戻す前の内容は、このチャット用の領域に控えとして保存します（自動では削除しません）。</div>
    <div class="fld"><label for="rv-scope">戻す範囲</label><select id="rv-scope"><option>このチャットの控えのある変更すべて</option></select></div>
    <div class="sect-h">対象のファイル 2件 <span class="cnt">戻せる 0・要確認 1・止める 1</span></div>
    <div class="fcard need">
      <div class="h"><span class="badge need">要確認</span><span class="p">C:/Users/wmasa/agentdock-p3-test/a.txt</span></div>
      <ul><li>turnの後に別の変更: a.txt: turnの後に、別の会話・エディタ等による変更があります</li>
          <li>一緒に戻る後続の変更: turn <span class="mono" title="01a122ad-6bb1-76c2-aaaa-64a373a721e1">01a122ad…</span>、<span class="mono" title="01a122ae-ec15-7b31-a4ce-99c1cc56480d">01a122ae…</span></li></ul>
      <label class="chk strong"><input type="checkbox" checked disabled>別の変更も含めて控えの内容に戻す（確認が必要）</label>
      <div class="confirm" role="alert">
        <h3>${I("alert")}別の変更も含めて、1 件を控えの内容に戻します。</h3>
        <ul><li><span class="mono">C:/Users/wmasa/agentdock-p3-test/a.txt</span><br>・turnの後に別の変更: a.txt: turnの後に、別の会話・エディタ等による変更があります</li></ul>
        <p>これらのファイルでは、別の会話・エディタによる変更や時期の分からない変更も消えます。ファイルは、選んだ区間の開始時の控えの内容になります。書き換える前の内容は控えに保存します（戻した後も控えから確認できます）。</p>
        <div class="acts"><button class="btn danger">別の変更も含めて戻す</button><button class="btn">やめる（何も変えません）</button></div>
      </div>
    </div>
    <div class="fcard block">
      <div class="h"><span class="badge block">${I("lock")}止める</span><span class="p">C:/Users/wmasa/agentdock-p3-test/b.txt</span></div>
      <ul><li>HEADが変わった（コミット・ブランチ切替など）: turnの後にGitのHEADまたはブランチが変わったため、控えへは戻せません。Gitの操作（git revert 等）で戻してください</li><li>強制しても戻せません。</li></ul>
    </div>
  </div>
  <footer><button class="btn">計画を作り直す</button><span class="grow"></span><button class="btn" data-close-dlg>閉じる</button><button class="btn primary" disabled>選んだ 0 件を戻す…</button></footer>
</section></div>`;

// ---------- 設定・保存と容量（25.webp の内容） ----------
const chats = [
  ["同時1", "135 KB", "134 KB", "1.3 KB", "0 B", "0 B", "0 B"],
  ["a.txt の内容を HELLO に、b.txt にも 1 …", "83.4 KB", "76.4 KB", "7.0 KB", "0 B", "0 B", "0 B"],
  ["同時3", "52.1 KB", "50.7 KB", "1.3 KB", "0 B", "0 B", "0 B"],
  ["apply patch ツールを使って、a.txt の内容を…", "36.5 KB", "27.4 KB", "8.4 KB", "0 B", "747 B", "0 B"],
  ["同時2", "27.8 KB", "26.5 KB", "1.3 KB", "0 B", "0 B", "0 B"],
  ["aa", "27.0 KB", "24.7 KB", "2.2 KB", "0 B", "0 B", "0 B"],
  ["Review the current code change…", "24.1 KB", "21.0 KB", "3.1 KB", "0 B", "0 B", "0 B"],
  ["test", "18.7 KB", "17.3 KB", "1.3 KB", "0 B", "0 B", "12 B"],
  ["こんにちわ、サブエージェントlunaを起動して足立区の一週間…b", "17.1 KB", "14.5 KB", "2.6 KB", "0 B", "0 B", "0 B"],
  ["選択肢付きの質問をする", "15.7 KB", "14.9 KB", "750 B", "0 B", "0 B", "0 B"],
  ["a.txt の内容を HELLO に、b.txt にも 1 …", "14.1 KB", "12.8 KB", "1.3 KB", "0 B", "0 B", "0 B"],
  ["Review the current code changes (staged,…（分岐）", "11.9 KB", "6.1 KB", "5.7 KB", "0 B", "0 B", "0 B"],
  ["json壊すa", "8.7 KB", "8.1 KB", "576 B", "0 B", "0 B", "0 B"],
  ["test", "6.2 KB", "5.4 KB", "875 B", "0 B", "0 B", "0 B"],
  ["シェルコマンドで a.txt の内容を HELLO にして", "6.1 KB", "3.1 KB", "1.3 KB", "1.4 KB", "289 B", "0 B"],
];
const td = (v) => `<td class="n${v === "0 B" ? " z" : ""}">${v}</td>`;
const legend = [
  ["b1", "メタデータ", "47.2 MB"], ["b2", "監視活動", "491 KB"], ["b3", "変更の控え", "1.4 KB"], ["b4", "戻す前の控え", "1.0 KB"],
  ["b5", "作業領域", "12 B"], ["b6", "添付", "0 B"], ["b7", "成果物", "0 B"],
];
const SETTINGS = String.raw`
<div class="scrim" id="d-settings" data-screen-of="settings"><section class="dlg wide settings" role="dialog" aria-modal="true" aria-labelledby="h-set">
  <header><h2 id="h-set">設定</h2><button class="btn subtle icon" data-close-dlg aria-label="閉じる">${I("x")}</button></header>
  <div class="set">
    <nav class="snav" aria-label="設定の分類">
      ${["全般", "通知", "入力", "モデル", "Codex", "MCP・Plugins", "Skills", "保存と容量"].map((t) => `<button ${t === "保存と容量" ? 'aria-current="page"' : ""}>${t}</button>`).join("")}
    </nav>
    <div class="sbody">
      <h3>保存と容量</h3>
      <div class="sec">
        <div class="sec-h"><h4>使用量（全体）</h4><button class="btn sm">${I("refresh")}再集計</button></div>
        <div class="big">47.7 MB</div>
        <div class="bar" role="img" aria-label="内訳: メタデータ 47.2 MB、監視活動 491 KB、変更の控え 1.4 KB、戻す前の控え 1.0 KB、作業領域 12 B、添付 0 B、成果物 0 B">
          <span style="flex:47.2;background:var(--b1)"></span><span style="flex:0.48;background:var(--b2)"></span><span style="flex:0.06;background:var(--b3)"></span><span style="flex:0.06;background:var(--b4)"></span>
        </div>
        <div class="legend">${legend.map(([b, n, v]) => `<span><i style="background:var(--${b})"></i>${n}<span class="v">${v}</span></span>`).join("")}</div>
        <p class="note">AgentDock の専用領域だけを数えます。Codex の履歴（~/.codex）は含みません。</p>
      </div>
      <div class="sec">
        <div class="sec-h"><h4>このチャット</h4><span class="num">1.2 KB</span></div>
        <div class="kv"><span>添付 <b>0 B</b></span><span>成果物 <b>0 B</b></span><span>作業領域 <b>0 B</b></span><span>監視活動 <b>177 B</b></span><span>メタデータ <b>1.0 KB</b></span><span>変更の控え <b>0 B</b></span><span>戻す前の控え <b>0 B</b></span></div>
      </div>
      <div class="sec">
        <h4>変更の控え</h4>
        <label class="chk strong"><input type="checkbox" checked>turnの開始時に変更の控えを取る（Gitリポジトリのみ）</label>
        <p class="note">オフにすると以後のturnは控えを取らず、「変更を戻す」の対象になりません。すでに取った控えは消えません（自動では削除しません）。</p>
        <div class="row-kv"><span>このチャットの控え: 変更の控え <b>0 B</b>／戻す前の控え <b>0 B</b></span><button class="btn sm">控えを削除…</button><span class="note">明示的に削除したときだけ消えます。</span></div>
      </div>
      <div class="sec">
        <h4>チャット別</h4>
        <div class="tblwrap"><table class="tbl">
          <thead><tr><th>チャット</th><th class="n">合計</th><th class="n">監視活動</th><th class="n">メタデータ</th><th class="n">変更の控え</th><th class="n">戻す前の控え</th><th class="n" title="添付・成果物・作業領域の合計">添付ほか</th></tr></thead>
          <tbody>${chats.map(([n, ...v]) => `<tr><td class="nm" title="${n}">${n}</td>${v.map(td).join("")}</tr>`).join("")}</tbody>
        </table></div>
      </div>
    </div>
  </div>
  <footer><span class="grow"></span><button class="btn primary" data-close-dlg>閉じる</button></footer>
</section></div>`;

// ---------- 部品見本 ----------
const PARTS = String.raw`
<div class="scrim" id="d-parts" data-screen-of="parts"><section class="dlg wide" role="dialog" aria-modal="true" aria-labelledby="h-parts">
  <header><h2 id="h-parts">部品見本（状態色・確認状況・帯・カード）</h2><button class="btn subtle icon" data-close-dlg aria-label="閉じる">${I("x")}</button></header>
  <div class="bd parts">
    <div class="sect-h">ボタン</div>
    <div class="acts"><button class="btn primary">送信</button><button class="btn">通常</button><button class="btn subtle">控えめ</button><button class="btn danger-line">削除…</button><button class="btn danger">別の変更も含めて戻す</button><button class="btn primary" disabled>選んだ 0 件を戻す…</button><button class="btn" disabled>無効</button></div>
    <div class="sect-h">エージェント状態（§4.1）— 形＋文字＋色</div>
    <div class="acts">${st("run", "実行中")}${st("run", "初期化中")}${st("wait", "対応待ち")}${st("fail", "失敗")}${st("int", "中断")}${st("done", "完了")}${st("done", "終了")}${st("idle", "待機（idle）")}${st("unk", "状態不明")}${st("done", "履歴で完了を確認", "hist")}${st("fail", "履歴で失敗を確認", "hist")}</div>
    <div class="acts">${st("run", "作業中（子が作業中）")}${st("fail", "作業中・子の失敗あり")}${st("done", "完了")}<span class="note-inline">（子2件の状態不明）</span></div>
    <div class="sect-h">鮮度（§4.2）— 状態とは別の見た目（塗りなし・文字色は控えめ）</div>
    <div class="acts"><span class="fr live"><i></i>受信中</span><span class="fr stale"><i></i>履歴取得済み・live未復旧</span><span class="fr check"><i></i>要照合</span><span class="fr off"><i></i>切断</span><span class="fr off"><i></i>非対応</span></div>
    <div class="sect-h">確認状況・受理（成功と書かない）</div>
    <div class="acts"><button class="hop">差分<span class="unv">未確認</span></button><button class="hop">戻す<span class="unv">未確認</span></button><span class="hint-l">受理済み: <b>gpt-6-luna／medium</b></span><span class="hint-l">選択: gpt-6.1-sol／low（未受理。次のターンから適用）</span><span class="hint-l">計画／実行: 選択 計画（Plan）／受理済み 計画（Plan） <span class="unv">未確認</span></span></div>
    <div class="sect-h">一覧の印</div>
    <div class="acts"><span class="mark wait">承認待ち</span><span class="mark wait">質問</span><span class="mark fail">失敗</span><span class="mark pend">削除保留</span><span class="tag ai">Codex</span><span class="tag">分岐元: 同時1</span></div>
    <div class="sect-h">帯（1つの原因につき1本）</div>
    <div class="banner warn">${I("info")}<div><b>アプリの再起動後のため、保存履歴から表示しています</b><br>送信すると続きから再開します（再開するまでライブ監視はしません）。保存履歴から取得した状態です（22:10:19）。実行中の途中経過は表示されません。送信待ちは照合が終わるまで保留します。<div class="acts"><button class="btn sm">状態を再照合</button><button class="btn sm">保存履歴を再取得</button></div></div></div>
    <div class="banner err">${I("alert")}<div>Codex との接続が切れています。状態は最後に受信した内容のままで、完了・失敗を意味しません。</div><button class="btn sm">再試行</button></div>
    <div class="banner info">${I("info")}<div>このチャットには変更の控えがありません（控えを取る前のturn、Gitリポジトリではない、または設定で控えの記録がオフです）。</div></div>
    <div class="sect-h">戻す結果（36.png）</div>
    <div class="banner err">${I("alert")}<div><b>一部のみ戻しました（戻せたもの 0 件、戻せなかったもの 1 件）。完了ではありません。</b><br>戻せなかったファイル: <span class="mono">C:/Users/wmasa/agentdock-p3-test/a.txt</span> — a.txt: 大文字小文字だけが違う名前の変更は、同じファイルを指すため戻せません<div class="acts"><button class="btn sm">もう一度確認する</button></div></div></div>
    <div class="sect-h">変更一覧の行（28.png）— 「戻し済み」を見落とさない</div>
    <div class="tblwrap"><table class="tbl"><thead><tr><th>種類</th><th>ファイル</th><th class="n">追加</th><th class="n">削除</th><th>turn</th></tr></thead>
      <tbody><tr class="sel"><td>変更</td><td><span class="mono">C:/Users/wmasa/agentdock-p3-test/a.txt</span> <span class="tag reverted">${I("check")}戻し済み</span></td><td class="n">+1</td><td class="n">−1</td><td><span class="mono" title="01a122a0-4290-7813-aee9-4e729d76a105">01a122a0…</span></td></tr></tbody></table></div>
    <div class="sect-h">ドックのカード</div>
    <div class="cards">
      ${card({ c: "wait", nm: "Explorer", rel: "子", path: "/root/explorer", st: st("wait", "対応待ち"), role: `<span class="kvp">役割: explorer</span><span class="kvp">担当: 未確認</span>`, act: "承認待ち", t: "07:12:40", src: "直接取得（ライブ）" })}
      ${card({ c: "fail", nm: "Worker", rel: "孫", path: "/root/explorer/worker", st: st("fail", "失敗"), role: `<span class="kvp">役割: worker</span><span class="kvp">担当: 未確認</span>`, act: "活動なし", t: "07:13:02", src: "直接取得（ライブ）", acts: `<button class="btn sm">${I("check")}失敗を確認済みにする</button>` })}
      ${card({ c: "run", nm: "同時2", rel: "メイン", st: st("run", "実行中"), role: "役割: メイン（会話の本体）", act: "コマンドを実行中: cargo check", t: "07:13:10", src: "直接取得（ライブ）" })}
    </div>
  </div>
  <footer><span class="grow"></span><button class="btn primary" data-close-dlg>閉じる</button></footer>
</section></div>`;

// ---------- 共通CSS ----------
const BASE = String.raw`
*{box-sizing:border-box}
html,body{margin:0;height:100%}
body{display:flex;flex-direction:column;font:var(--fs)/var(--lh) var(--font);color:var(--ink);background:var(--stage)}
button,input,select,textarea{font:inherit;color:inherit}
kbd{font:var(--fs-xs) var(--font);color:var(--ink3);margin-left:6px}
.mono{font-family:var(--mono);font-size:var(--fs-mono-s)}
svg.i{width:16px;height:16px;stroke:currentColor;fill:none;stroke-width:1.6;stroke-linecap:round;stroke-linejoin:round;flex:none}
:focus-visible{outline:2px solid var(--focus);outline-offset:1px}
@media (prefers-reduced-motion:reduce){*{transition:none!important;animation:none!important}}
/* mock controls (not part of the design) */
.mockbar{flex:none;display:flex;flex-wrap:wrap;gap:6px 16px;align-items:center;padding:6px 12px;background:#1E2428;color:#E8EDF0;font:12px/1.4 "Segoe UI","Yu Gothic UI",sans-serif}
.mockbar .g{display:inline-flex;gap:3px;align-items:center}
.mockbar button{background:#2F3940;border:1px solid #4A575F;color:#E8EDF0;height:24px;padding:0 8px;border-radius:3px;cursor:pointer;font:inherit}
.mockbar button[aria-pressed=true]{background:#E8EDF0;color:#1E2428}
.mockbar a{color:#9FD3FF}.mockbar a.cur{color:#fff;font-weight:700;text-decoration:none}
.mockbar .nt{color:#A9B6BD}
.stage{flex:1;min-height:0;display:flex;justify-content:center;padding:10px}
/* frame = 1 window */
.frame{container:app/inline-size;position:relative;width:100%;height:100%;display:flex;flex-direction:column;background:var(--bg);border:1px solid var(--frame-line);overflow:hidden;box-shadow:0 8px 30px rgba(0,0,0,.25)}
.ostitle{flex:none;height:32px;display:flex;align-items:center;gap:8px;padding-left:10px;background:var(--os-bg);font:12px "Segoe UI","Yu Gothic UI",sans-serif;color:var(--ink)}
.ostitle .ic{width:16px;height:16px;border-radius:3px;background:var(--accent);position:relative}
.ostitle .ic:after{content:"";position:absolute;left:3px;right:3px;bottom:3px;height:3px;border-radius:1px;background:var(--accent-ink)}
.ostitle .cap{margin-left:auto;display:flex}.ostitle .cap span{width:46px;height:32px;display:grid;place-items:center}
.menubar{flex:none;display:flex;align-items:center;gap:2px;padding:2px 6px;background:var(--chrome);border-bottom:1px solid var(--line)}
.menubar .m{height:26px;padding:0 10px;border:0;border-radius:var(--r1);background:none;cursor:pointer}
.menubar .m:hover{background:var(--hover)}
.vtag{margin-left:auto;height:22px;padding:0 8px;font-size:var(--fs-xs);color:var(--ink2);border:1px solid var(--line);border-radius:var(--r1);background:var(--surface);cursor:pointer}
.body{flex:1;min-height:0;display:grid;grid-template-columns:var(--lw) minmax(0,1fr) var(--rw);position:relative}
.pscrim{display:none;position:absolute;inset:0;z-index:24;background:var(--scrim-light)}
.only-l,.only-r{display:none!important}
/* buttons */
.btn{height:var(--ctl);padding:0 var(--bpad);border-radius:var(--r1);border:1px solid var(--line-strong);background:var(--surface);display:inline-flex;align-items:center;justify-content:center;gap:6px;white-space:nowrap;cursor:pointer}
.btn:hover{background:var(--hover-solid)}
.btn.primary{background:var(--accent);border-color:var(--accent);color:var(--accent-ink);font-weight:600}
.btn.primary:hover{background:var(--accent-hover)}
.btn.danger{background:var(--fail);border-color:var(--fail);color:var(--fail-on);font-weight:600}
.btn.danger-line{color:var(--fail-ink);border-color:var(--fail-line)}
.btn.subtle{border-color:transparent;background:transparent}
.btn.subtle:hover{background:var(--hover)}
.btn.icon{width:var(--ctl);padding:0}
.btn.sm{height:24px;padding:0 8px;font-size:var(--fs-s)}
.btn:disabled,.btn:disabled:hover{background:var(--disabled-bg);color:var(--disabled-ink);border-color:var(--disabled-line);cursor:not-allowed;font-weight:400}
.link{background:none;border:0;padding:0;color:var(--accent-text);cursor:pointer;font-size:inherit}
.link:hover{text-decoration:underline}
.chk{display:flex;align-items:center;gap:6px;font-size:var(--fs-s);color:var(--ink2)}
.chk.strong{font-size:var(--fs);color:var(--ink);margin:6px 0}
input[type=checkbox]{accent-color:var(--accent);width:15px;height:15px;margin:0}
/* state chip (§4.1): shape + text + color */
.st{display:inline-flex;align-items:center;gap:5px;font-size:var(--fs-s);font-weight:600;line-height:1.55;padding:0 var(--chip-px);border-radius:var(--chip-r);color:var(--c);background:var(--cbg);border:1px solid transparent;white-space:nowrap}
.st i{width:8px;height:8px;border-radius:50%;background:currentColor;flex:none}
.st.run{--c:var(--run);--cbg:var(--run-bg)}.st.wait{--c:var(--wait-ink);--cbg:var(--wait-bg)}.st.fail{--c:var(--fail-ink);--cbg:var(--fail-bg)}
.st.int{--c:var(--int);--cbg:var(--int-bg)}.st.done{--c:var(--done);--cbg:var(--done-bg)}.st.idle{--c:var(--idle);--cbg:var(--idle-bg)}.st.unk{--c:var(--unk);--cbg:transparent}
.st.wait i{border-radius:1px;transform:rotate(45deg);width:7px;height:7px}
.st.fail i{border-radius:1px}
.st.int i{border-radius:0;background:linear-gradient(90deg,currentColor 0 3px,transparent 3px 5px,currentColor 5px)}
.st.done i{background:none;border-radius:0;width:4px;height:8px;border:solid currentColor;border-width:0 2px 2px 0;transform:rotate(45deg) translate(-1px,-1px);margin:0 2px}
.st.idle i{background:none;border:1.5px solid currentColor}
.st.unk i{background:none;border:1.5px dashed currentColor}
.st.unk{border:1px dashed var(--line-strong)}
.st.hist{background:transparent;border:1px dashed currentColor}
/* freshness (§4.2): different grammar = no fill */
.fr{display:inline-flex;align-items:center;gap:5px;font-size:var(--fs-s);color:var(--ink2);white-space:nowrap}
.fr i{width:9px;height:9px;border-radius:50%;border:1.5px solid var(--live);background:radial-gradient(circle,var(--live) 0 2px,transparent 2.5px)}
.fr.stale,.fr.check{color:var(--stale)}.fr.stale i,.fr.check i{border:1.5px dashed currentColor;background:none}
.fr.off{color:var(--fail-ink)}.fr.off i{border-color:currentColor;background:linear-gradient(45deg,transparent 43%,currentColor 43% 57%,transparent 57%)}
.sep{width:1px;height:14px;background:var(--line-strong)}
/* verification (§3.14): neutral dashed outline, never yellow */
.unv{display:inline-flex;align-items:center;font-size:var(--fs-xs);font-weight:500;line-height:1.45;padding:0 4px;border:1px dashed var(--unv-line);border-radius:var(--r1);color:var(--unv-ink);background:var(--unv-bg);white-space:nowrap}
.tag{display:inline-flex;align-items:center;gap:3px;font-size:var(--fs-xs);line-height:1.6;padding:0 6px;border-radius:var(--r1);border:1px solid var(--line);color:var(--ink2);background:var(--surface2);white-space:nowrap}
.tag.ai{color:var(--accent-text);border-color:var(--accent-line);background:var(--accent-weak)}
.tag.reverted{font-size:var(--fs-s);font-weight:600;color:var(--done);border-color:currentColor;background:var(--done-bg)}
.tag.reverted svg{width:13px;height:13px}
.mark{font-size:var(--fs-xs);font-weight:700;line-height:1.6;padding:0 6px;border-radius:var(--r1);white-space:nowrap;flex:none}
.mark.wait{background:var(--wait-solid);color:var(--wait-on)}.mark.fail{background:var(--fail);color:var(--fail-on)}
.mark.pend{border:1px solid var(--line-strong);color:var(--ink2);font-weight:500}
.banner{display:flex;gap:8px;align-items:flex-start;padding:8px 10px;margin:0 0 8px;border-radius:var(--r2);border:1px solid var(--b-line);background:var(--b-bg);font-size:var(--fs-s)}
.banner>svg{color:var(--b-ic);margin-top:2px}.banner>div{flex:1;min-width:0}
.banner.warn{--b-bg:var(--wait-bg);--b-line:var(--wait-line);--b-ic:var(--wait-ink)}
.banner.err{--b-bg:var(--fail-bg);--b-line:var(--fail-line);--b-ic:var(--fail-ink)}
.banner.info{--b-bg:var(--info-bg);--b-line:var(--info-line);--b-ic:var(--info)}
/* left */
.left{grid-column:1;background:var(--rail);border-right:1px solid var(--line);display:flex;flex-direction:column;min-height:0;min-width:0}
.left-top{padding:var(--p2);display:flex;flex-direction:column;gap:var(--p1)}
.left-top .row1{display:flex;gap:6px}.left-top .row1 .btn.primary{flex:1}
.upd{font-size:var(--fs-xs);color:var(--ink3)}
.search{position:relative;display:block}.search svg{position:absolute;left:8px;top:50%;transform:translateY(-50%);color:var(--ink3);width:14px;height:14px}
.search input{width:100%;height:var(--ctl);padding:0 8px 0 28px;border:1px solid var(--line-strong);border-radius:var(--r1);background:var(--input-bg)}
.list{flex:1;overflow:auto;padding:0 var(--p1) var(--p2)}
.list-h{font-size:var(--fs-xs);color:var(--ink3);padding:var(--p2) var(--p2) 2px;font-weight:600}
.row{width:100%;display:flex;flex-direction:column;gap:1px;padding:var(--row-py) var(--p2) var(--row-py) calc(var(--p2) + 2px);border:0;border-radius:var(--r2);background:none;text-align:left;cursor:pointer;position:relative}
.row:hover{background:var(--hover)}
.row.sel{background:var(--sel-bg);box-shadow:var(--sel-shadow)}
.row.sel:before{content:"";position:absolute;left:2px;top:22%;bottom:22%;width:3px;border-radius:2px;background:var(--accent)}
.row .l1{display:flex;align-items:center;gap:6px;min-width:0}
.row .nm{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-weight:600}
.row .nm.tent{font-weight:400;color:var(--ink2)}
.row .sub{display:flex;align-items:center;gap:6px;font-size:var(--fs-s);color:var(--ink2);min-width:0}
.row .sub svg{width:13px;height:13px;color:var(--ink3)}
.row .kind{color:var(--ink3)}
.row .st{background:none;padding:0;font-weight:500;border:0}
.left-foot{border-top:1px solid var(--line);padding:6px var(--p2);font-size:var(--fs-s);color:var(--ink2);display:flex;gap:6px;align-items:center}
/* center */
.center{grid-column:2;display:flex;flex-direction:column;min-width:0;min-height:0;background:var(--surface)}
.chead{display:flex;align-items:center;gap:var(--p2);padding:var(--p2) var(--p3);border-bottom:1px solid var(--line);min-width:0}
.chead .tt{flex:1;min-width:0}
.chead .l1{display:flex;align-items:center;gap:8px;min-width:0}
.chead h1{font-size:var(--fs-title);font-weight:700;margin:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;min-width:0}
.chead .l2{display:flex;align-items:center;gap:8px;margin-top:2px;font-size:var(--fs-s);color:var(--ink2);min-width:0}
.chead .path{font-family:var(--mono);font-size:var(--fs-mono-s);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;min-width:0}
.ops{display:flex;align-items:center;gap:var(--ops-gap);flex-wrap:wrap;justify-content:flex-end}
.hop{display:inline-flex;align-items:center;gap:5px;height:var(--ctl);padding:0 9px;border:1px solid var(--line-strong);border-radius:var(--r1);background:var(--surface);cursor:pointer;white-space:nowrap}
.hop:hover{background:var(--hover-solid)}
.msgs{flex:1;overflow:auto;padding:var(--p4) var(--p3) var(--p2)}
.msg{max-width:var(--msg-w);margin:0 auto var(--p4)}
.msg .who{font-size:var(--fs-s);color:var(--ink3);margin-bottom:3px}
.msg.user{display:flex;flex-direction:column;align-items:flex-end}
.msg.user .bubble{background:var(--user-bg);border:1px solid var(--user-line);border-radius:var(--r3);padding:var(--p2) var(--p3);max-width:85%}
.msg.ai .mbody{line-height:var(--lh-body)}
.msg.ai .mbody p{margin:0 0 6px}
.macts{display:flex;gap:2px;margin-left:-6px}.macts .btn{height:24px;padding:0 6px;font-size:var(--fs-s);color:var(--ink2)}.macts svg{width:14px;height:14px}
.work{margin-top:4px;font-size:var(--fs-s);color:var(--ink2);border-left:2px solid var(--line-strong);padding:2px 0 2px 10px}.work summary{cursor:pointer}
.composer{max-width:calc(var(--msg-w) + 2 * var(--p3));width:100%;margin:0 auto;padding:var(--p1) var(--p3) var(--p3)}
.shell{border:1px solid var(--line-strong);border-radius:var(--r3);background:var(--input-bg);padding:var(--p1) var(--p2);box-shadow:var(--sh-1)}
.shell:focus-within{border-color:var(--accent);box-shadow:0 0 0 1px var(--accent)}
.shell textarea{width:100%;border:0;outline:0;resize:none;background:transparent;min-height:48px;padding:4px;line-height:var(--lh)}
.ctrl{display:flex;flex-wrap:wrap;align-items:center;gap:4px}.ctrl .grow{flex:1}
.pick{height:26px;max-width:100%;border:1px solid var(--pick-line);border-radius:var(--r1);background:var(--pick-bg);padding:0 4px;font-size:var(--fs-s)}
.pick:hover{border-color:var(--line-strong)}
.send{width:34px;height:34px;padding:0;border-radius:var(--r2)}
.hint{display:flex;justify-content:space-between;flex-wrap:wrap;gap:2px 12px;margin-top:6px;font-size:var(--fs-s);color:var(--ink2)}
.hint b,.hint-l b{font-weight:600;color:var(--ink)}
.hint-l{font-size:var(--fs-s);color:var(--ink2)}
/* dock */
.right{grid-column:3;background:var(--dock-bg);border-left:1px solid var(--line);display:flex;flex-direction:column;min-height:0;min-width:0}
.dock-h{padding:var(--p2);display:flex;flex-direction:column;gap:6px;border-bottom:1px solid var(--line)}
.dock-h .t{display:flex;align-items:center;gap:6px}.dock-h .t b{flex:1}
.seg{display:inline-flex;border:1px solid var(--line-strong);border-radius:var(--r1);overflow:hidden;align-self:flex-start;background:var(--surface);max-width:100%}
.seg button{border:0;background:none;height:26px;padding:0 10px;font-size:var(--fs-s);cursor:pointer;border-right:1px solid var(--line);white-space:nowrap}
.seg button:last-child{border-right:0}
.seg button[aria-pressed=true]{background:var(--accent);color:var(--accent-ink);font-weight:600}
.dock-note{font-size:var(--fs-xs);color:var(--ink3)}
.dock-s{flex:1;overflow:auto;overflow-x:hidden;padding:var(--p2)}
.tree{list-style:none;margin:0;padding:0}
.tree .tree{margin-left:10px;padding-left:10px;border-left:1.5px solid var(--tree-line)}
.tree .tree>li{position:relative}
.tree .tree>li:before{content:"";position:absolute;left:-10px;top:15px;width:9px;border-top:1.5px solid var(--tree-line)}
.tree.flat{margin-top:4px}
.card{--sc:var(--done);border:1px solid var(--card-line);border-left:var(--card-bar) solid var(--sc);border-radius:var(--r1);background:var(--card-bg);padding:var(--card-py) var(--card-px);margin:0 0 var(--card-gap);min-width:0;overflow-wrap:anywhere}
.card.run{--sc:var(--run)}.card.wait{--sc:var(--wait-solid)}.card.fail{--sc:var(--fail)}.card.int{--sc:var(--int)}.card.unk{--sc:var(--unk)}.card.idle{--sc:var(--idle)}
.card.stale{border-style:dashed}
.card .c1{display:flex;align-items:center;gap:6px;min-width:0}
.card .nm{font-weight:700;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.card .rel{font-size:var(--fs-xs);color:var(--ink3);flex:none}
.card .c1 .st{margin-left:auto;flex:none}
.card .c2{font-size:var(--fs-s);color:var(--ink2);display:flex;flex-wrap:wrap;column-gap:10px}
.card .kvp{max-width:100%}
.card .c2.em{color:var(--stale)}
.card .path{font-family:var(--mono);font-size:var(--fs-mono-s);color:var(--ink2)}
.card .c3{font-size:var(--fs-s);color:var(--ink)}
.card .c4{font-size:var(--fs-xs);color:var(--ink3);display:flex;flex-wrap:wrap;gap:0 8px}
.card .stale-t{color:var(--stale)}
.card .acts{margin-top:4px}
.grp{margin:8px 0 0}
.grp summary{list-style:none;cursor:pointer;font-size:var(--fs-s);color:var(--ink2);padding:3px 8px;border-radius:var(--r1);border:1px solid var(--line);background:var(--surface)}
.grp summary::-webkit-details-marker{display:none}
.grp summary:before{content:"▶";font-size:9px;margin-right:6px}.grp[open] summary:before{content:"▼"}
.dock-foot{border-top:1px solid var(--line);padding:6px var(--p2);font-size:var(--fs-xs);color:var(--ink3)}
.cards{display:grid;grid-template-columns:repeat(auto-fill,minmax(260px,1fr));gap:8px}
/* dialogs */
.scrim{position:absolute;inset:0;z-index:40;background:var(--scrim);display:none;align-items:center;justify-content:center;padding:16px}
.frame[data-screen=revert] #d-revert,.frame[data-screen=settings] #d-settings,.frame[data-screen=parts] #d-parts{display:flex}
.dlg{background:var(--dlg-bg);border:1px solid var(--dlg-line);border-radius:var(--r3);box-shadow:var(--sh-3);width:min(var(--dlg-w),100%);max-height:100%;display:flex;flex-direction:column;min-width:0;overflow-wrap:anywhere}
.dlg.wide{width:min(980px,100%);height:min(760px,100%)}
.dlg>header{flex:none;display:flex;align-items:center;gap:8px;padding:var(--p2) var(--p2) var(--p2) var(--p3);border-bottom:1px solid var(--dlg-hline)}
.dlg>header h2{flex:1;margin:0;font-size:var(--fs-dlg-title);font-weight:700}
.dlg .bd{padding:var(--p3);overflow:auto;min-height:0;flex:1}
.dlg>footer{flex:none;display:flex;align-items:center;flex-wrap:wrap;gap:8px;padding:var(--p2) var(--p3);border-top:1px solid var(--dlg-hline);background:var(--dlg-foot)}
.dlg>footer .grow{flex:1}
.lead{margin:0 0 8px}
.verify{display:flex;gap:8px;align-items:baseline;font-size:var(--fs-s);color:var(--ink2);margin:0 0 8px}
.about{font-size:var(--fs-s);color:var(--ink2);background:var(--surface2);border-radius:var(--r2);padding:8px 10px;margin:0 0 12px;line-height:var(--lh-body)}
.fld{display:grid;grid-template-columns:var(--lbl-w) minmax(0,1fr);gap:6px 12px;align-items:center;margin:0 0 12px}
.fld select{height:var(--ctl);border:1px solid var(--line-strong);border-radius:var(--r1);background:var(--input-bg);padding:0 8px;width:100%}
.sect-h{font-size:var(--fs-s);font-weight:700;color:var(--ink2);margin:14px 0 6px;display:flex;gap:8px;align-items:center}
.sect-h .cnt{font-weight:400;color:var(--ink3)}
.fcard{border:1px solid var(--line);border-radius:var(--r2);padding:8px 10px;margin:0 0 8px;background:var(--card-bg)}
.fcard .h{display:flex;gap:8px;align-items:center;min-width:0}.fcard .p{font-family:var(--mono);font-size:var(--fs-mono-s);overflow-wrap:anywhere;min-width:0}
.fcard ul{margin:6px 0 0;padding-left:18px;font-size:var(--fs-s);color:var(--ink2)}
.fcard.block{background:var(--surface2)}
.badge{display:inline-flex;align-items:center;gap:3px;font-size:var(--fs-xs);font-weight:700;line-height:1.6;padding:0 6px;border-radius:var(--r1);white-space:nowrap;flex:none}
.badge svg{width:12px;height:12px}
.badge.need{background:var(--wait-bg);color:var(--wait-ink);border:1px solid var(--wait-line)}
.badge.block{background:var(--surface);color:var(--ink2);border:1px solid var(--line-strong)}
.confirm{margin-top:8px;border:1px solid var(--fail-line);background:var(--fail-bg);border-radius:var(--r2);padding:10px 12px}
.confirm h3{display:flex;gap:6px;align-items:center;margin:0 0 4px;font-size:var(--fs);color:var(--fail-ink)}
.confirm ul{color:var(--ink)}.confirm p{margin:6px 0;font-size:var(--fs-s)}
.acts{display:flex;gap:6px;flex-wrap:wrap;align-items:center;margin-top:6px}
.note,.note-inline{font-size:var(--fs-s);color:var(--ink3);margin:6px 0 0}
/* settings */
.set{flex:1;min-height:0;display:grid;grid-template-columns:var(--nav-w) minmax(0,1fr)}
.snav{border-right:1px solid var(--line);padding:var(--p2);display:flex;flex-direction:column;gap:2px;overflow:auto;background:var(--surface2)}
.snav button{text-align:left;min-height:32px;border:0;background:none;border-radius:var(--r1);padding:0 12px;cursor:pointer;position:relative;white-space:nowrap}
.snav button:hover{background:var(--hover)}
.snav button[aria-current=page]{background:var(--sel-bg);font-weight:600;box-shadow:var(--sel-shadow)}
.snav button[aria-current=page]:before{content:"";position:absolute;left:2px;top:25%;bottom:25%;width:3px;border-radius:2px;background:var(--accent)}
.sbody{overflow:auto;padding:var(--p3);min-width:0}
.sbody h3{font-size:var(--fs-dlg-title);margin:0 0 12px}
.sec{border:1px solid var(--line);border-radius:var(--r2);padding:12px 14px;margin:0 0 12px;background:var(--card-bg)}
.sec h4{margin:0 0 6px;font-size:var(--fs)}
.sec-h{display:flex;align-items:center;gap:10px}.sec-h h4{margin:0;flex:1}.sec-h .num{font-weight:700;font-variant-numeric:tabular-nums}
.big{font-size:22px;font-weight:700;font-variant-numeric:tabular-nums;margin-top:4px}
.bar{display:flex;height:10px;border-radius:5px;overflow:hidden;background:var(--surface2);margin:8px 0;gap:1px}
.bar span{display:block;min-width:3px}
.legend{display:grid;grid-template-columns:repeat(auto-fill,minmax(160px,1fr));gap:2px 16px;font-size:var(--fs-s)}
.legend>span{display:flex;align-items:center}.legend i{display:inline-block;width:9px;height:9px;border-radius:2px;margin-right:6px;flex:none}
.legend .v{margin-left:auto;font-variant-numeric:tabular-nums;color:var(--ink2)}
.kv{display:grid;grid-template-columns:repeat(auto-fill,minmax(140px,1fr));gap:2px 16px;font-size:var(--fs-s);color:var(--ink2)}
.kv b{font-weight:600;color:var(--ink);font-variant-numeric:tabular-nums;float:right}
.row-kv{display:flex;flex-wrap:wrap;align-items:center;gap:6px 12px;margin-top:8px;font-size:var(--fs-s)}.row-kv .note{margin:0}
.tblwrap{overflow-x:auto}
.tbl{width:100%;border-collapse:collapse;font-size:var(--fs-s)}
.tbl th,.tbl td{padding:4px 8px;border-bottom:1px solid var(--line);text-align:left;vertical-align:middle}
.tbl th{color:var(--ink2);font-weight:600;background:var(--card-bg);position:sticky;top:0;white-space:nowrap}
.tbl .n{text-align:right;font-variant-numeric:tabular-nums;white-space:nowrap}
.tbl td.z{color:var(--ink3)}
.tbl td.nm{max-width:280px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.tbl tbody tr:hover{background:var(--hover)}
.tbl tr.sel{background:var(--accent-weak)}
/* narrow (D07): 1280 / 960 — mock uses container queries, app uses @media with the same boundaries */
@container app (max-width:1279px){
  .body{grid-template-columns:var(--lw) minmax(0,1fr)}
  .right{position:absolute;top:0;right:0;bottom:0;width:min(var(--rw),88%);z-index:25;display:none;box-shadow:var(--sh-3)}
  .frame.open-r .right{display:flex}
  .frame.open-r .pscrim{display:block}
  .only-r{display:inline-flex!important}
  .dlg.wide{width:100%;height:100%;border-radius:0;border:0}
  .scrim:has(.dlg.wide){padding:0}
}
@container app (max-width:959px){
  .body{grid-template-columns:minmax(0,1fr)}
  .left{position:absolute;top:0;left:0;bottom:0;width:min(300px,86%);z-index:25;display:none;box-shadow:var(--sh-3)}
  .center{grid-column:1}
  .frame.open-l .left{display:flex}
  .frame.open-l .pscrim{display:block}
  .only-l{display:inline-flex!important}
  .chead{flex-wrap:wrap;padding:var(--p2)}
  .chead .only-r{order:2}
  .ops{order:3;width:100%;justify-content:flex-start}
  .msgs{padding:var(--p3) var(--p2)}
  .composer{padding-left:var(--p2);padding-right:var(--p2)}
  .msg.user .bubble{max-width:92%}
  .fld{grid-template-columns:1fr}
  .set{grid-template-columns:1fr;grid-template-rows:auto minmax(0,1fr)}
  .snav{flex-direction:row;overflow-x:auto;border-right:0;border-bottom:1px solid var(--line)}
  .snav button[aria-current=page]:before{left:20%;right:20%;top:auto;bottom:0;width:auto;height:3px}
  .dlg{border-radius:var(--r2)}
}
`;

// ---------- 案ごとのトークンと差分 ----------
const V = {
  a: {
    name: "案A「業務ツール・高密度」（Windows 11 標準寄り）",
    note: "Segoe UI＋Yu Gothic UI、13px、Windows 11 のアクセント青、角丸4〜8px、カード2〜3行",
    css: String.raw`
:root{--font:"Segoe UI Variable Text","Segoe UI","Yu Gothic UI","Meiryo UI",sans-serif;--mono:"Cascadia Mono","Consolas","BIZ UDGothic",monospace;
--fs:13px;--fs-s:12px;--fs-xs:11px;--fs-title:15px;--fs-dlg-title:15px;--fs-mono-s:11.5px;--lh:1.55;--lh-body:1.7;
--p1:4px;--p2:8px;--p3:14px;--p4:16px;--ctl:28px;--bpad:10px;--row-py:5px;--r1:4px;--r2:6px;--r3:8px;--chip-px:6px;--chip-r:10px;
--card-py:5px;--card-px:8px;--card-gap:4px;--card-bar:3px;--lw:240px;--rw:320px;--msg-w:760px;--dlg-w:700px;--lbl-w:110px;--nav-w:170px;--ops-gap:4px}
[data-theme=light]{--stage:#C5CBD0;--frame-line:#9AA3A9;--os-bg:#EFEFEF;--bg:#F3F3F3;--chrome:#F3F3F3;--surface:#FFFFFF;--surface2:#F5F5F5;--rail:#F0F0F0;--dock-bg:#F6F6F6;--card-bg:#FFFFFF;--card-line:#E0E0E0;
--dlg-bg:#FFFFFF;--dlg-line:#CFCFCF;--dlg-hline:#E8E8E8;--dlg-foot:#F7F7F7;--input-bg:#FFFFFF;--ink:#1B1B1B;--ink2:#545454;--ink3:#686868;--line:#E3E3E3;--line-strong:#C2C2C2;
--hover:rgba(0,0,0,.045);--hover-solid:#F2F2F2;--sel-bg:#FFFFFF;--sel-shadow:0 0 0 1px #E0E0E0;--accent:#005FB8;--accent-hover:#0A5299;--accent-ink:#FFFFFF;--accent-weak:#E7F0FA;--accent-line:#B3CFEA;--accent-text:#005FB8;--focus:#1B1B1B;
--user-bg:#EFF4FA;--user-line:#D9E4F0;--pick-bg:transparent;--pick-line:#D9D9D9;--tree-line:#C6C6C6;--disabled-bg:#F2F2F2;--disabled-ink:#8A8A8A;--disabled-line:#E0E0E0;--scrim:rgba(0,0,0,.32);--scrim-light:rgba(0,0,0,.14);
--sh-1:0 1px 2px rgba(0,0,0,.05);--sh-3:0 10px 32px rgba(0,0,0,.22);
--run:#0F5FAF;--run-bg:#E7F0FA;--wait-ink:#7A4800;--wait-bg:#FFF4D4;--wait-line:#EDD08A;--wait-solid:#F2B600;--wait-on:#2B1D00;--fail:#C42B1C;--fail-ink:#A6261A;--fail-bg:#FDECEA;--fail-line:#EDB7B1;--fail-on:#FFFFFF;
--int:#6A4BA6;--int-bg:#F1ECFA;--done:#2B7440;--done-bg:#E8F4EB;--idle:#545454;--idle-bg:#EFEFEF;--unk:#646464;--live:#2B7440;--stale:#8A5300;--unv-ink:#545454;--unv-line:#8C8C8C;--unv-bg:transparent;
--info:#005FB8;--info-bg:#EEF4FB;--info-line:#C5D9EE;--b1:#005FB8;--b2:#4A9AD9;--b3:#7A5BC0;--b4:#C98A00;--b5:#2B7440;--b6:#8F8F8F;--b7:#BDBDBD}
[data-theme=dark]{--stage:#0B0D0F;--frame-line:#3A3A3A;--os-bg:#1C1C1C;--bg:#202020;--chrome:#202020;--surface:#2B2B2B;--surface2:#323232;--rail:#1C1C1C;--dock-bg:#232323;--card-bg:#2C2C2C;--card-line:#3D3D3D;
--dlg-bg:#2B2B2B;--dlg-line:#474747;--dlg-hline:#3A3A3A;--dlg-foot:#262626;--input-bg:#2D2D2D;--ink:#F2F2F2;--ink2:#C9C9C9;--ink3:#A8A8A8;--line:#3A3A3A;--line-strong:#575757;
--hover:rgba(255,255,255,.06);--hover-solid:#333333;--sel-bg:#2E2E2E;--sel-shadow:0 0 0 1px #3D3D3D;--accent:#4CC2FF;--accent-hover:#6CCDFF;--accent-ink:#00243A;--accent-weak:#15303F;--accent-line:#2B5A75;--accent-text:#7FD3FF;--focus:#FFFFFF;
--user-bg:#2F3640;--user-line:#3D4753;--pick-bg:transparent;--pick-line:#4A4A4A;--tree-line:#4D4D4D;--disabled-bg:#2A2A2A;--disabled-ink:#7C7C7C;--disabled-line:#3A3A3A;--scrim:rgba(0,0,0,.55);--scrim-light:rgba(0,0,0,.3);
--sh-1:none;--sh-3:0 10px 32px rgba(0,0,0,.6);
--run:#7CC4FF;--run-bg:#123047;--wait-ink:#FFD877;--wait-bg:#3A2E10;--wait-line:#6B5414;--wait-solid:#FFC83D;--wait-on:#2B1D00;--fail:#FF8A80;--fail-ink:#FFA59D;--fail-bg:#3D1F1C;--fail-line:#7A3530;--fail-on:#2B0A07;
--int:#C4A9FF;--int-bg:#2E2546;--done:#7DD58A;--done-bg:#1D3523;--idle:#C9C9C9;--idle-bg:#333333;--unk:#A8A8A8;--live:#7DD58A;--stale:#E8B04A;--unv-ink:#C9C9C9;--unv-line:#8A8A8A;--unv-bg:transparent;
--info:#7FD3FF;--info-bg:#15303F;--info-line:#2B5A75;--b1:#4CC2FF;--b2:#2E86C1;--b3:#A88BEB;--b4:#E3A72F;--b5:#6CCB7A;--b6:#8A8A8A;--b7:#5A5A5A}
/* A: 承認待ち・失敗のカードは面でも知らせる */
.card.wait{background:var(--wait-bg)}.card.fail{background:var(--fail-bg)}
.ops{gap:0}.ops .hop{border-radius:0;margin-left:-1px}.ops .hop:first-child{border-radius:var(--r1) 0 0 var(--r1)}.ops .hop:last-child{border-radius:0 var(--r1) var(--r1) 0}
`,
  },
  b: {
    name: "案B「読みやすさ重視・ゆとり」（現行の紺緑を継承）",
    note: "BIZ UDPゴシック、14px・行間1.75、角丸6〜14px、柔らかい影、カード4行",
    css: String.raw`
:root{--font:"BIZ UDPGothic","Meiryo",sans-serif;--mono:"Cascadia Mono","BIZ UDGothic","Consolas",monospace;
--fs:14px;--fs-s:12.5px;--fs-xs:11.5px;--fs-title:17px;--fs-dlg-title:16px;--fs-mono-s:12px;--lh:1.7;--lh-body:1.85;
--p1:6px;--p2:10px;--p3:18px;--p4:22px;--ctl:32px;--bpad:14px;--row-py:8px;--r1:6px;--r2:10px;--r3:14px;--chip-px:8px;--chip-r:12px;
--card-py:9px;--card-px:12px;--card-gap:8px;--card-bar:4px;--lw:264px;--rw:344px;--msg-w:780px;--dlg-w:740px;--lbl-w:120px;--nav-w:190px;--ops-gap:6px}
[data-theme=light]{--stage:#BCC7C6;--frame-line:#93A3A2;--os-bg:#EEF2F1;--bg:#F4F6F5;--chrome:#FFFFFF;--surface:#FFFFFF;--surface2:#F2F5F4;--rail:#EDF1F0;--dock-bg:#F5F7F6;--card-bg:#FFFFFF;--card-line:#DCE3E2;
--dlg-bg:#FFFFFF;--dlg-line:#D5DDDC;--dlg-hline:#E3E9E8;--dlg-foot:#F7F9F8;--input-bg:#FFFFFF;--ink:#17262D;--ink2:#46585F;--ink3:#5C6D74;--line:#DFE6E5;--line-strong:#BDC9C8;
--hover:rgba(30,77,107,.06);--hover-solid:#F0F4F5;--sel-bg:#FFFFFF;--sel-shadow:0 1px 3px rgba(23,38,45,.12);--accent:#1E4D6B;--accent-hover:#163C55;--accent-ink:#FFFFFF;--accent-weak:#E3EDF3;--accent-line:#B7CEDC;--accent-text:#1E5A80;--focus:#1E5A80;
--user-bg:#EDF3F5;--user-line:#D6E2E6;--pick-bg:#F6F8F8;--pick-line:#DCE3E2;--tree-line:#B8C6C5;--disabled-bg:#F1F3F3;--disabled-ink:#8A979C;--disabled-line:#DFE6E5;--scrim:rgba(20,32,38,.34);--scrim-light:rgba(20,32,38,.16);
--sh-1:0 1px 3px rgba(23,38,45,.08);--sh-3:0 16px 48px rgba(20,32,38,.25);
--run:#22609F;--run-bg:#E6EEF8;--wait-ink:#6E4E00;--wait-bg:#FFF5D9;--wait-line:#EBD596;--wait-solid:#E2B12F;--wait-on:#3A2B00;--fail:#B3261E;--fail-ink:#8E1D17;--fail-bg:#FBEAE8;--fail-line:#EBC2BE;--fail-on:#FFFFFF;
--int:#634C93;--int-bg:#EFEBF6;--done:#33664F;--done-bg:#E6F1EC;--idle:#46585F;--idle-bg:#EEF1F1;--unk:#5C6D74;--live:#2C6E4A;--stale:#8A5A00;--unv-ink:#46585F;--unv-line:#8FA3A8;--unv-bg:transparent;
--info:#1E5A80;--info-bg:#EAF2F7;--info-line:#C3D5E1;--b1:#1E4D6B;--b2:#4F8DB3;--b3:#7D68AE;--b4:#C29420;--b5:#3E7D61;--b6:#9AA8AB;--b7:#C9D3D2}
[data-theme=dark]{--stage:#0A0E10;--frame-line:#33424A;--os-bg:#141A1D;--bg:#151B1E;--chrome:#1A2225;--surface:#1D2528;--surface2:#232D31;--rail:#182023;--dock-bg:#1A2225;--card-bg:#202A2E;--card-line:#33424A;
--dlg-bg:#1D2528;--dlg-line:#3A4A51;--dlg-hline:#2E3B40;--dlg-foot:#1A2225;--input-bg:#202A2E;--ink:#E4ECEE;--ink2:#B4C3C7;--ink3:#93A5AA;--line:#2E3B40;--line-strong:#4A5B61;
--hover:rgba(140,196,230,.08);--hover-solid:#253136;--sel-bg:#24303A;--sel-shadow:0 0 0 1px #33424A;--accent:#8CC4E6;--accent-hover:#A6D3EE;--accent-ink:#0C2333;--accent-weak:#1B3445;--accent-line:#2F5870;--accent-text:#9DD0F0;--focus:#E4ECEE;
--user-bg:#22313A;--user-line:#2F4350;--pick-bg:#202A2E;--pick-line:#33424A;--tree-line:#46575D;--disabled-bg:#1F282B;--disabled-ink:#6F8186;--disabled-line:#2E3B40;--scrim:rgba(0,0,0,.55);--scrim-light:rgba(0,0,0,.3);
--sh-1:none;--sh-3:0 16px 48px rgba(0,0,0,.6);
--run:#8EC2F5;--run-bg:#16304A;--wait-ink:#F6D683;--wait-bg:#3A2F12;--wait-line:#6A5520;--wait-solid:#E8B83A;--wait-on:#2A1E00;--fail:#F49B92;--fail-ink:#F7B0A9;--fail-bg:#3E2220;--fail-line:#7A3A35;--fail-on:#2A0B08;
--int:#C2B0EC;--int-bg:#2C2741;--done:#8FCFB2;--done-bg:#1B3329;--idle:#B4C3C7;--idle-bg:#263034;--unk:#93A5AA;--live:#8FCFB2;--stale:#E5B45A;--unv-ink:#B4C3C7;--unv-line:#7D9095;--unv-bg:transparent;
--info:#9DD0F0;--info-bg:#1B3445;--info-line:#2F5870;--b1:#8CC4E6;--b2:#4F8DB3;--b3:#A893DA;--b4:#D9A93A;--b5:#6FB596;--b6:#6F8186;--b7:#46575D}
/* B: 柔らかいカード、上部メニューは白、操作は丸ボタン、設定は上タブ */
.card{box-shadow:var(--sh-1)}.card.wait{background:var(--wait-bg)}.card.fail{background:var(--fail-bg)}
.hop{border-radius:999px;padding:0 14px;background:var(--surface2);border-color:var(--line)}
.row{margin-bottom:2px}
.set{grid-template-columns:1fr;grid-template-rows:auto minmax(0,1fr)}
.snav{flex-direction:row;flex-wrap:wrap;gap:6px;border-right:0;border-bottom:1px solid var(--line);padding:10px 18px;background:none}
.snav button{border-radius:999px;border:1px solid var(--line);min-height:30px}
.snav button[aria-current=page]{background:var(--accent);color:var(--accent-ink);border-color:var(--accent);box-shadow:none}
.snav button[aria-current=page]:before{display:none}
.dlg>header{padding:var(--p3) var(--p2) var(--p2) var(--p3)}
`,
  },
  c: {
    name: "案C「ミニマル・アクセント最小」（色は状態だけ）",
    note: "Segoe UI＋Meiryo UI、13px、白黒基調、主ボタンは墨色、色は状態の文字と要対応の面だけ",
    css: String.raw`
:root{--font:"Segoe UI Variable Text","Segoe UI","Meiryo UI","Yu Gothic UI",sans-serif;--mono:"Cascadia Mono","Consolas",monospace;
--fs:13px;--fs-s:12px;--fs-xs:11px;--fs-title:16px;--fs-dlg-title:15px;--fs-mono-s:11.5px;--lh:1.6;--lh-body:1.75;
--p1:4px;--p2:8px;--p3:16px;--p4:20px;--ctl:28px;--bpad:12px;--row-py:6px;--r1:3px;--r2:4px;--r3:6px;--chip-px:0px;--chip-r:0;
--card-py:6px;--card-px:10px;--card-gap:6px;--card-bar:1px;--lw:236px;--rw:320px;--msg-w:720px;--dlg-w:700px;--lbl-w:110px;--nav-w:160px;--ops-gap:0}
[data-theme=light]{--stage:#CFCFCF;--frame-line:#A0A0A0;--os-bg:#F3F3F3;--bg:#FAFAFA;--chrome:#FAFAFA;--surface:#FFFFFF;--surface2:#F6F6F6;--rail:#FAFAFA;--dock-bg:#FAFAFA;--card-bg:#FFFFFF;--card-line:#E6E6E6;
--dlg-bg:#FFFFFF;--dlg-line:#D4D4D4;--dlg-hline:transparent;--dlg-foot:transparent;--input-bg:#FFFFFF;--ink:#111111;--ink2:#4D4D4D;--ink3:#666666;--line:#EAEAEA;--line-strong:#CCCCCC;
--hover:rgba(0,0,0,.04);--hover-solid:#F4F4F4;--sel-bg:#EEEEEE;--sel-shadow:none;--accent:#1F1F1F;--accent-hover:#000000;--accent-ink:#FFFFFF;--accent-weak:#F0F0F0;--accent-line:#CCCCCC;--accent-text:#1F1F1F;--focus:#0067C0;
--user-bg:#F4F4F4;--user-line:#EAEAEA;--pick-bg:transparent;--pick-line:transparent;--tree-line:#D0D0D0;--disabled-bg:#F4F4F4;--disabled-ink:#8C8C8C;--disabled-line:#E6E6E6;--scrim:rgba(0,0,0,.28);--scrim-light:rgba(0,0,0,.1);
--sh-1:none;--sh-3:0 12px 40px rgba(0,0,0,.18);
--run:#0B62C4;--run-bg:transparent;--wait-ink:#7D4A00;--wait-bg:#FFF6DE;--wait-line:#E9CF8C;--wait-solid:#E9A800;--wait-on:#231700;--fail:#C0281B;--fail-ink:#B02418;--fail-bg:#FDEEEC;--fail-line:#EDBDB7;--fail-on:#FFFFFF;
--int:#6343A3;--int-bg:transparent;--done:#4D4D4D;--done-bg:transparent;--idle:#666666;--idle-bg:transparent;--unk:#666666;--live:#2E7D32;--stale:#8A5300;--unv-ink:#4D4D4D;--unv-line:#8C8C8C;--unv-bg:transparent;
--info:#4D4D4D;--info-bg:#F6F6F6;--info-line:#E2E2E2;--b1:#1F1F1F;--b2:#6B6B6B;--b3:#6343A3;--b4:#C98A00;--b5:#2E7D32;--b6:#A8A8A8;--b7:#D4D4D4}
[data-theme=dark]{--stage:#080808;--frame-line:#333;--os-bg:#141414;--bg:#161616;--chrome:#161616;--surface:#1E1E1E;--surface2:#252525;--rail:#161616;--dock-bg:#191919;--card-bg:#1E1E1E;--card-line:#2E2E2E;
--dlg-bg:#1E1E1E;--dlg-line:#3A3A3A;--dlg-hline:transparent;--dlg-foot:transparent;--input-bg:#1E1E1E;--ink:#EDEDED;--ink2:#BDBDBD;--ink3:#9C9C9C;--line:#2A2A2A;--line-strong:#454545;
--hover:rgba(255,255,255,.05);--hover-solid:#262626;--sel-bg:#2A2A2A;--sel-shadow:none;--accent:#EDEDED;--accent-hover:#FFFFFF;--accent-ink:#111111;--accent-weak:#2A2A2A;--accent-line:#454545;--accent-text:#EDEDED;--focus:#6CB8FF;
--user-bg:#252525;--user-line:#2E2E2E;--pick-bg:transparent;--pick-line:transparent;--tree-line:#3A3A3A;--disabled-bg:#222;--disabled-ink:#777;--disabled-line:#2E2E2E;--scrim:rgba(0,0,0,.55);--scrim-light:rgba(0,0,0,.3);
--sh-1:none;--sh-3:0 12px 40px rgba(0,0,0,.6);
--run:#79B8FF;--run-bg:transparent;--wait-ink:#F0C060;--wait-bg:#2E2510;--wait-line:#5E4A18;--wait-solid:#F0BE4F;--wait-on:#231700;--fail:#FF8B80;--fail-ink:#FF9E95;--fail-bg:#2E1A18;--fail-line:#6A302A;--fail-on:#2A0B08;
--int:#BBA2F5;--int-bg:transparent;--done:#BDBDBD;--done-bg:transparent;--idle:#9C9C9C;--idle-bg:transparent;--unk:#9C9C9C;--live:#81C784;--stale:#E5B04F;--unv-ink:#BDBDBD;--unv-line:#777;--unv-bg:transparent;
--info:#BDBDBD;--info-bg:#232323;--info-line:#333;--b1:#EDEDED;--b2:#8A8A8A;--b3:#BBA2F5;--b4:#E3A72F;--b5:#81C784;--b6:#5A5A5A;--b7:#3A3A3A}
/* C: 色は状態の文字と、要対応（承認待ち・失敗）の帯だけ */
.st{background:transparent!important;font-weight:600}.st.unk{border:0}.st.hist{border:0;text-decoration:underline dashed;text-underline-offset:3px}
.card.run,.card.wait,.card.fail{border-left-width:3px}.card.wait{background:var(--wait-bg)}.card.fail{background:var(--fail-bg)}
.left{border-right-color:var(--line)}.row.sel:before{background:var(--ink)}
.hop{border:0;background:none;border-radius:0;padding:0 10px;border-left:1px solid var(--line)}.ops .hop:first-child{border-left:0}
.menubar{border-bottom-color:var(--line)}
.seg button[aria-pressed=true]{background:var(--ink);color:var(--surface)}
.about{background:none;border-left:2px solid var(--line-strong);border-radius:0;padding:2px 0 2px 12px}
.sec{border:0;border-bottom:1px solid var(--line);border-radius:0;padding:12px 0;background:none}
.snav{background:none}.snav button[aria-current=page]:before{background:var(--ink)}
.dlg>header{padding-top:var(--p3)}
.shell{box-shadow:none}.pick{color:var(--ink2)}.pick:hover{border-color:var(--line-strong)}
.tag.ai{background:none}
`,
  },
};

const page = (k) => {
  const v = V[k];
  return String.raw`<!doctype html>
<html lang="ja" data-theme="light">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>AgentDock デザイン比較 ${v.name}</title>
<style>${BASE}${v.css}</style></head>
<body>
${SPRITE}
<div class="mockbar" role="toolbar" aria-label="モックの操作（デザインの一部ではありません）">
  <b>${v.name}</b>
  <span class="g">画面: <button data-screen-btn="main" aria-pressed="true">主画面</button><button data-screen-btn="revert">変更を戻す</button><button data-screen-btn="settings">設定・容量</button><button data-screen-btn="parts">部品見本</button></span>
  <span class="g">幅: <button data-w="full" aria-pressed="true">全幅</button><button data-w="1279">1279px</button><button data-w="959">959px</button></span>
  <span class="g">テーマ: <button data-th="os" aria-pressed="true">OSに従う</button><button data-th="light">ライト</button><button data-th="dark">ダーク</button></span>
  <span class="g">案: <a href="mock-a.html" class="${k === "a" ? "cur" : ""}">A</a> <a href="mock-b.html" class="${k === "b" ? "cur" : ""}">B</a> <a href="mock-c.html" class="${k === "c" ? "cur" : ""}">C</a></span>
  <span class="nt">${v.note}。最上段の灰色の帯は Windows 標準のタイトルバーの見立て（アプリ内の疑似タイトルバーは廃止する案）。</span>
</div>
<div class="stage"><div class="frame" id="frame" data-screen="main">
  <div class="ostitle" aria-hidden="true"><span class="ic"></span>AgentDock<span class="cap"><span>${I("min")}</span><span>${I("max")}</span><span>${I("x")}</span></span></div>
  <nav class="menubar" aria-label="メニュー">${["チャット", "作業", "表示", "設定", "ヘルプ"].map((m) => `<button class="m" aria-haspopup="true">${m}</button>`).join("")}<button class="vtag" title="接続先（実接続／モック）を切り替える">Codex 0.160.0</button></nav>
  <div class="body">${LEFT}${CENTER}${DOCK}<div class="pscrim" data-close="lr"></div></div>
  ${REVERT}${SETTINGS}${PARTS}
</div></div>
<script>
(function(){
  var f=document.getElementById('frame'),root=document.documentElement,mq=matchMedia('(prefers-color-scheme: dark)'),th='os';
  function applyTheme(){root.dataset.theme=th==='os'?(mq.matches?'dark':'light'):th;}
  mq.addEventListener('change',applyTheme);applyTheme();
  function press(sel,btn){document.querySelectorAll(sel).forEach(function(b){b.setAttribute('aria-pressed',b===btn?'true':'false');});}
  function screen(s){f.dataset.screen=s;press('[data-screen-btn]',document.querySelector('[data-screen-btn="'+s+'"]'));}
  document.querySelectorAll('[data-screen-btn]').forEach(function(b){b.onclick=function(){screen(b.dataset.screenBtn);};});
  document.querySelectorAll('[data-w]').forEach(function(b){b.onclick=function(){f.style.width=b.dataset.w==='full'?'100%':b.dataset.w+'px';f.classList.remove('open-l','open-r');press('[data-w]',b);};});
  document.querySelectorAll('[data-th]').forEach(function(b){b.onclick=function(){th=b.dataset.th;applyTheme();press('[data-th]',b);};});
  document.querySelectorAll('[data-toggle]').forEach(function(b){b.onclick=function(){f.classList.toggle('open-'+b.dataset.toggle);};});
  document.querySelectorAll('[data-close]').forEach(function(b){b.onclick=function(){f.classList.remove('open-l','open-r');};});
  document.querySelectorAll('[data-close-dlg]').forEach(function(b){b.onclick=function(){screen('main');};});
  document.querySelectorAll('.menubar .m').forEach(function(b){b.onclick=function(){if(b.textContent==='設定')screen('settings');};});
  // URL の # で初期状態を指定できる（例: mock-a.html#revert,dark,959,r）
  location.hash.slice(1).split(',').forEach(function(p){
    if(['main','revert','settings','parts'].indexOf(p)>=0)screen(p);
    else if(p==='light'||p==='dark'){th=p;applyTheme();press('[data-th]',document.querySelector('[data-th="'+p+'"]'));}
    else if(p==='1279'||p==='959'){f.style.width=p+'px';press('[data-w]',document.querySelector('[data-w="'+p+'"]'));}
    else if(p==='l'||p==='r')f.classList.add('open-'+p);
  });
  addEventListener('keydown',function(e){if(e.key==='Escape'){if(f.dataset.screen!=='main')screen('main');else f.classList.remove('open-l','open-r');}});
})();
</script>
</body></html>`;
};

for (const k of ["a", "b", "c"]) fs.writeFileSync(OUT + `mock-${k}.html`, page(k), "utf8");
console.log("ok");
