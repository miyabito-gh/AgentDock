import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App.tsx'
import MonitorApp from './MonitorApp.tsx'
import './styles.css'
import { applyStoredAppearance } from './ui/appearance'

// 描画前に、最後に反映した表示設定（テーマ・文字サイズ）の写しを付ける（ちらつき防止）。正はホストの設定で、届いたら App 側が上書きする。
applyStoredAppearance()

// 監視窓は同じ index.html を `?view=monitor` で開く。
const isMonitor = new URLSearchParams(window.location.search).get('view') === 'monitor'

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    {isMonitor ? <MonitorApp /> : <App />}
  </React.StrictMode>,
)
