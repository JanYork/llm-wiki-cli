export function readKnowledgeLink(hash = location.hash): { space: string; slug: string } | null {
  const params = new URLSearchParams(hash.replace(/^#/, ''));
  const space = params.get('space'), slug = params.get('page');
  return space && /^[a-f0-9]{64}$/.test(space) && slug && slug.length <= 512 && !/[\u0000-\u001f\u007f]/.test(slug) ? { space, slug } : null;
}
export function knowledgeHash(space: string, slug: string): string {
  return `#${new URLSearchParams({ space, page: slug })}`;
}
