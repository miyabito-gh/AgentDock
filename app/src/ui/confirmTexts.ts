// アプリ内の確認ダイアログ（appConfirm）の文言。純粋。実行: node --test src/ui/confirmTexts.test.ts （tscの対象外）

export interface ConfirmSpec { title: string; body: string; okLabel: string; cancelLabel?: string; danger?: boolean }

/** 「開く」の確認。実行形式は、何が起きるか（プログラムが動く）と、信頼できるものだけ開くことを書く。 */
export function openFileConfirm(o: { executable: boolean; outsideProject: boolean; path: string }): ConfirmSpec {
  const lines: string[] = [];
  if (o.executable) {
    lines.push("実行形式のファイルです。開くと、このパソコンでプログラムが実行されることがあります。");
    lines.push("自分で用意した、または内容を信頼できる実行形式だけ開いてください。");
  }
  if (o.outsideProject) lines.push("このチャットの作業フォルダの外にあるファイルです。");
  if (lines.length === 0) lines.push("関連付けられたアプリでこのファイルを開きます。");
  lines.push("", o.path);
  return { title: o.executable ? "実行形式のファイルを開きますか" : "このファイルを開きますか", body: lines.join("\n"), okLabel: "開く", cancelLabel: "やめる", danger: o.executable };
}

/** MCP・Plugins の管理コマンド（codex.exe）を実行する前の確認。 */
export function extensionRunConfirm(what: string, detail: string): ConfirmSpec {
  return {
    title: "codex.exe のコマンドを実行しますか",
    body: `${what}\n\ncodex.exe のコマンドを実行します（${detail}）。~/.codex の設定が変わる可能性があります。\n実行の前後に設定を読み直して照合し、結果を表示します。自動では元に戻しません。`,
    okLabel: "実行する", cancelLabel: "やめる",
  };
}
