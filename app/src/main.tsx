import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App.tsx'
import MonitorApp from './MonitorApp.tsx'
import './styles.css'

// 監視窓は同じ index.html を `?view=monitor` で開く。
const isMonitor = new URLSearchParams(window.location.search).get('view') === 'monitor'

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    {isMonitor ? <MonitorApp /> : <App />}
  </React.StrictMode>,
)
