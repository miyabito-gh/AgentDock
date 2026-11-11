// Visual Editor 本体の遅延読み込み。初めて開くまで、画面のコード（プレビュー制御・Markdown編集）を読み込まない。
// 一度開いたあとは、閉じても読み込み済みのまま（enabled=false の間は何も描かず、プレビューのWebViewも作らない）。
import { lazy, Suspense, useState, type ComponentProps } from "react";
import "./visualShell.css";

const Workspace = lazy(() => import("./VisualWorkspace").then((m) => ({ default: m.VisualWorkspace })));
export type VisualGateProps = ComponentProps<typeof Workspace>;

export function VisualGate(props: VisualGateProps) {
  const [loaded, setLoaded] = useState(false);
  if (props.enabled && !loaded) setLoaded(true);
  return loaded ? <Suspense fallback={null}><Workspace {...props} /></Suspense> : null;
}
