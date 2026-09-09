import { createRoot } from 'react-dom/client'

import '@xterm/xterm/css/xterm.css'
import './terminal/terminal.css'
import './hierarchy/hierarchy.css'

import { App } from './App'
import { LocaleProvider } from './i18n/LocaleProvider'
import { RuntimeProvider } from './runtime/RuntimeProvider'

const root = document.getElementById('root')
if (!root) {
  throw new Error('Renderer root element is missing')
}

createRoot(root).render(<LocaleProvider><RuntimeProvider><App /></RuntimeProvider></LocaleProvider>)
