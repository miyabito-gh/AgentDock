import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App.tsx'
import MonitorApp from './MonitorApp.tsx'
import PetApp from './PetApp.tsx'
import './styles.css'
import { ErrorBoundary } from './ui/ErrorBoundary'
import { applyStoredAppearance } from './ui/appearance'

// 描画前に、最後に反映した表示設定（テーマ・文字サイズ）の写しを付ける（ちらつき防止）。正はホストの設定で、届いたら App 側が上書きする。
applyStoredAppearance()

// 監視窓・ペット窓は同じ index.html を `?view=monitor` / `?view=pet` で開く。
const view = new URLSearchParams(window.location.search).get('view')
const isMonitor = view === 'monitor'
const isPet = view === 'pet'
// ペット窓は背景を透明にする（透明部分は窓ごと下の画面が見える）。
if (isPet) document.documentElement.classList.add('pet-view')

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <ErrorBoundary>{isPet ? <PetApp /> : isMonitor ? <MonitorApp /> : <App />}</ErrorBoundary>
  </React.StrictMode>,
)
