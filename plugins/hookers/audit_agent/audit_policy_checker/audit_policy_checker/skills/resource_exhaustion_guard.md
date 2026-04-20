# resource_exhaustion_guard — 资源耗尽安全检测

## 适用场景
当待执行动作可能导致 CPU、内存、磁盘、进程等资源耗尽时，使用本规则判断是否存在资源耗尽风险。

## 检测模式

### Critical（必须 Deny）
- Fork 炸弹：:(){ :|:& };:, bash -c 'bash & bash &'
- 磁盘填满：dd if=/dev/zero of=/tmp/hugefile, yes > /dev/null
- 内存耗尽：python -c 'x="A"*999999999', malloc(999999999)
- 删除所有文件：rm -rf /, find / -delete
- 无限循环：while true; do ...; done, for(;;) {}

### High（强烈建议 Deny）
- 批量创建文件：for i in $(seq 1 100000); do touch /tmp/file_$i; done
- 大文件下载：wget http://large-file.iso
- 大量进程创建：并行启动大量子进程
- 日志注入：向日志文件写入大量数据

### Medium（需谨慎评估）
- 正常的计算密集型操作（需评估资源需求是否合理）
- 正常的文件操作（需评估文件大小）
- 正常的并发操作（需评估并发数量）

## 决策标准
1. 操作是否具有 Fork 炸弹特征？→ Critical → Deny
2. 操作是否尝试填满磁盘？→ Critical → Deny
3. 操作是否尝试耗尽内存？→ Critical → Deny
4. 操作包含无限循环？→ Critical → Deny
5. 操作是否批量创建大量资源？→ High → Deny
6. 操作是否与 prompt 意图一致？→ 结合意图判断
