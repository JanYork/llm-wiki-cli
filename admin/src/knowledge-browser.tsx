import { knowledgeHash, readKnowledgeLink } from './knowledge-link';
import { useEffect, useMemo, useState } from 'react';
import { BookOpen, FileText, Search, ChevronRight, LayoutDashboard, ArrowUpRight, Clock3, X, RotateCw, Link } from 'lucide-react';
import { Button } from './components/ui/button';
import { Input } from './components/ui/input';
import { Badge } from './components/ui/badge';
import { projectMarkdown, renderMarkdown } from '../../web/src/markdown';
import { display, type Language } from './text';

type Page = { slug: string; title: string; kind?: string; summary?: string; snippet?: string; body?: string; updated_at: string; links?: string[] };
type Props = { userId: string; space: string; refresh: number; language: Language; t: (value: string) => string; api: (path: string, body?: unknown) => Promise<any> };
const categories: Record<string, string> = { concept: '概念', entity: '实体', overview: '概览', purpose: '目标', schema: '规范', source: '来源', procedure: '流程', decision: '决策', reference: '参考', guide: '指南' };
export function KnowledgeBrowser({ userId, space, refresh, language, t, api }: Props) {
  const bookmarkKey = `lwc-reading-${userId}-${space}`;
  const [bookmark] = useState<{ slug: string; offset: number }>(() => {
    try { const value = JSON.parse(sessionStorage.getItem(bookmarkKey) || '{}'); return { slug: typeof value.slug === 'string' ? value.slug : '', offset: Number.isSafeInteger(value.offset) && value.offset >= 0 ? value.offset : 0 }; }
    catch { return { slug: '', offset: 0 }; }
  });
  const [pages, setPages] = useState<Page[]>([]), [page, setPage] = useState<Page | null>(null);
  const [offset, setOffset] = useState(bookmark.offset), [more, setMore] = useState(false), [filter, setFilter] = useState(''), [category, setCategory] = useState('');
  const [searchRows, setSearchRows] = useState<Page[] | null>(null), [searching, setSearching] = useState(false), [retry, setRetry] = useState(0);
  const [view, setView] = useState('pages'), [selected, setSelected] = useState(() => { const link = readKnowledgeLink(); return link?.space === space ? link.slug : bookmark.slug; }), [loading, setLoading] = useState(true), [reading, setReading] = useState(false), [error, setError] = useState('');
  const [shareNotice, setShareNotice] = useState('');
  useEffect(() => {
    const restore = () => { const link = readKnowledgeLink(); if (link?.space === space) { setSelected(link.slug); setView('pages'); setShareNotice(''); } };
    window.addEventListener('hashchange', restore); window.addEventListener('popstate', restore);
    return () => { window.removeEventListener('hashchange', restore); window.removeEventListener('popstate', restore); };
  }, [space]);
  const categoryName = (kind?: string) => t(categories[kind || ''] || '其他');
  useEffect(() => { let active = true; setLoading(true); setError(''); setPages([]);
    api(`/api/spaces/${space}/query`, { action: 'list', offset, limit: 100 }).then(result => { if (active) { setPages(result.data.pages); setMore(result.data.has_more); setSelected(current => current || result.data.pages[0]?.slug || ''); } }).catch(e => { if (active) setError(e.message); }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [space, offset, refresh, retry]);
  useEffect(() => { let active = true; setPage(null); if (!selected) { setReading(false); return; } setReading(true); setError('');
    api(`/api/spaces/${space}/query`, { action: 'get', slug: selected }).then(result => { if (active) setPage(result.data.page); }).catch(e => { if (active) setError(e.message); }).finally(() => { if (active) setReading(false); });
    return () => { active = false; };
  }, [space, selected, refresh, retry]);
  useEffect(() => {
    let active = true;
    const query = filter.trim();
    if (!query) { setSearchRows(null); setSearching(false); return; }
    setSearching(true); setSearchRows([]); setError('');
    const timer = setTimeout(() => {
      api(`/api/spaces/${space}/query`, { action: 'search', query, limit: 100 }).then(result => {
        if (active) setSearchRows(result.data.results.filter((r: { type: string }) => r.type === 'page').map((r: { identifier: string; title?: string; kind?: string; summary?: string; snippet: string }) => ({ slug: r.identifier, title: r.title || r.identifier, kind: r.kind, summary: r.summary, snippet: new DOMParser().parseFromString(renderMarkdown(r.snippet), 'text/html').body.textContent?.replace(/\s+/g, ' ').trim(), updated_at: '' })));
      }).catch(e => { if (active) setError(e.message); }).finally(() => { if (active) setSearching(false); });
    }, 300);
    return () => { active = false; clearTimeout(timer); };
  }, [space, filter, refresh, retry]);
  useEffect(() => {
    if (page && page.slug === selected) { if (!location.hash) history.replaceState(null, '', knowledgeHash(space, selected)); try { sessionStorage.setItem(bookmarkKey, JSON.stringify({ slug: selected, offset })); } catch { /* Reading preferences are optional. */ } }
  }, [bookmarkKey, page, selected, offset, space]);
  const directory = searchRows ?? pages;
  const visible = directory.filter(p => !category || (p.kind || 'other') === category);
  const groups = [...new Set(directory.map(p => p.kind || 'other'))];
  const markdown = useMemo(() => projectMarkdown(page?.body || '', page?.title || '', slug => knowledgeHash(space, slug)), [page, space]);
  const choose = (slug: string) => { if (readKnowledgeLink()?.slug !== slug || readKnowledgeLink()?.space !== space) history.pushState(null, '', knowledgeHash(space, slug)); setSelected(slug); setView('pages'); setShareNotice(''); };
  return <section className="knowledge" aria-label={t('知识库')}>
    <div className="knowledge-tabs"><Button variant="ghost" className={view === 'overview' ? 'active' : ''} onClick={() => { setView('overview'); setFilter(''); setSearchRows(null); setCategory(''); }}><LayoutDashboard />{t('概览')}</Button><Button variant="ghost" className={view === 'pages' ? 'active' : ''} onClick={() => setView('pages')}><BookOpen />{t('知识页面')}</Button><span className="muted">{t('阅读团队沉淀的知识与经验')}</span></div>
    {error && <div role="alert" className="knowledge-error"><span>{error}</span><Button variant="outline" size="sm" onClick={() => setRetry(v => v + 1)}><RotateCw />{t('重试')}</Button></div>}
    {view === 'overview' ? <div className="knowledge-overview"><div className="knowledge-stats"><div><span>{t('本页知识')}</span><strong>{pages.length}</strong></div><div><span>{t('知识分类')}</span><strong>{groups.length}</strong></div><div><span>{t('最近更新')}</span><strong className="stat-date">{pages.length ? display(pages.map(p => p.updated_at).sort().at(-1), 'updated_at', language) : '—'}</strong></div></div><section className="knowledge-panel"><h2>{t('分类分布')}</h2>{groups.map(kind => { const count = pages.filter(p => (p.kind || 'other') === kind).length; return <button className="distribution" key={kind} onClick={() => { setCategory(kind); setView('pages'); }}><span>{categoryName(kind)}</span><span className="distribution-track"><span style={{ width: `${100 * count / pages.length}%` }} /></span><span>{count}</span><ChevronRight size={14} /></button>; })}</section><section className="knowledge-panel"><h2>{t('页面一览')}</h2><div className="page-grid">{pages.map(p => <button key={p.slug} onClick={() => choose(p.slug)}><FileText size={16} /><span>{p.title}</span><ArrowUpRight size={14} /></button>)}</div>{!loading && !pages.length && <p className="empty">{t('还没有知识页面。智能体同步后，内容会出现在这里。')}</p>}</section></div> : <div className="knowledge-reader"><div className="page-directory"><div className="directory-tools"><div className="directory-search"><Search size={16} /><Input aria-label={t('搜索知识页面')} placeholder={t('搜索标题和正文')} maxLength={500} value={filter} onChange={e => { setFilter(e.target.value); setCategory(''); }} />{filter && <Button className="clear-search" size="icon-sm" variant="ghost" aria-label={t('清除搜索')} onClick={() => { setFilter(''); setCategory(''); }}><X /></Button>}</div>{searchRows !== null && <p className="search-scope">{t('搜索当前空间，最多显示 100 条相关页面')}</p>}<div className="category-chips"><Button size="sm" variant={!category ? 'secondary' : 'ghost'} onClick={() => setCategory('')}>{t('全部')}<span>{directory.length}</span></Button>{groups.map(kind => <Button key={kind} size="sm" variant={category === kind ? 'secondary' : 'ghost'} onClick={() => setCategory(kind)}>{categoryName(kind)}<span>{directory.filter(p => (p.kind || 'other') === kind).length}</span></Button>)}</div></div><div className="page-directory-list">{visible.map(p => <button key={p.slug} aria-label={`${p.title} ${categoryName(p.kind)}`} className={selected === p.slug ? 'active' : ''} onClick={() => choose(p.slug)} aria-current={selected === p.slug ? 'page' : undefined}><FileText size={16} /><span><strong>{p.title}</strong><small>{categoryName(p.kind)}</small>{searchRows !== null && p.snippet && <span className="search-excerpt">{p.snippet}</span>}</span><ChevronRight size={14} /></button>)}{!loading && !searching && !visible.length && <div className="empty"><p>{t(filter || category ? '没有找到匹配的页面，试试其他关键词。' : '还没有知识页面。智能体同步后，内容会出现在这里。')}</p>{(filter || category) && <Button variant="ghost" onClick={() => { setFilter(''); setCategory(''); }}>{t('查看全部页面')}</Button>}</div>}{(loading || searching) && <p className="empty" role="status">{t('正在加载知识…')}</p>}</div>{searchRows === null && <div className="directory-pagination"><Button variant="ghost" size="sm" disabled={!offset || loading} onClick={() => { setSelected(''); setOffset(v => Math.max(0, v - 100)); setCategory(''); }}>{t('上一页')}</Button><span>{offset / 100 + 1}</span><Button variant="ghost" size="sm" disabled={!more || loading} onClick={() => { setSelected(''); setOffset(v => v + 100); setCategory(''); }}>{t('下一页')}</Button></div>}</div><div className="reading-pane" key={selected}>{page && !reading ? <><div className="document-heading"><Badge variant="secondary">{categoryName(page.kind)}</Badge><h1>{page.title}</h1>{page.summary && <p>{page.summary}</p>}<div className="document-share"><Button variant="outline" size="sm" onClick={async () => { try { await navigator.clipboard.writeText(`${location.origin}${location.pathname}${knowledgeHash(space, page.slug)}`); setShareNotice('链接已复制，仅有权限的成员可访问。'); } catch { setShareNotice('复制失败，请重试。'); } }}><Link />{t('复制页面链接')}</Button><span role="status">{t(shareNotice)}</span></div><div className="document-meta"><Clock3 size={14} />{t('更新于')} {display(page.updated_at, 'updated_at', language)}</div></div>{!!markdown.toc.length && <details className="document-toc"><summary>{t('本文目录')}</summary><div>{markdown.toc.map(heading => <Button key={heading.id} variant="ghost" size="sm" onClick={() => document.getElementById(heading.id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })}>{heading.text}</Button>)}</div></details>}<article className="knowledge-prose" dangerouslySetInnerHTML={{ __html: markdown.html }} />{!!page.links?.length && <section className="document-links"><h2>{t('相关知识')}</h2>{page.links.map(link => <Button key={link} variant="outline" onClick={() => choose(link)}><FileText />{pages.find(p => p.slug === link)?.title || link}</Button>)}</section>}</> : <div className="reading-empty"><BookOpen size={40} /><h2>{t(reading || loading ? '正在加载知识…' : error ? '暂时无法显示知识' : '团队知识，从这里开始')}</h2><p>{t(reading || loading ? '正在读取所选页面' : error ? '请重试，或选择左侧的其他页面。' : '选择左侧页面阅读；智能体同步的新知识会出现在这里。')}</p></div>}</div></div>}
  </section>;
}
