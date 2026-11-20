import assert from 'node:assert/strict'

export function selectionPoint(value) {
  if (!value) return null
  // The frozen range ends at the footer span's child boundary. Native uses
  // the start of that span's text, with the same logical insertion position.
  const atFooterStart = value.id === 'footer:range' && value.node === 'SPAN' && value.utf16 === 0
  return [value.id, atFooterStart ? 0 : value.column]
}

export function historySelectionFixReport(differences) {
  const targets = [
    ['word-and-line', 2, 2, '三击末行'],
    ['footer-and-status', 2, 3, '加载时跨页脚选区'],
    ['append-with-page-refresh', 4, 3, '追加后显式刷新正文选区'],
  ]
  const counts = targets.map(([name, step, before, label]) => {
    const after = differences.filter(d => d.case === name && d.step === step
      && d.field.startsWith('selection.')).length
    assert.equal(after, 0, `${name}/${step} selection differences`)
    return `| ${label} | ${before} | ${after} |`
  })
  return `## 本轮修复复验

差异按用例、步骤和字段计数。原报告共 94 处，本轮复跑共 ${differences.length} 处。

| 行为 | 修复前 | 修复后 |
| --- | ---: | ---: |
${counts.join('\n')}

- 三击最后一条正文行时，终点落在下一块开头，复制文本含一个尾随 LF。无下边界提示时，下一块是页脚。冻结的页脚元素起点和原生页脚文字起点按同一位置比较，原始端点仍保存在两侧 JSON。
- 页脚选区在正文进入加载状态并回到就绪时保留。页脚范围、缺口和 Esc 按各自字段定位，两端仍需存在且文字相同。
- 正文直接刷新时，冻结组件按归档游标的 sequence 和 lineIndex 为行设键。原生按同一游标定位两端，并保留原字节列和选择方向。显示行号不变时端点留在原行，前方增减行或边界提示导致行号移动时，端点随原记录平移。同一端点的文字需相同。经 loading 切换结果时，正文选区清空。
- 保留原有 78 处小数像素差异，以及结果页切换后的 8 处空选区端点表示差异。空选区的复制文本在两侧都为空。
`
}
