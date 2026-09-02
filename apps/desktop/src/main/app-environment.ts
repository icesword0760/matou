export function resolvePackagedApplication(input: {
  electronPackaged: boolean
  developmentBundle?: string | undefined
}): boolean {
  return input.electronPackaged && input.developmentBundle !== '1'
}
