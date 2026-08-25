import type { SceneView } from './hierarchy-types'

export function SceneOverflowMenu({ scenes, hasUnread = () => false, onSelect }: {
  scenes: SceneView[]; hasUnread?(sceneId: string): boolean; onSelect(scene: SceneView): void
}) {
  return <div role="menu" className="scene-overflow-menu">
    {scenes.map((scene) => <button key={scene.id} role="menuitem" onClick={(event) => {
      event.currentTarget.scrollIntoView({ inline: 'center', block: 'nearest' })
      onSelect(scene)
    }}>
      {hasUnread(scene.id) && <span className="tab-overflow-dot" aria-hidden="true" />}
      <span className="tab-overflow-title">{scene.name}</span>
    </button>)}
  </div>
}
