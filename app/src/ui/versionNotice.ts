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
