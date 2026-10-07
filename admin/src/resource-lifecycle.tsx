import { useEffect, useState } from 'react';
import { Button } from './components/ui/button';
import { Card } from './components/ui/card';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from './components/ui/dialog';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from './components/ui/table';
import { display, type Language } from './text';

type Resource = { id: string; name: string; kind: 'team' | 'space'; revision: number; deleted_at?: number; affected_spaces?: number; archived?: boolean };
type Props = { team?: Record<string, unknown>; space?: { id: string; name: string; revision: number; role: string }; language: Language; t: (text: string) => string; api: (path: string, body?: unknown) => Promise<any>; changed: () => Promise<void> };
export function ResourceLifecycle({ team, space, language, t, api, changed }: Props) {
  const [trash, setTrash] = useState<Resource[]>([]), [offset, setOffset] = useState(0), [more, setMore] = useState(false);
  const [preview, setPreview] = useState<Resource | null>(null), [busy, setBusy] = useState(false), [notice, setNotice] = useState(''), [refresh, setRefresh] = useState(0), [needsPreview, setNeedsPreview] = useState(false);
  useEffect(() => {
    let active = true;
    api(`/api/admin?view=trash&offset=${offset}`).then(result => { if (active) { setTrash(result.rows); setMore(result.has_more); } }).catch(error => { if (active) setNotice(error.message); });
    return () => { active = false; };
  }, [offset, refresh]);
  async function prepare(kind: Resource['kind'], id: string) {
    setBusy(true); setNotice('');
    try { setPreview(await api('/api/manage', { action: `${kind}.delete.preview`, [`${kind}_id`]: id })); setNeedsPreview(false); }
    catch (error) { setNotice(error instanceof Error ? error.message : '操作未完成，请重试。'); }
    finally { setBusy(false); }
  }
  async function apply() {
    if (!preview || needsPreview) return;
    setBusy(true); setNotice('');
    try {
      await api('/api/manage', { action: `${preview.kind}.${preview.archived ? 'restore' : 'delete'}`, [`${preview.kind}_id`]: preview.id, expected_revision: preview.revision });
      setNotice(preview.archived ? '已恢复，成员可按现有权限重新访问。' : '已移入回收站，数据仍然保留。');
      setPreview(null); setOffset(0); setRefresh(v => v + 1); await changed();
    } catch (error) { setNotice(error instanceof Error ? error.message : '操作未完成，请重试。'); setNeedsPreview(true); }
    finally { setBusy(false); }
  }
  return <section className="resource-lifecycle">
    <Card className="card"><h2>{t('当前资源')}</h2><p className="muted">{t('删除会停止服务端访问与同步。内容保留在回收站，可由有权限的管理者恢复。')}</p>
      {team?.role === 'owner' && <div className="resource-row"><div><strong>{String(team.name)}</strong><p className="muted">{t('团队')}</p></div><Button variant="outline" disabled={busy} onClick={() => void prepare('team', String(team.id))}>{t('删除团队')}</Button></div>}
      {space?.role === 'manager' && <div className="resource-row"><div><strong>{space.name}</strong><p className="muted">{t('记忆空间')}</p></div><Button variant="outline" disabled={busy} onClick={() => void prepare('space', space.id)}>{t('删除空间')}</Button></div>}
      {team?.role !== 'owner' && space?.role !== 'manager' && <p>{t('请先选择你可管理的团队或空间。')}</p>}
    </Card>
    <Card className="card"><h2>{t('回收站')}</h2><p className="muted">{t('这里只显示你可以恢复的资源。恢复团队不会恢复此前单独删除的空间，也不会重新启用旧邀请。')}</p>
      {trash.length ? <Table><TableHeader><TableRow><TableHead>{t('名称')}</TableHead><TableHead>{t('类型')}</TableHead><TableHead>{t('删除时间')}</TableHead><TableHead>{t('操作')}</TableHead></TableRow></TableHeader><TableBody>{trash.map(row => <TableRow key={`${row.kind}-${row.id}`}><TableCell>{row.name}</TableCell><TableCell>{t(row.kind === 'team' ? '团队' : '记忆空间')}</TableCell><TableCell>{display(row.deleted_at, 'deleted_at', language)}</TableCell><TableCell><Button variant="ghost" disabled={busy} onClick={() => void prepare(row.kind, row.id)}>{t('恢复')}</Button></TableCell></TableRow>)}</TableBody></Table> : <p className="empty">{t('回收站暂无资源')}</p>}
      <div className="row"><Button variant="outline" disabled={busy || offset === 0} onClick={() => setOffset(v => Math.max(0, v - 100))}>{t('上一页')}</Button><Button variant="outline" disabled={busy || !more} onClick={() => setOffset(v => v + 100)}>{t('下一页')}</Button><Button variant="ghost" disabled={busy} onClick={() => setRefresh(v => v + 1)}>{t('刷新')}</Button></div>
    </Card>
    {notice && !preview && <p role="status">{t(notice)}</p>}
    <Dialog open={preview !== null} onOpenChange={open => { if (!open && !busy) setPreview(null); }}><DialogContent showCloseButton={false}><DialogHeader><DialogTitle>{t(preview?.archived ? '确认恢复' : '确认移入回收站')}</DialogTitle><DialogDescription>{t(preview?.archived ? '恢复后按现有成员与空间权限重新开放，不会覆盖记忆内容。' : '所有成员的服务端访问和同步将停止。已下载的本地记忆会保留。')}</DialogDescription></DialogHeader>
      <strong>{preview?.name}</strong>{preview?.kind === 'team' && <p>{t('受影响的记忆空间')}：{preview.affected_spaces}</p>}
      {notice && <p role="alert">{t(notice)}</p>}
      <div className="row"><Button variant="outline" disabled={busy} onClick={() => setPreview(null)}>{t('取消')}</Button>{needsPreview && <Button variant="outline" disabled={busy} onClick={() => preview && void prepare(preview.kind, preview.id)}>{t('重新查看')}</Button>}<Button variant={preview?.archived ? 'default' : 'destructive'} disabled={busy || needsPreview} onClick={() => void apply()}>{t(busy ? '正在处理…' : preview?.archived ? '确认恢复' : '移入回收站')}</Button></div>
    </DialogContent></Dialog>
  </section>;
}
