// 回答を届けられなかった要求の常設表示（DESIGN_ORG_R1 2.1）。回答は送っていない。解決済みとは別（Codex 側で待ち続けている可能性がある）。
import type { PendingRequest } from "../ipc/types";
import { Icon } from "./Icon";
import { GAP_GUIDE, GAP_GUIDE_PLAIN, gapText } from "./org";

/** 1行の理由。`guide` は次にできること（org＝中断と新しい試行での再送、plain＝中断、false＝出さない）。 */
export function RequestGapLine({ r, guide = "org" }: { r: PendingRequest; guide?: "org" | "plain" | false }) {
  return (
    <div className="req-gap small" role="status" data-req-gap={r.key.requestId}>
      <Icon name="alert" /> {r.detail.summary ? <b>{r.detail.summary}: </b> : null}{gapText(r)}
      {guide ? <span className="muted"> {guide === "org" ? GAP_GUIDE : GAP_GUIDE_PLAIN}</span> : null}
    </div>
  );
}
