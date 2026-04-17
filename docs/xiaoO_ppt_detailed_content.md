# xiaoO 项目演示文稿 - 详细内容

## 第1页：封面
- **标题：** xiaoO
- **副标题：** AgentOS的智能中枢
- **英文副标题：** Open-source Intelligence Hub of AgentOS
- **Slogan：** The butler that is the house.
- **版本信息：** v0.0.1
- **开源协议：** MulanPSL-2.0
- **开发语言：** Rust
- **项目Logo：** 建议使用 img/logo.jpeg

---

## 第2页：项目定位

### 什么是xiaoO？

**xiaoO是AgentOS的智能中枢**

让整个操作系统成为Agent的家——每一个资源、每一项服务、每一种能力，都在一个屋檐下精心策展和服务。

### 三大核心能力

1. **自治系统管理** (Self-governing System Management)
   - 智能化的系统资源调度
   - 自动化的运维决策
   
2. **无缝Agent编排** (Seamless Agent Orchestration)
   - 统一的Agent生命周期管理
   - 多Agent协作与通信
   
3. **开箱即用的智能能力** (Ready-to-use Smart Capabilities)
   - 跨用户渠道的统一接入
   - 丰富的预置Agent角色

---

## 第3页：核心价值

### 为什么选择xiaoO？

#### 🏠 让Agent拥有整个OS
- 不再是Agent在系统中运行
- 而是整个系统成为Agent的家园
- 所有资源触手可及

#### 🎯 统一的智能入口
- 所有资源、服务、能力集中管理
- 一个中枢，多种渠道（CLI、TUI、HTTP、飞书等）
- 统一的用户体验

#### ⚡ 高性能Rust实现
- 内存安全：编译时检查，无数据竞争
- 并发高效：媲美C/C++的性能
- 模块化架构：易于扩展和维护

---

## 第4页：技术架构

### 架构层次

```
┌─────────────────────────────────────────┐
│         用户接入层（Channels）            │
│  CLI  │  TUI  │  HTTP  │  飞书  │ 更多   │
└─────────────────────────────────────────┘
                   ↓
┌─────────────────────────────────────────┐
│           xiaoO Core（核心层）            │
│  Agent管理 │ 会话管理 │ 工具调用           │
└─────────────────────────────────────────┘
                   ↓
┌─────────────────────────────────────────┐
│          LLM客户端层（LLM Clients）       │
│  OpenAI │ Anthropic │ Ollama │ 更多      │
└─────────────────────────────────────────┘
```

**设计理念：**
- 分层清晰，职责明确
- 模块化设计，易于扩展
- 插件式架构，灵活组合

---

## 第5页：核心模块

### 关键组件一览

| 模块名称 | 功能说明 | 技术特点 |
|---------|---------|---------|
| **core** | 核心运行时 | Agent生命周期管理 |
| **agent-llm** | Agent与LLM交互 | 多提供商适配 |
| **memory** | 会话记忆管理 | 长短期记忆结合 |
| **tool** | 工具执行框架 | 安全沙箱机制 |
| **skill** | 技能系统 | 可插拔扩展 |
| **subagent** | 子Agent编排 | 任务分解与协作 |
| **trace** | 分布式追踪 | Moirai可观测性 |

---

## 第6页：快速开始

### 安装步骤

**前置条件：**
- Cargo >= 1.7
- Rust 1.70+

**安装命令：**
```bash
# 克隆仓库
git clone https://gitcode.com/openeuler/xiaoO.git
cd xiaoO

# 构建
cargo build --release

# 安装
cargo install --path apps/xiaoo-app
```

安装到 `~/.cargo/bin/xiaoo`，确保 `~/.cargo/bin` 在 PATH 中。

---

## 第7页：配置说明

### 配置文件示例

创建配置文件 `~/.config/xiaoo/config.toml`：

```toml
[llm]
provider = "openrouter"
model = "z-ai/glm-5"
api_key_env = "OPENROUTER_API_KEY"
max_tokens = 4096
context_window = 128000

[agent.code-reviewer]
description = "Reviews code for best practices"
prompt = "You are a code reviewer. Focus on security, performance, and maintainability."

[agent.code-reviewer.tools]
file_write = false
file_edit = false
```

**环境变量：**
```bash
export OPENROUTER_API_KEY="sk-or-..."
```

---

## 第8页：使用方式

### 三种交互模式

#### 1. TUI模式（终端界面）
```bash
xiaoo-tui
```
- 交互式界面
- Tab/Shift+Tab切换Agent角色
- 实时对话

#### 2. CLI模式（命令行）
```bash
xiaoo run -p "Your Command"
```
- 单次任务执行
- 快速测试
- 脚本集成

#### 3. Daemon模式（后台服务）
```bash
xiaoo-app daemon --port 8080
```
- RESTful API
- 支持外部系统集成
- 飞书Webhook接入

---

## 第9页：Agent角色系统

### 可扩展的Agent预设

**代码审查Agent示例：**
```toml
[agent.code-reviewer]
description = "Reviews code for best practices and potential issues"
prompt = "You are a code reviewer. Focus on security, performance, and maintainability."

[agent.code-reviewer.tools]
file_write = false
file_edit = false
```

**HTTP调用方式：**
```json
{
  "text": "Review this patch for security issues",
  "channel": "http",
  "sender_id": "demo-user",
  "conversation_id": "demo-conv",
  "agent": "code-reviewer"
}
```

---

## 第10页：核心特性

### 技术亮点

#### ✅ 多LLM提供商支持
- **商业服务：** OpenAI、Anthropic、DeepSeek、智谱AI
- **本地部署：** Ollama
- **聚合服务：** OpenRouter
- **灵活配置：** 支持自定义端点

#### ✅ 灵活的工具系统
- **文件操作：** 读写、编辑
- **Shell执行：** 安全沙箱
- **插件机制：** 可扩展

#### ✅ 智能记忆管理
- **上下文管理：** 自动窗口控制
- **会话压缩：** 智能摘要
- **持久化存储：** 长期记忆

#### ✅ 可观测性
- **分布式追踪：** Moirai框架
- **性能监控：** 实时指标
- **调试日志：** 完整链路

---

## 第11页：应用场景

### 典型应用

#### 💼 开发辅助
- 代码审查与优化建议
- 自动化测试生成
- 技术文档编写
- 代码重构建议

#### 🔧 系统运维
- 日志智能分析
- 故障根因诊断
- 自动化脚本生成
- 性能瓶颈识别

#### 📊 数据处理
- 数据清洗与转换
- 自动化报表生成
- 数据可视化建议
- 异常数据检测

#### 💬 智能客服
- 多渠道统一接入
- 知识库智能问答
- 工单自动处理
- 情感分析与路由

---

## 第12页：技术优势

### 为什么选择Rust？

#### 🛡️ 内存安全
- **编译时检查**：无运行时开销
- **无数据竞争**：所有权系统保证
- **零成本抽象**：性能不妥协

#### ⚡ 高性能
- **并发效率**：媲美C/C++
- **资源占用低**：内存效率高
- **启动速度快**：无GC停顿

#### 📦 现代工具链
- **Cargo**：强大的包管理
- **类型系统**：编译期错误检测
- **测试框架**：内置测试支持

#### 🔧 易于集成
- **FFI**：与C/C++互操作
- **WebAssembly**：跨平台支持
- **嵌入式**：资源受限环境

---

## 第13页：开发路线

### 项目规划

#### v0.1.0（当前版本）
- ✅ 核心Agent运行时
- ✅ 多LLM客户端支持
- ✅ 基础工具系统
- ✅ CLI/TUI界面
- ✅ HTTP API
- ✅ 飞书集成

#### v0.2.0（下一版本）
- 🔄 更多Agent角色预设
- 🔄 Web管理界面
- 🔄 插件市场
- 🔄 性能优化
- 🔄 文档完善

#### v1.0.0（长期愿景）
- 📋 企业级特性
- 📋 集群部署支持
- 📋 可视化编排
- 📋 多语言SDK
- 📋 社区生态建设

---

## 第14页：开源生态

### 开源贡献

**开源协议：** MulanPSL-2.0  
**代码仓库：** https://gitcode.com/openeuler/xiaoO

#### 欢迎参与
- 🌟 **Star支持** - 关注项目进展
- 🐛 **Issue反馈** - 报告问题和建议
- 💡 **功能建议** - 提出新功能需求
- 🔧 **代码贡献** - 参与功能开发

#### 贡献流程
1. Fork项目仓库
2. 创建特性分支
3. 编写代码和测试
4. 提交Pull Request
5. 参与代码评审

---

## 第15页：文档与支持

### 技术文档

**项目主页：** https://gitcode.com/openeuler/xiaoO

**核心文档：**
- `README.md` - 项目介绍和快速开始
- `docs/daemon_config.md` - Daemon模式配置
- `docs/feishu_deploy.md` - 飞书集成部署
- `docs/skill_usage.md` - 技能系统使用

### 问题反馈
- GitCode Issues
- 邮件列表
- 社区论坛

---

## 第16页：总结

### xiaoO - AgentOS的智能中枢

#### 核心优势
✅ **开箱即用** - 预置丰富Agent角色和能力  
✅ **无缝编排** - 统一的Agent生命周期管理  
✅ **多渠道接入** - CLI/TUI/HTTP/飞书全支持  
✅ **高性能** - Rust实现，安全高效  

#### 适用场景
🎯 开发辅助 · 系统运维 · 数据处理 · 智能客服

#### 立即开始
```bash
cargo install --path apps/xiaoo-app
xiaoo-tui
```

---

## 第17页：感谢页

### Thank You!

**让智能触手可及**

xiaoO团队  
2026

---

# 附录：设计建议

## 视觉风格
- **主色调：** 蓝色渐变（#667eea → #764ba2）
- **辅助色：** 橙色（#ff6b6b）- Rust特色
- **背景：** 纯白或浅灰
- **字体：** 微软雅黑/思源黑体（标题），苹方/思源宋体（正文）

## 配图建议
- 架构图：使用流程图或数据流图
- 场景图：使用扁平化插画风格
- 技术图：使用柱状图对比性能

## 动画效果
- 淡入淡出：0.5秒
- 列表逐条：点击或自动
- 避免过度动效，保持专业
