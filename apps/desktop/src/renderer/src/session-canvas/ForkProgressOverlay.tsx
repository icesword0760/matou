import type { ForkProgress, ForkStage } from '@matou/domain'

export function ForkProgressOverlay({ progress }: { progress: ForkProgress }) {
  const completed = Math.max(0, Math.min(progress.completedSteps, progress.totalSteps))
  const total = Math.max(1, progress.totalSteps)
  const percent = Math.round((completed / total) * 100)
  return <div className="fork-progress-overlay" role="status"
    aria-label={`正在创建分支：${stageLabel(progress.stage)}`}
    onPointerDown={(event) => event.stopPropagation()}>
    <div className="fork-progress-overlay__content">
      <span className="fork-progress-overlay__spinner" aria-hidden="true" />
      <strong>{stageLabel(progress.stage)}</strong>
      <p>{stageDescription(progress.stage)}</p>
      <div className="fork-progress-overlay__bar" role="progressbar"
        aria-valuemin={0} aria-valuemax={total} aria-valuenow={completed}>
        <span style={{ width: `${percent}%` }} />
      </div>
      <small>第 {completed + (completed < total ? 1 : 0)} / {total} 阶段</small>
    </div>
  </div>
}

export function activeForkProgress(progress: ForkProgress | undefined): ForkProgress | undefined {
  if (!progress || progress.stage === 'succeeded' || progress.stage === 'failed') return undefined
  return progress
}

function stageLabel(stage: ForkStage): string {
  switch (stage) {
    case 'queued': return '等待创建分支'
    case 'creating-worktree': return '正在创建独立工作目录'
    case 'applying-setup': return '正在准备分支环境'
    case 'binding-session': return '正在绑定分支会话'
    case 'restoring-provider': return '正在恢复智能体会话'
    case 'starting-window': return '正在打开分支窗口'
    case 'succeeded': return '分支已就绪'
    case 'failed': return '分支创建失败'
  }
}

function stageDescription(stage: ForkStage): string {
  switch (stage) {
    case 'queued': return '前面的分支任务完成后会自动继续'
    case 'creating-worktree': return '正在隔离文件修改，其他会话可继续工作'
    case 'applying-setup': return '正在执行此工作空间需要的准备步骤'
    case 'binding-session': return '正在把新会话连接到独立工作目录'
    case 'restoring-provider': return '正在续接原会话上下文并启动分支任务'
    case 'starting-window': return '分支窗口即将可以输入和查看输出'
    case 'succeeded': return '现在可以继续分支任务'
    case 'failed': return '请查看失败原因后重试'
  }
}
