import { readKnowledgeLink } from './knowledge-link';
import { KnowledgeBrowser } from './knowledge-browser';
import { ResourceLifecycle } from './resource-lifecycle';
import { BookOpen, LayoutDashboard, ListTodo, FolderKanban, RefreshCw, Users, Monitor, KeyRound, History, ArrowUpRight, Trash2 } from 'lucide-react';
import React, { createContext, useContext, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Button } from './components/ui/button';
import './theme.css';
import './style.css';
import { SearchChoice } from './components/search-choice';
import { Choice, ChoiceOption } from './components/choice';
import { Input } from './components/ui/input';
import { Label } from './components/ui/label';
import { Card } from './components/ui/card';
import { Alert, AlertDescription } from './components/ui/alert';
import { Badge } from './components/ui/badge';
import { Checkbox } from './components/ui/checkbox';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from './components/ui/dialog';
import { Table as DataTable, TableHeader, TableBody, TableRow, TableHead, TableCell } from './components/ui/table';
import { labels as names, display, errorText, translate, type Language } from './text';
const Locale = createContext<Language>('zh');
function useText() { const language = useContext(Locale); return (text: string) => translate(text, language); }
function LanguageSwitch({ language, change }: { language: Language; change: (value: Language) => void }) {
  return <Label className="language-switch">{language === 'zh' ? '界面语言' : 'Language'}<Choice value={language} onValueChange={event => change(event as Language)}><ChoiceOption value="zh">{language === 'zh' ? '中文' : 'Chinese'}</ChoiceOption><ChoiceOption value="en">{language === 'zh' ? '英文' : 'English'}</ChoiceOption></Choice></Label>;
}
type Row = Record<string, unknown>;
type Space = { team_id?: string; id: string; name: string; role: string; revision: number };
type Me = { user_id: string; spaces: Space[] };
async function api(path: string, body?: unknown) {
  const response = await fetch(path, { credentials: 'same-origin', ...(body === undefined ? {} : { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }) }).catch(() => { throw new Error(errorText('network_unavailable')); });
  const result = await response.json().catch(() => { throw new Error(errorText('network_unavailable')); });
  if (!response.ok) { if (response.status === 401 && ['unauthorized', 'server_token_required', 'invalid_server_token'].includes(result.error?.code)) window.dispatchEvent(new Event('lwc-session-expired')); throw new Error(errorText(result.error?.code)); }
  return result;
}
const navIcons = [LayoutDashboard, BookOpen, ListTodo, FolderKanban, RefreshCw, Users, Monitor, KeyRound, History, Trash2];
const navigation = [['overview', '总览'], ['memory', '记忆'], ['tasks', '计划与待办'], ['projects', '项目与集合'], ['replicas', '同步状态'], ['members', '团队成员'], ['devices', '设备与智能体'], ['keys', '访问密钥'], ['audit', '活动记录'], ['resources', '资源管理']] as const;
function Detail({ value, field = '' }: { value: unknown; field?: string }) {
  const t = useText(); const language = useContext(Locale);
  if (value === null || typeof value !== 'object') return <span className="detail-value">{display(value, field, language)}</span>;
  if (Array.isArray(value)) return <ol className="detail-items">{value.map((item, i) => <li key={i}><Detail value={item} field={field} /></li>)}</ol>;
  return <dl className="detail-fields">{Object.entries(value).filter(([key]) => names[key] && !['id','user_id','device_id','agent_id','team_id','namespace','hash','digest','accepted_digest','artifact_id','replica_id','batch_id','server_epoch','parent','principal','source_hash','content_hash','preview_id','expected_head','target_id','source_id','key','logical_key','slug','expires_at','expires_in','invitation_token'].includes(key)).map(([key, item]) => <div key={key}><dt>{t(names[key])}</dt><dd><Detail value={item} field={key} /></dd></div>)}</dl>;
}
function Table({ rows, select }: { rows: Row[]; select?: (row: Row) => void }) {
  const t = useText(); const language = useContext(Locale);
  const columns = ['title','name','nickname','device','kind','role','status','state','action','created_at','expires_at','updated_at','last_seen','pending_conflicts','revoked'].filter(key => rows.some(row => key in row));
  if (!rows.length) return <div className="empty">{t("当前范围暂无记录")}</div>;
  return <div className="table-scroll"><DataTable><TableHeader><TableRow>{columns.map(key => <TableHead key={key}>{t(names[key] ?? '属性')}</TableHead>)}{select && <TableHead>{t("详情")}</TableHead>}</TableRow></TableHeader><TableBody>{rows.map((row, i) => <TableRow key={String(row.id ?? row.key ?? i)}>{columns.map(key => <TableCell key={key} title={display(row[key], key, language)}>{['role','status','state'].includes(key) ? <Badge variant="secondary">{display(row[key], key, language)}</Badge> : <span>{display(row[key], key, language)}</span>}</TableCell>)}{select && <TableCell><Button variant="ghost" onClick={() => select(row)}>{t("查看")}</Button></TableCell>}</TableRow>)}</TableBody></DataTable></div>;
}
function App({ language, changeLanguage }: { language: Language; changeLanguage: (value: Language) => void }) {
  const [invitation, setInvitation] = useState(() => {
    const incoming = new URLSearchParams(location.hash.slice(1)).get('invite');
    if (incoming && /^[a-f0-9]{64}$/.test(incoming)) {
      sessionStorage.setItem('lwc-invitation', incoming);
      history.replaceState(null, '', location.pathname + location.search);
    }
    return sessionStorage.getItem('lwc-invitation') || '';
  });
  const [deviceRequest, setDeviceRequest] = useState(() => {
    const incoming = new URLSearchParams(location.hash.slice(1)).get('device');
    if (incoming && /^[a-fA-F0-9]{8}$/.test(incoming)) {
      sessionStorage.setItem('lwc-device-request', incoming);
      history.replaceState(null, '', location.pathname + location.search);
    }
    return sessionStorage.getItem('lwc-device-request') || '';
  });
  const [devicePreview, setDevicePreview] = useState<Row | null>(null), [invitePreview, setInvitePreview] = useState<Row | null>(null);
  const t = useText();
  const [gate, setGate] = useState(false), [providers, setProviders] = useState<Record<string, boolean>>({}), [me, setMe] = useState<Me | null>(null);
  const [authMethod, setAuthMethod] = useState(invitation ? 'account' : 'key'), [workspaceReady, setWorkspaceReady] = useState(false);
  const [booting, setBooting] = useState(true), [busy, setBusy] = useState(false), [notice, setNotice] = useState('');
  const [page, setPage] = useState('memory'), [teams, setTeams] = useState<Row[]>([]), [team, setTeam] = useState(''), [space, setSpace] = useState('');
  const [rows, setRows] = useState<Row[]>([]), [offset, setOffset] = useState(0), [more, setMore] = useState(false), [refresh, setRefresh] = useState(0);
  const [kind, setKind] = useState('page'), [tab, setTab] = useState('projects'), [detail, setDetail] = useState<unknown>(null), [challenge, setChallenge] = useState('');
  const [initialGrants, setInitialGrants] = useState<Record<string, string>>({});
  const [form, setForm] = useState<Record<string, string>>({});
  const [links, setLinks] = useState<string[]>([]), [options, setOptions] = useState<Row[]>([]), [linkReady, setLinkReady] = useState(false);

  const [members, setMembers] = useState<Row[]>([]), [memberOffset, setMemberOffset] = useState(0), [membersMore, setMembersMore] = useState(false);
  const [objects, setObjects] = useState<Row[]>([]), [objectOffset, setObjectOffset] = useState(0), [objectsMore, setObjectsMore] = useState(false);
  useEffect(() => {
    const openLink = () => {
      const params = new URLSearchParams(location.hash.slice(1));
      const invite = params.get('invite'), device = params.get('device');
      if (invite && /^[a-f0-9]{64}$/.test(invite)) { sessionStorage.setItem('lwc-invitation', invite); setInvitation(invite); setAuthMethod('account'); }
      else if (device && /^[a-fA-F0-9]{8}$/.test(device)) { sessionStorage.setItem('lwc-device-request', device); setDeviceRequest(device); }
      else { const target = readKnowledgeLink(); if (!target || !me) return; const destination = me.spaces.find(s => s.id === target.space); if (!destination) { setNotice('此链接指向的空间不存在或尚未授权。'); return; } setSpace(destination.id); setTeam(destination.team_id || ''); setPage('memory'); setKind('page'); setWorkspaceReady(true); setNotice(''); return; }
      history.replaceState(null, '', location.pathname + location.search);
      setNotice('');
    };
    window.addEventListener('hashchange', openLink);
    window.addEventListener('popstate', openLink);
    return () => { window.removeEventListener('hashchange', openLink); window.removeEventListener('popstate', openLink); };
  }, [me]);
  const visibleSpaces = me?.spaces.filter(s => (s.team_id || '') === team) || [];
  const selected = me?.spaces.find(s => s.id === space);
  const owner = teams.some(t => t.id === team && t.role === 'owner');
  const manager = selected?.role === 'manager';
  const field = (key: string, label: string, type = 'text', required = true) => {
    const verification = key === 'code' || key === 'link_code';
    const secret = key === 'token' || key === 'invitation';
    return <Label key={key}>{label}<Input type={type} required={required} minLength={key === 'personal_key' ? 73 : verification ? 6 : secret ? 64 : key === 'device_code' ? 8 : required ? 1 : undefined} maxLength={key === 'personal_key' ? 73 : verification ? 6 : secret ? 64 : key === 'device_code' ? 8 : type === 'email' ? 254 : 160} pattern={key === 'personal_key' ? 'lwc_user_[a-fA-F0-9]{64}' : verification ? '[0-9]{6}' : secret ? '[a-fA-F0-9]{64}' : key === 'device_code' ? '[a-fA-F0-9]{8}' : undefined} inputMode={verification ? 'numeric' : type === 'email' ? 'email' : undefined} value={form[key] ?? ''} onChange={e => setForm({ ...form, [key]: verification ? e.target.value.replace(/[^0-9]/g, '') : e.target.value })} autoComplete={verification ? 'one-time-code' : type === 'password' ? 'off' : type === 'email' ? 'email' : undefined} /></Label>;
  };
  async function loadMe() {
    const user: Me = await api('/api/me');
    let remembered = ''; try { remembered = localStorage.getItem(`lwc-last-space-${user.user_id}`) || ''; } catch { /* Optional preference. */ }
    const linked = readKnowledgeLink();
    const restored = user.spaces.find(s => s.id === linked?.space) || user.spaces.find(s => s.id === remembered);
    if (linked && !user.spaces.some(s => s.id === linked.space)) setNotice('此链接指向的空间不存在或尚未授权。');
    const data = await api('/api/admin?view=teams');
    setMe(user); setSpace(current => user.spaces.some(s => s.id === current) ? current : restored?.id || user.spaces[0]?.id || '');
    setWorkspaceReady(current => current || user.spaces.length <= 1 || !!restored);
    setTeams(data.rows); setTeam(current => data.rows.some((row: Row) => row.id === current) ? current : restored?.team_id || data.rows[0]?.id || '');
  }
  useEffect(() => { if (workspaceReady && me && space) { try { localStorage.setItem(`lwc-last-space-${me.user_id}`, space); } catch { /* Optional preference. */ } } }, [workspaceReady, me, space]);
  useEffect(() => { (async () => {
    try { setProviders(await api('/api/auth/providers')); setGate(true); try { await loadMe(); } catch { setMe(null); } } catch { setGate(false); }
    finally { setBooting(false); }
  })(); }, []);
  async function action(operation: () => Promise<void>) { setBusy(true); setNotice(''); try { await operation(); setRefresh(v => v + 1); } catch (error) { setNotice(error instanceof TypeError ? '网络连接失败，请检查服务地址。' : error instanceof Error ? error.message : '请求失败'); } finally { setBusy(false); } }
  async function manage(body: Row) { const result = await api('/api/manage', body); setDetail(null); await loadMe(); if (body.action === 'team.create') setTeam(result.id); if (body.action === 'invitation.accept') { setTeam(result.team_id); setPage('overview'); } if (body.action === 'space.create') setSpace(result.id); setForm({}); setNotice('已保存'); }
  useEffect(() => {
    let active = true; setInvitePreview(null);
    if (gate && invitation) api('/api/invitations/preview', { invitation_token: invitation }).then(value => { if (active) setInvitePreview(value); }).catch(error => { if (active) setNotice(error.message); });
    return () => { active = false; };
  }, [gate, invitation]);
  useEffect(() => {
    let active = true; setDevicePreview(null);
    if (me && deviceRequest) api('/api/auth/device/preview', { user_code: deviceRequest }).then(value => { if (active) setDevicePreview(value); }).catch(error => { if (active) setNotice(error.message); });
    return () => { active = false; };
  }, [me, deviceRequest]);
  useEffect(() => { if (!visibleSpaces.some(s => s.id === space)) setSpace(visibleSpaces[0]?.id || ''); }, [me, team, space]);
  useEffect(() => { setInitialGrants({}); }, [team]);
  useEffect(() => { setForm({}); setMemberOffset(0); setObjectOffset(0); }, [space]);
  useEffect(() => {
    let active = true; setMembers([]); setMembersMore(false);
    if (page === 'members' && manager && space) api(`/api/admin?view=space_members&scope=${encodeURIComponent(space)}&offset=${memberOffset}`).then(result => { if (active) { setMembers(result.rows); setMembersMore(result.has_more); } }).catch(error => { if (active) setNotice(error.message); });
    return () => { active = false; };
  }, [page, manager, space, memberOffset, refresh]);
  useEffect(() => {
    let active = true; setObjects([]); setObjectsMore(false);
    if (page === 'members' && manager && space && form.policy_kind && form.policy_kind !== '*') api(`/api/spaces/${space}/query`, { action: 'objects', kind: form.policy_kind, offset: objectOffset, limit: 100 }).then(result => { if (active) { setObjects(result.data.objects); setObjectsMore(result.data.has_more); } }).catch(error => { if (active) setNotice(error.message); });
    return () => { active = false; };
  }, [page, manager, space, form.policy_kind, objectOffset]);
  const memberPicker = (key: string) => <Label>{t('选择成员')}<SearchChoice value={form[key] || ''} selectedLabel={form[`${key}_label`]} placeholder={t('请选择成员')} searchLabel={t('搜索姓名或邮箱')} emptyLabel={t('未找到成员')} options={members.map((member, i) => ({ value: String(member.user_id), label: String(member.name || `${t('团队成员')} ${memberOffset + i + 1}`) }))} onSelect={(value, label) => setForm({ ...form, [key]: value, [`${key}_label`]: label })} />{(memberOffset > 0 || membersMore) && <span className="row"><Button type="button" variant="ghost" disabled={memberOffset === 0} onClick={() => setMemberOffset(v => Math.max(0, v - 100))}>{t('上一页')}</Button><Button type="button" variant="ghost" disabled={!membersMore} onClick={() => setMemberOffset(v => v + 100)}>{t('下一页')}</Button></span>}</Label>;
  useEffect(() => { setNotice(''); }, [language]);
  useEffect(() => { setDetail(null); }, [page, space, team, tab, kind, offset]);
  useEffect(() => {
    if (!me) return;
    let active = true; setRows([]); setMore(false);
    (async () => {
      try {
        if (page === 'overview' || page === 'resources' || (page === 'memory' && kind === 'page')) return;
        let result;
        if (page === 'audit' && tab === 'history') {
          if (!space) return;
          result = await api(`/api/spaces/${space}/recovery`, { action: 'history', limit: 100, offset });
          if (active) { setRows(result.history); setMore(result.history.length === 100); }
        } else if (page === 'memory' || page === 'tasks') {
          if (!space) return;
          result = await api(`/api/spaces/${space}/query`, { action: 'objects', kind: page === 'tasks' ? (kind === 'todo' ? 'todo' : 'plan') : kind, offset, limit: 100 });
          if (active) { setRows(result.data.objects); setMore(result.data.has_more); }
        } else {
          const view = page === 'projects' ? (['projects', 'collections'].includes(tab) ? tab : 'projects') : page === 'members' ? (['members', 'grants', 'policies'].includes(tab) ? tab : 'members') : page === 'devices' ? (['agents', 'identities'].includes(tab) ? tab : 'devices') : page;
          const scope = ['grants', 'policies', 'replicas'].includes(view) ? space : ['members', 'projects', 'collections'].includes(view) ? team : '';
          if (['grants', 'policies', 'replicas', 'members', 'projects', 'collections'].includes(view) && !scope) return;
          result = await api(`/api/admin?view=${view}&scope=${encodeURIComponent(scope)}&offset=${offset}`);
          if (active) { setRows(result.rows); setMore(result.has_more); }
        }
      } catch (error) { if (active) setNotice(error instanceof Error ? error.message : t("加载失败")); }
    })(); return () => { active = false; };
  }, [me, page, space, team, tab, kind, offset, refresh]);
  useEffect(() => {
    let active = true; setLinkReady(false); setLinks([]); setOptions([]);
    if (page !== 'projects' || !detail || typeof detail !== 'object' || !('id' in detail)) return;
    (async () => {
      try {
        const list = await api(`/api/admin?view=${tab === 'collections' ? 'collection_projects' : 'project_spaces'}&scope=${encodeURIComponent(String((detail as Row).id))}`);
        const available = tab === 'collections' ? (await api(`/api/admin?view=projects&scope=${encodeURIComponent(team)}`)).rows : me?.spaces.filter(s => s.team_id === team) || [];
        if (list.has_more) throw new Error(t("关联超过当前页面容量，请通过管理接口分页查看后编辑。"));
        if (active) { setLinks(list.rows.map((r: Row) => String(r.id))); setOptions(available); setLinkReady(true); }
      } catch (error) { if (active) setNotice(error instanceof Error ? error.message : t("关联加载失败")); }
    })(); return () => { active = false; };
  }, [page, tab, detail, team, me]);
  async function changePolicy(remove?: Row) {
    const user = String(remove?.user_id || form.policy_user || '');
    const current = await api(`/api/admin?view=policy&scope=${encodeURIComponent(space)}&user=${encodeURIComponent(user)}`);
    const rule = remove ? { kind: remove.kind, key: remove.logical_key, action: remove.action } : { kind: form.policy_kind || '*', key: form.policy_key || '*', action: form.policy_action || 'update' };
    const remaining = current.rows.filter((r: Row) => !(r.kind === rule.kind && r.key === rule.key && r.action === rule.action));
    await manage({ action: 'space.policy', space_id: space, user_id: user, expected_revision: selected?.revision, denials: remove ? remaining : [...remaining, rule] });
  }
  function dismissInvitation() { sessionStorage.removeItem('lwc-invitation'); setInvitation(''); setInvitePreview(null); }
  function clearAccount() { setMe(null); setWorkspaceReady(false); setPage('memory'); setSpace(''); setTeam(''); setRows([]); setDetail(null); setForm({}); setChallenge(''); }
  useEffect(() => {
    const expired = () => { if (me) { clearAccount(); setNotice(errorText('unauthorized')); } };
    window.addEventListener('lwc-session-expired', expired);
    return () => window.removeEventListener('lwc-session-expired', expired);
  }, [me]);
  const status = <div className="notice" role="status">{(busy || notice) && <Alert><AlertDescription>{busy ? t("正在处理…") : t(notice)}</AlertDescription></Alert>}</div>;
  if (booting) return <main className="login"><Card className="card">{t("正在连接…")}</Card></main>;
  if ((!gate || !me) && authMethod === 'key') return <main className="login"><Card className="card auth"><LanguageSwitch language={language} change={changeLanguage} /><div className="brand" aria-label="LWC">LW<b className="brand-accent">C</b> <span>{t('团队记忆')}</span></div><h1>{t('登录团队工作区')}</h1><p className="muted">{t('使用管理员为你开通的个人密钥，登录后选择可访问的空间。')}</p><form onSubmit={e => { e.preventDefault(); void action(async () => { await api('/api/auth/key', { key: form.personal_key }); setForm({}); setGate(true); setProviders(await api('/api/auth/providers')); await loadMe(); }); }}>{field('personal_key', t('个人访问密钥'), 'password')}<Button disabled={busy}>{t('登录')}</Button></form><Button variant="ghost" onClick={() => { setAuthMethod('account'); setNotice(''); setForm({}); }}>{t('使用邮箱或其他账号登录')}</Button>{status}</Card></main>;
  if (me && !workspaceReady && !invitation && !deviceRequest) return <main className="login"><Card className="card auth"><LanguageSwitch language={language} change={changeLanguage} /><div className="brand" aria-label="LWC">LW<b className="brand-accent">C</b> <span>{t('团队记忆')}</span></div><h1>{t('选择记忆空间')}</h1><p className="muted">{t('这里只显示你有权访问的空间。')}</p>{me.spaces.map(item => <Button variant="outline" key={item.id} onClick={() => { setSpace(item.id); setTeam(item.team_id || ''); setWorkspaceReady(true); setPage('memory'); setKind('page'); }}>{item.name} · {t(names[item.role])}</Button>)}<Button variant="ghost" onClick={() => void action(async () => { await api('/api/logout', {}); clearAccount(); })}>{t('使用其他账号登录')}</Button></Card></main>;
  if (!gate || !me) return <main className="login"><Card className="card auth"><LanguageSwitch language={language} change={changeLanguage} /><div className="brand" aria-label="LWC">LW<b className="brand-accent">C</b> <span>{t("团队记忆")}</span></div><h1>{gate ? t("登录团队工作区") : t("连接团队服务")}</h1><p className="muted">{gate ? t("登录后仅能访问明确授权的空间。") : t("请输入部署管理员提供的服务端接入密钥。")}</p>{invitation && <Alert><AlertDescription>{invitePreview ? `${t('受邀加入团队')}：${String(invitePreview.team_name)}` : t('你收到了一份团队邀请。')}{t('请使用收到邀请的邮箱登录。')}<Button type="button" variant="ghost" size="sm" onClick={dismissInvitation}>{t('暂不加入')}</Button></AlertDescription></Alert>}{deviceRequest && <Alert><AlertDescription>{t('登录后确认你刚刚发起的设备连接。')}</AlertDescription></Alert>}<form onSubmit={e => { e.preventDefault(); void action(async () => {
    if (!gate) { await api('/api/access', { token: form.token }); setForm({}); setProviders(await api('/api/auth/providers')); setGate(true); }
    else if (!challenge) { const data = await api('/api/auth/email/challenge', { email: form.email }); setChallenge(data.challenge_id); setNotice(t("验证码已发送")); }
    else { await api('/api/auth/email/verify', { challenge_id: challenge, code: form.code }); setForm({}); await loadMe(); }
  }); }}>{!gate ? field('token', t("服务端密钥"), 'password') : providers.email ? <>{field('email', t("邮箱"), 'email')}{challenge && field('code', t("验证码"))}</> : <p>{t(providers.github || providers.feishu ? "请选择下方的登录方式。" : "团队服务尚未配置登录方式，请联系部署管理员启用邮箱或账号登录。")}</p>}{(!gate || providers.email) && <Button disabled={busy}>{!gate ? t("连接服务端") : challenge ? t("验证并登录") : t("发送验证码")}</Button>}</form>{gate && <div className="row">{['github', 'feishu'].filter(p => providers[p]).map(provider => <Button key={provider} variant="outline" onClick={() => location.assign(`/api/auth/${provider}/start`)}>{provider === 'github' ? t("代码托管账号登录") : t("飞书登录")}</Button>)}</div>}<Button variant="ghost" onClick={() => { setAuthMethod('key'); setNotice(''); setForm({}); }}>{t('使用个人密钥登录')}</Button>{status}</Card></main>;
  return <div className="shell"><aside><div className="brand" aria-label="LWC">LW<b className="brand-accent">C</b> <span>{t("团队记忆")}</span></div><Label className="muted">{t("当前团队")}<Choice value={team} onValueChange={e => { setTeam(e); setOffset(0); }}>{me.spaces.some(s => !s.team_id) && <ChoiceOption value="">{t("个人空间")}</ChoiceOption>}{!teams.length && !me.spaces.some(s => !s.team_id) && <ChoiceOption value="" disabled>{t('尚未加入团队')}</ChoiceOption>}{teams.map(t => <ChoiceOption key={String(t.id)} value={String(t.id)}>{String(t.name)}</ChoiceOption>)}</Choice></Label><p className="nav-label">{t("工作区")}</p><nav>{navigation.map(([id, title], index) => (id !== 'members' || owner || manager) && <button key={id} className={page === id ? 'selected' : ''} onClick={() => { history.replaceState(null, '', location.pathname + location.search); setPage(id); setTab(id === 'members' ? 'members' : id === 'devices' ? 'devices' : id === 'audit' ? 'audit' : 'projects'); setOffset(0); setNotice(''); setForm({}); }}>{React.createElement(navIcons[index], { size: 18 })}<span>{t(title)}</span></button>)}</nav><div className="account"><LanguageSwitch language={language} change={changeLanguage} /><small>{t("已登录")}</small><Button variant="ghost" onClick={() => void action(async () => { await api('/api/logout', {}); clearAccount(); })}>{t("退出登录")}</Button></div></aside><main className={page === 'memory' ? 'knowledge-main' : undefined}><header><div><p className="eyebrow">{t("LWC / 团队工作区")}</p><h1>{page === 'memory' && kind === 'page' ? selected?.name || t('知识库') : t(navigation.find(([id]) => id === page)?.[1] || '')}</h1><p className="muted">{t("管理团队记忆、成员与同步。")}</p></div><Button variant="outline" disabled={busy} onClick={() => void action(async () => { setDetail(null); await loadMe(); })}>{t("刷新")}</Button></header><div className="toolbar"><Label>{t("记忆空间")}<Choice value={space} onValueChange={e => { setSpace(e); setOffset(0); }}><ChoiceOption value="" disabled>{t("选择有权访问的空间")}</ChoiceOption>{visibleSpaces.map(s => <ChoiceOption key={s.id} value={s.id}>{s.name} · {t(names[s.role])}</ChoiceOption>)}</Choice></Label>{page === 'memory' && <Label>{t("记忆类型")}<Choice value={kind} onValueChange={e => { setKind(e); setOffset(0); }}>{['page', 'source', 'memory', 'discussion', 'semantic_relation', 'tag', 'source_revision', 'memory_audit', 'retrieval_weight', 'retrieval_feedback', 'ingest', 'draft_intent', 'work_audit'].map(k => <ChoiceOption key={k} value={k}>{t(names[k])}</ChoiceOption>)}</Choice></Label>}{page === 'tasks' && <Label>{t("任务类型")}<Choice value={kind === 'todo' ? 'todo' : 'plan'} onValueChange={e => { setKind(e); setOffset(0); }}><ChoiceOption value="plan">{t("计划")}</ChoiceOption><ChoiceOption value="todo">{t("待办")}</ChoiceOption></Choice></Label>}{['projects', 'members', 'devices', 'audit'].includes(page) && <Label>{t("查看")}<Choice value={tab} onValueChange={e => { setTab(e); setOffset(0); }}>{(page === 'projects' ? ['projects', 'collections'] : page === 'members' ? ['members', 'grants', 'policies'] : page === 'audit' ? ['audit', 'history'] : ['devices', 'agents', 'identities']).map(v => <ChoiceOption key={v} value={v}>{({ projects: t("项目"), collections: t("集合"), members: t("团队成员"), grants: t("空间授权"), policies: t("动作限制"), devices: t("我的设备"), agents: t("我的智能体"), identities: t("登录身份"), audit: t("成员活动"), history: t("记忆修改记录") } as Record<string, string>)[v]}</ChoiceOption>)}</Choice></Label>}</div>{status}
    {invitation && <Card className="card"><h2>{t('接受团队邀请')}</h2>{invitePreview && <p>{t('受邀加入团队')}：{String(invitePreview.team_name)}</p>}<p>{t('使用收到邀请的邮箱登录，即可加入团队。')}</p><Button disabled={busy || !invitePreview} onClick={() => void action(async () => { await manage({ action: 'invitation.accept', invitation_token: invitation }); sessionStorage.removeItem('lwc-invitation'); setInvitation(''); setNotice(t('已加入团队。只有明确授权的记忆空间才会显示。')); })}>{t('加入团队')}</Button><Button variant="ghost" disabled={busy} onClick={() => void action(async () => { await api('/api/logout', {}); clearAccount(); setChallenge(''); setForm({}); })}>{t('使用其他账号登录')}</Button><Button variant="ghost" onClick={dismissInvitation}>{t('暂不加入')}</Button></Card>}
    {deviceRequest && <Card className="card"><h2>{t('确认设备连接')}</h2>{devicePreview && <p>{String(devicePreview.name)}</p>}<p>{t('仅确认你刚刚发起的连接。确认后，该设备将使用你的账号访问已授权记忆。')}</p><div className="row"><Button disabled={busy || !devicePreview} onClick={() => void action(async () => { await api('/api/auth/device/approve', { user_code: deviceRequest }); sessionStorage.removeItem('lwc-device-request'); setDeviceRequest(''); setNotice(t('设备已连接，请返回智能体继续。')); })}>{t('确认连接')}</Button><Button variant="outline" onClick={() => { sessionStorage.removeItem('lwc-device-request'); setDeviceRequest(''); }}>{t('取消')}</Button></div></Card>}
    {page === 'resources' ? <ResourceLifecycle team={teams.find(row => row.id === team)} space={selected} language={language} t={t} api={api} changed={loadMe} /> : page === 'overview' ? <><section className="metrics"><Card className="card"><span>{t("可访问空间")}</span><strong>{visibleSpaces.length}</strong></Card><Card className="card"><span>{t("可管理空间")}</span><strong>{visibleSpaces.filter(s => s.role === 'manager').length}</strong></Card><Card className="card"><span>{t("已加入团队")}</span><strong>{teams.length}</strong></Card></section><Card className="card"><h2>{t("空间目录")}</h2><div className="space-grid">{visibleSpaces.map(item => <button className="space-card" key={item.id} onClick={() => { setSpace(item.id); setKind('page'); setPage('memory'); }}><span className="space-icon"><BookOpen size={22} /></span><ArrowUpRight className="space-arrow" size={18} /><strong>{item.name}</strong><span className="muted">{t('知识、经验与协作记忆')}</span><span className="space-card-footer"><Badge variant="secondary">{t(names[item.role])}</Badge><span>{t('打开知识库')} →</span></span></button>)}</div></Card><details className="card"><summary>{t("连接我的智能体")}</summary><p>{t("先配置服务端密钥，再登录并加入空间。密钥不替代成员授权。")}</p><Button variant="outline" disabled={!space} onClick={() => void action(async () => { await navigator.clipboard.writeText(`${t('请帮助我连接团队记忆。先单独配置管理员提供的接入密钥；通过私密标准输入传入，不要写进命令、日志或记忆。配置成功后，再分别登录和加入空间。')}\n\nlwc config server --server ${location.origin} --token-stdin\n\nlwc login --server ${location.origin}\n\nlwc space join ${space} --server ${location.origin}`); setNotice(t("连接说明已复制，请交给你的智能体。")); })}>{t("复制给智能体的连接说明")}</Button><p className="muted">{t('智能体会提供授权链接。打开后核对设备名称并确认，无需填写授权码。')}</p></details></> : page === 'memory' && kind === 'page' ? (space ? <KnowledgeBrowser key={`${me.user_id}-${space}`} userId={me.user_id} space={space} refresh={refresh} language={language} t={t} api={api} /> : <Card className="card empty">{t('选择一个有权访问的空间，开始阅读团队知识。')}</Card>) : <Card className="card list"><Table rows={rows} select={row => { if (page === 'memory' || page === 'tasks') void action(async () => { const result = await api(`/api/spaces/${space}/query`, { action: 'object', kind: row.kind, key: row.key }); setDetail(result.data); }); else setDetail(row); }} /><footer><span>{language === 'zh' ? `第 ${Math.floor(offset / 100) + 1} 页 · ${rows.length} 条记录` : `Page ${Math.floor(offset / 100) + 1} · ${rows.length} records`}</span><div className="row"><Button variant="outline" disabled={offset === 0} onClick={() => setOffset(v => Math.max(0, v - 100))}>{t("上一页")}</Button><Button variant="outline" disabled={!more} onClick={() => setOffset(v => v + 100)}>{t("下一页")}</Button></div></footer></Card>}
    <Dialog open={detail !== null} onOpenChange={open => { if (!open) setDetail(null); }}><DialogContent className="record-dialog sm:max-w-2xl max-h-[85vh] overflow-y-auto" showCloseButton={false}><DialogHeader><div className="row spread"><DialogTitle>{t(detail !== null && typeof detail === 'object' && 'invitation_token' in detail ? '邀请已准备好' : '记录详情')}</DialogTitle><Button variant="ghost" onClick={() => setDetail(null)}>{t('关闭')}</Button></div><DialogDescription>{t('查看内容与可用操作。')}</DialogDescription></DialogHeader><Detail value={detail} />
{page === 'audit' && tab === 'history' && selected?.role !== 'viewer' && detail !== null && typeof detail === 'object' && 'accepted_head' in detail && <Card className="card"><h2>{t("恢复修改")}</h2><p>{t("撤销这次修改，保留之后其他人新增的内容。内容有冲突时，由智能体继续合并。")}</p><Button disabled={busy} onClick={() => void action(async () => { setDetail(await api(`/api/spaces/${space}/recovery`, { action: 'preview', revert_head: (detail as Row).accepted_head })); })}>{t("准备恢复")}</Button></Card>}
{page === 'audit' && detail !== null && typeof detail === 'object' && 'preview_id' in detail && 'conflict_count' in detail && <Card className="card"><h2>{t("恢复预览")}</h2><p>{Number((detail as Row).conflict_count) ? t("这条记忆有重叠修改，需要智能体合并；原始内容已保留。") : t("已准备好恢复，原始记录和未同步内容都会保留。")}</p>{Number((detail as Row).conflict_count) === 0 && <Button disabled={busy} onClick={() => void action(async () => { const requestId = Array.from(crypto.getRandomValues(new Uint8Array(32)), n => n.toString(16).padStart(2, '0')).join(''); setDetail(await api(`/api/spaces/${space}/recovery`, { action: 'apply', preview_id: (detail as Row).preview_id, digest: (detail as Row).digest, request_id: requestId })); setNotice(t("已追加恢复修改版本")); })}>{t("应用恢复修改")}</Button>}</Card>}
{page === 'projects' && owner && detail !== null && typeof detail === 'object' && 'id' in detail && <Card className="card"><h2>{tab === 'collections' ? t("集合内的项目") : t("项目内的记忆空间")}</h2><div className="check-grid">{options.map(option => <Label key={String(option.id)}><Checkbox checked={links.includes(String(option.id))} disabled={!linkReady || busy} onCheckedChange={checked => setLinks(checked === true ? [...links, String(option.id)] : links.filter(id => id !== option.id))} />{String(option.name)}</Label>)}</div><Button disabled={!linkReady || busy} onClick={() => void action(() => manage({ action: tab === 'collections' ? 'collection.projects' : 'project.spaces', id: (detail as Row).id, expected_revision: (detail as Row).revision, items: links }))}>{t("保存关联")}</Button><p className="muted">{t("关联只改变目录组织，不扩大任何成员的记忆权限。")}</p></Card>}
{page === 'members' && manager && tab === 'grants' && detail !== null && typeof detail === 'object' && 'user_id' in detail && <Card className="card"><h2>{t("撤销空间授权")}</h2><p>{t("撤销后，后续服务端访问将被拒绝。已下载的本地数据无法远程收回。")}</p><Button variant="outline" disabled={busy} onClick={() => void action(() => manage({ action: 'space.revoke', space_id: space, user_id: (detail as Row).user_id, expected_revision: selected?.revision }))}>{t("撤销所选用户的空间授权")}</Button></Card>}
{page === 'devices' && tab !== 'identities' && detail !== null && typeof detail === 'object' && 'id' in detail && <Card className="card"><h2>{t('撤销连接')}</h2><p>{t('后续服务端请求将被拒绝；已下载数据无法远程收回。')}</p><Button variant="outline" disabled={busy || Boolean((detail as Row).revoked)} onClick={() => void action(async () => { await manage({ action: tab === 'agents' ? 'agent.revoke' : 'device.revoke', id: (detail as Row).id }); setDetail(null); })}>{t('撤销所选连接')}</Button></Card>}
{page === 'members' && tab === 'members' && owner && detail !== null && typeof detail === 'object' && 'user_id' in detail && <Card className="card"><h2>{t("移除团队成员")}</h2><p>{t("将撤销此成员的空间授权和副本连接。最后一位所有者或空间管理者不能被移除。")}</p><Button variant="outline" disabled={busy} onClick={() => void action(async () => { await manage({ action: 'member.remove', team_id: team, user_id: (detail as Row).user_id, expected_revision: teams.find(t => t.id === team)?.revision }); setDetail(null); })}>{t("移除所选成员")}</Button></Card>}
    {page === 'members' && manager && tab === 'policies' && detail !== null && typeof detail === 'object' && 'logical_key' in detail && <Button variant="outline" disabled={busy} onClick={() => void action(async () => { await changePolicy(detail as Row); setDetail(null); })}>{t('移除此条限制')}</Button>}
    {page === 'devices' && tab === 'identities' && detail !== null && typeof detail === 'object' && 'provider' in detail && <Button variant="outline" disabled={busy} onClick={() => void action(async () => { await manage({ action: 'identity.unlink', provider: (detail as Row).provider, namespace: (detail as Row).namespace, subject: (detail as Row).subject }); setDetail(null); })}>{t('解绑所选身份')}</Button>}
    {detail !== null && typeof detail === 'object' && 'personal_key' in detail && <><p>{t('密钥仅在本次显示，请复制并通过可信渠道交给对应成员。')}</p><Input type="password" readOnly value={String((detail as Row).personal_key)} aria-label={t('个人访问密钥')} /><Button onClick={() => void action(async () => { await navigator.clipboard.writeText(String((detail as Row).personal_key)); setNotice(t('密钥已复制')); })}>{t('复制密钥')}</Button></>}
    {page === 'keys' && detail !== null && typeof detail === 'object' && 'id' in detail && !(detail as Row).revoked && <Button variant="outline" disabled={busy} onClick={() => void action(() => manage({ action: 'key.revoke', key_id: (detail as Row).id }))}>{t('撤销此密钥')}</Button>}
    {detail !== null && typeof detail === 'object' && 'invitation_token' in detail && <><p>{t('将邀请链接发给该成员。链接仅能由指定邮箱使用。')}</p><p className="muted">{language === 'zh' ? `有效期 ${Math.ceil(Number((detail as Row).expires_in) / 86400)} 天` : `Valid for ${Math.ceil(Number((detail as Row).expires_in) / 86400)} days`}</p><Button onClick={() => void action(async () => { await navigator.clipboard.writeText(`${location.origin}/#invite=${String((detail as Row).invitation_token)}`); setNotice('邀请链接已复制'); })}>{t('复制邀请链接')}</Button></>}
    </DialogContent></Dialog>
    {page === 'members' && manager && tab === 'policies' && <Card className="card"><h2>{t("增加动作限制")}</h2><form className="inline-form" onSubmit={e => { e.preventDefault(); void action(() => changePolicy()); }}>{memberPicker('policy_user')}<Label>{t("对象类型")}<Choice value={form.policy_kind || '*'} onValueChange={e => { setForm({ ...form, policy_kind: e, policy_key: '*' }); setObjectOffset(0); }}>{['*', 'page', 'source', 'memory', 'todo', 'plan', 'discussion', 'source_revision', 'memory_audit', 'draft_intent', 'work_audit'].map(v => <ChoiceOption key={v} value={v}>{t(names[v])}</ChoiceOption>)}</Choice></Label><Label>{t('适用记忆')}<Choice value={form.policy_key || '*'} disabled={!form.policy_kind || form.policy_kind === '*'} onValueChange={e => setForm({ ...form, policy_key: e })}><ChoiceOption value="*">{t('该类型的全部记忆')}</ChoiceOption>{objects.map(object => <ChoiceOption key={String(object.key)} value={String(object.key)}>{String(object.title)}</ChoiceOption>)}</Choice>{(objectOffset > 0 || objectsMore) && <span className="row"><Button type="button" variant="ghost" disabled={objectOffset === 0} onClick={() => setObjectOffset(v => Math.max(0, v - 100))}>{t('上一页')}</Button><Button type="button" variant="ghost" disabled={!objectsMore} onClick={() => setObjectOffset(v => v + 100)}>{t('下一页')}</Button></span>}</Label><Label>{t("禁止动作")}<Choice value={form.policy_action || 'update'} onValueChange={e => setForm({ ...form, policy_action: e })}>{Object.entries({ create: t("创建"), update: t("修改"), delete: t("删除"), compact: t("压缩原始记忆"), rollback: t("回滚恢复"), export: t("导出"), '*': t("全部写动作") }).map(([v, label]) => <ChoiceOption key={v} value={v}>{label}</ChoiceOption>)}</Choice></Label><Button disabled={busy || !form.policy_user}>{t("增加限制")}</Button></form>{detail !== null && typeof detail === 'object' && 'logical_key' in detail && <Button variant="outline" disabled={busy} onClick={() => void action(() => changePolicy(detail as Row))}>{t("移除此条限制")}</Button>}</Card>}
    {page === 'overview' && <details className="card" open={teams.length === 0}><summary>{t('创建团队')}</summary><p className="muted">{t('为团队命名，然后创建记忆空间并邀请成员。')}</p><form className="inline-form" onSubmit={e => { e.preventDefault(); void action(() => manage({ action: 'team.create', name: form.team_name })); }}>{field('team_name', t('团队名称'))}<Button disabled={busy}>{t('创建团队')}</Button></form></details>}
    {page === 'overview' && team && visibleSpaces.length === 0 && <Alert><AlertDescription>{t(owner ? '先创建一个记忆空间，再邀请成员一起使用。' : '你已加入团队，尚未获得记忆空间权限。请联系团队管理员授权。')}</AlertDescription></Alert>}
    {page === 'projects' && owner && <Card className="card"><h2>{t("新建")}{tab === 'collections' ? t("集合") : t("项目")}</h2><form className="inline-form" onSubmit={e => { e.preventDefault(); void action(() => manage({ action: tab === 'collections' ? 'collection.create' : 'project.create', team_id: team, name: form.name })); }}>{field('name', t("名称"))}<Button disabled={busy}>{t("创建")}</Button></form><p className="muted">{t("项目与集合用于组织目录，不会自动授予记忆读取权限。")}</p></Card>}
    {page === 'overview' && owner && <Card className="card"><h2>{t("创建团队记忆空间")}</h2><form className="inline-form" onSubmit={e => { e.preventDefault(); void action(() => manage({ action: 'space.create', team_id: team, name: form.name })); }}>{field('name', t("空间名称"))}<Button disabled={busy}>{t("创建空间")}</Button></form></Card>}
    {page === 'members' && owner && <Card className="card"><h2>{t('开通团队成员')}</h2><p className="muted">{t('选择成员可访问的空间，开通时一并完成授权。未选择的空间保持不可访问。')}</p><form className="inline-form" onSubmit={e => { e.preventDefault(); void action(async () => { const result = await api('/api/manage', { action: 'member.create', team_id: team, name: form.member_name, days: Number(form.key_days || 90), grants: Object.entries(initialGrants).map(([space_id, role]) => ({ space_id, role, expected_revision: me.spaces.find(s => s.id === space_id)?.revision })) }); setDetail(result); setForm({}); setInitialGrants({}); await loadMe(); }); }}>{field('member_name', t('成员昵称'))}<Label>{t('密钥有效期')}<Choice value={form.key_days || '90'} onValueChange={value => setForm({ ...form, key_days: value })}>{[7,30,90,365].map(days => <ChoiceOption key={days} value={String(days)}>{days} {t('天')}</ChoiceOption>)}</Choice></Label><fieldset className="initial-grants"><legend>{t('初始空间权限')}</legend>{visibleSpaces.filter(s => s.role === 'manager').map(s => <div className="initial-grant" key={s.id}><Label><Checkbox checked={!!initialGrants[s.id]} onCheckedChange={checked => setInitialGrants(current => { const next = { ...current }; if (checked) next[s.id] = 'viewer'; else delete next[s.id]; return next; })} />{s.name}</Label>{initialGrants[s.id] && <Choice label={`${s.name} ${t('权限')}`} value={initialGrants[s.id]} onValueChange={role => setInitialGrants(current => ({ ...current, [s.id]: role }))}>{['viewer', 'editor', 'manager'].map(role => <ChoiceOption key={role} value={role}>{t(names[role])}</ChoiceOption>)}</Choice>}</div>)}{!visibleSpaces.some(s => s.role === 'manager') && <p className="muted">{t('你目前没有可授权的空间，可稍后由空间管理者授权。')}</p>}</fieldset><Button disabled={busy}>{t('开通并生成密钥')}</Button></form></Card>}
    {page === 'keys' && <Card className="card"><h2>{t('创建我的访问密钥')}</h2><p className="muted">{t('密钥继承你的现有权限；撤销会同时断开由该密钥授权的会话与智能体。')}</p><form className="inline-form" onSubmit={e => { e.preventDefault(); void action(async () => { const result = await api('/api/manage', { action: 'key.create', name: form.key_name, days: Number(form.key_days || 90) }); setForm({}); setDetail(result); }); }}>{field('key_name', t('用途名称'))}<Label>{t('密钥有效期')}<Choice value={form.key_days || '90'} onValueChange={value => setForm({ ...form, key_days: value })}>{[7,30,90,365].map(days => <ChoiceOption key={days} value={String(days)}>{days} {t('天')}</ChoiceOption>)}</Choice></Label><Button disabled={busy}>{t('生成密钥')}</Button></form></Card>}
    {page === 'members' && <>{owner && <Card className="card"><h2>{t("邀请团队成员")}</h2><form className="inline-form" onSubmit={e => { e.preventDefault(); void action(async () => { const result = await api('/api/manage', { action: 'invitation.create', team_id: team, email: form.email }); setDetail({ invitation_token: result.invitation_token, expires_in: result.expires_in }); }); }}>{field('email', t("成员邮箱"), 'email')}<Button disabled={busy}>{t("创建邀请")}</Button></form></Card>}{manager && <Card className="card"><h2>{t("空间授权")}</h2><form className="inline-form" onSubmit={e => { e.preventDefault(); void action(() => manage({ action: 'space.grant', space_id: space, user_id: form.user_id, role: form.role || 'viewer', expected_revision: selected?.revision })); }}>{memberPicker('user_id')}<Label>{t("角色")}<Choice value={form.role || 'viewer'} onValueChange={e => setForm({ ...form, role: e })}>{['viewer', 'editor', 'manager'].map(v => <ChoiceOption key={v} value={v}>{t(names[v])}</ChoiceOption>)}</Choice></Label><Button disabled={busy || !form.user_id}>{t("保存授权")}</Button></form></Card>}</>}
    {page === 'devices' && tab === 'identities' && <Card className="card"><h2>{t("登录身份")}</h2><p>{t("通过当前账号显式绑定新身份。始终保留至少一种登录方式。")}</p><div className="row">{['github', 'feishu'].filter(p => providers[p]).map(p => <Button key={p} variant="outline" onClick={() => location.assign(`/api/auth/${p}/start?link=true`)}>{t("绑定")}{p === 'github' ? t("代码托管账号") : t("飞书")}</Button>)}</div>{providers.email && <form className="inline-form" onSubmit={e => { e.preventDefault(); void action(async () => { if (!challenge) { const result = await api('/api/auth/email/challenge', { email: form.link_email, link: true }); setChallenge(result.challenge_id); setNotice(t("验证码已发送")); } else { await api('/api/auth/email/verify', { challenge_id: challenge, code: form.link_code }); setChallenge(''); setForm({}); setNotice(t("邮箱已绑定")); } }); }}>{field('link_email', t("绑定邮箱"), 'email')}{challenge && field('link_code', t("验证码"))}<Button disabled={busy}>{challenge ? t("验证并绑定") : t("发送验证码")}</Button></form>}{detail !== null && typeof detail === 'object' && 'provider' in detail && <Button variant="outline" disabled={busy} onClick={() => void action(async () => { await manage({ action: 'identity.unlink', provider: (detail as Row).provider, namespace: (detail as Row).namespace, subject: (detail as Row).subject }); setDetail(null); })}>{t("解绑所选身份")}</Button>}</Card>}
  </main></div>;
}
function Root() {
  const [language, setLanguage] = useState<Language>(() => { try { return localStorage.getItem('lwc-language') === 'en' ? 'en' : 'zh'; } catch { return 'zh'; } });
  useEffect(() => { document.documentElement.lang = language === 'zh' ? 'zh-CN' : 'en'; document.title = language === 'zh' ? 'LWC · 团队记忆' : 'LWC · Team Memory'; try { localStorage.setItem('lwc-language', language); } catch { /* Private browser storage may be unavailable. */ } }, [language]);
  return <Locale.Provider value={language}><App language={language} changeLanguage={setLanguage} /></Locale.Provider>;
}
createRoot(document.getElementById('root')!).render(<Root />);
