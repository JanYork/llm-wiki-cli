import { describe, expect, it } from 'vitest'
import { boundedGraph, graphDegrees } from './graph'

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

  it('keeps parallel code relationships between the same nodes', () => {
    const graph = boundedGraph(
      [{ id: 'source', label: 'source' }, { id: 'target', label: 'target' }],
      [
        { id: 'imports', source: 'source', target: 'target' },
        { id: 'calls', source: 'source', target: 'target' },
      ],
    )

    expect(graph.edges).toHaveLength(2)
  })

  it('counts connections so hubs can be emphasized without oversized nodes', () => {
    const degrees = graphDegrees(
      ['hub', 'a', 'b', 'c'].map(id => ({ id, label: id })),
      ['a', 'b', 'c'].map(target => ({ id: `hub-${target}`, source: 'hub', target })),
    )

    expect(degrees.get('hub')).toBe(3)
    expect(degrees.get('a')).toBe(1)
  })
})
