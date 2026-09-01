import { useEffect, useRef, useState, type PointerEvent } from 'react'
import { createRoot } from 'react-dom/client'
import { Glass } from '@samasante/liquid-glass'

import './styles.css'

type Task = {
  id: string
  title: string
}

type Workspace = {
  id: string
  title: string
  home?: boolean
  tasks: Task[]
}

const workspaces: Workspace[] = [
  {
    id: 'matou',
    title: 'matou_workspace',
    tasks: [
      { id: 'task-3', title: '新事项 3' },
      { id: 'task-2', title: '新事项 2' },
      { id: 'task-1', title: '新事项' },
      { id: 'default-matou', title: '默认' },
    ],
  },
  {
    id: 'icesword',
    title: 'icesword',
    home: true,
    tasks: [{ id: 'default-home', title: '默认' }],
  },
  {
    id: 'aiwritex',
    title: 'AIWriteX',
    tasks: [{ id: 'default-ai', title: '默认' }],
  },
]

function Chevron({ open = true }: { open?: boolean }) {
  return (
    <svg className={`chevron ${open ? 'is-open' : ''}`} viewBox="0 0 16 16" aria-hidden="true">
      <path d="m4.4 6.15 3.6 3.7 3.6-3.7" />
    </svg>
  )
}

function FolderIcon({ home = false }: { home?: boolean }) {
  return home ? (
    <svg className="nav-icon" viewBox="0 0 18 18" aria-hidden="true">
      <path d="M2.8 8.1 9 2.8l6.2 5.3v6.6H11v-4H7v4H2.8Z" />
    </svg>
  ) : (
    <svg className="nav-icon" viewBox="0 0 18 18" aria-hidden="true">
      <path d="M2.1 4.5h5l1.4 1.6h7.4v8H2.1Z" />
    </svg>
  )
}

function TaskIcon() {
  return (
    <svg className="task-icon" viewBox="0 0 18 18" aria-hidden="true">
      <path d="M3 5.2h12M4.6 9h8.8M6.2 12.8h5.6" />
    </svg>
  )
}

function GlassTask({
  task,
  active,
  onSelect,
}: {
  task: Task
  active: boolean
  onSelect: () => void
}) {
  return (
    <button
      className={`task-row ${active ? 'is-active' : ''}`}
      onClick={onSelect}
      type="button"
    >
      <span className="task-row__content">
        <TaskIcon />
        <span>{task.title}</span>
      </span>
      <span className="task-row__more" aria-hidden="true">•••</span>
    </button>
  )
}

function Sidebar() {
  const [activeTask, setActiveTask] = useState('task-3')

  useEffect(() => {
    const sidebar = document.querySelector<HTMLElement>('.liquid-sidebar')
    if (!sidebar) return

    let targetX = sidebar.clientWidth * 0.45
    let targetY = 110
    let currentX = targetX
    let currentY = targetY
    let animationFrame = 0

    const animate = () => {
      currentX += (targetX - currentX) * 0.12
      currentY += (targetY - currentY) * 0.12
      sidebar.style.setProperty('--glass-x', `${currentX}px`)
      sidebar.style.setProperty('--glass-y', `${currentY}px`)
      animationFrame = requestAnimationFrame(animate)
    }

    const onPointerMove = (event: globalThis.PointerEvent) => {
      const rect = sidebar.getBoundingClientRect()
      targetX = event.clientX - rect.left
      targetY = event.clientY - rect.top
    }

    const onPointerLeave = () => {
      targetX = sidebar.clientWidth * 0.44
      targetY = 118
    }

    sidebar.addEventListener('pointermove', onPointerMove)
    sidebar.addEventListener('pointerleave', onPointerLeave)
    animationFrame = requestAnimationFrame(animate)

    return () => {
      cancelAnimationFrame(animationFrame)
      sidebar.removeEventListener('pointermove', onPointerMove)
      sidebar.removeEventListener('pointerleave', onPointerLeave)
    }
  }, [])

  return (
    <Glass
      aria-label="工作空间与事项"
      className="liquid-sidebar"
      optics={{
        strength: 0.015,
        scaleX: 0.011,
        scaleY: 0.004,
        depth: 0.12,
        dispersion: 0.022,
        frost: 42,
        saturate: 0.78,
        brightness: 0.15,
        specular: 0.62,
        sheenAngle: 270,
        glow: 0.1,
        glowSpread: 0.16,
        glowFalloff: 3.2,
        sheen: 0.7,
        sheenWidth: 1.25,
        sheenFalloff: 3.6,
        curvature: 0.032,
        splay: 0.38,
        bend: 0.48,
        bendWidth: 0.045,
      }}
      radius={28}
      style={{ display: 'flex' }}
    >
      <div className="liquid-sidebar__refraction" aria-hidden="true" />
      <div className="liquid-sidebar__specular" aria-hidden="true" />

      <header className="sidebar-titlebar">
        <div className="traffic-lights" aria-label="窗口控制">
          <i className="traffic-light is-red" />
          <i className="traffic-light is-yellow" />
          <i className="traffic-light is-green" />
        </div>
        <button className="new-workspace" type="button">
          <span className="pencil" aria-hidden="true">⌁</span>
          <strong>新增工作空间</strong>
        </button>
        <button className="sidebar-tool" type="button" aria-label="通知">⌁</button>
      </header>

      <nav className="workspace-list">
        {workspaces.map((workspace) => (
          <section className={`workspace ${workspace.id === 'matou' ? 'is-current' : ''}`} key={workspace.id}>
            <div className="workspace__heading">
              <button className="workspace__main" type="button">
                <Chevron />
                <FolderIcon home={workspace.home} />
                <strong>{workspace.title}</strong>
                {workspace.id === 'icesword' && <span className="workspace-badge">默认</span>}
                {workspace.id === 'matou' && <span className="pin" aria-hidden="true">⌖</span>}
              </button>
              {workspace.id === 'matou' && (
                <div className="workspace__actions" aria-hidden="true">
                  <span>＋</span><span>•••</span>
                </div>
              )}
            </div>
            <div className="workspace__tasks">
              {workspace.tasks.map((task) => (
                <GlassTask
                  active={activeTask === task.id}
                  key={task.id}
                  onSelect={() => setActiveTask(task.id)}
                  task={task}
                />
              ))}
            </div>
          </section>
        ))}
      </nav>

      <div className="sidebar-depth-edge" aria-hidden="true" />
    </Glass>
  )
}

function TerminalCard({ title, active, shell }: { title: string; active?: boolean; shell?: boolean }) {
  return (
    <article className={`terminal-card ${active ? 'is-active' : ''}`}>
      <header>
        <strong>{title}</strong>
        <div className="card-actions" aria-hidden="true"><span>⌘</span><span>⌁</span><span>⊖</span></div>
      </header>
      {!shell && (
        <div className="warning-banner">
          <strong>原 Claude Code 对话已失效</strong>
          <span>当前已切换到 Shell，可继续使用终端</span>
        </div>
      )}
      <div className="terminal-content">
        {shell ? (
          <p><span>(base)</span> <b>➜</b> <em>matou_workspace</em></p>
        ) : (
          <>
            <div className="claude-intro">
              <div className="claude-mark" aria-hidden="true"><i/><i/><i/><i/><i/></div>
              <div>
                <p>Claude Code v2.1.251</p>
                <p>Opus 5 (1M context) with xhigh effort</p>
                <p>Claude Max</p>
                <p>~/matou_workspace</p>
              </div>
            </div>
            <p className="login-notice">⚠ Your login expires in 2 days · run /login to renew</p>
          </>
        )}
        {!shell && <div className="terminal-prompt"><span>›</span><i /></div>}
      </div>
    </article>
  )
}

function MatouDemo() {
  const stageRef = useRef<HTMLDivElement>(null)

  const trackLight = (event: PointerEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect()
    const x = ((event.clientX - rect.left) / rect.width) * 100
    const y = ((event.clientY - rect.top) / rect.height) * 100
    event.currentTarget.style.setProperty('--pointer-x', `${x}%`)
    event.currentTarget.style.setProperty('--pointer-y', `${y}%`)
  }

  return (
    <main ref={stageRef} className="prototype-stage" onPointerMove={trackLight}>
      <div className="ambient ambient--cyan" aria-hidden="true" />
      <div className="ambient ambient--blue" aria-hidden="true" />
      <div className="ambient ambient--warm" aria-hidden="true" />

      <section className="app-window" aria-label="Matou Liquid Glass 网页原型">
        <div className="window-underlay" aria-hidden="true">
          <i className="underlay-orb underlay-orb--one" />
          <i className="underlay-orb underlay-orb--two" />
          <i className="underlay-line" />
        </div>
        <Sidebar />

        <div className="workspace-stage">
          <header className="tabbar">
            <button className="tab is-active" type="button">Claude · /Users/icesword/... <span>×</span></button>
            <button className="tab" type="button">新画布</button>
            <button className="tab" type="button">新画布 2</button>
            <button className="tab" type="button">新画布 3</button>
            <button className="tab-add" type="button">＋</button>
            <div className="toolbar" aria-hidden="true"><span>⌘</span><span>◧</span><span>│</span></div>
          </header>

          <div className="cards-scroller">
            <TerminalCard title="Claude" />
            <TerminalCard active title="测试1" />
            <TerminalCard shell title="测试1" />
          </div>

          <footer className="bottom-bar">
            <div>← <strong>Claude 的子会话</strong> · 9 个会话</div>
            <div><span>DEFAULT</span><b>Opus Plan</b><span>~/matou_workspace</span></div>
          </footer>
        </div>
      </section>

      <div className="prototype-hint" aria-hidden="true">
        移动鼠标观察玻璃高光与折射 · 点击事项切换选中态
      </div>
    </main>
  )
}

const root = document.getElementById('root')
if (!root) throw new Error('Prototype root is missing')

createRoot(root).render(<MatouDemo />)
