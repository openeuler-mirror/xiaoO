//! 门面层：本地会话宿主、会话句柄、一轮对话与事件流。
//!
//! 【L0 门面层；本阶段为占位】
//!
//! 阶段 2 将在此模块实现以下对象，构成 xiaoo-api 对外的"API 故事"主线：
//!
//! ```text
//! LocalSessionHostBuilder ──build()──▶ LocalSessionHost（进程级资源）
//!                                           │ open_session(SessionOptions)
//!                                           ▼
//!                                        Session（会话句柄）
//!                                           │ run_turn(text) / send(text)
//!                                           ▼
//!                                        TurnHandle（事件流 + 取消 + 追加输入 + 结果）
//! ```
//!
//! 90+ 个底层符号被压缩到这条主线之后：调用方按
//! `builder().build() → open_session() → run_turn() → close() → shutdown()`
//! 的顺序走完全生命周期，每一步只有一个明显的入口。
//!
//! 设计纪律：门面不新增运行时行为——每个方法的内部实现都是对现有代码路径
//! 的调用或提炼（`refactor.md` §3.3.8 的逐方法映射表是实现对照基准）。
