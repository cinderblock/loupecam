import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { TargetPage } from './components/Target.tsx'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {location.pathname.replace(/\/+$/, '') === '/target' ? <TargetPage /> : <App />}
  </StrictMode>,
)
