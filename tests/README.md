# tests/ — 全仓库唯一测试根目录

功能源码文件不再承载测试代码，全部测试收归本目录，按 unit / system 两类分目录存放。

> **当前状态：目录骨架已建立，存量迁移待开展。** 存量测试（约 231 个源码文件
> 内嵌的 `#[cfg(test)]` 测试）将分批迁入本目录；每批迁移前后以
> `cargo test --workspace -- --list` 的清单与计数比对，保证不丢测试。

## 目录结构

| 目录 | 内容 | 接入方式 |
|---|---|---|
| `unit/` | 单元测试，按 crate 镜像 src 结构 | 源文件尾部一行 `#[cfg(test)] #[path = "…"] mod …;` 声明 |
| `system/` | 系统测试（Cargo 集成测试） | crate 内 `[[test]]` 显式 `path` 指向包外路径 |

`plugins/` 目录（含 `plugins/tests/`）不参与本约定，维持原位不动；
`cerberus/` 未入 workspace 暂缓处理，加入后按本约定同步整改。

## 命名规则

单元测试文件一律命名为 `<被测文件名>_test.rs`：

| 源文件（被测） | 测试文件 |
|---|---|
| `<pkg>/src/a/b/foo.rs` | `tests/unit/<pkg>/a/b/foo_test.rs` |
| `<pkg>/src/a/b/foo/mod.rs` | `tests/unit/<pkg>/a/b/foo_test.rs`（与 `foo/` 子目录测试平级共存） |
| `<pkg>/src/lib.rs` / `main.rs` | `tests/unit/<pkg>/lib_test.rs` / `main_test.rs` |
| `<pkg>/src/a/b/foo/tests.rs`（存量拆分文件） | 移动 + 重命名为 `tests/unit/<pkg>/a/b/foo_test.rs` |
| 拆分产生的主题测试 | `<模块>_<主题>_test.rs` |

系统测试：`tests/system/<pkg>/<场景>_test.rs`。

辅助文件（不含 `#[test]` 的桩、fixture 构造等）保留原名，不受 `_test.rs` 规则约束。

## 源文件中的最小残留

功能文件尾部仅保留一行模块声明（测试模块须位于被测模块内才能访问私有项）：

```rust
// crates/core/src/agent_loop.rs 文件尾
#[cfg(test)]
#[path = "../../../tests/unit/core/agent_loop_test.rs"]
mod tests;
```

- 迁出文件中 `use super::*` 语义不变（模块树位置未动，仅文件挪位），私有项访问保留；
- 每个源文件至多一个此类声明（杜绝多测试模块回潮，由门禁强制）；
- 多测试模块的源文件须先按职责拆分功能代码，测试跟随被测代码走。

## 系统测试接线

```toml
# crates/mcp/Cargo.toml
[[test]]
name = "mcp_streamable_http"
path = "../../tests/system/mcp/streamable_http_test.rs"
```

`cargo test --workspace` 自动发现；fixture（如 mock server 脚本）放
`tests/system/<pkg>/fixtures/`。

## 运行方式

```bash
bash tests/run.sh                 # 全量（= cargo test --workspace）
bash tests/run.sh -p xiaoo-core   # 参数原样透传给 cargo test，聚焦单 crate
cargo test --workspace -- --list  # 仅列出全部测试（基线核对用）
```

## 门禁

- `scripts/check-tests-hygiene.sh`：测试卫生门禁——src 中禁止 `#[test]`、
  至多一行 `#[cfg(test)] #[path]` 声明（白名单 seam 除外）、`tests/` 下含
  `#[test]` 的文件必须以 `_test.rs` 结尾。扫描范围由 workspace members 动态推导，
  `plugins/` 天然排除。暂未接入 `scripts/ci.sh`，存量迁移完成后启用。
