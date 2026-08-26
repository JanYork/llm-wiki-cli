<h1 align="center">LWC — 面向 AI Agent 的主动记忆</h1>

<p align="center">
  <strong>Agent 驱动 · 持久化 · 来源可追溯</strong>
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@i-xor/lwc"><img alt="npm: @i-xor/lwc" src="https://img.shields.io/badge/npm-%40i--xor%2Flwc-CB3837?logo=npm"></a>
  <a href="https://crates.io/crates/lwc"><img alt="crates.io: lwc" src="https://img.shields.io/crates/v/lwc.svg"></a>
  <img alt="Node.js 22 or newer" src="https://img.shields.io/badge/node-%3E%3D22-5FA04E?logo=nodedotjs">
  <img alt="平台：macOS、Linux、Windows" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-666666">
  <a href="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://skills.sh/janyork/llm-wiki-cli/using-lwc"><img alt="skills.sh: using-lwc" src="https://img.shields.io/badge/skills.sh-using--lwc-000000?logo=vercel"></a>
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> ·
  <a href="docs/readme/README.ja.md">日本語</a> · <a href="docs/readme/README.es.md">Español</a> ·
  <a href="docs/readme/README.pt-BR.md">Português (Brasil)</a> · <a href="docs/readme/README.fr.md">Français</a> ·
  <a href="docs/readme/README.ru.md">Русский</a>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-agent-memory-zh.png" alt="LWC Agent 记忆" width="100%">
</p>

`lwc` 是一个由 Agent 驱动的主动记忆 CLI，让 AI Agent 能够跨会话自主召回、维护和
演进持久化、来源可追溯的知识。

**兼容 Claude Code、Codex、Cursor、OpenCode、Gemini CLI、Kiro、Hermes、
Antigravity 和 pi。**

LWC 把经过筛选的文档转化为可长期维护的 Wiki。Agent 负责理解与综合，`lwc` 负责
保存来源、页面、引用、链接、索引和历史，让知识持续积累，而不是每次查询都重新拼接
原始片段。

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-overview-zh.png" alt="LWC 产品概览" width="820">
</p>

## LWC 是 Agent 记忆，不是 RAG

RAG 和 LWC 都能帮助大模型使用外部文档，但二者把状态留在不同的地方。典型的
RAG 会在每次查询时检索原始片段，再生成一次性答案：

```text
查询 -> 检索片段 -> 生成答案
```

LWC 会把已经完成的有效工作保留下来：

```text
任务 -> 读取持续维护的 Wiki -> 结合来源与已有综合进行推理
     -> 把值得复用的改进写回
```

检索只是 LWC 的一项操作，而不是它的组织原则。LWC 的核心产物是一个来源可追溯、
持续修订的 Wiki，其中的页面、引用、链接、矛盾和历史会随着认识变化而更新。因此，
LWC 不依赖 embedding 或向量数据库，也不会在回答完成后丢弃本次综合结果。它可以
与 RAG 配合，但它本身不是查询时 RAG。

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-source-grounding-zh.png" alt="LWC 来源追溯与可靠回答" width="820">
</p>

### LWC 应当由 Agent 操作

`lwc` 是提供给 Agent 的机器接口，不是面向人类的笔记应用。正常使用时，人类负责
选择来源、提出目标和问题，并审阅答案或投影出来的 Markdown；Agent 负责调用 CLI、
管理作用域、整合来源、维护引用与链接，以及判断哪些知识值得读取或写回。

除非正在开发或排查 `lwc` 本身，否则人类不应手工驱动日常工作流。需要使用 LWC
时，请让 Agent 激活规范的 `using-lwc` Skill，通常调用名为 `$using-lwc`。

## 让 Agent 自动完成安装与配置

把下面的提示词交给你正在使用的 Agent。它会安装全局 CLI，把已支持宿主的配置交给
LWC 的幂等 AgentTarget 安装器；只有尚未注册的 Agent 才按自身官方规范完成配置。

<details>
<summary><strong>复制完整配置提示词</strong></summary>

```text
请为当前用户完整安装并配置 LWC。请直接执行并验证，不要只输出一份让我手工执行的
教程。

权威来源：
- https://github.com/JanYork/llm-wiki-cli
- https://github.com/JanYork/llm-wiki-cli/tree/main/skills/using-lwc

要求：
1. 阅读本 README、`SECURITY.md` 和 `skills/using-lwc/SKILL.md`。如果 `lwc` 尚不能
   全局调用，安装经过校验和验证的官方 Release；日常命令不得拼接私有二进制路径
   或 `LWC_PROJECT_ROOT`。
2. 运行 `lwc --version`；全局记忆缺失时仅执行一次 `lwc --scope global init`；然后
   执行 `lwc agent install --yes`。该命令会自动检测已安装的受支持 Agent，并按官方
   路径安全安装 MCP、Skill、Hook 与 Instructions。不得手工重写这套逻辑，也不得给
   同一个 Agent 同时安装原生包和直接配置。
3. 检查 `lwc agent status --target all --location global`。按需重启受影响的 Agent，
   并完成宿主正常的 Hook 信任审查。没有项目级明确授权时，不得初始化项目 Wiki 或
   任一图能力。
4. 如果当前运行时不在 LWC 已注册 AgentTarget 中，才按该运行时的官方用户级规范安装
   规范 `using-lwc` Skill、追加式指导区块、`lwc serve --mcp`，并只在官方支持时安装
   有界会话 Hook。保留已有配置、保证幂等；某项能力没有官方配置入口时就报告不支持，
   不得猜测路径或配置键。

最后报告 LWC 版本、检测并配置的 Target、status 结果、修改文件、不支持的能力，以及
仍需完成的重启或信任操作。
```

</details>

## 思想来源与致谢

`lwc` 以 Andrej Karpathy 提出的
[LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)
模式为核心准则：让 LLM 增量构建并持续维护一个持久、互联的 Wiki，而不是每次查询
都从原始文档重新组织知识。项目的 CLI 架构与部分实现细节也参考了
[`nashsu/llm_wiki`](https://github.com/nashsu/llm_wiki)。

`lwc` 在此基础上采用 Rust 与 SQLite，实现面向 Agent 的本地命令行工具。

## 核心设计

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-architecture-zh.png" alt="LWC 架构图" width="100%">
</p>

LWC 将长期知识分成四个边界清晰的层次：

| 层次 | 用途 |
| --- | --- |
| 原始来源 | 保存经过筛选的不可变证据快照 |
| Wiki | 保存由 Agent 维护的页面、引用、链接和来源证明 |
| 时序记忆 | 记录变化、决策、结果和未解决事项 |
| Schema 与 Purpose | 约束项目后续的知识维护方式 |

SQLite 是权威数据源；Markdown、全文索引和可选图存储都是可重建投影。Agent 通过
CLI 修改知识，所有结果使用结构化 JSON，便于审计和恢复。

[查看总体架构 →](https://github.com/JanYork/llm-wiki-cli/wiki/Architecture-Overview-zh-CN)

## 分层检索与知识图

LWC 会在文档、段落和句子三个粒度索引 Source 与 Wiki 页面。Agent 可以先取得小而
相关的上下文，只在必要时展开精确片段；内容变化后，旧定位符会明确失效。

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-memory-graph-zh.png" alt="LWC 记忆图" width="100%">
</p>

可选文档图连接页面、来源、引用、链接和显式语义关系。SQLite 始终保持权威，
Grafeo 或 SurrealDB 只负责可重建的图遍历。显式关系保留理由、来源证明、置信度和
证据来源。

### 文档转换与 Office 读取

可选的 Anydoc 或 MarkItDown 适配器先把受支持的本地文件转换为可审核的 Markdown，
再交给 LWC 摄取。OfficeCLI 则为 Word、Excel 和 PowerPoint 提供独立、需授权、
只读的读取路径。两项能力都不会被静默安装或启用，也不会修改源 Office 文件。

[检索与索引 →](https://github.com/JanYork/llm-wiki-cli/wiki/Retrieval-and-Indexing-zh-CN) ·
[文档知识图 →](https://github.com/JanYork/llm-wiki-cli/wiki/Document-Knowledge-Graph-zh-CN) ·
[文档转换 →](https://github.com/JanYork/llm-wiki-cli/wiki/Document-Conversion-zh-CN)

## 可选 Learning Suite

Tutor、Book 和 Practice 是三项相互独立、默认关闭的第一方能力，并分别使用私有存储：

- **Tutor** 保存教学回合、学习者证据、目标、计划以及私有 Soul/Wiki。
- **Book** 按经过验证的来源顺序导入受支持的书籍，保证完整、有据可查的阅读。
- **Practice** 保存版本化题库、试卷、作答、评分、闪卡和 FSRS 复习状态。

各运行时按需下载，与 LWC 版本绑定并校验哈希；关闭能力不会删除规范数据。配套
Agent Skill 负责恢复与持久化，不向学习者暴露例行控制面操作。

[查看 Learning Suite 契约 →](docs/learning-suite-contracts.md)

## 安装

多数用户只需要一条包管理命令：

    npm install --global @i-xor/lwc

LWC 也支持 Homebrew、crates.io、经过校验和验证的 GitHub Release，以及本地
Cargo 构建。

[查看安装与升级指南 →](https://github.com/JanYork/llm-wiki-cli/wiki/Installation-and-Upgrades-zh-CN)

## 配套 Agent Skill

仓库内置的 [using-lwc Skill](skills/using-lwc) 会把 LWC 变成主动记忆层：召回有界
上下文，区分项目与全局知识，整合来源、维护引用，并且只写回值得复用的已验证知识。

通过 [skills.sh](https://skills.sh/JanYork/llm-wiki-cli) 安装：

    npx skills add JanYork/llm-wiki-cli --skill using-lwc -g

标准触发方式是 <code>$using-lwc</code>。Skill 不绑定具体 Agent，并为记忆、文档图、
Word Graph、CodeGraph、强标签、文档转换、首次配置、恢复和维护提供聚焦指引。

### 原生 Agent 配置

LWC 能检测受支持的 Agent，并通过幂等 AgentTarget 适配器安装其可用的 MCP、Skill、
Hook 和 Instructions：

    lwc agent install --yes

统一的只读 MCP 在不扩大工作区边界的前提下提供有界 Wiki 记忆和可选代码上下文。
支持 Claude Code、Codex、Cursor、OpenCode、Gemini CLI、Kiro、Hermes、
Antigravity 和 pi。

图能力始终先判断任务适用性和授权：文档关系任务使用物理图，代码结构任务使用
CodeGraph，不能仅因运行时存在就自动启用。Office 读取遵循相同的明确授权边界。

[查看 AgentTarget 集成 →](https://github.com/JanYork/llm-wiki-cli/wiki/AgentTarget-Installation-and-Integration-zh-CN)

## 快速开始

正常使用时，人只需描述目标并审核结果，由 Agent 操作 CLI。完整流程见
[快速开始 Wiki](https://github.com/JanYork/llm-wiki-cli/wiki/Quick-Start-zh-CN)。

### 1. 初始化项目 Wiki

Agent 创建项目本地 Wiki，并定义用途与维护规则。除非明确选择版本化，项目状态只会
加入 Git 的本地排除，不改动仓库的 .gitignore。

### 2. 加入来源材料

经过筛选的文件会成为不可变、去重的快照。LWC 跟踪其来源路径，并能判断当前文件是
未变化、已修改、缺失还是已被新版本取代。

### 3. 分析并整合一个来源

Agent 读取完整的有界来源，写入带引用的来源摘要，更新共享知识，并只在两层内容保持
一致后完成摄取。

### 4. 查询已沉淀的 Wiki

搜索优先返回维护后的页面，并保留来源依据。需要核验声明时，Agent 再打开精确的
原始证据。

## Agent 工作流

日常循环只有四步：

1. 召回相关的已维护知识。
2. 在时效性重要时检查当前来源或代码。
3. 完成最小且经过验证的更新。
4. 验证检索、链接和适用的图投影。

大范围修订使用原子 changeset。完整的信任边界、前置条件、恢复方式和完成证据见
[Agent 工作流](docs/agent-workflow.md)。

## 时序记忆

时序记忆用紧凑事件记录发生了什么、为何作出决策、尝试过什么、结果如何，以及还有
哪些问题未解决。它与 Wiki 分工明确：时序召回解释历史，Wiki 表达当前稳定知识。

保留策略有明确上限，并保护置顶、未解决和仍然开放的矛盾事件。事件以结构化数据保存，
不会存成原始聊天记录，也不会静默合并相似事件。

[查看持久记忆指南 →](https://github.com/JanYork/llm-wiki-cli/wiki/Persistent-Memory-zh-CN)

## 多机器同步

Sync 通过 SSH 协调项目记忆、全局记忆或两者，同时把 Wiki 语义状态与 Git 发布分开。
Merge 会保留两边的唯一对象，冲突则以有界数据包交给 Agent 明确解决。

同步会话可持久恢复。LWC 不复制正在使用的 SQLite、WAL 或 SHM 文件，不重置工作树，
并把权威数据发布与可重建的搜索、图投影分成独立阶段。

[查看同步工作流与安全契约 →](docs/agent-workflow.md)

## 原子化多命令变更

Changeset 让多步骤知识更新在审核和校验完成前保持不可见。提交只在一个事务中发布
实际触及的权威实体，不影响无关的在线工作；同一实体发生版本冲突时会安全失败。

成功提交会为受支持的操作保存精确逆向补丁，从而在不替换整套 Wiki 的前提下执行
受保护的回滚。

[查看 Changeset 指南 →](https://github.com/JanYork/llm-wiki-cli/wiki/Changesets-zh-CN)

## 作用域

| 作用域 | 用途 |
| --- | --- |
| project | 最近项目 Wiki 拥有的项目知识 |
| global | 可跨项目复用的知识 |
| all | 合并只读召回和协调式 Sync |

写入始终明确指向一个存储，LWC 不会隐式创建跨项目引用或链接。

[查看作用域与项目发现 →](https://github.com/JanYork/llm-wiki-cli/wiki/Scopes-and-Project-Discovery-zh-CN)

## 搜索与 CJK 文本

搜索是确定性的词法检索，并优先返回整理后的页面。标题、路径、摘要、正文、来源证明和
图证据分别计分；支持页面、来源、类型筛选，也能解释完整的评分过程。

CJK 文本使用相邻二元组并保留有用的一元词，拉丁文本使用小写字母数字词项。它不依赖
词典，因此对产品名、代码符号、中英混排和新兴词汇保持稳定。

### 显式检索权重与反馈

可审计的文档权重表达长期重要性；查询反馈只对同一词序指纹的候选项重排，并只保存
指纹而非原始查询。两者都不能让不相关内容进入结果。

[查看搜索与上下文指南 →](https://github.com/JanYork/llm-wiki-cli/wiki/Search-and-Context-zh-CN)

## 只读预览与 CodeGraph

本地 Viewer 通过仅回环、仅 GET/HEAD 的接口展示页面、来源、Markdown、文档关系和
代码结构，不会执行迁移、刷新或建图。

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-codegraph-zh.png" alt="LWC 代码图" width="100%">
</p>

CodeGraph 仅用于项目，并且必须显式初始化。它可以回答符号、调用方、被调用方、依赖、
文件和影响范围，同时保持遥测关闭，并按所属文件原子更新图数据。

固定运行时识别 TypeScript、TSX、JavaScript、JSX、ArkTS、Python、Go、Rust、
Java、C、C++、C#、Razor、PHP、Ruby、Swift、Kotlin、Dart、Svelte、Vue、
Astro、Liquid、Pascal、Scala、Lua、Luau、Objective-C、R、Solidity、Nix、
YAML、Twig、XML、.properties、CFML、CFScript、CFQuery、COBOL、VB.NET、
Erlang 和 Terraform。

[查看 Viewer 指南 →](https://github.com/JanYork/llm-wiki-cli/wiki/Read-Only-Viewer-zh-CN) ·
[查看 CodeGraph 指南 →](https://github.com/JanYork/llm-wiki-cli/wiki/Code-Graph-zh-CN)

## 维护与投影

Lint、重建索引、Markdown 物化、压缩、checkpoint 和图投影都是显式操作。耗时工作
可持久追踪、观察、恢复，并按有界的单文档单元执行。

SQLite 始终保持权威；搜索索引、Markdown 和图存储都可以重建，不会改写来源历史或
当前 Wiki 知识。

[查看维护与诊断 →](https://github.com/JanYork/llm-wiki-cli/wiki/Maintenance-and-Diagnostics-zh-CN)

## 基准测试集

可选基准在调用者提供的脱敏语料上测量导入时间、搜索延迟、Recall@5/10、MRR 和
存储占用。公平比较需要固定机器、语料、查询集和运行条件，并比较多次运行的中位数。

[查看基准方法 →](benchmarks/README.md)

## 持久 Todo 与当前 Plan

Todo 保存延后处理的工作；Plan 保存当前执行目标、步骤、进度和 revision。两项能力
相互独立、按需启用，也不会自动互相转换。

有界生命周期上下文让 Agent 在新会话或上下文压缩后恢复当前计划与到期提醒，同时
避免暴露不必要的私密细节。

[查看 Todo 与 Plan 工作流 →](docs/agent-workflow.md#todo-and-plan)

## 限制与非目标

当前设计约束：

- 单机、单用户知识库；
- UTF-8 文本工作流；
- 每个 schema、purpose、source 或 page body 的输入上限为 64 MiB；
- 提供词法搜索，不提供语义向量检索。

这个 CLI 当前明确不做：

- 不内置 LLM 调用；
- 不接入向量数据库；
- 不提供守护进程或后台服务；
- 不提供 Web UI 或桌面 UI；
- 不提供直接编辑数据库的工作模式。

如果投影出来的 Markdown 发生漂移，就重建它；如果 SQLite 数据库结构有问题，就通过
CLI 和迁移修复，不要手工修改。

## 参与贡献

欢迎提交 issue 和 pull request，尤其是围绕以下方向：

- Agent 工作流的人机工程；
- 确定性的投影行为；
- 持久化引用与页面维护约定；
- 面向多语言技术语料的搜索质量。

提交 Pull Request 前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。安全问题请按照 [SECURITY.md](SECURITY.md) 报告。

## 许可证

本项目使用 [Apache License 2.0](LICENSE)。
