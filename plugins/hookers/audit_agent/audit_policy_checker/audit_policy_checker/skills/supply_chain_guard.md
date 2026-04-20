# supply_chain_guard — 供应链安全检测

## 适用场景
当待执行动作涉及安装依赖包、导入库、拉取镜像时，使用本规则判断是否存在供应链攻击风险。

## 检测模式

### Critical（必须 Deny）
- Typosquatting 包名：安装与知名包名相似的恶意包（如 numpy1, reqeusts）
- 恶意安装钩子：package.json 中的 preinstall/postinstall 执行恶意命令
- 已知恶意包：安装安全数据库中标记为恶意的包
- 不可信的包仓库：从非官方仓库安装包

### High（强烈建议 Deny）
- 安装版本过旧且已知漏洞的包
- 安装依赖链过深的包（间接依赖可能包含恶意代码）
- pip install 从 Git URL 安装（绕过 PyPI 安全检查）
- npm install 从非 npmjs.com 的 URL 安装
- Docker pull 不可信镜像

### Medium（需谨慎评估）
- 安装来自官方仓库的包（需检查版本和已知漏洞）
- 安装开发依赖（需检查是否为生产环境）
- 包版本锁定文件修改（需检查变更内容）

## 决策标准
1. 包名是否为 typosquatting？→ Critical → Deny
2. 安装脚本是否执行可疑命令？→ Critical → Deny
3. 包是否在已知恶意包列表中？→ Critical → Deny
4. 包来源是否为官方仓库？→ 非官方 → High → Deny
5. 安装操作是否与 prompt 意图一致？→ 结合意图判断
