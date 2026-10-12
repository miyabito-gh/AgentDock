import React from "react";

/** 画面の描画中の例外を受け、全面が消えないよう理由と「再読み込み／閉じる」を出す。状態や会話は触らない。 */
export class ErrorBoundary extends React.Component<{ children: React.ReactNode }, { error: Error | null; dismissed: boolean }> {
  state = { error: null as Error | null, dismissed: false };
  static getDerivedStateFromError(error: Error) { return { error, dismissed: false }; }
  componentDidCatch(error: Error, info: React.ErrorInfo) { console.error("画面の描画エラー", error, info.componentStack); }
  render() {
    const { error, dismissed } = this.state;
    if (!error) return this.props.children;
    return (
      <div role="alert" style={{ padding: 24, maxWidth: 640, margin: "10vh auto", font: "14px/1.6 sans-serif" }}>
        <h2 style={{ marginTop: 0 }}>画面の表示中にエラーが起きました</h2>
        <p>処理中の会話や保存済みのデータには影響していません。再読み込みすると画面を作り直します。</p>
        <pre style={{ whiteSpace: "pre-wrap", fontSize: 12, opacity: 0.8 }}>{error.message}</pre>
        {dismissed ? <p>閉じました。この画面は表示できないため、再読み込みしてください。</p> : null}
        <button type="button" onClick={() => window.location.reload()}>再読み込み</button>{" "}
        <button type="button" onClick={() => this.setState({ dismissed: true })}>閉じる</button>
      </div>
    );
  }
}
