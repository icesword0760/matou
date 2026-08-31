import { mkdir, writeFile } from 'node:fs/promises'
import os from 'node:os'
import { basename, join } from 'node:path'

const ZSH_ENV = `if [[ -n "$MATOU_ORIGINAL_ZDOTDIR" && "$MATOU_ORIGINAL_ZDOTDIR" != "$ZDOTDIR" && -r "$MATOU_ORIGINAL_ZDOTDIR/.zshenv" ]]; then
  source "$MATOU_ORIGINAL_ZDOTDIR/.zshenv"
fi
`

const ZSH_RC = `if [[ -n "$MATOU_ORIGINAL_ZDOTDIR" && "$MATOU_ORIGINAL_ZDOTDIR" != "$ZDOTDIR" && -r "$MATOU_ORIGINAL_ZDOTDIR/.zshrc" ]]; then
  source "$MATOU_ORIGINAL_ZDOTDIR/.zshrc"
fi

_matou_preexec() {
  local _matou_command_base64
  _matou_command_base64=$(printf '%s' "$1" | base64 | tr -d '\\n')
  printf '\\033]633;E;%s\\007' "$_matou_command_base64"
  printf '\\033]133;C\\007'
}

_matou_precmd() {
  local _matou_exit_code=$?
  printf '\\033]133;D;%d\\007\\033]133;A\\007' "$_matou_exit_code"
}

typeset -ga preexec_functions precmd_functions
preexec_functions=(_matou_preexec \${preexec_functions:#_matou_preexec})
precmd_functions=(_matou_precmd \${precmd_functions:#_matou_precmd})
`

export async function shellIntegrationEnvironment(
  dataRoot: string,
  executable: string
): Promise<Record<string, string>> {
  if (basename(executable) !== 'zsh') return {}
  const directory = join(dataRoot, 'shell-integration', 'zsh')
  await mkdir(directory, { recursive: true, mode: 0o700 })
  await Promise.all([
    writeFile(join(directory, '.zshenv'), ZSH_ENV, { encoding: 'utf8', mode: 0o600 }),
    writeFile(join(directory, '.zshrc'), ZSH_RC, { encoding: 'utf8', mode: 0o600 })
  ])
  return {
    ZDOTDIR: directory,
    MATOU_ORIGINAL_ZDOTDIR: process.env.ZDOTDIR ?? process.env.HOME ?? os.homedir()
  }
}
