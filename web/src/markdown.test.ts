import { describe, expect, it } from 'vitest'
import { renderMarkdown } from './markdown'

describe('renderMarkdown', () => {
  it('renders useful markdown and removes executable markup', () => {
    const result = renderMarkdown('# Safe\n\n[bad](javascript:alert(1))\n\n<img src="https://example.com/tracker.png">\n\n<span style="display:none">visible</span>')

    expect(result).toContain('<h1>Safe</h1>')
    expect(result).not.toContain('javascript:')
    expect(result).not.toContain('https://example.com')
    expect(result).not.toContain('style=')
    expect(result).toContain('visible')
  })
})
