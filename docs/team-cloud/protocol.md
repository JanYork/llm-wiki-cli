# 团队复制合同 v1

本合同冻结首版双方的边界；接口交付状态以LWC Plan为准。上层网络信封为`lwc-team-sync/1`、`share_schema=1`；规范化SQLite沿用`sync_manifest/sync_objects/sync_blobs`，不传输本机活动数据库文件。新增来源/领域历史需要Store schema 20。未知协议、Store版本或对象类型必须拒绝，不能忽略后返回成功。

## 未发布候选：资源生命周期

删除/回收站/恢复及控制库 schema 7 属于未发布候选，尚未部署生产。具体权限、并发、客户端保全和验收边界见 [资源生命周期](resource-lifecycle.md)。当前公开 0.19.4 使用 schema 6；升级后的控制库不能直接交给旧服务端读取。

## 核心覆盖与身份

控制台页面链接使用 `#space=<空间ID>&page=<页面slug>`，不包含登录密钥，不授予访问权；打开时仍按当前身份向服务端读取。正文 `[[slug|标题]]` 和相对 Markdown 页面链接进入同一空间，浏览器后退恢复上一页。

`member.create` 可附带 `grants: [{space_id, role, expected_revision}]`，最多 256 项且空间不重复。团队所有者必须同时拥有所选空间的管理权限，空间必须属于该团队；新身份、个人密钥、初始空间授权及审计在一个事务中提交，任一校验/CAS 失败全部回滚。不提供 grants 时继续默认无空间权限。

| 规范化类型 | 权威内容/原表 | 跨机身份 |
|---|---|---|
| meta | schema、purpose；不包括凭据、副本绑定 | 固定语义键 |
| source | sources、原始内容blob | 内容SHA-256 |
| page | pages、page_sources、page_provenance | slug；来源按内容摘要引用 |
| tag | tags、page_tags | 标签名及成员slug |
| semantic_relation | semantic_relations | 关系ID；source端点替换为内容摘要 |
| ingest | ingest_jobs语义分析及结果 | 来源摘要 |
| retrieval_weight、retrieval_feedback | 权重、反馈、理由 | 既有语义复合键 |
| memory | events、fragments、changes、evidence、relations、feedback | 事件ID；保留语义context |
| todo | todo_items、todo_tags | Todo ID |
| plan | plans、tags、constraints、steps、history | Plan/step ID；历史内容身份排除本地ordinal |
| discussion | discussions、discussion_revisions | Discussion/item ID、request_id与完整历史内容；本地revision不是跨机事件ID |
| source_revision | source_path_revisions语义投影 | 源Store ID、稳定路径摘要、修订号、来源摘要、观察时间 |
| memory_audit | 原始核心领域operations | 源Store ID、源operation ID、规范化payload摘要 |
| draft_intent、work_audit | 已有可移植意图和终态工作审计 | 沿用既有来源命名空间合同；不复制运行权 |

`replica_core_history_roundtrip_is_complete_and_does_not_echo`枚举核心Store的所有普通表，新增未分类表使检查失败。FTS/shadow表、links、search_spans、memory_state由复制内容重建；tracks、bindings、hint状态、活动changeset执行句柄留本机。插件自有库不属于核心Store。原始绝对宿主路径只保留稳定不可执行摘要，不在其他设备建立路径订阅。

领域历史只追加并去重。导入、同步回执、索引重建不再次成为原始领域事件。团队副本以本机meta.replica_space标记关闭独立时序淘汰；容量不足拒绝新增事务，不删除旧记忆。未绑定个人库继续沿用原策略。

## 合并与发布

复用双端基线。结构化新增按稳定ID合并；Plan/Discussion历史去除本地ordinal/revision后比较完整证据，保留双方新增；重复request_id有不同内容时进入冲突。合并后的展示序号可以重排，时间戳不决定丢弃哪一方。跨字段领域约束仍由发布校验；不能把“字段不重叠”当作领域状态合法。

resolution v1继续支持候选选择与保留双方；v2增加完整payload：

```json
{
  "version": 2,
  "decisions": [{
    "kind": "page",
    "logical_key": "example",
    "conflict_id": "<固定冲突材料的SHA-256>",
    "strategy": "merge",
    "payload": {"<完整当前规范化字段>": "<合并后的内容>"}
  }]
}
```

以上为结构示意，不是可执行输入。每批最多20个冲突、256KiB决议；大对象使用既有保留双方路径，不能截断正文伪装完整payload。核对kind/key、conflict_id、候选摘要、完整字段集合、既有history、来源引用；发布另行验证领域状态、引用闭包和Store CAS。任何校验失败都不得修改live Store，原候选材料保留到确认发布与复制完成。

云端head、batch回执、审计和权威内容必须同事务提交；已接受的batch复查回执，不能重放。客户端确认固定批次快照，不把上传期间新写入的本机内容误标记为已同步。首次空副本加入是pull，不能把空库解释为删除所有远端对象。

## Agent信号合同

冲突状态：`pending → processing → resolved_local → replicated`；中断或领取超时回到pending。投递本身只记delivered，不进入resolved。信号kind固定`replica.conflict.required`，包含已绑定space、材料入口、固定输入摘要和立即处理指引。Hook和下一条CLI/MCP响应读取同一持久状态；Agent观察后在当前事务安全边界立即优先处理，不能把它视为可选的日后任务。

CLI/MCP目标只允许已加入的本机空间；信号不得授权加入空间、变更权限或启动Agent。LWC只做同步、确定性合并、校验和通知，不调用模型，不启动/嵌入Agent。Agent负责语义判断；无Agent时持久保全候选并继续其他非冲突对象/空间。

## 身份与授权实现依据

浏览器登录使用HttpOnly、SameSite Cookie和同源写请求；GitHub使用随机state及S256 PKCE，见[GitHub官方OAuth合同](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps)。飞书身份采用应用/租户命名空间及open_id，见[用户信息官方接口](https://open.feishu.cn/document/server-docs/authentication-management/login-state-management/get)和[官方SDK字段](https://github.com/larksuite/oapi-sdk-go/blob/v3_main/service/authen/v1/model.go)。provider邮箱相同不自动合并账号。

HTTPS调用关闭重定向、设置连接与总超时、限制响应大小；SMTP使用STARTTLS与系统提供的成熟库。身份源真实回调、邮件投递属于配置后验收，单元检查不能替代它们。

## 已实现的复制入口

- `lwc space join SPACE --server ORIGIN`：首次下载并校验快照、创建本地 SQLite 和 Wiki 投影、注册设备，默认启动本机同步 worker。`--manual` 仅用于显式手动同步。
- `lwc --space REFERENCE <核心命令>`：将核心 Wiki、时序记忆、Discussion、Todo/Plan 等操作路由到已加入空间；不允许与 global/all 混用，不路由插件运行时或控制库。
- `lwc space sync REFERENCE`：执行一次可恢复的双向合并和增量传输；固定批次提交前后保留原始输入、候选和回执。提交响应丢失时先查询回执，不重复生成 head。上传期间发生的本地写入通过 CAS 与后续重合并保全，不误标为已同步。
- `lwc space configure REFERENCE --interval-ms 2000 --automatic true`：配置自动同步。空闲轮次比较 Store identity 和服务器 head，不重新导出库；失败退避重试。`space watch` 可由部署者的用户服务以前台方式管理。
- `lwc space conflicts REFERENCE`：读取固定会话和摘要绑定的冲突批次；`space candidate ...` 按字符分页读取完整候选 JSON，不要求 Agent 直接读数据库。
- `lwc space resolve REFERENCE --session ID --if-digest SHA --file resolution.json`：在新候选副本上验证决议，原子切换当前材料，留下原决议和候选；随后 worker 自动继续同步。
- MCP `lwc_space` 提供 status/sync/conflicts/candidate/resolve 结构化动作。CLI 的已选空间响应及此 MCP 工具返回 `replica.conflict.required`。

复制恢复先确认远端固定 batch，再发布本地合并结果。原始记忆写入始终先发生在本机；远端确认与本地合并发布之间发生的新写入，使用保存的输入重合并。基线始终指向远端实际接受的固定快照，不使用重新导出的当前库替代它。

项目默认绑定、核心 MCP 写动作、Hook 投递与冲突领取、原生用户服务、补偿恢复与 epoch 恢复均已实现；部署说明见 `deploy/team/README.md`。发布状态与外部身份源验收单独记录，不能由本地检查推断。

## 访问与恢复扩展

[访问、策略与恢复合同](access-policy-recovery.md)补充云端只读、智能体主体、签名策略、真实差异动作校验、补偿恢复和撤销来源追踪。复制保持完整空间副本；过滤读取不能生成全量替换快照。缺少所需策略或恢复协议的客户端拒绝共享写入，不默认为可编辑。

### v6 实现增量：云端只读、归属登记与服务端对象限制

当前工作树提供 `lwc cloud --server ORIGIN --space SPACE_ID search/get/list/objects/object/blob` 和 MCP `lwc_cloud`。它们使用已有账号凭据，不初始化本地Wiki/SQLite、不注册副本或启动worker。search/get/list复用云端Store；objects/object读取已接受的规范化快照，涵盖所有导出的核心领域及历史；blob按最多64KiB窗口读取来源正文。单响应上限1MiB，超大对象明确报错。跨页读取应同时传head与epoch；云端变化返回head_changed，调用方重新查询，不能混合版本。

`lwc config team --server ORIGIN --email EMAIL --nickname NAME --agent NAME` 在登录后保存用户级资料并登记设备与Agent。首次采集OS/架构/LWC版本和安装级加盐MAC摘要；没有可用MAC不阻断登记，重试沿用原device_id/agent_id。邮箱未匹配已验证身份时标记email_verified=false；不会绑定账号或提升权限。登记资料不构成授权；独立智能体凭据通过下述 delegate 命令生成，服务端按实际凭据记录主体。

管理接口新增`space.policy`：manager使用expected_revision为现有空间成员设置denials（kind/key/action，支持`*`，动作create/update/delete）。服务端push在授权临界区比较规范化前后快照的真实差异，命中限制时整批拒绝，head不推进；不信任客户端操作标签。本地未共享变化保留。

云端只读仍以完整空间读取权限为边界，不支持部分授权的过滤副本。本地签名策略守卫、服务端差异检查、智能体委托与补偿恢复见后文。控制库当前为 schema 6，核心库为 schema 20。

### 实例接入 Token（新增强制门禁）

`lwc server init` 在服务端私有数据目录生成 `server-access.token`（256 位随机数，Unix 0600）；输出仅含文件路径，不输出密钥。部署管理员通过可信渠道分发。客户端先执行：

```sh
lwc config server --server https://memory.example.com --token-stdin < /private/server-access.token
lwc login --server https://memory.example.com
```

配置按精确服务端 origin 隔离、保存到用户私有凭据目录，普通 config show 不包含 Token；不写入 Wiki、SQLite 记忆或复制产物。所有 CLI/MCP/worker 网络请求使用 `X-LWC-Server-Token`，不跟随重定向。缺少配置在本机直接失败；服务端缺少或错误 Token 返回 401，包括登录/提供者/健康接口。Token 只允许接入，后续用户会话、Agent 委托和空间权限仍必须通过。

浏览器先打开 `/access` 输入 Token，经同源 POST `/api/access` 换取 HttpOnly、SameSite=Lax 的接入 Cookie（HTTPS 使用 Secure）。Token 不放在 URL、localStorage 或页面响应中；此页和激活端点是公开入口，不提供记忆或账号数据。登录和 OAuth 回调继续受接入 Cookie 保护。

`lwc server rotate-token --data /private/team-data` 原子更换 Token，不输出密钥；下一请求即拒绝旧 Token 和旧浏览器 Cookie。用户需重新配置，worker 保留待同步数据并按原退避策略重试；轮换不删除用户会话、记忆或空间授权。此密钥是实例共同接入凭据，不能用来区分人员；泄露后轮换，单人撤权仍使用账号/设备/Agent 的独立撤销。

### 已实现的访问与本地迁入补充

- `lwc config delegate --server ORIGIN --agent-id ID --grant-space ID [--write] --output PATH` 生成空间范围、24 小时有效的独立 Agent 凭据；输出只有私有文件路径。`LWC_TEAM_CREDENTIALS_FILE` 选择该文件，服务端仍交叉校验账号、设备、Agent、空间与当前授权。撤销后下一请求失效；委托不能调用账号治理接口。
- 服务端签发 Ed25519 策略，绑定真实凭据摘要、主体、空间、epoch、权限修订号与 15 分钟有效期。本地 Store 使用连接级 SQLite 触发器守卫正常写入；过期、换凭据、viewer、命中限制均拒绝。服务端独立检查真实规范化 diff；新事件的 supersedes 也检查被替代原记录的限制。文件直接修改不能绕过云端检查。
- head manifest 与提交回执签名；客户端首次认证连接固定公钥，后续换钥、摘要不符、同 head 换内容或 head 倒退失败关闭并保留本地工作。公钥轮换的连续证明尚未实现，不能静默接受新钥。
- `lwc space bind REFERENCE --import-project` 先经既有全核心 archive merge 迁入当前项目，再写入用户私有项目绑定；原项目库完整保留。遇冲突返回可恢复 import 会话，Agent 先合并再绑定，不自动覆盖。绑定后普通核心 CLI、MCP 查询与 Hook 默认走团队空间；`space unbind` 仅解除本目录绑定，不删除任何记忆。
- MCP `lwc_core` 接收 `projectPath`、可选 `space`、核心 CLI 参数数组与可选标准输入；复用真实 CLI/Store 合同，不执行 shell，不接受服务端、凭据或跨 scope 管理命令。写入与 CLI 使用相同权限及冲突信号。`lwc_cloud` 仍是不创建本地记忆的只读入口。
- 管理端源码在 `admin/`，React + TypeScript + shadcn/ui 组件，复用 `web/src/tokens.css`。`admin_assets` 指向构建目录；公共静态页面不包含记忆，数据 API 继续经过接入门禁、用户会话与空间授权。同源浏览器只读查询不能使用复制写接口。

当前新增公开服务端管理列表使用固定查询与分页，不暴露会话、邀请码或密钥表；项目/集合关系不派生授权。原始 checkpoint 全库替换禁止用于共享副本，防止覆盖当前权限及同步历史；恢复通过追加补偿提交实现，保留原始证据与后续无关修改。真实登录提供者与发布渠道需要各自的运行验收。

### 实例门禁与消费者界面补充

服务初始化生成私有 `server-access.token`。部署者通过受保护渠道分发接入密钥；客户端使用 `lwc config server --server ORIGIN --token-stdin` 配置。普通团队 API（包括账号登录与健康检查）独立校验实例接入凭据；个人密钥登录接口例外，直接验证个人密钥。浏览器以同源接入接口换取 HttpOnly Cookie；密钥不进入 URL 或浏览器持久存储。`lwc server rotate-token --data DIR` 轮换后，旧请求头与旧接入 Cookie 立即失效。实例门禁不能代替账号、设备、智能体委托和空间权限。

冲突可使用 `lwc space claim SPACE --session SESSION --if-digest DIGEST` 领取 120 秒租约，携带 `--claim CLAIM` 续租或提交决议。有效租约阻止其他持有不同领取标识的决议；超时可重新领取。会话与摘要 CAS 始终有效；旧客户端未领取时仍受相同 CAS 约束。CLI/MCP 不运行或嵌入模型。

`lwc space supervise` 供原生用户服务运行，按已保存的自动同步设置恢复各空间工作进程；单空间进程锁避免与命令触发重复启动。部署目录提供 macOS/Linux 用户服务与 Windows 登录任务安装入口；运行身份与凭据路径沿用当前用户，不将密钥写进服务文件。

已知在线 401/403 会暂停本地共享写入，直至取得有效签名许可。完整性异常留在暂存区并向下一次命令或 Hook 发出恢复信号；不重置固定公钥、head 或策略来绕过校验。未确认的本地工作保留。

## Recovery and deployment completion notes

- The stopped-server lifecycle lock gates `server backup` and `server restore`. Backups contain the whole private data directory and gain a completion marker only on success; restore always uses a new directory and rotates space epochs without overwriting the source.
- A client receiving a new epoch verifies the same pinned server signing identity and fresh subject-bound policy, preserves its old record, pending evidence and baselines, then performs ordinary directional merge. The first recovered download is full to avoid applying a delta against a mismatched backup baseline. Undeclared head regressions and key changes still fail closed.
- A rejected unchanged batch is held locally until content, server head or policy revision changes. It is not uploaded repeatedly while the Agent has yet to repair it.
- Replica reports carry only status/counts. The console displays current, conflict/retry and last connection state rather than inferring that all configured replicas have synchronized.
- Disaster restore requires `--authority-data CURRENT_DATA_DIRECTORY`, preserves current identities, grants, revocations, sessions and instance token, and refuses a missing/damaged authority or different signing identity. Newer spaces absent from the backup keep their current contents. Policy revisions cannot regress across epochs.
- Self-host deployment files and native current-user supervisor installers are in `deploy/team/`; real provider credentials remain deployment inputs.

## 浏览器加入与设备确认

CLI 登录生成一次性设备请求，输出 `/devices/authorize#device=…` 链接。片段中的短码在浏览器读取后从地址栏移除，仅在当前标签页会话中保留，跨登录继续；不要求用户转抄。登录后的同源预览只返回设备名称与到期时间，必须明确确认才允许 CLI 换取凭据。读取预览不会授权，过期、已确认、已消费请求不能重复确认。

邀请采用 7 天有效、单次使用、指定已验证邮箱的链接。持有邀请且通过实例门禁后，可预览团队名称与期限，不返回成员名单或邮箱。邀请码不作为输入控件显示；登录后确认加入，错误账号可切换登录。加入成功选中该团队，空间仍按显式授权默认拒绝；没有授权时显示等待管理员授权的引导。邀请码与实例接入密钥是不同边界，邀请不能绕过实例门禁。

### 个人密钥与空间选择

控制库 schema 6 仅保存个人密钥摘要、所有者、签发者、期限和撤销状态。初始化生成私有 `administrator.key`；团队所有者可开通全新成员并签发密钥，不能为已有成员冒签身份。成员可为自己生成密钥。密钥不是空间授权：新成员默认没有空间读取权。个人密钥登录受请求体限额、来源校验及按来源地址限流保护，浏览器只接收 HttpOnly 会话 Cookie，密钥不进入浏览器持久存储。

`lwc login --server ORIGIN --key-stdin` 完成命令行登录；个人密钥与会话按现有私有凭据文件规则保存。撤销或过期会使该密钥派生的会话、设备和智能体委托失效；服务端每次调用检查，不等待客户端同步。浏览器登录后选择已授权空间，只记住空间偏好，并在恢复时重新核验授权。
