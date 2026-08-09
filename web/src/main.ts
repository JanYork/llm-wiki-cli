import Graph from 'graphology'
import forceAtlas2 from 'graphology-layout-forceatlas2'
import { LitElement, html, nothing } from 'lit'
import { customElement, state } from 'lit/decorators.js'
import Sigma from 'sigma'
import { boundedGraph, type GraphEdge, type GraphNode } from './graph'
import { renderMarkdown } from './markdown'
import './styles.css'

type Section = 'overview' | 'pages' | 'sources' | 'knowledge' | 'code'
interface ApiStatus { database: string; revision: number; operation_id: string; read_only: boolean }
interface Page { slug: string; title?: string; updated_at?: string; body?: string }
interface Source { id: number; origin: string; title?: string; bytes?: number }
interface GraphPayload { nodes: GraphNode[]; edges: GraphEdge[]; available?: boolean; message?: string }

async function api<T>(path: string): Promise<T> {
  const response = await fetch(path, { headers: { Accept: 'application/json' } })
  if (!response.ok)
    throw new Error(`Request failed (${response.status})`)
  return response.json() as Promise<T>
}

@customElement('lwc-graph')
class LwcGraph extends LitElement {
  @state() accessor message = 'Loading graph…'
  private renderer?: Sigma
  private container?: HTMLDivElement
  private path = '/api/graphs/knowledge'

  static properties = { path: { type: String } }
  createRenderRoot() { return this }
  render() { return html`<div class="graph" role="img" aria-label="Interactive graph visualization"></div><p class="muted" aria-live="polite">${this.message}</p>` }
  firstUpdated() { this.container = this.querySelector('.graph') ?? undefined; void this.load() }
  updated(changed: Map<string, unknown>) { if (changed.has('path')) void this.load() }
  disconnectedCallback() { this.renderer?.kill(); super.disconnectedCallback() }

  private async load() {
    if (!this.container) return
    this.renderer?.kill()
    try {
      const payload = await api<GraphPayload>(this.path)
      if (payload.available === false) {
        this.message = payload.message ?? 'Graph is not available.'
        return
      }
      const visible = boundedGraph(payload.nodes ?? [], payload.edges ?? [])
      const graph = new Graph()
      const accent = getComputedStyle(document.documentElement).getPropertyValue('--color-accent').trim()
      for (const [index, node] of visible.nodes.entries())
        graph.addNode(node.id, { label: node.label, size: 4, color: accent, x: Math.cos(index), y: Math.sin(index) })
      for (const edge of visible.edges)
        if (!graph.hasEdge(edge.id)) graph.addEdgeWithKey(edge.id, edge.source, edge.target, { size: 1 })
      if (graph.order > 1) forceAtlas2.assign(graph, { iterations: Math.min(100, graph.order * 2) })
      this.renderer = new Sigma(graph, this.container, { renderEdgeLabels: false })
      this.message = `${graph.order} nodes · ${graph.size} edges${visible.truncated ? ' · limited to 1,000 nodes / 5,000 edges' : ''}`
    } catch (error) {
      this.message = error instanceof Error ? error.message : String(error)
    }
  }
}

@customElement('lwc-view')
class LwcView extends LitElement {
  @state() accessor section: Section = 'overview'
  @state() accessor status: ApiStatus | undefined = undefined
  @state() accessor pages: Page[] = []
  @state() accessor sources: Source[] = []
  @state() accessor selectedPage: Page | undefined = undefined
  @state() accessor error = ''

  createRenderRoot() { return this }
  connectedCallback() { super.connectedCallback(); void this.load() }

  private async load() {
    try {
      const [status, pageResponse, sourceResponse] = await Promise.all([
        api<ApiStatus>('/api/status'),
        api<{ pages?: Page[]; items?: Page[] }>('/api/pages?limit=1000'),
        api<{ sources?: Source[]; items?: Source[] }>('/api/sources?limit=1000'),
      ])
      this.status = status
      this.pages = pageResponse.pages ?? pageResponse.items ?? []
      this.sources = sourceResponse.sources ?? sourceResponse.items ?? []
    } catch (error) {
      this.error = error instanceof Error ? error.message : String(error)
    }
  }

  private select(section: Section) { this.section = section }
  private async showPage(slug: string) {
    this.section = 'pages'
    try {
      const response = await api<{ page: Page }>(`/api/pages/${encodeURIComponent(slug)}`)
      this.selectedPage = response.page
    } catch (error) {
      this.error = error instanceof Error ? error.message : String(error)
    }
  }

  render() {
    return html`<div class="shell">
      <nav class="rail" aria-label="LWC project sections">
        <span class="wordmark">LWC</span>
        ${this.nav('overview', 'Overview')}${this.nav('pages', 'Pages')}${this.nav('sources', 'Sources')}${this.nav('knowledge', 'Knowledge graph')}${this.nav('code', 'Code graph')}
      </nav>
      <main class="workspace">
        <header class="topline">
          <div><h1>${this.heading()}</h1><p class="muted">Read-only project inspection. No migration, refresh, or write is triggered.</p></div>
          <div class="status-row">${this.status ? html`<span class="status-chip">revision ${this.status.revision}</span><span class="status-chip">read only</span>` : nothing}</div>
        </header>
        ${this.error ? html`<p class="notice error" role="alert">${this.error}</p>` : nothing}
        ${this.content()}
        <footer class="foot-line">Project-local · loopback only · ${this.status?.database ?? 'loading database'}</footer>
      </main>
    </div>`
  }

  private nav(section: Section, label: string) {
    return html`<button class="nav-button" aria-current=${this.section === section ? 'page' : nothing} @click=${() => this.select(section)}>${label}</button>`
  }
  private heading() { return ({ overview: 'Project condition', pages: 'Wiki pages', sources: 'Grounded sources', knowledge: 'Knowledge graph', code: 'Code graph' })[this.section] }
  private content() {
    if (this.section === 'overview') return html`<section class="split"><article class="panel"><header class="panel-head"><h2>Store</h2></header><table><tbody><tr><th>Database</th><td>${this.status?.database ?? 'Loading…'}</td></tr><tr><th>Operation</th><td>${this.status?.operation_id ?? '—'}</td></tr><tr><th>Pages</th><td>${this.pages.length}</td></tr><tr><th>Sources</th><td>${this.sources.length}</td></tr></tbody></table></article><article class="panel"><header class="panel-head"><h2>Boundaries</h2></header><div class="document"><p>Historical revisions remain frozen. This server exposes GET and HEAD only, binds to 127.0.0.1, and never starts graph construction.</p></div></article></section>`
    if (this.section === 'pages') return html`<section class="split"><article class="panel"><header class="panel-head"><h2>${this.pages.length} pages</h2></header><ul class="list">${this.pages.map(page => html`<li><button class="list-button" @click=${() => this.showPage(page.slug)}><strong>${page.title ?? page.slug}</strong><span class="muted">${page.slug}</span></button></li>`)}</ul></article><article class="panel document">${this.selectedPage ? html`<div .innerHTML=${renderMarkdown(this.selectedPage.body ?? '')}></div>` : html`<p class="muted">Select a page to render its Markdown.</p>`}</article></section>`
    if (this.section === 'sources') return html`<section class="panel table-wrap"><table><thead><tr><th>ID</th><th>Source</th><th>Bytes</th></tr></thead><tbody>${this.sources.map(source => html`<tr><td>${source.id}</td><td>${source.title ?? source.origin}</td><td>${source.bytes ?? '—'}</td></tr>`)}</tbody></table></section>`
    const path = this.section === 'knowledge' ? '/api/graphs/knowledge' : '/api/graphs/code'
    return html`<section class="panel"><header class="panel-head"><h2>${this.heading()}</h2></header><lwc-graph .path=${path}></lwc-graph></section>`
  }
}
