import ForceGraph3D, { type ForceGraph3DInstance } from '3d-force-graph'
import { LitElement, html, nothing } from 'lit'
import { customElement, state } from 'lit/decorators.js'
import SpriteText from 'three-spritetext'
import { boundedGraph, graphDegrees, type GraphEdge, type GraphNode } from './graph'
import { DEFAULT_LOCALE, graphCount, messages, pageCount, type Locale } from './i18n'
import { renderMarkdown } from './markdown'
import './styles.css'

type Section = 'overview' | 'pages' | 'sources' | 'knowledge' | 'code'
interface ApiStatus { database: string; revision: string; operation_id: string; read_only: boolean }
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
  @state() accessor message: string = messages[DEFAULT_LOCALE].loadingGraph
  private renderer3d?: ForceGraph3DInstance
  private container?: HTMLDivElement
  private path = '/api/graphs/knowledge'
  private locale: Locale = DEFAULT_LOCALE
  private request = 0

  static properties = { path: { type: String }, locale: { type: String } }
  createRenderRoot() { return this }
  render() {
    const copy = messages[this.locale]
    return html`<div class="graph" role="img" aria-label=${copy.graphLabel}></div><p class="muted" aria-live="polite">${this.message}</p>`
  }
  firstUpdated() {
    this.container = this.querySelector('.graph') ?? undefined
    window.addEventListener('resize', this.resize)
    void this.load()
  }
  updated(changed: Map<string, unknown>) { if (changed.has('path') || changed.has('locale')) void this.load() }
  disconnectedCallback() {
    window.removeEventListener('resize', this.resize)
    this.clear()
    super.disconnectedCallback()
  }

  private readonly resize = () => {
    if (!this.container || !this.renderer3d) return
    this.renderer3d.width(this.container.clientWidth).height(this.container.clientHeight)
  }

  private clear() {
    this.renderer3d?._destructor()
    this.renderer3d = undefined
    this.container?.replaceChildren()
  }

  private async load() {
    if (!this.container) return
    const request = ++this.request
    this.clear()
    this.message = messages[this.locale].loadingGraph
    try {
      const payload = await api<GraphPayload>(this.path)
      if (request !== this.request) return
      if (payload.available === false) {
        this.message = this.locale === 'en' ? (payload.message ?? messages.en.graphUnavailable) : messages['zh-CN'].graphUnavailable
        return
      }
      const visible = boundedGraph(payload.nodes ?? [], payload.edges ?? [])
      const styles = getComputedStyle(document.documentElement)
      const accent = styles.getPropertyValue('--color-primary').trim()
      const labelColor = styles.getPropertyValue('--color-graph-label').trim()
      const nodeColor = styles.getPropertyValue('--color-graph-node').trim()
      const edgeColor3d = styles.getPropertyValue('--color-graph-edge-3d').trim()
      const graphSurface = styles.getPropertyValue('--color-graph-surface').trim()
      const degrees = graphDegrees(visible.nodes, visible.edges)
      const copy = messages[this.locale]
      const nodes = visible.nodes.map((node) => {
        const degree = degrees.get(node.id) ?? 0
        return {
          ...node,
          color: degree >= 5 ? accent : nodeColor,
          val: 0.65 + Math.log2(degree + 1) * 0.35,
        }
      })
      const links = visible.edges.map(edge => ({ ...edge, color: edgeColor3d }))
      const renderer = new ForceGraph3D(this.container, { controlType: 'orbit' })
        .width(this.container.clientWidth)
        .height(this.container.clientHeight)
        .backgroundColor(graphSurface)
        .showNavInfo(false)
        .nodeLabel('label')
        .nodeVal('val')
        .nodeRelSize(1.15)
        .nodeColor('color')
        .nodeOpacity(0.82)
        .nodeResolution(8)
        .nodeThreeObject((node: object) => {
          const label = new SpriteText(String((node as GraphNode).label), 1.7, labelColor)
          label.material.depthWrite = false
          label.center.y = -0.65
          return label
        })
        .nodeThreeObjectExtend(true)
        .linkColor('color')
        .linkOpacity(0.12)
        .linkWidth(0)
        .warmupTicks(100)
        .cooldownTicks(220)
      const charge = renderer.d3Force('charge')
      charge?.strength?.(-15)
      charge?.distanceMax?.(140)
      const frameGraph = () => {
        const bounds = renderer.getGraphBbox()
        const center = {
          x: (bounds.x[0] + bounds.x[1]) / 2,
          y: (bounds.y[0] + bounds.y[1]) / 2,
          z: (bounds.z[0] + bounds.z[1]) / 2,
        }
        const span = Math.max(bounds.x[1] - bounds.x[0], bounds.y[1] - bounds.y[0], bounds.z[1] - bounds.z[0])
        renderer.cameraPosition({ x: center.x, y: center.y, z: center.z + Math.max(100, span * 1.15) }, center, 350)
      }
      renderer.onEngineStop(frameGraph).graphData({ nodes, links })
      this.renderer3d = renderer
      window.setTimeout(() => {
        if (this.renderer3d === renderer) frameGraph()
      }, 800)
      this.message = `${graphCount(this.locale, nodes.length, links.length)}${visible.truncated ? ` · ${copy.limited}` : ''} · ${copy.graph3dControls}`
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
  @state() accessor locale: Locale = localStorage.getItem('lwc-view-locale') === 'zh-CN' ? 'zh-CN' : DEFAULT_LOCALE

  createRenderRoot() { return this }
  connectedCallback() {
    super.connectedCallback()
    document.documentElement.lang = this.locale
    void this.load()
  }

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
  private toggleLocale() {
    this.locale = this.locale === 'en' ? 'zh-CN' : 'en'
    document.documentElement.lang = this.locale
    localStorage.setItem('lwc-view-locale', this.locale)
  }
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
    const copy = messages[this.locale]
    return html`<div class="shell">
      <nav class="rail" aria-label="LWC project sections">
        <span class="wordmark">LW<b>C</b></span>
        <div class="nav-links">${this.nav('overview', copy.overview)}${this.nav('pages', copy.pages)}${this.nav('sources', copy.sources)}${this.nav('knowledge', copy.knowledgeGraph)}${this.nav('code', copy.codeGraph)}</div>
        <button class="language-button" aria-label=${copy.switchLanguageLabel} @click=${this.toggleLocale}>${copy.switchLanguage}</button>
      </nav>
      <main class="workspace">
        <header class="topline">
          <div><h1>${this.heading()}</h1><p class="muted">${copy.readOnlyDescription}</p></div>
          <div class="status-row">${this.status ? html`<span class="status-chip" title=${this.status.revision}>${copy.revision} ${this.status.revision.slice(0, 12)}…</span><span class="status-chip">${copy.readOnly}</span>` : nothing}</div>
        </header>
        ${this.error ? html`<p class="notice error" role="alert">${this.error}</p>` : nothing}
        ${this.content()}
        <footer class="foot-line">${copy.projectLocal} · ${copy.loopbackOnly}</footer>
      </main>
    </div>`
  }

  private nav(section: Section, label: string) {
    return html`<button class="nav-button" aria-current=${this.section === section ? 'page' : nothing} @click=${() => this.select(section)}>${label}</button>`
  }
  private heading() {
    const copy = messages[this.locale]
    return ({ overview: copy.projectCondition, pages: copy.wikiPages, sources: copy.groundedSources, knowledge: copy.knowledgeGraph, code: copy.codeGraph })[this.section]
  }
  private content() {
    const copy = messages[this.locale]
    if (this.section === 'overview') return html`<section class="split overview-grid"><article class="panel"><header class="panel-head"><h2>${copy.store}</h2></header><dl class="metrics"><div class="metric metric-wide"><dt>${copy.database}</dt><dd class="path-value">${this.status?.database ?? copy.loadingDatabase}</dd></div><div class="metric"><dt>${copy.operation}</dt><dd>${this.status?.operation_id ?? '—'}</dd></div><div class="metric"><dt>${copy.pages}</dt><dd>${this.pages.length}</dd></div><div class="metric"><dt>${copy.sources}</dt><dd>${this.sources.length}</dd></div></dl></article><article class="panel panel-dark"><header class="panel-head"><h2>${copy.boundaries}</h2></header><div class="document"><p>${copy.frozenBoundary}</p></div></article></section>`
    if (this.section === 'pages') return html`<section class="split pages-grid"><article class="panel"><header class="panel-head"><h2>${pageCount(this.locale, this.pages.length)}</h2></header><ul class="list">${this.pages.map(page => html`<li><button class="list-button" aria-current=${this.selectedPage?.slug === page.slug ? 'page' : nothing} @click=${() => this.showPage(page.slug)}><strong>${page.title ?? page.slug}</strong><span class="muted">${page.slug}</span></button></li>`)}</ul></article><article class="panel document">${this.selectedPage ? html`<div .innerHTML=${renderMarkdown(this.selectedPage.body ?? '')}></div>` : html`<p class="muted">${copy.selectPage}</p>`}</article></section>`
    if (this.section === 'sources') return html`<section class="panel table-wrap"><table><thead><tr><th>${copy.id}</th><th>${copy.source}</th><th>${copy.bytes}</th></tr></thead><tbody>${this.sources.map(source => html`<tr><td>${source.id}</td><td>${source.title ?? source.origin}</td><td>${source.bytes ?? '—'}</td></tr>`)}</tbody></table></section>`
    const path = this.section === 'knowledge' ? '/api/graphs/knowledge' : '/api/graphs/code'
    return html`<section class="panel graph-panel"><header class="panel-head"><h2>${copy.interactiveMap}</h2></header><lwc-graph .path=${path} .locale=${this.locale}></lwc-graph></section>`
  }
}
