import DOMPurify from 'dompurify'
import { Renderer, Marked, marked, type Token, type Tokens } from 'marked'

export interface MarkdownHeading {
  depth: number
  id: string
  text: string
}

export interface MarkdownProjection {
  html: string
  toc: MarkdownHeading[]
}

const TOC_MIN_HEADINGS = 3

function inlineText(tokens: Token[]): string {
  return tokens.map((token) => {
    if ('tokens' in token && token.tokens)
      return inlineText(token.tokens)
    if ('text' in token && typeof token.text === 'string')
      return token.text.replace(/<[^>]*>/g, '')
    return token.type === 'br' ? ' ' : ''
  }).join('').replace(/\s+/g, ' ').trim()
}

function normalizedTitle(value: string): string {
  return value.replace(/\s+/g, ' ').trim().toLowerCase()
}

function withoutMatchingTitle(source: string, title: string): string {
  const tokens = marked.lexer(source)
  const index = tokens.findIndex(token => token.type !== 'space')
  const heading = tokens[index]
  if (heading?.type !== 'heading' || heading.depth !== 1)
    return source
  if (normalizedTitle(inlineText(heading.tokens ?? [])) !== normalizedTitle(title))
    return source
  const start = tokens.slice(0, index).reduce((length, token) => length + token.raw.length, 0)
  return source.slice(0, start) + source.slice(start + heading.raw.length)
}

function anchorBase(text: string): string {
  return text
    .normalize('NFKC')
    .toLocaleLowerCase()
    .replace(/[^\p{Letter}\p{Number}]+/gu, '-')
    .replace(/^-+|-+$/g, '') || 'section'
}

export function projectMarkdown(source: string, title: string, wikiLink?: (slug: string) => string): MarkdownProjection {
  const body = withoutMatchingTitle(source, title)
  const headings: MarkdownHeading[] = []
  const anchors = new Map<string, number>()
  const renderer = new Renderer()
  renderer.heading = function (heading: Tokens.Heading) {
    const content = this.parser.parseInline(heading.tokens)
    if (heading.depth < 2 || heading.depth > 4)
      return `<h${heading.depth}>${content}</h${heading.depth}>\n`
    const text = inlineText(heading.tokens)
    const base = anchorBase(text)
    const occurrence = (anchors.get(base) ?? 0) + 1
    anchors.set(base, occurrence)
    const id = occurrence === 1 ? base : `${base}-${occurrence}`
    headings.push({ depth: heading.depth, id, text })
    return `<h${heading.depth} id="${id}">${content}</h${heading.depth}>\n`
  }
  const parser = new Marked();
  if (wikiLink) {
    const escape = (value: string) => value.replace(/[&<>"']/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[char]!));
    parser.use({ extensions: [{
      name: 'wikilink', level: 'inline',
      start: (src: string) => src.indexOf('[['),
      tokenizer(src: string) {
        const match = /^\[\[([^\]\n|]+)(?:\|([^\]\n]+))?\]\]/.exec(src);
        if (match) return { type: 'wikilink', raw: match[0], slug: match[1].trim(), label: (match[2] || match[1]).trim() };
      },
      renderer(token) { return `<a href="${escape(wikiLink(String(token.slug)))}">${escape(String(token.label))}</a>`; },
    }] });
    const nativeLink = renderer.link;
    renderer.link = function(link: Tokens.Link) {
      if (!/^(?:[a-z][a-z0-9+.-]*:|\/\/|#)/i.test(link.href) && /\.md(?:#.*)?$/.test(link.href)) {
        try { link = { ...link, href: wikiLink(decodeURIComponent(link.href.split('#')[0].split('/').at(-1)!.slice(0, -3))) }; } catch { /* Leave malformed links unchanged. */ }
      }
      return nativeLink.call(this, link);
    };
  }
  const html = DOMPurify.sanitize(parser.parse(body, { async: false, renderer }), {
    FORBID_ATTR: ['style'],
    FORBID_TAGS: ['img', 'iframe', 'object', 'embed', 'script', 'style'],
  })
  return { html, toc: headings.length >= TOC_MIN_HEADINGS ? headings : [] }
}

export function renderMarkdown(source: string): string {
  return DOMPurify.sanitize(marked.parse(source, { async: false }), {
    FORBID_ATTR: ['style'],
    FORBID_TAGS: ['img', 'iframe', 'object', 'embed', 'script', 'style'],
  })
}
