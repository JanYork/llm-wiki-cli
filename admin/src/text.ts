// Interface text is Chinese; stored user content is displayed unchanged.
export const labels: Record<string, string> = {
  id: '标识', user_id: '用户', name: '名称', title: '标题', role: '角色', member: '团队成员', revision: '版本', nickname: '昵称', email_hint: '登记邮箱', device: '设备', device_id: '设备标识', agent_id: '智能体标识', registered_at: '登记时间', last_seen: '最近连接', ack_head: '已确认版本', revoked: '已撤销', kind: '类型', key: '对象', logical_key: '对象', action: '动作', target: '目标', actor: '操作者', created_at: '创建时间', updated_at: '更新时间', provider: '登录方式', subject: '身份', team_id: '团队', metadata_json: '机器资料', namespace: '身份范围',
  page: '知识页', source: '来源', memory: '时序记忆', discussion: '讨论', semantic_relation: '语义关系', tag: '标签', source_revision: '来源历史', memory_audit: '记忆审计', retrieval_weight: '检索权重', retrieval_feedback: '检索反馈', ingest: '采集记录', draft_intent: '草稿意图', work_audit: '执行审计', plan: '计划', todo: '待办',
  viewer: '只读成员', editor: '编辑者', manager: '管理者', owner: '所有者', github: '代码托管账号', feishu: '飞书', email: '邮箱',
  create: '创建', update: '修改', delete: '删除', compact: '压缩', rollback: '恢复', export: '导出', '*': '全部',
  pending_conflicts: '待合并内容', unknown: '等待首次同步', offline: '暂未连接', recovery_required: '等待安全恢复', status: '状态', state: '状态', pending: '待处理', in_progress: '进行中', completed: '已完成', active: '有效', blocked: '受阻', abandoned: '已放弃', cancelled: '已取消', resolved: '已解决', conflict: '有冲突', synced: '已同步', current: '已是最新', retry: '等待重试',
  body: '正文', summary: '摘要', content: '内容', payload: '记忆内容', hash: '校验摘要', digest: '校验摘要', provenance: '来源依据', links: '关联', tags: '标签', sources: '来源', slug: '页面标识', description: '说明', objective: '目标', constraints: '约束', steps: '步骤', result: '结果', evidence: '证据', items: '条目', revisions: '修订', history: '历史', messages: '消息', text: '内容', ordinal: '顺序', verify: '验收条件', done_when: '完成标准', event_type: '事件类型', context: '上下文', confidence: '可信度', relation_type: '关系类型', target_id: '目标标识', source_id: '来源标识', source_hash: '来源摘要', content_hash: '内容摘要', content_text: '内容正文', fragments: '记忆片段', changes: '变更', feedback: '反馈', relations: '关系',
  accepted_head: '提交版本', accepted_digest: '提交摘要', committed: '已提交', artifact_id: '快照标识', replica_id: '副本标识', batch_id: '批次标识', server_epoch: '服务代次', parent: '前一版本', principal: '执行主体', recovery: '恢复记录', revert_head: '撤销版本', preview_id: '恢复预览标识', expected_head: '预期版本', conflict_count: '冲突数量', conflicts: '冲突材料', expires_at: '失效时间', expires_in: '有效秒数', next_action: '下一步', apply: '应用恢复', resolve: '处理冲突', requires_human_approval: '需要人工审批', invitation_token: '邀请密钥',
  'agent-observed': '智能体观察', 'user-provided': '用户提供', 'source-derived': '来源归纳', 'agent-inferred': '智能体推断',
  'team.create': '创建团队', 'project.create': '创建项目', 'collection.create': '创建集合', 'project.spaces': '关联项目空间', 'collection.projects': '关联集合项目', 'invitation.create': '创建邀请', 'invitation.accept': '接受邀请', 'member.remove': '移除成员', 'identity.unlink': '解绑身份', 'space.create': '创建空间', 'space.grant': '授予空间权限', 'space.revoke': '撤销空间权限', 'space.policy': '更新动作限制', 'agent.revoke': '撤销智能体', 'device.revoke': '撤销设备', bootstrap: '初始化服务',
};
export const errors: Record<string, string> = {
  invalid_invitation: '邀请已失效，或当前账号尚未验证受邀邮箱。请使用受邀邮箱登录，或在登录身份中绑定该邮箱。',
  invalid_personal_key: '个人密钥不正确、已过期或已撤销。',
  network_unavailable: '暂时无法连接服务，请稍后重试。',
  unauthorized: '登录已失效，请重新登录。', server_token_required: '请先配置服务端接入密钥。', forbidden: '当前账号或智能体没有此操作的权限。', invalid_server_token: '服务端接入密钥不正确。', rate_limited: '操作过于频繁，请稍后重试。', revision_conflict: '记录已更新，请刷新后重试。', head_changed: '记忆版本已变化，请重新准备恢复。', provider_unavailable: '此登录方式或服务尚未配置。', login_failed: '登录验证失败，请重新发起登录。', invalid_email: '请输入有效的邮箱地址。', last_identity: '至少需要保留一种登录方式。', last_owner: '团队必须保留一位所有者。', last_manager: '请先指定另一位空间管理者。', recovery_expired: '恢复预览已过期，请重新准备。', recovery_conflicts: '请先由智能体处理所有恢复冲突。', space_quota_exceeded: '空间存储容量不足，请联系部署管理员。', query_result_too_large: '记录超过单次读取容量，请通过命令行分段读取。', database_error: '数据操作失败，请检查服务端运行状态。', invalid_request: '请求内容不完整，请检查输入。', invitation_invalid: '邀请密钥已失效或不适用于当前账号。',
};
export function errorText(code: unknown) { return errors[String(code)] || '操作未完成，请检查输入、权限和服务状态后重试。'; }
export function display(value: unknown, field = '', language: Language = 'zh'): string {
  if (value == null) return '—';
  if (field === 'revoked' && typeof value === 'number') return translate(value ? '是' : '否', language);
  if (['created_at', 'updated_at', 'expires_at', 'last_seen', 'registered_at'].includes(field) && (typeof value === 'number' || typeof value === 'string')) { const date = new Date(typeof value === 'number' ? value * 1000 : value); if (!Number.isNaN(date.valueOf())) return date.toLocaleString(language === 'zh' ? 'zh-CN' : 'en-US', { dateStyle: 'medium', timeStyle: 'short' }); }
  if (typeof value === 'boolean') return translate(value ? '是' : '否', language);
  if (['kind','role','provider','status','state','action','next_action','provenance'].includes(field) && typeof value === 'string') return translate(labels[value] || '其他', language);
  if (typeof value === 'object') return Array.isArray(value) ? `${value.length} ${language === 'zh' ? '项' : 'items'}` : translate('查看详情', language);
  return String(value);
}
export type Language = 'zh' | 'en';
export const english: Record<string, string> = {
  "暂不加入":"Not now",
"搜索知识页面":"Search knowledge pages",
"搜索标题和正文":"Search titles and content",
"清除搜索":"Clear search",
"搜索当前空间，最多显示 100 条相关页面":"Search this space \u00b7 up to 100 relevant pages",
"重试":"Retry",
"没有找到匹配的页面，试试其他关键词。":"No matching pages. Try another search.",
"查看全部页面":"Browse all pages",
"本文目录":"On this page",
"尚未加入团队":"No teams yet",

"知识库":"Knowledge base",
"概念":"Concept",
"实体":"Entity",
"概览":"Overview",
"规范":"Schema",
"流程":"Procedure",
"决策":"Decision",
"参考":"Reference",
"指南":"Guide",
"知识页面":"Pages",
"阅读团队沉淀的知识与经验":"Explore your team\u2019s knowledge and experience",
"本页知识":"Pages in this batch",
"知识分类":"Categories",
"最近更新":"Last updated",
"分类分布":"Categories at a glance",
"页面一览":"Browse pages",
"还没有知识页面。智能体同步后，内容会出现在这里。":"No pages yet. Knowledge will appear here after your agent syncs.",
"筛选当前页":"Filter this page",
"没有匹配的知识页面":"No matching pages",
"正在加载知识…":"Loading knowledge\u2026",
"更新于":"Updated",
"相关知识":"Related pages",
"团队知识，从这里开始":"Your team\u2019s knowledge starts here",
"正在读取所选页面":"Opening the selected page",
"选择左侧页面阅读；智能体同步的新知识会出现在这里。":"Select a page on the left. New knowledge will appear here after your agent syncs.",
"知识、经验与协作记忆":"Knowledge, experience and shared memory",
"打开知识库":"Open knowledge base",
"选择一个有权访问的空间，开始阅读团队知识。":"Choose an accessible space to start reading your team\u2019s knowledge.",

  '总览':'Overview','知识与记忆':'Knowledge & memory','计划与待办':'Plans & to-dos','项目与集合':'Projects & collections','同步与副本':'Sync & replicas','成员与授权':'Members & access','设备与智能体':'Devices & agents','审计记录':'Audit log',
  '标识':'ID','用户':'User','名称':'Name','标题':'Title','角色':'Role','版本':'Revision','昵称':'Nickname','登记邮箱':'Registered email','设备':'Device','设备标识':'Device ID','智能体标识':'Agent ID','登记时间':'Registered','最近连接':'Last seen','已确认版本':'Acknowledged head','已撤销':'Revoked','类型':'Type','对象':'Object','动作':'Action','目标':'Target','操作者':'Actor','创建时间':'Created','更新时间':'Updated','登录方式':'Sign-in method','身份':'Identity','团队':'Team','机器资料':'Machine information','身份范围':'Identity scope',
  '知识页':'Wiki page','来源':'Source','时序记忆':'Temporal memory','讨论':'Discussion','语义关系':'Semantic relation','标签':'Tag','来源历史':'Source history','记忆审计':'Memory audit','检索权重':'Retrieval weight','检索反馈':'Retrieval feedback','采集记录':'Ingestion','草稿意图':'Draft intent','执行审计':'Work audit','计划':'Plan','待办':'To-do',
  '只读成员':'Reader','编辑者':'Editor','管理者':'Manager','所有者':'Owner','代码托管账号':'GitHub','飞书':'Feishu','邮箱':'Email','创建':'Create','修改':'Update','删除':'Delete','压缩':'Compact','恢复':'Restore','导出':'Export','全部':'All',
  '状态':'Status','待处理':'Pending','进行中':'In progress','已完成':'Completed','有效':'Active','受阻':'Blocked','已放弃':'Abandoned','已取消':'Cancelled','已解决':'Resolved','有冲突':'Conflict','已同步':'Synced','已是最新':'Up to date','等待重试':'Retry pending',
  '正文':'Body','摘要':'Summary','内容':'Content','记忆内容':'Memory content','校验摘要':'Checksum','来源依据':'Provenance','关联':'Links','页面标识':'Page ID','说明':'Description','约束':'Constraints','步骤':'Steps','结果':'Result','证据':'Evidence','条目':'Items','修订':'Revisions','历史':'History','消息':'Messages','顺序':'Order','验收条件':'Acceptance criteria','完成标准':'Completion criteria','事件类型':'Event type','上下文':'Context','可信度':'Confidence','关系类型':'Relation type','目标标识':'Target ID','来源标识':'Source ID','来源摘要':'Source checksum','内容摘要':'Content checksum','内容正文':'Source content','记忆片段':'Fragments','变更':'Changes','反馈':'Feedback','关系':'Relations',
  '提交版本':'Committed head','提交摘要':'Commit checksum','已提交':'Committed','快照标识':'Snapshot ID','副本标识':'Replica ID','批次标识':'Batch ID','服务代次':'Server epoch','前一版本':'Parent head','执行主体':'Principal','恢复记录':'Recovery record','撤销版本':'Reverted head','恢复预览标识':'Recovery preview ID','预期版本':'Expected head','冲突数量':'Conflict count','冲突材料':'Conflict materials','失效时间':'Expires','有效秒数':'Lifetime in seconds','下一步':'Next action','应用恢复':'Apply recovery','处理冲突':'Resolve conflicts','需要人工审批':'Human approval required','邀请密钥':'Invitation token',
  '智能体观察':'Agent observation','用户提供':'User supplied','来源归纳':'Source derived','智能体推断':'Agent inference','创建团队':'Create team','创建项目':'Create project','创建集合':'Create collection','关联项目空间':'Update project spaces','关联集合项目':'Update collection projects','创建邀请':'Create invitation','接受邀请':'Accept invitation','移除成员':'Remove member','解绑身份':'Unlink identity','创建空间':'Create space','授予空间权限':'Grant space access','撤销空间权限':'Revoke space access','更新动作限制':'Update restrictions','撤销智能体':'Revoke agent','撤销设备':'Revoke device','初始化服务':'Initialize server',
  '登录已失效，请重新登录。':'Your session expired. Sign in again.','请先配置服务端接入密钥。':'Configure the server access token first.','当前账号或智能体没有此操作的权限。':'This account or agent does not have permission.','服务端接入密钥不正确。':'The server access token is incorrect.','操作过于频繁，请稍后重试。':'Too many requests. Try again later.','记录已更新，请刷新后重试。':'The record changed. Refresh and retry.','记忆版本已变化，请重新准备恢复。':'Memory changed. Prepare a new recovery preview.','此登录方式或服务尚未配置。':'This sign-in method or service is not configured.','登录验证失败，请重新发起登录。':'Verification failed. Start sign-in again.','请输入有效的邮箱地址。':'Enter a valid email address.','至少需要保留一种登录方式。':'Keep at least one sign-in method.','团队必须保留一位所有者。':'The team must retain an owner.','请先指定另一位空间管理者。':'Assign another space manager first.','恢复预览已过期，请重新准备。':'The preview expired. Prepare another preview.','请先由智能体处理所有恢复冲突。':'An agent must resolve all recovery conflicts first.','空间存储容量不足，请联系部署管理员。':'Space storage is full. Contact the deployment administrator.','记录超过单次读取容量，请通过命令行分段读取。':'This record exceeds the read limit. Read it in chunks through the CLI.','数据操作失败，请检查服务端运行状态。':'The data operation failed. Check server status.','请求内容不完整，请检查输入。':'The request is incomplete. Check your input.','邀请密钥已失效或不适用于当前账号。':'The invitation expired or is not for this account.','操作未完成，请检查输入、权限和服务状态后重试。':'The operation failed. Check your input, permissions and server status.','网络连接失败，请检查服务地址。':'Connection failed. Check the server address.',
  '当前范围暂无记录':'No records in this scope','属性':'Field','详情':'Details','查看':'View','加载失败':'Failed to load','关联超过当前页面容量，请通过管理接口分页查看后编辑。':'There are too many links for this editor. Use the paginated management API.','关联加载失败':'Failed to load links','正在处理…':'Working…','正在连接…':'Connecting…','团队记忆':'Team memory','登录团队工作区':'Sign in to your workspace','连接团队服务':'Connect to your team server','登录后仅能访问明确授权的空间。':'You can access only explicitly authorized spaces.','请输入部署管理员提供的服务端接入密钥。':'Enter the server access token supplied by your administrator.','验证码已发送':'Verification code sent','服务端密钥':'Server access token','验证码':'Verification code','本实例未启用邮箱登录。':'Email sign-in is disabled on this server.','连接服务端':'Connect','验证并登录':'Verify and sign in','发送验证码':'Send code','代码托管账号登录':'Sign in with GitHub','飞书登录':'Sign in with Feishu','当前团队':'Current team','个人空间':'Personal spaces','工作区':'Workspace','账号 ·':'Account ·','退出登录':'Sign out','LWC / 团队工作区':'LWC / Team workspace','本地优先 · 程序自主同步 · 智能体自主合并':'Local first · Automatic sync · Agent-managed merging','刷新':'Refresh','记忆空间':'Memory space','选择有权访问的空间':'Select an authorized space','记忆类型':'Memory type','任务类型':'Task type','项目':'Project','集合':'Collection','团队成员':'Team members','空间授权':'Space access','动作限制':'Restrictions','我的设备':'My devices','我的智能体':'My agents','登录身份':'Sign-in identities','治理审计':'Administration audit','记忆版本历史':'Memory history','可访问空间':'Accessible spaces','可管理空间':'Managed spaces','已加入团队':'Joined teams','空间目录':'Space directory','连接命令行与智能体':'Connect CLI and agents','先配置服务端密钥，再登录并加入空间。密钥不替代成员授权。':'Configure the server access token, sign in, and join a space. The token does not replace member permissions.','连接命令已复制':'Connection commands copied','复制连接命令':'Copy connection commands','设备已授权':'Device authorized','命令行设备授权码':'CLI device code','授权设备':'Authorize device','第':'Page','页 · 当前':'· Showing','条':'records','上一页':'Previous','下一页':'Next','记录详情':'Record details','关闭':'Close','补偿恢复':'Compensating recovery','撤销此提交的变化，保留之后的无关编辑。出现重叠冲突时交给智能体合并。':'Reverse this commit while keeping later unrelated edits. An agent resolves overlapping changes.','准备恢复':'Prepare recovery','恢复预览':'Recovery preview','存在重叠修改。智能体可通过恢复工具读取材料并自主合并。':'Overlapping changes exist. An agent can read the recovery materials and merge them.','没有未解决冲突，可追加补偿版本。历史记录与本地待同步内容将保留。':'No unresolved conflicts. Apply a compensating commit while preserving history and local pending changes.','已追加补偿恢复版本':'Compensating recovery committed','应用补偿恢复':'Apply recovery','集合内的项目':'Projects in this collection','项目内的记忆空间':'Spaces in this project','保存关联':'Save links','关联只改变目录组织，不扩大任何成员的记忆权限。':'Links organize the directory without granting memory access.','增加动作限制':'Add restriction','空间成员标识':'Space member ID','对象类型':'Object type','对象标识（* 表示全部）':'Object ID (* means all)','禁止动作':'Denied action','压缩原始记忆':'Compact original memory','回滚恢复':'Rollback and recovery','全部写动作':'All write actions','增加限制':'Add restriction','移除此条限制':'Remove this restriction','撤销空间授权':'Revoke space access','撤销后，后续服务端访问将被拒绝。已下载的本地数据无法远程收回。':'Future requests will be denied. Previously downloaded data cannot be recalled.','撤销所选用户的空间授权':'Revoke selected user’s access','接受团队邀请':'Accept team invitation','加入团队':'Join team','需要使用邀请指定的已验证邮箱；加入后仍需明确授权空间。':'Use the verified email addressed by the invitation. Space access still requires an explicit grant.','新建':'Create ','项目与集合用于组织目录，不会自动授予记忆读取权限。':'Projects and collections organize the directory without granting access.','创建团队记忆空间':'Create team memory space','空间名称':'Space name','邀请团队成员':'Invite a team member','被邀请者已验证邮箱':'Invitee’s verified email','团队用户标识':'Team user ID','保存授权':'Save access','撤销连接':'Revoke connection','撤销':'Revoke','后续服务端请求将被拒绝；已下载数据无法远程收回。':'Future requests will be denied. Downloaded data cannot be recalled.','移除团队成员':'Remove team member','将撤销此成员的空间授权和副本连接。最后一位所有者或空间管理者不能被移除。':'Revokes this member’s space access and replicas. The last owner or space manager cannot be removed.','移除所选成员':'Remove selected member','通过当前账号显式绑定新身份。始终保留至少一种登录方式。':'Explicitly link identities to this account. Keep at least one sign-in method.','绑定':'Link ','邮箱已绑定':'Email linked','绑定邮箱':'Email to link','验证并绑定':'Verify and link','解绑所选身份':'Unlink selected identity','已保存':'Saved','请求失败':'Request failed','是':'Yes','否':'No','其他':'Other','查看详情':'View details',
};
export function translate(text: string, language: Language) { return language === 'en' ? english[text] ?? text : text; }
Object.assign(english, {
  '记忆': 'Memory', '同步状态': 'Sync status', '活动记录': 'Activity', '管理团队记忆、成员与同步。': 'Manage your team’s memory, members and sync.', '已登录': 'Signed in', '选择成员': 'Choose a member', '请选择成员': 'Select a member', '适用记忆': 'Memory scope', '该类型的全部记忆': 'All memories of this type', '撤销所选连接': 'Disconnect selected device or agent', '连接我的智能体': 'Connect my agent', '恢复修改': 'Undo changes', '已追加恢复修改版本': 'Changes restored', '应用恢复修改': 'Restore changes', '成员活动': 'Member activity', '记忆修改记录': 'Memory changes', '撤销这次修改，保留之后其他人新增的内容。内容有冲突时，由智能体继续合并。': 'Undo these changes while keeping later additions. An agent will handle any overlapping edits.', '已准备好恢复，原始记录和未同步内容都会保留。': 'Ready to restore. Original records and unsynced changes will be preserved.', '这条记忆有重叠修改，需要智能体合并；原始内容已保留。': 'This memory has overlapping edits. An agent needs to merge them; the originals are preserved.', '中文':'Chinese', '英文':'English', '界面语言':'Language', 'LWC · 团队记忆':'LWC · Team Memory',
});

Object.assign(english, { '搜索姓名或邮箱': 'Search by name or email', '未找到成员': 'No matching members', '查看内容与可用操作。': 'View content and available actions.' });

Object.assign(english, { '待合并内容': 'Pending merges', '等待首次同步': 'Awaiting first sync', '暂未连接': 'Not connected', '等待安全恢复': 'Awaiting safe recovery' });

Object.assign(english, { '使用收到邀请的邮箱登录，即可加入团队。': 'Sign in with the invited email to join the team.', '将邀请链接发给该成员。链接仅能由指定邮箱使用。': 'Send this invitation link to the member. Only the invited email can use it.', '邀请链接已复制': 'Invitation link copied', '复制邀请链接': 'Copy invitation link' });

Object.assign(english, { '暂时无法连接服务，请稍后重试。': 'Unable to reach the service. Please try again shortly.' });

Object.assign(english, {
  '请选择下方的登录方式。': 'Choose a sign-in method below.',
  '团队服务尚未配置登录方式，请联系部署管理员启用邮箱或账号登录。': 'Sign-in is not configured yet. Ask your deployment administrator to enable email or account sign-in.',
  '创建团队': 'Create team', '团队名称': 'Team name',
  '为团队命名，然后创建记忆空间并邀请成员。': 'Name your team, then create a memory space and invite members.',
});

Object.assign(english, {
  '请帮助我连接团队记忆。先单独配置管理员提供的接入密钥；通过私密标准输入传入，不要写进命令、日志或记忆。配置成功后，再分别登录和加入空间。': 'Help me connect to team memory. Configure the administrator-provided access token separately using private standard input; never place it in commands, logs or memory. After configuration succeeds, sign in and join the space as separate steps.',
  '连接说明已复制，请交给你的智能体。': 'Connection instructions copied. Share them with your agent.',
  '复制给智能体的连接说明': 'Copy instructions for my agent',
});

Object.assign(english, { '邀请已准备好': 'Invitation ready' });

Object.assign(english, {
 '邀请已失效，或当前账号尚未验证受邀邮箱。请使用受邀邮箱登录，或在登录身份中绑定该邮箱。': 'This invitation expired or your account has not verified the invited email. Sign in with that email or link it under Sign-in identities.',
 '受邀加入团队': 'Invited to team', '你收到了一份团队邀请。': 'You have a team invitation.', '请使用收到邀请的邮箱登录。': 'Sign in with the email that received the invitation.',
 '登录后确认你刚刚发起的设备连接。': 'Sign in to confirm the device connection you just requested.',
 '智能体会提供授权链接。打开后核对设备名称并确认，无需填写授权码。': 'Your agent will provide an authorization link. Open it, check the device name and confirm. No code entry is needed.',
 '已加入团队。只有明确授权的记忆空间才会显示。': 'You joined the team. Only explicitly granted memory spaces will appear.',
 '确认设备连接': 'Confirm device connection', '仅确认你刚刚发起的连接。确认后，该设备将使用你的账号访问已授权记忆。': 'Only confirm a connection you just requested. This device will access authorized memory using your account.',
 '设备已连接，请返回智能体继续。': 'Device connected. Return to your agent to continue.', '确认连接': 'Confirm connection', '取消': 'Cancel',
 '先创建一个记忆空间，再邀请成员一起使用。': 'Create a memory space, then invite members to use it together.',
 '你已加入团队，尚未获得记忆空间权限。请联系团队管理员授权。': 'You joined the team but have no memory space access yet. Ask a team administrator to grant access.', '成员邮箱': 'Member email',
});

Object.assign(english, { '使用其他账号登录': 'Sign in with another account' });

Object.assign(english, {
 '访问密钥':'Access keys', '使用管理员为你开通的个人密钥，登录后选择可访问的空间。':'Use your administrator-issued personal key, then choose an authorized space.',
 '个人访问密钥':'Personal access key', '登录':'Sign in', '使用邮箱或其他账号登录':'Use email or another account', '使用个人密钥登录':'Use a personal key',
 '选择记忆空间':'Choose a memory space', '这里只显示你有权访问的空间。':'Only spaces you can access are shown.',
 '开通团队成员':'Provision team member', '为新成员生成个人密钥。开通后还需明确授予空间权限。':'Generate a personal key for a new member. Space access must still be granted explicitly.',
 '成员昵称':'Member name', '密钥有效期':'Key validity', '天':'days', '开通并生成密钥':'Create member and key', '创建我的访问密钥':'Create my access key',
 '密钥继承你的现有权限；撤销会同时断开由该密钥授权的会话与智能体。':'Keys inherit your existing permissions. Revocation also disconnects sessions and agents authorized through that key.',
 '用途名称':'Purpose', '生成密钥':'Generate key', '密钥仅在本次显示，请复制并通过可信渠道交给对应成员。':'This key is shown only now. Copy it and deliver it securely to its intended member.',
 '密钥已复制':'Key copied', '复制密钥':'Copy key', '撤销此密钥':'Revoke this key', '个人密钥不正确、已过期或已撤销。':'This personal key is invalid, expired or revoked.',
});

Object.assign(english, { '暂时无法显示知识': 'Knowledge is temporarily unavailable', '请重试，或选择左侧的其他页面。': 'Retry or choose another page from the directory.' });

Object.assign(english, { '选择成员可访问的空间，开通时一并完成授权。未选择的空间保持不可访问。': 'Choose spaces to grant access when creating the member. Unselected spaces remain inaccessible.', '初始空间权限': 'Initial space access', '权限': 'Permissions', '你目前没有可授权的空间，可稍后由空间管理者授权。': 'You cannot grant access to any spaces yet. A space manager can grant access later.' });

Object.assign(english, { '此链接指向的空间不存在或尚未授权。': 'This space does not exist or you do not have access.', '链接已复制，仅有权限的成员可访问。': 'Link copied. Only authorized members can open it.', '复制失败，请重试。': 'Copy failed. Please retry.', '复制页面链接': 'Copy page link' });
