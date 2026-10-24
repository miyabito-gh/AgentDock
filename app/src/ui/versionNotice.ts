// 版の注意（版の不一致・版不明の全体バナー）の表示判定。閉じた版を `AppSettings.dismissedVersionNotice` に1項目だけ保存する。
// 接続断・起動失敗のバナーは閉じられない（常に出す）。画面上部の接続先表示・設定・ヘルプの根拠表示は、この判定の対象外（消さない）。

export const UNKNOWN_VERSION_KEY = "unknown";

type Conn = { kind: string };
type Ver = { kind: "match"; version: string } | { kind: "mismatch"; expected: string; actual: string } | { kind: "unknown"; message: string };

/** 閉じられる版の注意の識別子。版が一致（注意なし）なら null。版不明は専用の値。 */
export function versionNoticeKey(version: Ver): string | null {
  if (version.kind === "mismatch") return `mismatch:${version.actual}`;
  if (version.kind === "unknown") return UNKNOWN_VERSION_KEY;
  return null;
}

/** 版の注意を出すか。接続断・起動失敗の最中は、閉じた記録に関わらず別バナー側で扱う（ここでは版の注意だけ判定）。 */
export function showVersionNotice(version: Ver, dismissed: string | null | undefined): boolean {
  const key = versionNoticeKey(version);
  return key !== null && key !== dismissed;
}

/** 接続の状態バナー（閉じられない）を出すか。 */
export function isConnectionProblem(c: Conn): boolean {
  return c.kind === "launchFailed" || c.kind === "disconnected";
}

/** 全体バナーの文言と見せ方。起動失敗は直し方、版の注意は事実と影響を書く。非緊急の版の注意は role="status"。 */
export function globalBannerView(c: { kind: string; message?: string | null }, version: Ver): { text: string; cls: "warn" | "err"; role: "alert" | "status" } | null {
  if (c.kind === "launchFailed") {
    return { text: `Codex を起動できませんでした（${c.message ?? "理由は取得できません"}）。codex.exe の場所が正しいか確認するか、AgentDock を再起動してください。`, cls: "err", role: "alert" };
  }
  if (c.kind === "disconnected") {
    return { text: `Codex との接続が切れています${c.message ? `（${c.message}）` : ""}。表示は最後に受信した内容のままで、完了や失敗を意味しません。`, cls: "err", role: "alert" };
  }
  if (version.kind === "mismatch") {
    return { text: `Codex の版が対象と異なります（動作は続けますが、一部が正しく動かない可能性があります）。使用中 ${version.actual}／対象 ${version.expected}`, cls: "warn", role: "status" };
  }
  if (version.kind === "unknown") {
    return { text: `Codex の版を確認できません（動作は続けますが、一部が正しく動かない可能性があります）。${version.message}`, cls: "warn", role: "status" };
  }
  return null;
}
