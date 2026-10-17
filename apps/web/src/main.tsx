import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { TargetPage } from './components/Target.tsx'
import { CastPage } from './components/Cast.tsx'

const page = location.pathname.replace(/\/+$/, '')

createRoot(document.getElementById('root')!).render(
  <StrictMode>{page === '/target' ? <TargetPage /> : page === '/cast' ? <CastPage /> : <App />}</StrictMode>,
)
