# LWC 团队管理端：功能、视觉与 ASCII 原型

状态：管理端设计稿 v2（全核心记忆与冲突状态补齐），React + shadcn/ui 技术选择来自用户；尚未实现。日期：2026-09-28。

执行状态以 LWC Plan `84715af7b63db12278d87a62cf00f13c` 为准：`lwc --scope project plan brief 84715af7b63db12278d87a62cf00f13c`。本文件定义 UI 合同，不复制 Plan 进度。[整体架构](design.md)、[编码计划](implementation-plan.md)、[参考评估](reference-tencent.md)。

## 1. 产品定位与界面原则

管理端用于查看团队记忆、管理共享范围、理解同步状态和追踪变更。Agent继续通过本地CLI/MCP生成与整理知识；后台不运行Agent、不接LLM、不设置人工冲突审批。

采用 React + TypeScript + Vite + shadcn/ui，沿用LWC现有浅色视觉。现有 `web/` 是Lit本地Viewer；团队管理端独立放在 `admin/`，复用视觉token和Markdown安全规则，保持原Viewer。React是用户明确要求，不为此改写现有Viewer。

界面顺序固定为：当前团队和范围 → 当前页面标题 → 筛选 → 内容 → 分页。管理操作有稳定位置；数据状态有明确含义，不用大面积警告色营造紧张感。

设计约束：

- 同类页面共用一个框架；标题、筛选、表格、分页沿同一组左右边界排列。
- 每页最多一个突出的主操作，其余用次级按钮或行末菜单；没有主要动作的只读页不硬放蓝色按钮。
- 首页三个等宽指标，下面是需要关注的事项和最近变化；不堆图表或巨大总数。
- 列表用表格；详情用列表加阅读区；设置用纵向分组表单。相同问题只使用一种布局。
- 首版中文为默认界面语言；原型采用英文等宽占位以严格保持ASCII对齐。实现时验证中文最长标签，不按英文宽度硬裁切。
- 所有原型数据均为示例，不是实际团队、设备、同步或运行情况。

## 2. 信息架构与功能范围

| 导航 | 页面 / Tab | 查看内容 | 可以执行的管理动作 |
|---|---|---|---|
| 总览 | 概览 | 可访问空间、最新上报副本状态、已保留冲突、最近变化 | 进入相关空间或同步记录 |
| 项目与集合 | 项目 / 集合 | 项目、所属集合、主空间、本人权限 | 有目录权限者创建/重命名/归档项目、整理集合；集合不自动扩权 |
| 记忆空间 | 空间列表 → 知识 / 时间线 / Discussion / 来源 / 关系 / 任务 / 授权 | 全部核心记忆：正文、时序事件、可见讨论、证据、反馈、修订、关系；均为云端已接受版本 | 查看加入命令；manager调整该空间grant；正文首版只读 |
| 任务与计划 | Plan / Todo | 状态、步骤、结果、证据、历史、最近提交者 | 筛选、查看、复制工具命令；不在Web启动任务或替Agent完成计划 |
| 同步中心 | 副本 / 冲突 / 发布历史 | 已确认head、最新报告时间、离线/追赶/拒绝状态、候选和决议 | 查看诊断；本人注销自身副本或manager撤销空间副本接入；不远程控制设备 |
| 成员与权限 | 成员 / 邀请 / 空间授权 | 团队成员角色、邀请状态、自己可管理空间的显式grant | owner邀请/移除成员；space manager授予/撤销该空间角色 |
| 操作审计 | 治理操作 / 记忆发布 | 谁、何时、在哪个范围、改变什么、结果 | 授权范围内搜索、筛选和查看；不编辑历史 |
| 团队设置 | 基本信息 | 团队名称、标识、owner等治理信息 | owner更新可变信息；首版不提供永久删除整队的一键入口 |
| 我的账号 | 身份 / 会话 | 本人邮箱/飞书/GitHub绑定、浏览器与CLI会话 | 近期重新认证后绑定/解绑；退出、撤销本人会话 |
| 实例设置 | 登录方式 / 存储与备份 / 实例信息 | provider配置状态、回调地址、版本、存储使用和最近备份证据 | 仅实例运维角色可见；复制部署配置说明，不展示或在线编辑secret |
| 登录流程 | 登录 / 验证码 / 设备授权 / 邀请接受 | 本实例启用的方式、设备码、邀请团队 | 完成登录与设备授权；成功登录不会自动获得空间权限 |

侧栏分为“工作区”“团队治理”两组。账号在底部；实例设置只对实例运维角色出现。普通成员不显示没有意义的治理入口；某个已知项目的详情页仍可解释本人为什么没有空间内容权限。

项目与集合共用一个导航，Plan与Todo共用一个导航，副本与冲突共用一个导航，避免侧栏无限增长。首版导航不超过上述8个团队页面入口。

## 3. 权限与可见性

| 身份 | 默认可见 | 管理边界 |
|---|---|---|
| 普通成员 | 获grant的空间与相关知识、任务、同步记录 | 自己的账号、会话和副本 |
| 空间viewer | 该空间正文、来源、任务、合法历史与状态 | 不写入记忆，不改授权 |
| 空间editor | viewer能力；CLI/MCP写入 | 不因可编辑记忆获得grant管理权 |
| 空间manager | editor能力及该空间grant/副本治理 | 不自动获得整个团队的成员管理权 |
| 团队owner | 团队目录、成员、邀请与治理审计 | 未获空间grant时不能读正文、来源、冲突内容或内容统计 |
| 实例运维角色 | 实例配置状态、容量、健康和备份 | UI内容访问仍走grant；拥有服务器磁盘是独立运维边界 |

总览的知识数量、任务数量、冲突数量及最近变化先按可读空间过滤，再聚合。不能先算全队再隐藏行，否则数字、摘要和搜索提示仍会泄露内容。治理范围与内容范围在API和UI中都区分。

授权页采用“选择一个空间 → 成员/角色表”，不用几百列的成员×全部空间矩阵。manager只能管理自己有权管理的空间；team owner没有隐式越权开关。

邀请默认只赋予团队成员资格。若要同时授权，必须列出具体空间和角色，且操作者必须有这些空间的manager权限；默认不勾选任何空间。移除团队成员停止后续云端访问；对话框明确说明已下载的本地副本无法收回。

## 4. LWC视觉规范

配色直接核对自 `web/src/tokens.css`，不是重新选一套主题。

| 用途 | 现有token / 值 | 管理端使用方式 |
|---|---|---|
| 页面底色 | `--color-canvas` / `#F7F8FC` | 页面与外部留白 |
| 内容表面 | `--color-surface-card` / `#FFFFFF` | 表格、详情和表单区域 |
| 弱背景 | `--color-surface-soft` / `#F0F3FA` | 分组标题、次级按钮和浅底区 |
| 分隔线 | `--color-hairline` / `#DFE5F2` | 1px边框，避免厚重外框 |
| 主色 | `--color-primary` / `#005CFF` | 主按钮、选中导航、链接和焦点 |
| 主色按下 | `--color-primary-active` / `#0049CC` | hover/active与深色强调 |
| 标题 | `--color-ink` / `#11141B` | 页标题、表头、关键数字 |
| 正文 | `--color-body` / `#3D465B` | 普通内容 |
| 次级文字 | `--color-muted` / `#687187` | 时间、辅助标签与说明 |
| 状态色 | success `#5DB872`、warning `#D4A017`、error `#C64545` | 图标/点/淡底；小字号正文用深色前景，不直接把浅黄浅绿当文字 |

首版以现有浅色主题为基准。通过CSS变量映射shadcn/ui的background/card/foreground/primary/border/ring/sidebar等语义token；浅底和主蓝色均使用上表。共享token只有一个来源，页面组件不散写hex值。

字体用现有Inter/系统无衬线字体，增加中文系统回退；英文品牌展示可保留现有字标风格。管理页标题不用营销页的大号衬线标题。

| 文本 | 字号 / 行高 | 规则 |
|---|---|---|
| 页面标题 | 28 / 36px，600 | 每页一个；长标题换行后按钮仍锚定操作区 |
| 模块标题 | 16 / 24px，600 | 与表格左边界一致 |
| 正文/表格/控件 | 14 / 20px | 管理页默认密度，不缩成12px表格 |
| 辅助说明 | 12 / 18px | 不承担唯一关键状态信息 |
| 指标数字 | 28 / 36px | 同组等大，启用tabular-nums |
| ID/命令/摘要 | 12–13px等宽 | 短显示+复制，详情可读完整值 |

## 5. 对齐、尺寸与间距合同

设计基准为1440px桌面视口；以下是实现必须遵守的几何规则。

| 项目 | 固定规则 |
|---|---|
| 侧栏 | 展开240px、折叠72px；底部账号固定，导航区独立滚动 |
| 顶栏 | 64px；左侧品牌/面包屑，右侧搜索/账号，对齐内容边界 |
| 页面边距 | 桌面32px，中屏24px，小屏16px |
| 内容宽度 | 常规页最大1280px；阅读正文最大72ch，不能拉满超宽屏 |
| 基础间距 | 4 / 8 / 12 / 16 / 24 / 32 / 48px；不出现随意的13、19、27px间距 |
| 页面节奏 | 页头→筛选24px；筛选→表格16px；区块间24px |
| 指标卡片 | 同一行等宽、104px高；内部20px；卡片间16px |
| 面板 | 12px圆角、1px边框；默认无投影，浮层才有轻阴影 |
| 按钮/输入框 | 桌面36px高、8px圆角；表单标签上方固定、标签到控件8px |
| 触达区域 | 触控场景至少44px；密集图标操作保留足够点击范围 |
| 表格 | 表头40px、标准数据行48px；单元格水平16px；不同页同密度 |
| 数字列 | 右对齐；状态/时间列固定宽度；操作列末端固定，不随内容漂移 |
| 状态标签 | 最小88px、24px高；同类标签同宽，文字+图标，不只靠颜色 |
| Sheet | 桌面480px；小屏全宽；标题、表单和底部操作共用24px边距 |
| 对话框 | 默认480px；更长表单采用Sheet或页面，不把小弹窗塞成大页面 |
| 对比内容 | 两栏严格等宽；共同基线独立折叠；各栏标题、来源与正文起点对齐 |
| 分页 | 与表格左右边界齐平；左侧总数/页大小，右侧页码/上一页/下一页 |

全局只有一个页面纵向滚动主区域；知识列表可以独立滚动，但不再增加常驻第三列。右侧元信息用按需Sheet，避免“侧栏+列表+正文+元信息”四栏挤压。

视口≥1280px：完整侧栏、三个指标同排、知识列表256px+阅读区。1024–1279px：侧栏折叠为72px，二栏保留；768–1023px：侧栏收进Sheet，详情可分屏切换。小于768px：列表→详情单页导航，筛选折叠；表格可在自身区域滚动，页面整体不横向溢出。

长中文名称最多两行，有完整详情入口；表格普通列可省略但提供完整名称。数字/状态/操作不换行。空单元格统一用 `--`，未知数据绝不能显示成0。加载骨架保留最终高度，权限变化或请求更新不让页面跳动。

## 6. 首版关键页面原型

以下全部是ASCII，统一104列；边框、列起点、主要操作右边界一致。原型边框表示布局，不意味着成品要画这么多深色线；成品使用上面的细线与留白。

### 6.1 总览

三个指标等宽；关注事项优先，最近变化其次。只展示本人有权查看的空间。计数必须带统计范围，副本状态标注最近报告时间。

```text
+------------------------------------------------------------------------------------------------------+
| LWC / TEAM         |  Acme Team / Overview                           [ Search... ]  [ Account v ]    |
+--------------------+---------------------------------------------------------------------------------+
| [Acme Team v]      |  Overview                                                      [ View sync ]    |
|                    |  Your authorized spaces only. Updated 14:32:08.                                 |
| WORKSPACE          |                                                                                 |
| > Overview         |  +---------------------+   +---------------------+   +---------------------+    |
|   Projects         |  | Accessible spaces   |   | Replicas up to date |   | Conflicts preserved |    |
|   Memory spaces    |  | 12                  |   | 08 / 10             |   | 02                  |    |
|   Tasks & plans    |  | 3 collections       |   | Latest reported ACK |   | Agent tools ready   |    |
|   Sync center      |  +---------------------+   +---------------------+   +---------------------+    |
|                    |                                                                                 |
| GOVERNANCE         |  Attention                                                                      |
|   Members & access |  +------------------------+-----------------------+------------------+-----+    |
|   Audit log        |  | Space                  | Status                | Last report      |     |    |
|   Team settings    |  +------------------------+-----------------------+------------------+-----+    |
|                    |  | Frontend core          | 2 preserved conflicts | 3 seconds ago    | ... |    |
|                    |  | Platform notes         | Replica offline       | 18 minutes ago   | ... |    |
|   Instance         |  +------------------------+-----------------------+------------------+-----+    |
|   My account       |                                                                                 |
|                    |  Recent changes                                                 [ View all ]    |
|                    |  +---------------------------+----------------------+--------------+-------+    |
|                    |  | Change                    | Space                | Actor        | Time  |    |
|                    |  +---------------------------+----------------------+--------------+-------+    |
|                    |  | Release checklist         | Frontend core        | Agent / Lin  | 14:31 |    |
|                    |  | Deploy plan: 3 / 5        | Platform notes       | Agent / Chen | 14:28 |    |
|                    |  +---------------------------+----------------------+--------------+-------+    |
+--------------------+---------------------------------------------------------------------------------+
```

### 6.2 项目与集合

项目与集合共用列表骨架。主操作在右上角；目录信息与本人空间权限分列，不把集合成员资格当空间授权。

```text
+------------------------------------------------------------------------------------------------------+
| LWC / TEAM         |  Acme Team / Projects                           [ Search... ]  [ Account v ]    |
+--------------------+---------------------------------------------------------------------------------+
| [Acme Team v]      |  Projects & collections                                    [ + New project ]    |
|                    |  Collections organize projects; access is granted per space.                    |
| WORKSPACE          |                                                                                 |
|   Overview         |  [ Projects ]   Collections                                                     |
| > Projects         |                                                                                 |
|   Memory spaces    |  [ Search projects... ]                                [ Collection: All v ]    |
|   Tasks & plans    |  +---------------------------+------------------------+--------------+-----+    |
|   Sync center      |  | Project / collection      | Primary space          | My access    |     |    |
|                    |  +---------------------------+------------------------+--------------+-----+    |
| GOVERNANCE         |  | Console / Product         | Frontend core          | Manager      | ... |    |
|   Members & access |  | API / Product             | API knowledge          | Editor       | ... |    |
|   Audit log        |  | Infra / Engineering       | Platform notes         | Viewer       | ... |    |
|   Team settings    |  +---------------------------+------------------------+--------------+-----+    |
|                    |                                                                                 |
|                    |  Selected collection: Product                                                   |
|   Instance         |                                                                                 |
|   My account       |  2 projects   /   1 shared space                       [ Manage collection ]    |
|                    |                                                                                 |
|                    |  Adding a project does not grant access to its memory.                          |
+--------------------+---------------------------------------------------------------------------------+
```

### 6.3 空间知识浏览

页头显示空间与云端已接受版本。左侧对象列表、右侧阅读区；引用/历史按需展开。没有网页知识编辑器，也没有把云端视图称作本地最新状态。

```text
+------------------------------------------------------------------------------------------------------+
| LWC / TEAM         |  Acme Team / Memory spaces                      [ Search... ]  [ Account v ]    |
+--------------------+---------------------------------------------------------------------------------+
| [Acme Team v]      |  Frontend core                                         [ Join instructions ]    |
|                    |  Cloud-accepted memory. Local unpushed edits are not shown.                     |
| WORKSPACE          |                                                                                 |
|   Overview         |  [ Knowledge ] Timeline Discussion Sources Relations Tasks Access               |
|   Projects         |                                                                                 |
| > Memory spaces    |  ---------------------------------------------------------------------------    |
|   Tasks & plans    |  Pages                   | Release checklist                                    |
|   Sync center      |  [ Find page... ]        | Shared head 104   |   Updated 14:31                  |
|                    |                          |                                                      |
| GOVERNANCE         |  > Release checklist     | Before release                                       |
|   Members & access |    Deployment notes      |   1. Verify the candidate revision.                  |
|   Audit log        |    UI conventions        |   2. Confirm targeted checks.                        |
|   Team settings    |    API contracts         |   3. Record the published receipt.                   |
|                    |                          |                                                      |
|                    |  Sources: 3              | Sources  [1] [2] [3]                                 |
|   Instance         |  Relations: 5            | [ View history ]   [ Source details ]                |
|   My account       |                          | [ Copy CLI command ]                                 |
|                    |  ---------------------------------------------------------------------------    |
|                    |                                                                                 |
|                    |  Body is read-only here. Agents edit through local CLI / MCP.                   |
+--------------------+---------------------------------------------------------------------------------+
```

### 6.4 Todo与Plan查看

同一导航的两个Tab，首版列表+详情。Plan展示真实步骤、证据和记录状态，不把“正在执行”当Agent在线状态，不提供后台启动任务按钮。

```text
+------------------------------------------------------------------------------------------------------+
| LWC / TEAM         |  Acme Team / Tasks & plans                      [ Search... ]  [ Account v ]    |
+--------------------+---------------------------------------------------------------------------------+
| [Acme Team v]      |  Tasks & plans                                                                  |
|                    |  Shared records and evidence; execution stays in each Agent host.               |
| WORKSPACE          |                                                                                 |
|   Overview         |  [ Plans ]   Todos                                                              |
|   Projects         |                                                                                 |
|   Memory spaces    |  [ Search... ]  [ State: All v ]                            [ Space: All v ]    |
| > Tasks & plans    |  +------------------------------+------------+---------------+-------------+    |
|   Sync center      |  | Plan                         | Progress   | State         | Updated     |    |
|                    |  +------------------------------+------------+---------------+-------------+    |
| GOVERNANCE         |  | Team release                 | 3 / 5      | Active        | 14:28       |    |
|   Members & access |  | API cleanup                  | 2 / 4      | Blocked       | 13:50       |    |
|   Audit log        |  | Install hotfix               | 4 / 4      | Complete      | Yesterday   |    |
|   Team settings    |  +------------------------------+------------+---------------+-------------+    |
|                    |                                                                                 |
|                    |  Team release                                                                   |
|   Instance         |                                                                                 |
|   My account       |  3 / 5 steps completed                                     [ Tool commands ]    |
|                    |                                                                                 |
|                    |  [x] Freeze contracts                                                           |
|                    |  [x] Implement sync transport                                                   |
|                    |  [x] Validate explicit space access                                             |
|                    |  [>] Complete focused acceptance                                                |
|                    |  [ ] Publish verified artifacts                                                 |
+--------------------+---------------------------------------------------------------------------------+
```

### 6.5 同步中心

副本、冲突、历史放在同一导航。服务端知道ACK与收到的上报，不知道离线机器后来新增了多少内容；未上报字段显示未知。

```text
+------------------------------------------------------------------------------------------------------+
| LWC / TEAM         |  Acme Team / Sync center                        [ Search... ]  [ Account v ]    |
+--------------------+---------------------------------------------------------------------------------+
| [Acme Team v]      |  Sync center                                                                    |
|                    |  Last reported replica state; unknown local changes stay unknown.               |
| WORKSPACE          |                                                                                 |
|   Overview         |  [ Replicas ]   Conflicts (2)   History                                         |
|   Projects         |                                                                                 |
|   Memory spaces    |  [ Space: All v ]  [ State: All v ]                              [ Refresh ]    |
|   Tasks & plans    |  +----------------------+--------------------+--------------+--------------+    |
| > Sync center      |  | Device / member      | Space              | ACK / head   | State        |    |
|                    |  +----------------------+--------------------+--------------+--------------+    |
| GOVERNANCE         |  | MacBook / Lin        | Frontend core      | 104 / 104    | Up to date   |    |
|   Members & access |  | Windows / Chen       | Frontend core      | 102 / 104    | Catching up  |    |
|   Audit log        |  | Linux / Wang         | Platform notes     | 71 / 71      | Offline      |    |
|   Team settings    |  +----------------------+--------------------+--------------+--------------+    |
|                    |                                                                                 |
|                    |  Windows / Chen                                                                 |
|   Instance         |                                                                                 |
|   My account       |  Last report: 14:31:52                                           [ Details ]    |
|                    |  Server head        104                                                         |
|                    |  Confirmed head     102                                                         |
|                    |  Local pending      --  (not reported)                                          |
|                    |  Next attempt       Report unavailable                                          |
|                    |                                                                                 |
|                    |  No browser action starts or controls an Agent.                                 |
+--------------------+---------------------------------------------------------------------------------+
```

### 6.6 冲突详情

两个候选等宽对齐；状态区分“待投递、已投递、处理中、本地已合并、云端已确认、可恢复阻塞”。“已投递”不等于Agent已读，“已保全”不等于已解决；Agent观察后必须立即优先合并。只提供读取材料和复制CLI/MCP入口，不设人工接受某一侧或启动Agent按钮。

```text
+------------------------------------------------------------------------------------------------------+
| LWC / TEAM         |  Acme Team / Sync center                        [ Search... ]  [ Account v ]    |
+--------------------+---------------------------------------------------------------------------------+
| [Acme Team v]      |  Conflict / Release checklist                                                   |
|                    |  Preserved on both sides. No human approval is required.                        |
| WORKSPACE          |                                                                                 |
|   Overview         |  [ Details ]   Sources   Resolution history                                     |
|   Projects         |                                                                                 |
|   Memory spaces    |  Status: Pending Agent pickup                           [ Copy CLI command ]    |
|   Tasks & plans    |                                                                                 |
| > Sync center      |  +------------------------------------+------------------------------------+    |
|                    |  | Candidate A                        | Candidate B                        |    |
| GOVERNANCE         |  +------------------------------------+------------------------------------+    |
|   Members & access |  | Lin / head 102                     | Chen / head 103                    |    |
|   Audit log        |  |                                    |                                    |    |
|   Team settings    |  | Release after checks pass.         | Release after checks and rollback  |    |
|                    |  | Source: Release policy v1          | rehearsal pass.                    |    |
|                    |  |                                    | Source: Operations policy v2       |    |
|   Instance         |  |                                    |                                    |    |
|   My account       |  +------------------------------------+------------------------------------+    |
|                    |                                                                                 |
|                    |  [ Show common baseline ]   [ Inspect related sources ]                         |
|                    |                                                                                 |
|                    |  Agent workflow: notice -> immediate packet -> resolve -> sync                  |
|                    |                                                                                 |
|                    |  Preserved != resolved. Delivery and resolution receipts are separate.          |
+--------------------+---------------------------------------------------------------------------------+
```

### 6.7 成员与空间授权

成员、邀请、授权分Tab；按空间查看grant。角色变动形成草稿，底部显示差异与一个保存按钮；CAS失败重新拉取，不覆盖别人刚修改的授权。

```text
+------------------------------------------------------------------------------------------------------+
| LWC / TEAM         |  Acme Team / Members & access                   [ Search... ]  [ Account v ]    |
+--------------------+---------------------------------------------------------------------------------+
| [Acme Team v]      |  Members & access                                                               |
|                    |  Team membership and space access are independent.                              |
| WORKSPACE          |                                                                                 |
|   Overview         |  Members   Invitations   [ Space access ]                                       |
|   Projects         |                                                                                 |
|   Memory spaces    |  [ Space: Frontend core v ]                                 [ + Add access ]    |
|   Tasks & plans    |  +----------------------------+--------------------+-----------------+-----+    |
|   Sync center      |  | Member                     | Space role         | Grant           |     |    |
|                    |  +----------------------------+--------------------+-----------------+-----+    |
| GOVERNANCE         |  | Lin                        | [ Manager v ]      | Explicit        | ... |    |
| > Members & access |  | Chen                       | [ Editor  v ]      | Explicit        | ... |    |
|   Audit log        |  | Wang                       | [ Viewer  v ]      | Explicit        | ... |    |
|   Team settings    |  +----------------------------+--------------------+-----------------+-----+    |
|                    |                                                                                 |
|                    |  Team owner has no automatic permission to read this space.                     |
|   Instance         |                                                                                 |
|   My account       |  Changes                                                                        |
|                    |    Chen: Viewer -> Editor                                                       |
|                    |                                                                                 |
|                    |  Revoking access stops future sync; downloaded copies remain.                   |
|                    |                                                                                 |
|                    |  [ Cancel ]                                                [ Save 1 change ]    |
+--------------------+---------------------------------------------------------------------------------+
```

### 6.8 实例设置

部署者查看登录配置、回调、版本和备份状态；配置存在与真实回调已验证分开显示。凭据只留部署配置，界面不显示secret，不提供模型配置。

```text
+------------------------------------------------------------------------------------------------------+
| LWC / TEAM         |  Acme Team / Instance                           [ Search... ]  [ Account v ]    |
+--------------------+---------------------------------------------------------------------------------+
| [Acme Team v]      |  Instance                                                                       |
|                    |  Instance operators only. Secrets remain in deployment files.                   |
| WORKSPACE          |                                                                                 |
|   Overview         |  [ Login providers ]   Storage & backup   Instance                              |
|   Projects         |                                                                                 |
|   Memory spaces    |  +-----------------------+-------------------+-----------------------+-----+    |
|   Tasks & plans    |  | Provider              | Configured        | Live callback         |     |    |
|   Sync center      |  +-----------------------+-------------------+-----------------------+-----+    |
|                    |  | Email OTP             | Yes               | Verified              | ... |    |
| GOVERNANCE         |  | Feishu                | Yes               | Not verified          | ... |    |
|   Members & access |  | GitHub                | Yes               | Not verified          | ... |    |
|   Audit log        |  +-----------------------+-------------------+-----------------------+-----+    |
|   Team settings    |                                                                                 |
|                    |  GitHub setup                                                                   |
|                    |    Client identifier     Present in deployment config                           |
| > Instance         |    Callback URL                                                                 |
|   My account       |    https://memory.example.com/api/v1/auth/github/callback                       |
|                    |                                                                                 |
|                    |  [ Copy callback URL ]   [ Deployment configuration guide ]                     |
|                    |                                                                                 |
|                    |  This page never reveals provider secrets or configures an LLM.                 |
+--------------------+---------------------------------------------------------------------------------+
```

### 6.9 登录

居中的窄表单，三种入口同宽同高；验证码在下一状态原位展开，保留输入邮箱。未配置的provider不出现在公共登录页。

```text
+------------------------------------------------------------------------------------------------------+
|                                                                                                      |
|                                                                                                      |
|                                                                                                      |
|                           LWC / TEAM                                                                 |
|                                                                                                      |
|                           Sign in to your team memory                                                |
|                                                                                                      |
|                           Work email                                                                 |
|                           [ you@company.com                         ]                                |
|                                                                                                      |
|                           [ Send verification code                  ]                                |
|                                                                                                      |
|                           -------------------- or -------------------                                |
|                                                                                                      |
|                           [ Continue with Feishu                    ]                                |
|                           [ Continue with GitHub                    ]                                |
|                                                                                                      |
|                           Available methods depend on this deployment.                               |
|                                                                                                      |
|                                                                                                      |
|                                                                                                      |
+------------------------------------------------------------------------------------------------------+
```

### 6.10 时序记忆与Discussion

复用空间详情的同一内容轴和筛选行，新增“时间线”“Discussion”两个Tab，不增加侧栏项。时间线按发生时间展示，并独立显示记录时间和有效区间；事件详情列出context、片段、变化、证据、关系、pin、反馈、原始历史与同步状态。Discussion使用同款列表加详情，查看问题/回答/总结/确认/撤回/历史；只展示正式持久记录，不抓取宿主隐藏提示或思维链。

来源详情增加版本链，检索反馈在对象详情内查看，不再新建一级导航。页面显示各核心类型的同步数量/最后确认head；“无该类型记忆”和“不支持/同步失败”不能显示成同一种空白。

```text
+------------------------------------------------------------------------------------------------------+
| LWC / TEAM         |  Acme Team / Memory spaces                      [ Search... ]  [ Account v ]    |
+--------------------+---------------------------------------------------------------------------------+
| [Acme Team v]      |  Frontend core / Timeline                                                       |
|                    |  All core memory is replicated. This view shows cloud-accepted records.         |
| WORKSPACE          |                                                                                 |
|   Overview         |  Knowledge [ Timeline ] Discussion Sources Relations Tasks Access               |
|   Projects         |                                                                                 |
| > Memory spaces    |  [ Type: All v ]  [ Since: Sep 28 v ]                     [ Find memory... ]    |
|   Tasks & plans    |  +----------------------+--------------------------+------------+----------+    |
|   Sync center      |  | Occurred / type      | Summary                  | Evidence   | Sync     |    |
|                    |  +----------------------+--------------------------+------------+----------+    |
| GOVERNANCE         |  | 14:31 / decision     | Release gate updated     | 3 sources  | Accepted |    |
|   Members & access |  | 14:28 / change       | Plan step added          | 2 sources  | Accepted |    |
|   Audit log        |  | 14:20 / observation  | Retry delay observed     | 1 source   | Conflict |    |
|   Team settings    |  +----------------------+--------------------------+------------+----------+    |
|                    |                                                                                 |
|                    |  Release gate updated                                                           |
|   Instance         |                                                                                 |
|   My account       |  Occurred: 14:31    Recorded: 14:32    Valid from: Sep 28                       |
|                    |  Context: Release verification                                                  |
|                    |  Fragments: 2   Changes: 1   Relations: 3   Feedback: 1                         |
|                    |  Sources and original history remain available.                                 |
|                    |                                                                                 |
|                    |  [ Event details ]   [ Related discussion ]   [ Tool commands ]                 |
+--------------------+---------------------------------------------------------------------------------+
```

## 7. 交互和异常状态

| 状态 | 页面反馈 | 布局和动作 |
|---|---|---|
| 首次没有团队/空间 | 简洁的Empty区域，说明下一步 | 可创建者给一个创建按钮；普通成员显示邀请/加入说明 |
| 搜索无结果 | 保留筛选，提示没有匹配项 | 提供清除筛选，不把整个表格换成巨大插画 |
| 网络加载 | 表格/详情内Skeleton | 保留标题、筛选、列宽与高度 |
| 服务不可达 | 保留已加载数据并注明截至时间 | 页内Alert+重试；不把旧数据伪装成实时 |
| 登录过期 | 保留安全的当前路由，重新登录 | 成功返回原位置；只允许同源返回地址 |
| 权限撤销 | 清空相关查询缓存和内容视图 | 显示当前没有权限，不继续展示页面旧正文 |
| 副本离线 | 中性标签+最后报告时间 | 离线不等于错误、不等于没有本地变化 |
| 有语义分歧 | 已保全及实际投递/处理状态 | 材料、代次和回执可读；Agent观察后立即优先处理，无人工审批按钮 |
| 发布/校验失败 | 明确失败码、最近尝试、影响范围 | 行内状态+详情，不能用toast替代可追踪记录 |
| 保存授权成功 | 局部更新+简短确认 | 焦点和滚动位置保持，不重载整个页面 |
| 保存时发生并发变化 | 展示最新授权与未保存改动 | 不静默覆盖，保留用户表单内容供重新提交 |

同步列表每10秒刷新一次已打开页面的状态，标签页隐藏时暂停；按server/team/space组成请求和缓存键，切团队后取消旧请求，防止旧团队响应覆盖新页面。手动刷新只查询服务端，不让浏览器代替设备执行sync。

服务端“可达副本”根据最近的报告判断，默认60秒内收到认证报告算近期可达，超过阈值显示最后时间。已追平还要求最后ACK等于当前head；这只是上报时的状态，不保证客户端当前没有未提交编辑。设备上报的pending值要附采集时间；没有上报就是未知。

注销副本、撤销会话、移除成员等只描述确切影响：停止未来接入，不远程删除本地库。普通检索和浏览不要求确认。UI不能给没有对应API支持的按钮假成功。

## 8. 路由与接口清单

路由是目标设计；身份/space校验由服务端执行，前端隐藏按钮只是体验。

| 路由 | 主要读取接口 | 写入或操作 |
|---|---|---|
| `/login`、`/device`、`/invite` | providers、device challenge、有效邀请摘要 | 验证码/OAuth/设备确认/接受邀请 |
| `/t/:team/overview` | 当前用户可见空间的摘要与最近发布 | 无记忆写入 |
| `/t/:team/projects` | projects、collections、当前grant | 目录CRUD与集合关联 |
| `/t/:team/spaces` | 授权空间列表和状态 | 创建/归档权限按policy；加入说明 |
| `/t/:team/spaces/:space` | pages、memory/events及反馈、discussions及history、sources/version链、relations、revisions | 首版正文只读；grant单独接口 |
| `/t/:team/tasks` | 可见空间Todo/Plan列表、详情、历史 | 复制本地工具命令 |
| `/t/:team/sync` | replicas、head、receipts、conflicts、诊断摘要 | 有权限者撤销副本接入 |
| `/t/:team/sync/conflicts/:id` | 基线、候选、引用、决议历史 | 复制工具调用材料入口 |
| `/t/:team/members` | memberships、invitations、可管理grant | 邀请、移除、显式授权，均带revision |
| `/t/:team/audit` | 治理审计/获准空间发布记录 | 分页筛选，首版无批量导出 |
| `/t/:team/settings` | 基本团队信息 | owner更新名称等资料 |
| `/account` | identities、sessions | 主动绑定/解绑、撤销本人会话 |
| `/instance` | 已脱敏provider状态、健康、容量、备份manifest摘要 | 部署配置说明；不在线编辑凭据或恢复活跃卷 |

Web新增的聚合接口不能绕过既有policy。按20条默认分页，筛选和排序在服务端执行；跨空间知识查询先做授权交集。全局搜索仅限当前团队内有grant的空间，不默认混入个人记忆。

页面详情用space+对象逻辑ID定位；不能接受任意服务器文件路径。来源正文和Markdown在渲染前做净化；外链限制安全协议。浏览器只用同源HttpOnly会话，CSRF/Origin校验复用服务端设计。

## 9. React实现边界与组件复用

拟新增目录，当前不代表代码已存在：

```text
admin/
  package.json
  components.json
  vite.config.ts
  src/
    app.tsx
    styles.css
    lib/api.ts
    components/ui/       shadcn/ui按需引入
    components/shell.tsx
    components/page-header.tsx
    components/status-badge.tsx
    pages/               按导航实现，不按数据库表拆页面
```

导航、PageHeader、筛选条、状态标签、Table/Sheet/Dialog只做有限共用。使用shadcn/ui的Sidebar、Breadcrumb、Button、Input、Select、Tabs、Table、Sheet、Dialog、AlertDialog、Badge、Skeleton、Empty等必要组件；只引入实际使用的部分。

首版使用原生fetch和局部React状态；没有复杂表格需求时用Table加服务端分页，不预装数据分析和拖拽看板库。路由选一套维护中的常规React方案；不引入SSR、BFF、Redux或第二套服务端认证。Markdown复用已验证的解析/净化规则。

构建时从 `web/src/tokens.css` 引入LWC基础token，在admin样式中映射shadcn语义变量，不复制两份会漂移的颜色表。后台自身增加的尺寸变量放在admin；日后确有第二个React消费者再提取共享组件包。

Docker构建 `admin/dist`，与Rust服务一并交付静态资源目录，由 `lwc server` 同源提供。前端深链接回退不能吞掉 `/api/` 的404；secret不进入Vite环境变量或静态包。客户端原有分发不被迫携带React依赖，服务端镜像包含完整UI产物。

官方实现依据：[Vite接入](https://ui.shadcn.com/docs/installation/vite)、[CSS变量主题](https://ui.shadcn.com/docs/theming)、[Sidebar组件](https://ui.shadcn.com/docs/components/sidebar)。本设计沿用其接入方式和主题能力，具体版本在实施时锁定。

## 10. 实施顺序和必要验收

1. 先做React应用入口、LWC token映射、Shell、PageHeader、Table和状态标签，建立唯一尺寸基准。
2. 接登录/设备确认和当前账号，再接团队切换、目录与grant。使用真实权限结果，不用前端常量冒充角色。
3. 接空间阅读、来源引用、Todo/Plan查看；所有数据取云端已接受版本。
4. 接同步、冲突对比、审计和实例配置状态；把未知/离线/保留/待投递/处理中/已合并/已同步的状态文案做对。
5. 运行受影响组件检查、typecheck和生产构建；按1440、1024、390三种宽度做一次关键页面视觉检查。

重点检查：侧栏/内容轴线一致；按钮与输入同高；卡片和对比栏等宽；中文长标题、200%缩放、键盘焦点、空数据/加载/错误布局稳定；普通成员和无grant管理员不能看到隐藏空间统计或冲突材料。浅黄浅绿只作辅助，不作为唯一小字前景。

本轮只验证ASCII字符宽度、文档链接和方案一致性，不启动业务服务或做无关全仓测试。实际React视觉和交互验收在实现后执行，不能用文字原型替代真实浏览器验收。

## 11. v6：设备、Agent、有效权限与恢复

以下是新增目标设计，沿用既有React/shadcn布局与LWC token。增加“设备与Agent”和“版本与恢复”导航；空间详情增加权限标签页，账号页显示验证邮箱与昵称。只读云访问设备标记remote-read，不显示虚假的副本落后量。

| 页面 | 内容与交互 |
|---|---|
| 设备与Agent | 用户归属、设备名称/OS/登记指纹、Agent声明宿主、凭据状态、最近活动；展开查看委托范围，按权限撤销 |
| 有效权限 | user/device/agent/space/object选择；动作矩阵显示允许/拒绝、规则来源、策略版本与离线许可到期；服务器计算结果 |
| 版本与恢复 | 对象/批次时间线、主体归属、前后diff、恢复目标与影响范围；有权限可预览并提交补偿，显示新head |
| 隔离与恢复状态 | 上传拒绝/下载验证失败/撤权未共享修改/待Agent合并分别展示；材料仅对有权用户可见 |

管理端不是Agent合并的人工审批队列。Agent可直接经CLI/MCP完成同一恢复合同；页面只提供可观察性和管理员主动操作入口。昵称不代替不可变身份，完整MAC不展示、不存储。

以下线框每行104列，实际页面继续遵守既有间距、同高控件和响应式规范；英文占位便于严格等宽核查，产品按中文文案实现。
```text
+------------------------------------------------------------------------------------------------------+
| LWC / Team                         Devices & Agents                                  [ Register ]    |
|                                                                                                      |
| Navigation          |  [ All users       v ]  [ All modes       v ]  [ Search agent...          ]    |
| --------------------+------------------------------------------------------------------------------- |
| Overview            |  USER          DEVICE          AGENT             MODE          STATUS          |
| Spaces              |  Alice         Mac Pro         Codex             replica       Active          |
| Devices & Agents    |  Bob           Runner-01       CI reader         remote-read   Active          |
| Permissions         |                                                                                |
| Versions & Recovery |  Selected: CI reader                                    [ Revoke credential ]  |
| Audit               |  Owner         Bob / usr_...       Device          dev_...                     |
| Settings            |  Host claim    CI                  Agent           agt_...                     |
|                     |  Scope         docs-space          Actions         read                        |
|                     |  Policy        revision 42         Last request    10:32:18                    |
|                     |                                                                                |
|                     |  Credential scope is enforced by the server. Host name is declared metadata.   |
+------------------------------------------------------------------------------------------------------+
```

```text
+------------------------------------------------------------------------------------------------------+
| LWC / Team / Space                 Versions & Recovery                                               |
|                                                                                                      |
| [ Object or batch...                     ]  [ Time range      v ]                 [ Refresh ]        |
|                                                                                                      |
| VERSION     AUTHOR / AGENT         CHANGE                              STATE                         |
| h184        Alice / Codex          Update page and references           Accepted                     |
| h185        Bob / Assistant        Replace source summary              Selected                      |
| h186        Alice / Codex          Add independent task                 Accepted                     |
|                                                                                                      |
| Recovery target: revert batch h185                                     [ Preview compensation ]      |
| ---------------------------------------------------------------------------------------------------- |
| Affected objects: 2             Conflicts: 0             Preserved later edits: 1                    |
|                                                                                                      |
| BEFORE / REJECTED VERSION                           | RESTORED CONTENT / NEW VERSION                 |
| Source summary at h185                             | Previous summary + retained later changes       |
|                                                                                                      |
| Base head: h186                 Preview digest: ...                    [ Apply compensation ]        |
| Result: a new head is created; historical evidence and unrelated changes remain.                     |
+------------------------------------------------------------------------------------------------------+
```

## 消费者交互与双语修订（用户确认）

本节更新早期管理后台原型的交互和用语。界面面向普通消费者，不能要求理解数据库、摘要、对象标识或同步协议。

- 中文统一使用“计划、待办、智能体”；英文界面完整使用英文。LWC 是唯一保留的界面品牌。语言偏好保存在本机；记忆正文和用户命名保持原文。
- 实际采用官方 shadcn/ui 组件：选择器、可搜索组合框、输入框、标签、复选框、按钮、表格、卡片、状态提示、徽标和对话框。组件源代码随仓库交付，主题复用 LWC 颜色。
- 团队切换后只显示该团队的记忆空间；成员按姓名/邮箱选择，设备从列表直接撤销，关联通过勾选完成，操作按钮随权限和有效选择启用。
- 原始标识、摘要、版本协议、原始 JSON 不进入主列表。记忆以标题、正文、来源和可理解的状态展示；连接命令按需展开并复制，用户不需要逐段拼接。
- 选择浮层在触发框下方展开，左对齐、等宽、保留统一间距；靠近窗口边缘时自动避让。不得恢复浏览器原生弹层与页面风格混用。
- 邮箱、名称、验证码等保留必要输入；限制长度和格式，验证码支持自动填充。成员与对象禁止要求手输内部标识。
- 风险操作提供清楚的影响说明。智能体冲突合并仍走工具与权限校验，不给普通用户增加逐次人工审批流程。

当前组件目录为 `admin/src/components/ui/`，通过 `admin/components.json` 管理；`admin/src/theme.css` 将组件主题连接到 LWC 配色。`admin/check-text.mjs` 检查界面双语覆盖和用户正文不被翻译。
