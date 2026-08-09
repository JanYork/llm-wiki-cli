import { describe, expect, it } from 'vitest'
import { boundedGraph } from './graph'

describe('boundedGraph', () => {
  it('caps nodes and removes edges whose endpoints are not visible', () => {
    const graph = boundedGraph(
      Array.from({ length: 1002 }, (_, index) => ({ id: `${index}`, label: `${index}` })),
      [{ id: 'keep', source: '0', target: '1' }, { id: 'drop', source: '0', target: '1001' }],
    )

    expect(graph.nodes).toHaveLength(1000)
    expect(graph.edges).toEqual([{ id: 'keep', source: '0', target: '1' }])
    expect(graph.truncated).toBe(true)
  })
})
