# 自建团队版

服务端独立运行，客户端继续保留本地记忆。管理界面支持中文、英文；部署配置不会展示给普通成员。

## 启动

在本目录复制 `server.container.example.jsonc` 为 `server.jsonc`，填写对外 HTTPS 地址和需要启用的登录方式。字段说明见 `server.example.jsonc`；邮件、飞书和代码托管账号由部署者配置，未配置的登录入口自动隐藏。密钥通过环境变量提供，不提交到仓库。

```sh
docker compose build
docker compose run --rm memory server init --data /data --admin-email owner@example.com
docker compose up -d
```

把 HTTPS 反向代理指向 `127.0.0.1:8787`，保留原始请求路径、查询参数和请求体，配置适合记忆快照的上传大小与超时。容器端口仅绑定宿主机回环地址。对外地址必须与配置一致；禁止直接向公网暴露未加密的 HTTP。

初始化只输出私密文件的位置。`administrator.key` 是初始管理员的个人访问密钥，可直接登录；`server-access.token` 是传统账号登录的实例接入密钥，不代表用户身份。不要把任何密钥放在网址、聊天记录或公共配置中。

浏览器打开对外地址，使用个人访问密钥登录并选择可访问的空间。管理员可在成员页开通新成员并生成一次性展示的个人密钥；也可使用邮箱邀请和已配置的账号登录。空间权限单独授予，默认不可读。个人密钥到期或撤销后，其派生登录、设备和智能体凭据一并失效。

## 客户端自动同步

个人密钥可用 `lwc login --server https://memory.example.com --key-stdin` 从标准输入读取，完成实例连接与用户登录。以下是使用实例密钥和浏览器设备确认的另一种登录方式：

```sh
lwc config server --server https://memory.example.com --token-stdin
lwc login --server https://memory.example.com
lwc space join SPACE_ID --server https://memory.example.com
lwc space configure SPACE_ID --interval-ms 2000 --automatic true
```

首条命令从标准输入读取密钥，避免命令历史保存密钥。命令行登录的设备授权码可在管理界面总览的“连接我的智能体”中确认。`join` 默认启动自动同步；`--manual` 仅用于明确需要手动同步的副本。轮询间隔范围为 250–300000 毫秒。

需要跨登录和进程退出恢复时，运行本目录的当前用户服务安装器：macOS/Linux 使用 `sh install-sync-service.sh /absolute/path/to/lwc`，Windows 使用 `install-sync-service.ps1 -Executable C:\path\lwc.exe`。委托智能体先设置 `LWC_TEAM_CREDENTIALS_FILE` 再安装；服务仅保存凭据路径，不复制密钥。原生服务管理器重启监督进程，监督进程恢复已开启自动同步的副本。

只读取云端的智能体使用 `lwc cloud --server ORIGIN --space SPACE_ID ...`，无需加入空间或创建本地记忆库。写入、压缩、恢复等操作仍由身份与空间策略分别控制。

## 备份和恢复

先停止服务，再使用 `lwc server backup --data DATA --output NEW_BACKUP_DIRECTORY`。命令拒绝仍被服务占用的数据目录，备份完整控制库、记忆、快照和私密签名材料；完成后才写入完成标记。备份目录应保存在加密磁盘或可信加密备份系统中。

恢复使用 `lwc server restore --backup BACKUP_DIRECTORY --authority-data CURRENT_DATA_DIRECTORY --output NEW_DATA_DIRECTORY`，始终写入新目录，不覆盖原数据。失败的输出目录不能投入使用。检查成功结果后，把服务配置中的 `data` 切到新目录并重新启动。容器部署需将备份和新目录挂载进容器后调用相同命令；不要把运行中的卷直接解压覆盖。

恢复从当前服务目录保留完整账号、权限、撤销记录和接入密钥，只恢复备份中的记忆；当前权限目录必须停机，且与备份属于同一签名身份。备份后新建的空间保留当前数据。恢复为所有空间生成新代次。客户端只接受原已信任签名密钥的恢复边界，保留旧基线、未上传内容和冲突材料，再执行双向合并；未声明新代次的版本倒退、密钥变化、校验失败都会停止发布并提醒智能体处理。日常撤销错误记忆应优先使用管理界面的“恢复修改”，生成可审计的补偿版本，不恢复整个服务端。

当前权限库缺失或损坏时，恢复命令拒绝继续，不能用旧授权替代当前授权。需要部署者先恢复可信的最新权限副本；客户端跨新代次也拒绝策略版本倒退。记忆补偿恢复不回退账号和权限。

## 升级

停止服务并完成备份，更新镜像或二进制及界面制品，再启动并核对健康状态和客户端同步。数据库迁移由程序执行；不要手工编辑控制库或客户端副本状态。版本不兼容时使用保留的数据目录恢复，而不是让旧二进制打开已升级的数据。

发布前须分别记录：Rust 定向回归、前端构建、容器构建、实际部署和部署者三种真实登录验收。静态配置和本地界面预览不能替代真实登录验收。
