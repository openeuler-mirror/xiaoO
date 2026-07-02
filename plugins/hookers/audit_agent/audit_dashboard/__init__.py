"""xiaoO Audit Dashboard — 安全策略可视化管理平台

基于 FastAPI 的轻量 Web 服务，提供：
- 层级开关管理（L1/L2/L3）
- 规则分类管理（启用/禁用整类或单条规则）
- Skill 管理（启用/禁用/新增/删除）
- 审计统计与诊断信息

所有配置操作写入 ~/.config/xiaoo/audit_runtime.json，
audit-agent 每次调用自动读取最新配置 → 立即热生效。
"""
