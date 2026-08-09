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
