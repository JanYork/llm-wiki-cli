<h1 align="center">lwc</h1>

<p align="center">
  <strong>面向 LLM Agent 的持久化、来源可追溯 Wiki。</strong>
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a>
</p>

`lwc` 是一个面向 Agent 的 CLI，它把经过筛选的文档转化为可长期维护的
Wiki。Agent 负责理解与综合，`lwc` 负责保存来源、页面、引用、链接、索引和历史，
让知识持续积累，而不是每次查询都重新拼接原始片段。

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

### LWC 应当由 Agent 操作

`lwc` 是提供给 Agent 的机器接口，不是面向人类的笔记应用。正常使用时，人类负责
选择来源、提出目标和问题，并审阅答案或投影出来的 Markdown；Agent 负责调用 CLI、
管理作用域、整合来源、维护引用与链接，以及判断哪些知识值得读取或写回。

除非正在开发或排查 `lwc` 本身，否则人类不应手工驱动日常工作流。需要使用 LWC
时，请让 Agent 激活配套的 `using-lwc` Skill，通常调用名为 `$using-lwc`。如果当前
Agent 运行时支持命名 Skill 命令，下面的配置还会注册 `$using-wiki` 作为便捷别名。

## 让 Agent 自动完成安装与配置

把下面的提示词交给你正在使用的 Agent。它会使用自身原生设置安装 CLI 和用户级
Skill、初始化全局记忆，并在支持 Hook 时加入最小化的会话启动提醒；整个过程不会
覆盖已有配置。

<details>
<summary><strong>复制完整配置提示词</strong></summary>

```text
请为当前用户和正在执行本提示词的 Agent 运行时完整安装并配置 LWC。请直接执行并
验证，不要只输出一份让我手工执行的教程。

权威来源：
- https://github.com/JanYork/llm-wiki-cli
- https://github.com/JanYork/llm-wiki-cli/tree/main/skills/using-lwc

要求：
1. 执行前，完整阅读仓库 README、SECURITY.md、
   skills/using-lwc/SKILL.md，以及它直接要求的脚本和参考文件。记录本次安装所用的
   源码 commit SHA。
2. 使用你自身原生的用户级 Skill、全局指令和生命周期 Hook 位置及配置机制。不要让
   用户解释你的配置文件，不要套用其他 Agent 的文件名，也不要在未明确要求时顺带
   配置机器上的其他 Agent 运行时。
3. 保留所有已有用户配置；修改任何已有文件或 Skill 前先创建带时间戳的备份。所有
   操作必须幂等，再次运行本提示词时不得重复追加配置块。
4. 把 `skills/using-lwc` 安装或更新为规范的用户级 Skill。若运行时支持命名 Skill
   命令，以它注册 $using-lwc，并把 $using-wiki 注册为指向规范 Skill 的轻量别名；
   不要复制出两份独立维护的实现。若运行时使用其他调用语法，则注册最接近的原生
   别名，并在报告中写明准确名称。在能力支持时验证两个入口都可发现。
5. 在当前宿主授权的工作区根目录中，严格按照规范 Skill 的要求运行 bootstrap。
   缺少兼容 CLI 时，允许它安装经过 SHA-256 校验的官方 LWC Release，并初始化
   ~/.lwc 全局记忆。校验 bootstrap 返回的 JSON、lwc 绝对路径、`lwc --version`
   和全局 Wiki。除非用户针对某个项目明确要求，否则不要初始化任何项目 Wiki；
   也不得用全局记忆代替项目级写入。
6. 只通过你自身的原生配置加入最小且互补的集成：
   - 在追加式的用户级全局 system/developer instructions 中加入一条简短的 LWC
     路由规则，不得替换内置提示词。
   - 在你的用户级全局指令文件中合并一个 LWC 区块，不论该文件叫什么，都使用固定
     注释 `<!-- LWC_START -->` 与 `<!-- LWC_END -->` 包围。以后重复执行时，只替换
     这两个标记之间的内容，标记之外的所有用户内容一律不得改动。如果只找到一个
     标记，应停止修改，不能猜测用户内容的边界。区块应要求在实质性的项目、研究、
     规划、调试、决策或文档摄取工作中使用规范的 LWC Skill；先读取再重复调查；把
     可长期复用的结论写回；分离项目与全局作用域；遵守当前授权工作区边界；绝不
     保存秘密、原始思维链、临时日志或没有依据的猜测。
   - 如果你支持生命周期 Hook，则创建或合并一个原生的用户级会话启动 Hook。Hook
     只应注入一条简短提醒，让你在合适时机判断并使用 LWC Skill；Hook 本身不得读取、
     初始化或修改任何项目 Wiki。不要增加每次提交提示词都会运行的 Hook，不要替换
     无关 Hook，也不要绕过 Hook 信任审查。若你不支持 Hook，则使用全局指令作为
     降级方案并报告限制，不得臆造机制。
7. Skill、全局指令和 Hook 应保持简短并各司其职，不要复制整份策略；规范 Skill
   始终是唯一权威工作流。
8. 校验每个修改过的配置文件和 Hook 可执行文件，确认旧配置仍然保留，并运行最小
   安全冒烟检查。只使用你原生支持的配置，不得臆造字段或文件名。

最后给出简洁报告，包含：识别到的 Agent 运行时、LWC 版本与路径、源码 commit、
Skill 与别名路径、全局 Wiki 路径、修改过的 Hook/配置文件、备份路径、验证结果、
不受支持的集成，以及哪些变化需要新开 Agent 会话或完成正常的 Hook 信任确认后
才能生效。
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

```text
+-----------------------------------------------------------------------+
|                              AGENT PLANE                              |
+-----------------------------------------------------------------------+
| User Task -> LLM Agent -> using-lwc Skill                             |
|                         trigger | bootstrap | recall | write-back     |
+-----------------------------------------------------------------------+
                                   |
                          JSON / stdin / files
                                   v
+-----------------------------------------------------------------------+
|                               CLI LAYER                               |
+-----------------------------------------------------------------------+
| clap command router                                                   |
| init | schema | purpose | source | page | ingest | search | context   |
| graph | lint | maintenance | checkpoint | log                         |
+-----------------------------------------------------------------------+
                                   |
                                   v
+----------------------------------+------------------------------------+
| SCOPE RESOLVER                   | IMPORT / VALIDATION                |
| project | global | all (merge)   | UTF-8 | size | ext | symlink       |
+----------------------------------+------------------------------------+
                                   |
                                   v
+-----------------------------------------------------------------------+
|                             SQLITE STORE                              |
+-----------------------------------------------------------------------+
| Canonical | WAL | foreign keys | transactions | migrations            |
| meta | sources | pages | page_sources | links | ingest_jobs           |
| operations | search_fts                                               |
+-----------------------------------------------------------------------+
                                   |
                                   v
+-----------------------+-----------------------+-----------------------+
| SEARCH PIPELINE       | GRAPH ENGINE          | MARKDOWN PROJECTION   |
| CJK n-grams + Latin   | links + citations     | raw/ + wiki/          |
| contentless FTS5/BM25 | structural evidence   | index/log/overview    |
+-----------------------+-----------------------+-----------------------+
```

持久化知识模型分为三个逻辑层：

| 层 | 内容 | 约束 |
| --- | --- | --- |
| Raw sources | 经过筛选的输入内容的不可变快照 | 通过 `source` 加入，不改写来源事实。 |
| Wiki | Agent 维护的页面、引用和链接 | 通过 `page` 更新，让事实声明有来源依据。 |
| Schema and purpose | 维护规则与项目目标 | 约束后续每一次 ingest 和修订。 |

SQLite 是唯一的规范事实源。Markdown 树是供人和 Obsidian 等工具使用的可重建
投影。Agent 通过 `lwc` 修改知识，而不是直接编辑 `.lwc/wiki.db` 或投影出来的
Markdown。命令成功时向 stdout 返回 JSON，失败时向 stderr 返回结构化 JSON。

对于当前格式的 store，读取命令保持只读；新版 CLI 第一次打开可写的旧版 store
时，会先在事务中完成一次 schema 迁移，再继续读取。

## 安装

大多数用户应直接使用上面的 Agent 配置提示词。下面的手动命令主要用于维护、排障，
或无法安装配套 Skill 的 Agent 环境。

从 GitHub 安装：

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh | sh
```

安装脚本支持 x86_64/aarch64 的 macOS、glibc Linux 和 Windows Git Bash，校验
Release 文件的 SHA-256 后安装或更新 `lwc`。默认安装到 `~/.local/bin`；
如果 `~/.local/bin` 或 `~/.cargo/bin` 中已有 `lwc`，则会原地更新。也可以指定
安装目录：

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh | LWC_INSTALL_DIR="$HOME/bin" sh
```

也可以使用 Cargo 从 GitHub 源码构建并安装：

```bash
cargo install --locked --git https://github.com/JanYork/llm-wiki-cli
```

也可以安装本地检出的源码：

```bash
git clone https://github.com/JanYork/llm-wiki-cli.git
cd llm-wiki-cli
cargo install --locked --path .
```

## 配套 Agent Skill

仓库内置 [`skills/using-lwc`](skills/using-lwc) Agent Skill，让 `lwc` 在有长期
价值的会话中主动承担外部记忆层。应将它安装到当前 Agent 运行时的用户级 Skill
目录。以 Codex 为例，可在本地检出目录中执行：

```bash
mkdir -p "${CODEX_HOME:-$HOME/.codex}/skills"
cp -R skills/using-lwc "${CODEX_HOME:-$HOME/.codex}/skills/"
```

规范调用名是 `$using-lwc`。上面的配置提示词还会创建可选的 `$using-wiki` 别名，
但不会复制出另一份 Skill 实现。

Skill 触发后会：

- 查找兼容 CLI，缺失时安装经过校验的官方 Release；
- 首次自动初始化 `~/.lwc/` 全局记忆；
- 在重复调查前读取有界的全局与项目上下文；
- 用户显式调用时初始化当前项目，否则创建项目级 `.lwc/` 前先询问；
- 拒绝向当前授权工作区根目录之外写入项目内容；
- 区分项目事实与可跨项目复用的全局知识；
- 完整整合来源，并把值得保留的答案写回 Wiki。

Skill 会先用 `LWC_PROJECT_ROOT` 约束规范化后的授权工作区边界，再收窄到选定的当前
项目；项目发现和初始化不能越过该边界向上查找。

设置 `LWC_AUTO_INSTALL=0` 可禁用自动安装。自动安装执行 Skill 随附、经过审查
的本地安装器；其信任边界是当前仓库与 GitHub Release 发布权限，并使用
`SHA256SUMS` 验证下载归档完整性。该校验不是发布者代码签名。Release 二进制覆盖
x86_64/aarch64 的 macOS、glibc Linux，以及 Windows Git Bash。`SKILL.md` 遵循
Agent Skills 的资源目录形式，`agents/openai.yaml` 提供 OpenAI/Codex 元数据。
CLI 本身不绑定具体运行时：任何能够执行 CLI，并加载或适配 Skill 指令的 Agent 都能
使用 LWC；Skill 命令、全局指令和 Hook 的注册方式由各运行时决定，因此上面的配置
提示词会先识别并适配当前宿主。

## 快速开始

本节记录的是 Agent 实际执行的 CLI 协议；正常使用时，人类不需要手工运行这些命令。

### 1. 初始化项目 Wiki

```bash
cd your-project
lwc init
printf '# Schema\nEvery factual page cites sources.\n' | lwc schema set -
printf '# Purpose\nBuild a durable project Wiki.\n' | lwc purpose set -
```

初始化项目时，LWC 会按需把项目相对路径 `.lwc/` 加入 Git 的本地
`info/exclude`，不会修改仓库 `.gitignore`。只有明确准备版本化 Wiki 时才使用
`lwc init --no-git-exclude`。

### 2. 加入来源材料

```bash
lwc source add-dir docs/
```

没有显式标题的文件会确定性地使用来源路径作为可读标题；内容相同的文件会通过
SHA-256 去重。

解析后位于当前项目 Wiki 根目录之外的来源必须显式传入
`--allow-external-source`。检测到高置信度凭证特征时默认拒绝；只有确认不可变
快照安全后，才能传入 `--acknowledge-sensitive-source`。

需要原子导入经过筛选的一组来源时，可使用相对 manifest 所在目录解析的 JSON：

```json
{
  "sources": [
    {"path": "ARCHITECTURE.md", "title": "Architecture contract"},
    {"path": "src/store.rs", "title": "SQLite store"}
  ]
}
```

```bash
lwc source add-manifest lwc-sources.json
```

### 3. 分析并整合一个来源

```bash
lwc ingest next --context-limit 50 --source-max-chars 100000
lwc ingest analyze 1 --file analysis.md
```

如果 manifest 或调度器已经选定明确的 pending source ID，使用
`lwc ingest claim 7` 精确领取。

如果返回的 `source_window.has_more` 为 true，就从
`source_window.next_offset_chars` 继续读取：

```bash
lwc source show 1 --offset-chars 100000 --max-chars 100000
```

完成 ingest 之前，既要创建带引用的 source-summary 页面，也要把这个来源的贡献
整合进至少一个非 source 页面：

```bash
lwc page put source-1 \
  --title "Source 1 Summary" \
  --kind source \
  --summary "What this source contributes" \
  --file source-summary.md \
  --source 1

lwc page put durable-concept \
  --title "Durable Concept" \
  --kind concept \
  --summary "How this source changes shared knowledge" \
  --file concept.md \
  --source 1

lwc ingest complete 1
```

两层都必需：source 页面负责导航和来源追溯，非 source 页面让知识真正持续积累。
如果某个来源确实不应改变任何共享页面，需要记录一条具体且可审计的说明：

```bash
lwc ingest complete 1 \
  --no-derived-pages-reason "Duplicate evidence; existing synthesis already covers every supported claim"
```

### 4. 查询已沉淀的 Wiki

```bash
lwc context --limit 50
lwc search "question keywords" --limit 20
lwc search "concept only" --type page --kind concept
lwc search "exact evidence" --type source
lwc page show source-1
```

## Agent 工作流

标准工作流如下：

1. 收集不可变来源。
2. 用有界的 `lwc ingest next` 领取任务；已经明确 source ID 时使用
   `ingest claim <ID>`。
3. 读完所有来源窗口，以及返回的 schema、purpose 和有界上下文。
4. 先分析，再生成页面。
5. 用显式 `--source` 引用写入或修订 source 摘要与共享知识页面。
6. 只有两道整合门禁都通过，或明确记录无需更新共享页面的原因后，才能 complete。
7. 用 `search`、`context`、`graph` 和 `lint` 持续维护 Wiki 的一致性。

完整操作约定见 [docs/agent-workflow.md](docs/agent-workflow.md)。
运行 `lwc --help` 或 `lwc <command> --help`，可以查看面向 Agent 编写的前置条件、状态变化、副作用和下一步动作。

## 作用域

`lwc` 支持三种 scope：

| Scope | Store | 用途 |
| --- | --- | --- |
| `project` | 最近祖先目录中的 `.lwc/wiki.db` | 默认使用，保存项目级知识 |
| `global` | `~/.lwc/wiki.db` | 保存可跨项目复用的知识 |
| `all` | project 与 global | 仅用于合并 `search` 和 `context` |

示例：

```bash
lwc --scope global init
lwc --scope global source add shared.md
lwc --scope all search "shared term"
lwc --scope all context
```

知识写入始终是显式的。`all` 不会隐式创建跨 store 的引用或链接；`search --record`
只会向每个选中的 store 追加查询操作记录。

## 搜索与 CJK 文本

搜索是词法型（lexical）且确定性的。

- 搜索词是纯文本，不是原始 FTS 语法。
- 默认的 `--type auto` 会优先返回已编译的 Wiki 页面、隐藏与其配对的 raw
  source，并在页面不足时回退到 raw source。
- 用 `--type page`、`--type source` 或 `--type all` 选择检索层；可重复传入
  `--kind` 限定页面类型，例如 `--kind concept --kind synthesis`。
- 多字 CJK 查询使用相邻 bigram；索引还会保留非停用单字，使单字查询仍可检索。
- 拉丁文本会被切成小写的字母数字 token。
- 排名使用固定的 title/summary/body 权重，因此在 `--scope all` 下，project 和 global 的结果仍可直接比较。

这里刻意不依赖词典分词。目标是在产品名、代号、混合语言术语和新出现词汇上保持稳定行为，而不依赖外部分词词典。

## 维护与投影

常用维护命令：

```bash
lwc lint
lwc maintenance reindex
lwc maintenance materialize
lwc maintenance compact
lwc checkpoint create before-large-update
lwc checkpoint list
lwc log --limit 20
```

说明：

- `lint` 默认完全只读；只有这次检查确实需要进入持久操作历史时才加 `--record`。
- `maintenance reindex` 从 SQLite 重建派生搜索产物。
- `maintenance materialize` 从 SQLite 重建投影出来的 Markdown 树。
- `maintenance compact` 优化 contentless FTS5 索引并尝试执行 WAL truncate
  checkpoint。应在 Wiki 空闲时运行，并检查返回的 `busy` 与 `after_bytes`。
- 搜索查询默认是私有的；只有需要把查询文本写入持久化操作日志时，才加 `--record`。

`lwc checkpoint create <NAME>` 使用 SQLite 在线备份 API。执行
`lwc checkpoint restore <NAME>` 时，LWC 会先创建 `pre-restore-*` 安全
checkpoint，再恢复数据库并重建投影。受保护删除使用 `source remove <ID>` 和
`page remove <SLUG>`：仍被页面引用的来源、仍有入链的页面都会被拒绝删除。

需要文件系统级外部备份时，应先停止正在运行的 `lwc` 命令并复制完整 `.lwc/`
目录；写入进程可能仍在使用 WAL 文件时，不要只复制 `wiki.db`。

## 基准测试集

可选基准会把本地 UTF-8 语料导入临时 Wiki，并报告导入耗时、搜索 P50/P95、
Recall@5/10、MRR，以及 compact 前后的存储占用。Ground truth 使用 JSONL
描述查询与期望命中的语料相对路径：

```bash
cargo build --release
LWC_BENCH_CORPUS=/path/to/sanitized-corpus \
LWC_BENCH_QUERY_SET=/path/to/query-set.jsonl \
LWC_BENCH_BINARY="$PWD/target/release/lwc" \
cargo test --test search_benchmark -- --ignored --nocapture
```

常规 `cargo test --all-targets` 覆盖 page-first 搜索、type/kind 过滤、UTF-8
来源窗口、ingest 完成门禁、图关系精度、迁移、lint 与 WAL compact。工作负载约定
和公平前后对比规则见 [benchmarks/README.md](benchmarks/README.md)。

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

如果投影出来的 Markdown 漂移了，就重建它；如果 SQLite schema 有问题，就通过 CLI 和 migration 修复，而不是手改。

## 参与贡献

欢迎提交 issue 和 pull request，尤其是围绕以下方向：

- Agent 工作流的人机工程；
- 确定性的投影行为；
- 持久化引用与页面维护约定；
- 面向多语言技术语料的搜索质量。

提交 Pull Request 前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。安全问题请按照 [SECURITY.md](SECURITY.md) 报告。

## 许可证

本项目使用 [Apache License 2.0](LICENSE)。
