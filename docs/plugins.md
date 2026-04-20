# Plugin Installation and Usage

## Cerberus Plugin (Optional)

Cerberus provides secure command execution with policy-based sandboxing. It is included in the workspace but requires the eBPF toolchain (Linux only).

```bash
# Install with eBPF support (default, requires nightly Rust + eBPF toolchain)
cargo install --path crates/cerberus/cerberus-cli

# Install without eBPF if toolchain is unavailable
cargo install --path crates/cerberus/cerberus-cli --no-default-features -p cerberus-core
```

If `cargo build --release` fails due to Cerberus/eBPF, you can skip it:

```bash
cargo build --release --workspace --keep-going
```

## Plugins

Pre-built hookers and skills are placed in `<your_xiaoO>/plugins`. They are **not installed by default**.

To install hookers, run:

```bash
cd <your_xiaoO>/plugins/hookers
./config.sh
```

You can also develop your own hookers and place them in `<your_xiaoO>/plugins/hookers`. See `how-to-develop-a-plugin-hooker.md` for details.

Custom skills can be installed directly into `~/.xiaoo/skills`.
