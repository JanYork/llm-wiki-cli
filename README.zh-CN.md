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

## 思想来源与致谢

`lwc` 以 Andrej Karpathy 提出的
[LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)
模式为核心准则：让 LLM 增量构建并持续维护一个持久、互联的 Wiki，而不是每次查询
都从原始文档重新组织知识。项目的 CLI 架构与部分实现细节也参考了
[`nashsu/llm_wiki`](https://github.com/nashsu/llm_wiki)。

`lwc` 在此基础上采用 Rust 与 SQLite，实现面向 Agent 的本地命令行工具。

## 核心设计

```text
+----------------+  ingest  +--------------------+  JSON I/O +----------------+
| Source Files   | -------> | lwc CLI            | <-------> | LLM Agent      |
| immutable      |          | ingest/page/query  |           | reason/update  |
+----------------+          +---------+----------+           +----------------+
                                      |
                               transactions
                                      |
                                      v
         +----------------------------------------------------------+
         | SQLite Stores (Canonical)                                |
         | project: .lwc/wiki.db   global: ~/.lwc/wiki.db           |
         | sources | pages | citations | links | FTS                |
         | schema | purpose | ingest queue | operation log          |
         +----------------------------+-----------------------------+
                                      |
                           materialize / rebuild
                                      |
                                      v
         +----------------------------------------------------------+
         | Markdown Projection (Derived)                            |
         | .lwc/raw/ | .lwc/wiki/ | schema.md | purpose.md          |
         +----------------------------------------------------------+
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

## 为什么它不只是 RAG

传统 RAG 每次查询都会检索原始片段，再重新组织答案。`lwc` 维护的是持久化
Wiki：来源保持不可变，有价值的综合结果会成为持久页面，矛盾与链接能够跨会话
保留，更好的答案也可以继续写回知识库。

最终积累下来的是知识，而不只是一次检索结果。

## 安装

从 GitHub 安装：

```bash
cargo install --locked --git https://github.com/JanYork/llm-wiki-cli
```

也可以安装本地检出的源码：

```bash
git clone https://github.com/JanYork/llm-wiki-cli.git
cd llm-wiki-cli
cargo install --locked --path .
```

## 快速开始

### 1. 初始化项目 Wiki

```bash
cd your-project
lwc init
printf '# Schema\nEvery factual page cites sources.\n' | lwc schema set -
printf '# Purpose\nBuild a durable project Wiki.\n' | lwc purpose set -
```

### 2. 加入来源材料

```bash
lwc source add-dir docs/
```

### 3. 分析并整合一个来源

```bash
lwc ingest next --context-limit 50
lwc ingest analyze 1 --file analysis.md
```

在完成该 ingest 任务之前，先创建至少一个带引用的 source-summary 页面：

```bash
lwc page put source-1 \
  --title "Source 1 Summary" \
  --kind source \
  --summary "What this source contributes" \
  --file source-summary.md \
  --source 1

lwc ingest complete 1
```

这个 `kind=source` 步骤是必需的。一个来源不会因为已经导入，或者只更新了 concept/entity 页面，就被视为已经完成整合。

### 4. 查询已沉淀的 Wiki

```bash
lwc context --limit 50
lwc search "question keywords" --limit 20
lwc page show source-1
```

## Agent 工作流

标准工作流如下：

1. 收集不可变来源。
2. 用 `lwc ingest next` 领取一个 ingest 任务。
3. 读取返回的 schema、purpose、来源快照和有界上下文。
4. 先分析，再生成页面。
5. 用显式 `--source` 引用来写入或修订持久化页面。
6. 只有在存在一个带引用的 `kind=source` 摘要页后，才能 complete。
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
lwc log --limit 20
```

说明：

- `lint` 报告确定性的结构问题，并记录这次 lint。
- `maintenance reindex` 从 SQLite 重建派生搜索产物。
- `maintenance materialize` 从 SQLite 重建投影出来的 Markdown 树。
- 搜索查询默认是私有的；只有需要把查询文本写入持久化操作日志时，才加 `--record`。

备份前应停止正在运行的 `lwc` 命令，并复制完整的 `.lwc/` 目录。写入进程可能仍在使用 WAL 文件时，不要只复制 `wiki.db`。

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
