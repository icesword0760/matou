import { createRoot } from 'react-dom/client'

import '@xterm/xterm/css/xterm.css'
import './terminal/terminal.css'

import { App } from './App'

const root = document.getElementById('root')
if (!root) {
  throw new Error('Renderer root element is missing')
}

createRoot(root).render(<App />)
