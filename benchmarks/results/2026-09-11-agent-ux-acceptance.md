# LWC Agent UX 与 CodeGraph 验收（2026-09-11）

基线提交：`09e922b4c800d52053cac9ef702c15b6982152e0`，交付为其上的未提交工作区修改。执行状态由 Plan `e7fd17e3b63387117000c28c2f2d711e` 维护。本报告是验收证据，不代表安装或发布。

## 已落实

- CG CLI 透传原生 stdout、stderr、退出码；MCP 保留完整原生结果。诊断、新鲜度检查与查询载荷分离。
- 统一 doctor、checkout 与索引 owner；没有索引不静默换 owner，不跨授权工作区查询。
- CLI/MCP 共用输入契约、示例和字段错误；原生 CG 参数 schema 从运行时读取。
- Plan 定点修订保留步骤身份、CAS、历史与免除/替代语义；reconcile 只读提供候选证据。
- 精简回执和 Hook；历史事件标记 latest-known，不冒充实时状态；明确知识主要维护位置。
- 路径复用系统 canonicalization；Windows 使用文件身份比较普通与 verbatim 路径，保留越界及命名空间限制，不以全路径小写比较替代系统语义。

## 验证矩阵

| 环境 | 编译与运行 | 已通过证据 |
| --- | --- | --- |
| 本机 macOS | all-features 回归、release 构建 | 合并隔离复验后 865 通过，17 原有 ignored；fmt、Clippy warnings-denied、Node 7 与 Skill 策略通过 |
| Pro macOS arm64 | 原生编译、运行 | Hook 91、UX 9、CG 8、MCP 26、Plan 4、Temporal 25，共 163；路径修正后的相关 UX/CG/MCP 通过 |
| Pro Linux arm64 | Zig 交叉编译，在隔离 Debian VM 执行 | UX 9、CG 8、MCP 26、Plan 4、Temporal 25，共 72 |
| Pro Windows x64 GNU | MinGW 交叉编译，在隔离 Wine 11.17 执行 | Windows CG 1、Agent 2、UX 6、Plan 4、Temporal 25、MCP 16，共 54；最终文件身份实现与最新测试产物的路径用例单独通过 1 项 |

按用户要求，最后 Windows 专用依赖及测试条件导入调整只增量编译 UX 测试并执行 1 个相关路径用例（2.88 秒），没有重跑 macOS/Linux 或全套回归。三平台共享行为已在前述路径修正轮验证；Windows 最终专用实现另有定向证据。

Windows 完整 54 项运行使用先前构建的测试驱动调用更新后的候选主程序；追加 1 项明确使用新增依赖后重新构建的测试驱动。测试范围不等于全部目标架构或原生 Windows 内核验证。

初轮本机并发构建引起 8 项启动预算失败，串行针对性复验通过；另一个 MCP 进程回收用例隔离复验通过，未修改原超时断言。未将首次失败隐去，也未将 ignored 计入通过。

## 实际 CodeGraph 场景与成本

`2026-09-11-agent-ux.json` 保存真实小型 Rust 仓库/工作树、CodeGraph 1.6.0 的结果：原生字节等值、worktree 索引隔离、逻辑仓库关联、fresh/dirty/untracked、重复查询等值均通过。

- 查询返回 935 → 703 字节（减少 24.8%）；Hook 4550 → 3549 字节（减少 22.0%）。每次查询调用数仍为 1；未测 tokenizer，不能当作 token 数。
- 冷 CLI 每组 5 个样本：基线 P50/P95 83.05/87.23 ms，候选 85.53/1802.88 ms；没有整体提速结论。
- MCP 首次查询 227.1 ms；热查询 20 个样本 P50/P95 0.34/1.10 ms。
- `2026-09-11-agent-ux-concurrent.json` 保留并发负载下更大长尾样本，未择优删除。

指定文件 SHA256 只证明检查时刻该范围与索引一致，不证明整个仓库、反射/动态调用或关系完整；不以空结果证明代码可删除。

## 产物和运行环境

SHA256（测试用候选，非发布包）：

- 本机 release：`509a590606af88d5301bc8d036aa7fab8031919054fcf9a5e96700219ba053ec`
- Pro macOS：`08066161f61f3601d617457cc6dfe437b08aae78c1be6721925081283f9d756b`
- Pro Linux：`f54f78e366c359253caa3fb76b1a5cb67337f02d4831503199db4bf9b464fe2e`
- Pro Windows：`c55b88b67b193688f9b6b0ccd0b83dc6a02846bc8eae27bc8f27af9c1542a35e`

Pro 原 Wine 11 的新 prefix 在程序启动前失败。临时下载校验后的 [Wine 11.17](https://github.com/Gcenx/macOS_Wine_builds/releases/tag/11.17) 可运行；没有修改全局 Wine。Wine 压缩包 SHA256 `c2b3a8274dbc594deaa64e40469b607cbc4aa8ef5656dec4c5f6f3dac0da770c`。Windows Git 相关用例使用隔离 MinGit 2.55.0.5。Linux VM 已停止；隔离源码、候选和日志保留供追查。

完整 Pro 日志保存在本机 `/tmp/lwc-ux-pro-evidence/`，远端隔离工作目录 `/Users/muyouzhi/workspace/My/lwc-agent-ux-20260911/`。本机历史测试日志为 `/tmp/lwc-ux-*.log`。临时路径不是永久 CI 归档。

Wiki 已经审计 changeset 更新；lint 0、原词/改写检索与独立 graph verify 通过。用户既有 web/dist 删除及其他无关修改保留。未提交、推送、替换已安装工具或发布。
