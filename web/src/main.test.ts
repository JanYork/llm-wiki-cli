import { afterEach, describe, expect, it, vi } from 'vitest'
import './main'

interface TestView extends HTMLElement {
  section: string
  selectedPage?: Record<string, unknown>
  updateComplete: Promise<boolean>
}

afterEach(() => {
  document.body.replaceChildren()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

describe('page projection', () => {
  it('presents the canonical title, summary, metadata, and an on-demand outline', async () => {
    vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => undefined })
    vi.stubGlobal('fetch', vi.fn(async (input: string | URL | Request) => {
      const path = String(input)
      const body = path.includes('/api/status')
        ? { database: '/tmp/wiki.db', revision: 'revision-1', operation_id: 'op-1', read_only: true }
        : path.includes('/api/pages')
          ? { pages: [] }
          : { sources: [] }
      return new Response(JSON.stringify(body), {
        headers: { 'content-type': 'application/json' },
        status: 200,
      })
    }))
    const view = document.createElement('lwc-view') as TestView
    document.body.append(view)
    await view.updateComplete

    view.section = 'pages'
    view.selectedPage = {
      slug: 'elastic-contract',
      title: 'Elastic Wiki contract',
      summary: 'A flexible contract for readable pages.',
      kind: 'concept',
      provenance: ['agent-observed'],
      source_ids: [7],
      created_at: '2026-08-28T00:00:00Z',
      updated_at: '2026-08-29T00:00:00Z',
      body: '# Elastic Wiki contract\n\n## Purpose\n\nA\n\n## Shape\n\nB\n\n## Checks\n\nC',
    }
    await view.updateComplete

    expect(view.querySelector('.topline h1')?.textContent).toBe('Elastic Wiki contract')
    expect(view.querySelectorAll('h1')).toHaveLength(1)
    expect(view.querySelector('.page-summary')?.textContent).toContain('A flexible contract')
    expect(view.querySelector('.page-metadata')?.textContent).toContain('concept')
    expect(view.querySelector('.page-metadata')?.textContent).toContain('agent-observed')
    expect(view.querySelector('.page-metadata')?.textContent).toContain('7')
    expect(view.querySelector('.page-toc')?.textContent).toContain('Purpose')
  })
})
