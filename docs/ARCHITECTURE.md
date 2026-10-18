# 架构与安全边界

## 数据流

```text
Rust 主进程 -> codex app-server (stdio)
  ├─ account/rateLimits/read ──> 账号额度快照
  └─ account/usage/read ───────> 账号每日 Token 活动

CODEX_HOME/{sessions,archived_sessions}/**/*.{jsonl,jsonl.zst}
  └─ Rust 只读流式索引 ────────> AppData/usage-index-v1.sqlite3
                                  └─ 本机任务、模型、目录 Token 派生统计

Codex v1 派生摘要 + Claude Code / OpenCode / WorkBuddy / WorkBuddy AI 本机记录
  └─ Rust 只读多来源索引 ──────> AppData/usage-index-v2.sqlite3
                                  └─ 来源、时间、模型、脱敏项目标签和 Token 派生统计

用户手动续费时间
  └─ 类型化 Tauri command ─────> AppData/settings-v1.sqlite3

三类数据经过标准化后进入 `DashboardSnapshot`：Codex 服务端额度、账号每日活动和 `deviceUsage` 本机多来源 Token 活动。界面始终将服务端额度与本机活动分开；Credits 不参与本机 Token 相加。快照编排层只向 WebView 暴露脱敏后的稳定结构，并在返回前关闭 App Server 子进程。手动续费时间有独立来源标识，永远不会冒充官方返回值。
```

## App Server 连接

- 默认使用子进程 stdio，不开启网络监听端口。
- 每次连接先发送 `initialize`，收到成功响应后才发送 `initialized`。
- 以 `rateLimitsByLimitId` 为主，旧的 `rateLimits` 单 bucket 视图仅作回退。
- `account/rateLimits/updated` 只作为缓存失效信号；收到通知后重新读取完整快照。
- 每个请求有超时；子进程退出时拒绝所有未完成请求并清理句柄。
- 运行时记录 Codex 版本，未知字段忽略，缺失字段降级，不记录原始响应。

## Rollout 索引

- 逐行读取，不把整个日志载入内存。
- 普通日志增量扫描；Zstandard 压缩日志仅首次或文件变化时流式解压。会话与事件键不随压缩、移动或归档改变。
- 已记录历史独立保留。源暂时不可读或消失只显示提示；完整成功重读后才替换对应会话的快照，损坏压缩流不会提交部分结果。
- 数据库升级前创建含 WAL 的一致性备份，迁移不清空历史；重建仅失效扫描状态并重新读取可用源。
- 活跃文件的未完成末行延迟到下次处理。
- 使用 `total_token_usage` 相邻累计快照的差值，不直接求和 `last_token_usage`。
- 新格式使用 `subagent_history_start_ordinal` 识别子代理自有后缀。
- 旧格式使用通信元数据做保守边界；无法证明所有权的历史默认不进入总量。
- 被排除的父历史单独计入 `filteredParentEvents`，不会混入真正的解析跳过数。
- Token 差值归属到最近的 `turn_context`，目录和模型缺失时进入“未知”。
- 路径仅用于本机聚合；对外输出默认使用目录末级名或不可逆标识。

## 多来源索引

- v2 是独立 SQLite/WAL 数据库，不修改、删除或原地迁移 v1；首次扫描建立索引，后续仅重读有变化的源文件或数据库 WAL。
- Claude Code 仅读取 `~/.claude/projects/**/*.jsonl` 中带 `message.usage` 的 assistant 记录，优先以会话和 `message.id` 的不可逆键去重，缺少回复 ID 才使用日志 UUID。同一回复的流式更新采用最后一份有效用量快照，不相加。
- OpenCode 桌面版仅读取 `~/.local/share/opencode/opencode.db` 的 `part.type = step-finish` Token 记录；SQLite 以只读方式打开，能读取活动 WAL。
- WorkBuddy 与 WorkBuddy AI 分别只扫描自身用户级 `traces` 目录中标准 `trace_<32位十六进制>.json` 文件，排除工作区、会话副本和修改备份；数据库只用于会话目录末级标签与来源级 Credits 映射。
- WorkBuddy 先读取 `providerData.rawUsage`，再回退规范化 usage 字段；Credits 不计入 Token。
- Cursor 在具备官方个人数据来源前保持“暂不支持”，不读取本地密钥、账号状态或未公开接口。

## 永不采集

- ChatGPT 或 Codex access token；
- `auth.json` 内容；
- 用户提示、助手回答、工具调用正文；
- 账号邮箱、账号 ID 或原始服务端响应；
- API 平台账单数据。
- Claude、OpenCode、WorkBuddy、WorkBuddy AI 的对话正文、标题、原始 JSON、会话 ID、完整路径、账号或凭据。

## 桌面壳

- WebView 只开放快照事件监听和类型化 Tauri commands，不开放 shell、filesystem、process 或通用 SQL 权限。
- Rust 后台每 60 秒刷新一次完整快照，手动按钮和托盘菜单也可触发刷新。
- 标题栏关闭隐藏到托盘；开机启动由用户显式勾选；第二实例只唤起首实例。
- SQLite 使用 WAL、完整行提交和持久化解析检查点。未写完的 JSONL 末行留待下一轮处理。
- NSIS 采用 current-user 安装并内置 WebView2 离线安装器。
