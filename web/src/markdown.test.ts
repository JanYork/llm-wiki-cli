import { describe, expect, it } from 'vitest'
import { projectMarkdown, renderMarkdown } from './markdown'

describe('renderMarkdown', () => {
  it('renders useful markdown and removes executable markup', () => {
    const result = renderMarkdown('# Safe\n\n[bad](javascript:alert(1))\n\n<img src="https://example.com/tracker.png">\n\n<span style="display:none">visible</span>')

    expect(result).toContain('<h1>Safe</h1>')
    expect(result).not.toContain('javascript:')
    expect(result).not.toContain('https://example.com')
    expect(result).not.toContain('style=')
    expect(result).toContain('visible')
  })

  it('removes only a leading H1 that matches the canonical title', () => {
    const matching = projectMarkdown('# Canonical title\n\nBody', 'Canonical title')
    const mismatching = projectMarkdown('# Historical title\n\nBody', 'Canonical title')
    const compatibilityVariant = projectMarkdown('# ＡPI\n\nBody', 'API')

    expect(matching.html).not.toContain('<h1')
    expect(matching.html).toContain('<p>Body</p>')
    expect(mismatching.html).toContain('<h1>Historical title</h1>')
    expect(compatibilityVariant.html).toContain('<h1>ＡPI</h1>')
  })

  it('adds a table of contents only for three or more sections', () => {
    const short = projectMarkdown('## One\n\nA\n\n## Two\n\nB', 'Page')
    const structured = projectMarkdown(
      '## Context\n\nA\n\n### Detail\n\nB\n\n## Context\n\nC',
      'Page',
    )

    expect(short.toc).toEqual([])
    expect(structured.toc).toEqual([
      { depth: 2, id: 'context', text: 'Context' },
      { depth: 3, id: 'detail', text: 'Detail' },
      { depth: 2, id: 'context-2', text: 'Context' },
    ])
    expect(structured.html).toContain('<h2 id="context">Context</h2>')
    expect(structured.html).toContain('<h2 id="context-2">Context</h2>')
  })
})
