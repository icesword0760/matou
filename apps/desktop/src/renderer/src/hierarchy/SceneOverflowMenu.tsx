import type { SceneView } from './hierarchy-types'

export function SceneOverflowMenu({ scenes, onSelect }: {
  scenes: SceneView[]; onSelect(scene: SceneView): void
}) {
  return <div role="menu" className="scene-overflow-menu">
    {scenes.map((scene) => <button key={scene.id} role="menuitem" onClick={(event) => {
      event.currentTarget.scrollIntoView({ inline: 'center', block: 'nearest' })
      onSelect(scene)
    }}>{scene.name}</button>)}
  </div>
}
