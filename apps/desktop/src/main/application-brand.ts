interface ApplicationNameTarget {
  setName(name: string): void
}

export function applyApplicationBrand(app: ApplicationNameTarget, displayName: string): void {
  app.setName(displayName)
}
