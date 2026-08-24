import { describe, expect, it } from 'vitest'

import {
  normalizeLayout,
  removeMount,
  splitMount,
  type LayoutMountNode
} from './layout'

describe('Scene layout', () => {
  it('splits the active mount to the right and collapses one-child splits', () => {
    const split = splitMount(mount('a'), 'a', mount('b'), 'horizontal')
    expect(split).toEqual({
      id: expect.any(String),
      kind: 'split',
      direction: 'horizontal',
      children: [mount('a'), mount('b')]
    })
    expect(removeMount(split, 'b')).toEqual(mount('a'))
  })

  it('flattens nested same-direction splits and rejects duplicate mounts', () => {
    expect(normalizeLayout({
      id: 'outer', kind: 'split', direction: 'horizontal', children: [
        mount('a'),
        {
          id: 'inner', kind: 'split', direction: 'horizontal',
          children: [mount('b'), mount('c')]
        }
      ]
    })).toMatchObject({
      direction: 'horizontal',
      children: [mount('a'), mount('b'), mount('c')]
    })

    expect(() => normalizeLayout({
      id: 'duplicate', kind: 'split', direction: 'vertical',
      children: [mount('a'), mount('a')]
    })).toThrow('duplicate mount a')
  })
})

function mount(id: string): LayoutMountNode {
  return { id: `node-${id}`, kind: 'mount', mountId: id }
}
