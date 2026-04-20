# skill_installation_guard — Skill/插件安装安全检测

## 适用场景
当待执行动作涉及安装新 Skill、插件、扩展时，使用本规则判断是否存在安全风险。

## 检测模式

### Critical（必须 Deny）
- 安装扫描报告为高危风险的 Skill
- 安装包含恶意代码的 Skill（扫描检测到后门/反弹 Shell）
- 安装来源不可信的 Skill（非官方市场/仓库）
- Skill 安装脚本执行危险操作（修改系统文件、建立网络连接）

### High（强烈建议 Deny）
- 安装扫描失败的 Skill（无法验证安全性）
- 安装包含混淆代码的 Skill
- 安装请求过多权限的 Skill
- 安装未经验证来源的第三方依赖

### Medium（需谨慎评估）
- 安装来自官方市场的 Skill（需检查版本和评分）
- 安装包含外部依赖的 Skill（需检查依赖安全性）
- 更新已安装的 Skill（需检查更新内容）

## 决策标准
1. Skill 是否通过安全扫描？→ 扫描失败/高危 → Critical → Deny
2. Skill 来源是否可信？→ 不可信 → Critical → Deny
3. Skill 是否请求超出需要的权限？→ High → Deny
4. Skill 是否包含混淆代码？→ High → Deny
5. Skill 安装是否与 prompt 意图一致？→ 结合意图判断
