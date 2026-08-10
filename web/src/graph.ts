export interface GraphNode {
  id: string
  label: string
  [key: string]: unknown
}

export interface GraphEdge {
  id: string
  source: string
  target: string
  [key: string]: unknown
}

export interface VisibleGraph {
  nodes: GraphNode[]
  edges: GraphEdge[]
  truncated: boolean
}

export function wordGraphPath(query: string, offset: number): string {
  if (!query.trim()) return ''
  const parameters = new URLSearchParams({
    query: query.trim(),
    limit: '25',
    term_limit: '30',
    offset: String(Math.max(0, offset)),
  })
  return `/api/graphs/words?${parameters}`
}

export function boundedGraph(
  nodes: GraphNode[],
  edges: GraphEdge[],
  nodeLimit = 1000,
  edgeLimit = 5000,
): VisibleGraph {
  const visibleNodes = nodes.slice(0, nodeLimit)
  const ids = new Set(visibleNodes.map(node => node.id))
  const visibleEdges = edges
    .filter(edge => ids.has(edge.source) && ids.has(edge.target))
    .slice(0, edgeLimit)

  return {
    nodes: visibleNodes,
    edges: visibleEdges,
    truncated: visibleNodes.length < nodes.length || visibleEdges.length < edges.length,
  }
}

export function graphDegrees(nodes: GraphNode[], edges: GraphEdge[]): Map<string, number> {
  const degrees = new Map(nodes.map(node => [node.id, 0]))
  for (const edge of edges) {
    degrees.set(edge.source, (degrees.get(edge.source) ?? 0) + 1)
    degrees.set(edge.target, (degrees.get(edge.target) ?? 0) + 1)
  }
  return degrees
}
