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

## 为什么它不只是 RAG

传统 RAG 每次查询都会检索原始片段，再重新组织答案。`lwc` 维护的是持久化
Wiki：来源保持不可变，有价值的综合结果会成为持久页面，矛盾与链接能够跨会话
保留，更好的答案也可以继续写回知识库。

最终积累下来的是知识，而不只是一次检索结果。

## 安装

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
价值的会话中主动承担外部记忆层。可在本地检出目录中安装到 Codex：

```bash
mkdir -p "${CODEX_HOME:-$HOME/.codex}/skills"
cp -R skills/using-lwc "${CODEX_HOME:-$HOME/.codex}/skills/"
```

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
Agent Skills 的资源目录形式，`agents/openai.yaml` 提供 OpenAI/Codex 元数据；
其他 Agent 运行时只有在支持该目录约定时才可加载，本项目不宣称普遍兼容。

## 快速开始

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
