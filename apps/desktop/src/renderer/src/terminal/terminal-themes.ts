import type { ITheme } from '@xterm/xterm'

export type TerminalThemeKey = 'dark' | 'light'

export const DEFAULT_TERMINAL_THEME: TerminalThemeKey = 'light'

export const TERMINAL_THEMES: Record<TerminalThemeKey, ITheme> = {
  dark: {
    background: '#1B1B1B', foreground: '#FAFAFA', cursor: '#FF7809', cursorAccent: '#0d1117',
    selectionBackground: '#264f78', black: '#484f58', red: '#ff7b72', green: '#3fb950',
    yellow: '#d29922', blue: '#58a6ff', magenta: '#bc8cff', cyan: '#39d353', white: '#b1bac4',
    brightBlack: '#6e7681', brightRed: '#ffa198', brightGreen: '#56d364', brightYellow: '#e3b341',
    brightBlue: '#79c0ff', brightMagenta: '#d2a8ff', brightCyan: '#56d364', brightWhite: '#f0f6fc'
  },
  light: {
    background: '#FCFCFD', foreground: '#2F3547', cursor: '#FF7809', cursorAccent: '#FFFFFF',
    selectionBackground: '#bfceff', black: '#383a42', red: '#e45649', green: '#50a14f',
    yellow: '#c18401', blue: '#4078f2', magenta: '#a626a4', cyan: '#0184bc', white: '#a0a1a7',
    brightBlack: '#4f525e', brightRed: '#e06c75', brightGreen: '#98c379', brightYellow: '#e5c07b',
    brightBlue: '#61afef', brightMagenta: '#c678dd', brightCyan: '#56b6c2', brightWhite: '#383a42'
  }
}
