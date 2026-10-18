# 多来源 Token 索引（v0.2.0）

## 目标与范围

本功能把本机可验证的 AI Token 活动汇总到一个独立看板，但不把它伪装成服务端套餐额度。当前接入顺序为 Claude Code、OpenCode 桌面版、WorkBuddy、WorkBuddy AI；Codex 保留原有 v1 增量索引。Cursor 显示等待官方个人数据来源。

所有读取均为本机只读：不联网、不登录、不扫描工作区产物，也不改动任何 AI 应用的原始数据。

## 数据源与稳定键

| 来源 | 只读数据 | 稳定去重键 | 说明 |
| --- | --- | --- | --- |
| Codex | `CODEX_HOME/sessions` 与 `archived_sessions` 的 `.jsonl`、`.jsonl.zst` | 会话 UUID、事件序号与子代理边界 | 普通、压缩和归档副本只统计一次 |
| Claude Code | `~/.claude/projects/**/*.jsonl` | 会话 + `message.id` 的不可逆哈希，缺失时回退日志 UUID | 同一回复的多次流式快照仅保留最后一份有效用量 |
| OpenCode 桌面版 | `~/.local/share/opencode/opencode.db` 与 WAL | `part.id` 的不可逆哈希 | 仅 `step-finish` Token 行 |
| WorkBuddy | `~/.workbuddy/traces` | trace 稳定键的不可逆哈希 | 仅标准 trace 文件 |
| WorkBuddy AI | `~/.workbuddy-ai/traces` | trace 稳定键的不可逆哈希 | 不扫描 `C:\Users\<你的用户名>\WorkBuddy AI` 工作区 |
| Cursor | 无 | 无 | 等待官方个人数据来源 |

## 索引内容与隐私

`usage-index-v2.sqlite3` 仅保存来源、不可逆键、时间、模型、脱敏项目标签和 Token 派生数字（新输入、缓存读取、缓存写入、输出、推理输出、总 Token）。它不保存对话文字、标题、完整路径、原始 JSON、会话 ID、账号或凭据。

源文件完整成功重读后可纠正对应记录；源文件删除、移动或暂时无法读取时，已记录历史继续保留并显示提示。相同回复出现在多个 Claude JSONL 副本中时只统计一次，旧副本不能覆盖更新的快照。升级前备份原数据库并保留历史；无法核实的新旧去重关系会明确提示，不擅自抹去旧记录。

重建索引会先创建 SQLite 一致性备份，再重新读取可用源，不清空历史。费用继续根据当前保存的价格设置重算，是本机估算值。

## 状态和失败隔离

每个来源独立显示：已就绪、未检测到、暂不支持或读取失败。一个来源失败不会阻断其他来源、本机 v2 索引，或 Codex 服务端额度区。来源级 Credits 与 Token 是不同单位，绝不相加。
