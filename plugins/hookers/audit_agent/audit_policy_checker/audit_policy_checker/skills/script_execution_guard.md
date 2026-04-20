# script_execution_guard — 命令/脚本执行安全检测

## 适用场景
当待执行动作涉及命令执行、脚本运行、Shell 操作时，使用本规则判断是否存在安全风险。

## 检测模式

### Critical（必须 Deny）
- 破坏性操作：rm -rf /, rm -rf ~, mkfs, dd if=/dev/zero
- 提权操作：sudo, su, chmod 777, chown root, pkexec
- 远程代码执行：curl ... | bash, wget ... | sh, eval "$(curl ...)"
- 反弹 Shell：bash -i >& /dev/tcp/, nc -e /bin/bash, python -c 'import socket...'
- 内核模块操作：insmod, rmmod, modprobe
- 修改系统二进制：覆盖 /usr/bin/*, /bin/*, /sbin/*

### High（强烈建议 Deny）
- 批量操作：find / -delete, rm -rf /var/log/*
- 隐藏命令执行：base64 -d | sh, xxd -r | bash, echo <hex> | sh
- 修改文件权限：chmod 666 /etc/passwd, chmod +s /bin/bash
- 修改定时任务：crontab -r, echo "..." >> /etc/crontab
- 进程信号发送：kill -9 1, killall systemd
- 禁用安全模块：setenforce 0, systemctl stop firewall

### Medium（需谨慎评估）
- 普通软件安装：pip install, npm install（需检查包来源）
- 环境变量修改：export PATH=..., LD_PRELOAD=...
- 重定向输出：command > file（需检查目标路径）
- 管道操作：command1 | command2（需检查最终效果）

## 决策标准
1. 命令是否具有破坏性（不可逆）？→ Critical → Deny
2. 命令是否涉及提权？→ Critical → Deny
3. 命令是否从远程下载并执行？→ Critical → Deny
4. 命令是否尝试建立反弹 Shell？→ Critical → Deny
5. 命令是否批量操作大量文件？→ High → Deny
6. 命令是否试图隐藏执行痕迹？→ High → Deny
7. 命令执行是否与 prompt 意图一致？→ 结合意图判断
