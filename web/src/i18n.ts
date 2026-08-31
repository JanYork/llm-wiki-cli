export type Locale = 'en' | 'zh-CN'

export const DEFAULT_LOCALE: Locale = 'en'

export const messages = {
  en: {
    overview: 'Overview', pages: 'Pages', sources: 'Sources', knowledgeGraph: 'Knowledge graph', codeGraph: 'Code graph', wordGraph: 'Word graph',
    projectCondition: 'Project condition', wikiPages: 'Wiki pages', groundedSources: 'Grounded sources',
    readOnlyDescription: 'Read-only project inspection. No migration, refresh, or write is triggered.',
    revision: 'revision', readOnly: 'read only', store: 'Store', boundaries: 'Boundaries', database: 'Database', operation: 'Operation',
    frozenBoundary: 'Historical revisions remain frozen. This server exposes GET and HEAD only, binds to 127.0.0.1, and never starts graph construction.',
    selectPage: 'Select a page to render its Markdown.', id: 'ID', source: 'Source', bytes: 'Bytes', interactiveMap: 'Interactive map',
    loadingGraph: 'Loading graph…', graphUnavailable: 'Graph is not available.', graphLabel: 'Interactive graph visualization',
    limited: 'limited to 1,000 nodes / 5,000 edges', sampleLimited: 'bounded sample', graph3dControls: 'drag to rotate · scroll to zoom',
    projectLocal: 'Project-local', loopbackOnly: 'loopback only', loadingDatabase: 'loading database', switchLanguage: '中文', switchLanguageLabel: 'Switch to Chinese',
    wordQuery: 'Search terms', wordQueryPlaceholder: 'Enter up to 8 terms', search: 'Search', previous: 'Previous', next: 'Next', samplePage: 'Sample page', wordPrompt: 'Search first to load a bounded term-document sample.',
    kind: 'Type', created: 'Created', updated: 'Updated', provenance: 'Provenance', citedSources: 'Sources', contents: 'Contents',
  },
  'zh-CN': {
    overview: '概览', pages: '页面', sources: '来源', knowledgeGraph: '知识图谱', codeGraph: '代码图谱', wordGraph: '词网图',
    projectCondition: '项目状态', wikiPages: 'Wiki 页面', groundedSources: '可追溯来源',
    readOnlyDescription: '只读查看项目，不会触发迁移、刷新或写入。',
    revision: '版本', readOnly: '只读', store: '存储', boundaries: '安全边界', database: '数据库', operation: '操作序号',
    frozenBoundary: '历史版本保持冻结。服务只开放 GET 和 HEAD，仅绑定 127.0.0.1，并且不会启动图构建。',
    selectPage: '选择一个页面以渲染 Markdown。', id: '编号', source: '来源', bytes: '字节数', interactiveMap: '交互式图谱',
    loadingGraph: '正在加载图谱…', graphUnavailable: '图谱不可用。', graphLabel: '交互式图谱可视化',
    limited: '最多显示 1,000 个节点 / 5,000 条边', sampleLimited: '局部样本已达上限', graph3dControls: '拖动旋转 · 滚轮缩放',
    projectLocal: '项目本地', loopbackOnly: '仅限回环地址', loadingDatabase: '正在加载数据库', switchLanguage: 'EN', switchLanguageLabel: 'Switch to English',
    wordQuery: '检索词', wordQueryPlaceholder: '最多输入 8 个词', search: '检索', previous: '上一页', next: '下一页', samplePage: '样本页', wordPrompt: '先检索，再按上限加载词与文档的局部样本。',
    kind: '类型', created: '创建时间', updated: '更新时间', provenance: '来源性质', citedSources: '引用来源', contents: '目录',
  },
} as const

export function pageCount(locale: Locale, count: number): string {
  return locale === 'en' ? `${count} pages` : `${count} 个页面`
}

export function graphCount(locale: Locale, nodes: number, edges: number): string {
  return locale === 'en' ? `${nodes} nodes · ${edges} edges` : `${nodes} 个节点 · ${edges} 条边`
}
