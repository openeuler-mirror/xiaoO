#!/usr/bin/env python3
"""
config.py - 配置插件脚本

用法:
    python3 config.py install   - 安装插件
    python3 config.py uninstall - 卸载插件

功能:
    install:
        1. 从 plugin.json.template 生成 plugin.json（填充绝对路径）
        2. 将当前目录的 plugin.json 路径添加到 ~/.config/xiaoo/config.toml
        3. 检查并补全 [hooker] 段的必要字段

    uninstall:
        1. 从 config.toml 的 plugins 列表中移除本插件路径
        2. 删除生成的 plugin.json
"""

import argparse
import json
import os
import re
import sys


def prompt_default_mode() -> str:
    """询问用户默认模式"""
    print("\n[hooker] 段缺少 default 字段。")
    print("default 字段决定插件的默认行为：")
    print("  - All: 默认允许所有工具调用，仅对 plugins 列表中的插件进行拦截")
    print("  - None: 默认拦截所有工具调用，仅对 plugins 列表中的插件放行")
    print()
    while True:
        choice = input("请选择 default 模式 (All/None) [默认: All]: ").strip()
        if not choice:
            return "All"
        if choice in ("All", "None"):
            return choice
        print("无效输入，请输入 All 或 None")


def prompt_alert_log_path() -> str:
    """询问用户告警日志路径"""
    print("\n请配置告警日志路径（用于记录 Cerberus 沙箱拦截事件）。")
    default_path = "/tmp/cerberus_alert.log"
    while True:
        choice = input(f"告警日志路径 [默认: {default_path}]: ").strip()
        if not choice:
            return default_path
        return choice


def ensure_hooker_fields(lines: list, hooker_idx: int) -> list:
    """确保 [hooker] 段包含所有必要字段"""
    # 找到 [hooker] 段的结束位置（下一个 [section] 或文件末尾）
    next_section_idx = len(lines)
    for i in range(hooker_idx + 1, len(lines)):
        stripped = lines[i].strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            next_section_idx = i
            break

    # 解析 [hooker] 段已有的字段
    existing_fields = {}
    for i in range(hooker_idx + 1, next_section_idx):
        line = lines[i].strip()
        if "=" in line and not line.startswith("#"):
            key = line.split("=")[0].strip()
            existing_fields[key] = i

    # 需要的字段及其默认值生成函数
    required_fields = {
        "default": None,  # 需要询问用户
        "plugins": lambda: "plugins = []\n",
        "enabled": lambda: "enabled = []\n",
        "disabled": lambda: "disabled = []\n",
        "policies": lambda: "policies = {}\n",
    }

    # 检查 default 字段
    default_val_str = None
    if "default" not in existing_fields:
        default_value = prompt_default_mode()
        default_val_str = f'default = "{default_value}"\n'

    # 确定插入位置
    insert_idx = next_section_idx
    new_lines = []

    if default_val_str:
        new_lines.append(default_val_str)
        print("已添加字段: default")

    for field, gen in required_fields.items():
        if field == "default":
            continue
        if field not in existing_fields:
            new_lines.append(gen())
            print(f"已添加字段: {field}")

    if new_lines:
        # 在 next_section_idx 之前插入新字段
        for i, new_line in enumerate(new_lines):
            lines.insert(insert_idx + i, new_line)

    return lines


def find_hooker_section(lines: list) -> int | None:
    """查找 [hooker] 段的索引"""
    for i, line in enumerate(lines):
        if line.strip() == "[hooker]":
            return i
    return None


def find_next_section(lines: list, start_idx: int) -> int:
    """找到下一个 [section] 的索引"""
    for i in range(start_idx + 1, len(lines)):
        stripped = lines[i].strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            return i
    return len(lines)


def parse_plugins_from_lines(lines: list, plugins_line_idx: int) -> tuple[list[str], int]:
    """从 lines 中解析 plugins 数组，返回 (路径列表, 数组结束行索引)"""
    # 找到 [ 所在行
    first_line = lines[plugins_line_idx]
    bracket_line = plugins_line_idx
    if "[" not in first_line:
        return [], plugins_line_idx

    # 收集从 [ 到 ] 的所有内容
    raw = first_line[first_line.find("[") + 1 :]
    end_line = plugins_line_idx
    while "]" not in raw:
        end_line += 1
        if end_line >= len(lines):
            break
        raw += lines[end_line]

    # 提取 ] 之前的内容
    if "]" in raw:
        raw = raw[: raw.find("]")]
    else:
        raw = ""

    # 解析字符串
    strings = re.findall(r'["\']([^"\']*)["\']', raw)
    return strings, end_line


def format_plugins_array(paths: list[str]) -> str:
    """格式化 plugins 数组为多行 TOML 格式"""
    if not paths:
        return "plugins = []\n"
    lines = "plugins = [\n"
    for p in paths:
        lines += f'  "{p}",\n'
    lines += "]\n"
    return lines


def check_cerberus_installed() -> bool:
    """检查 cerberus 是否已安装"""
    # 检查 ~/.cargo/bin/cerberus
    cerberus_path = os.path.expanduser("~/.cargo/bin/cerberus")
    if os.path.exists(cerberus_path):
        return True

    # 检查 PATH 中是否有 cerberus
    import subprocess
    try:
        subprocess.run(
            ["cerberus", "--version"],
            capture_output=True,
            check=True,
        )
        return True
    except (subprocess.CalledProcessError, FileNotFoundError):
        return False


def do_install(script_dir: str):
    """执行安装"""
    plugin_json_template_path = os.path.join(script_dir, "plugin.json.template")
    plugin_json_path = os.path.join(script_dir, "plugin.json")
    wrap_script_path = os.path.join(script_dir, "wrap_bash_with_cerberus.py")
    alert_script_path = os.path.join(script_dir, "alert_on_cerberus_permission.py")
    policy_file_path = os.path.join(script_dir, "policy.toml")
    config_toml_path = os.path.expanduser("~/.config/xiaoo/config.toml")

    print(f"脚本目录: {script_dir}")

    # 检查 cerberus 是否已安装
    if not check_cerberus_installed():
        print("错误: cerberus 未安装")
        print("请先安装 cerberus:")
        print("  cargo install --path crates/cerberus/cerberus-cli")
        print("或参考 README.md 中的安装说明")
        sys.exit(1)

    print("cerberus 已安装")

    # 检查必要文件
    if not os.path.exists(plugin_json_template_path):
        print("错误: plugin.json.template 不存在")
        sys.exit(1)
    if not os.path.exists(wrap_script_path):
        print("错误: wrap_bash_with_cerberus.py 不存在")
        sys.exit(1)
    if not os.path.exists(alert_script_path):
        print("错误: alert_on_cerberus_permission.py 不存在")
        sys.exit(1)
    if not os.path.exists(policy_file_path):
        print("错误: policy.toml 不存在")
        sys.exit(1)

    # 询问告警日志路径
    alert_log_path = prompt_alert_log_path()

    # 从 template 生成 plugin.json
    with open(plugin_json_template_path, "r", encoding="utf-8") as f:
        content = f.read()

    # 替换占位符
    content = content.replace("<wrap_bash_with_cerberus_script.py>", wrap_script_path)
    content = content.replace("<alert_on_cerberus_permission_script.py>", alert_script_path)
    content = content.replace("<policy_file_path>", policy_file_path)
    content = content.replace("<alert_log_path>", alert_log_path)

    with open(plugin_json_path, "w", encoding="utf-8") as f:
        f.write(content)

    print(f"已从模板生成 plugin.json")
    print(f"  - wrap_bash_with_cerberus.py: {wrap_script_path}")
    print(f"  - alert_on_cerberus_permission.py: {alert_script_path}")
    print(f"  - policy.toml: {policy_file_path}")
    print(f"  - alert_log_path: {alert_log_path}")

    # 读取并更新 config.toml
    config_dir = os.path.dirname(config_toml_path)
    os.makedirs(config_dir, exist_ok=True)

    if os.path.exists(config_toml_path):
        with open(config_toml_path, "r", encoding="utf-8") as f:
            content = f.read()
    else:
        content = ""

    lines = content.splitlines(keepends=True)
    if not lines or (lines and not lines[-1].endswith("\n")):
        if lines:
            lines[-1] = lines[-1].rstrip("\n") + "\n"
        else:
            lines = []

    # 查找 [hooker] 段
    hooker_section_idx = find_hooker_section(lines)

    if hooker_section_idx is None:
        # 没有 [hooker] 段，询问 default 模式后创建完整段
        default_value = prompt_default_mode()
        lines.append("\n[hooker]\n")
        lines.append(f'default = "{default_value}"\n')
        lines.append(format_plugins_array([plugin_json_path]))
        lines.append("enabled = []\n")
        lines.append("disabled = []\n")
        lines.append("policies = {}\n")
        print("已添加完整的 [hooker] 段")
    else:
        # 检查并补全 [hooker] 段的字段
        lines = ensure_hooker_fields(lines, hooker_section_idx)

        # 重新查找 [hooker] 段（可能已被修改）
        hooker_section_idx = find_hooker_section(lines)
        next_section_idx = find_next_section(lines, hooker_section_idx)

        plugins_line_idx = None
        for i in range(hooker_section_idx + 1, next_section_idx):
            if lines[i].strip().startswith("plugins"):
                plugins_line_idx = i
                break

        # 检查路径是否已存在
        content = "".join(lines)
        if plugin_json_path in content:
            print("plugin.json 路径已存在于 config.toml 中")
        elif plugins_line_idx is not None:
            # plugins 行存在，解析并添加路径
            existing_paths, end_line = parse_plugins_from_lines(lines, plugins_line_idx)
            new_paths = [plugin_json_path] + existing_paths
            new_plugins_block = format_plugins_array(new_paths)

            # 替换原来的 plugins 定义（可能是多行）
            del lines[plugins_line_idx : end_line + 1]
            # 插入新的多行格式
            lines.insert(plugins_line_idx, new_plugins_block)
            print(f"已添加到 config.toml 的 plugins 列表")
        else:
            # plugins 行不存在，在 [hooker] 后插入
            insert_idx = hooker_section_idx + 1
            lines.insert(insert_idx, format_plugins_array([plugin_json_path]))
            print(f"已在 [hooker] 段下添加 plugins 行")

    with open(config_toml_path, "w", encoding="utf-8") as f:
        f.writelines(lines)

    print("安装完成!")


def do_uninstall(script_dir: str):
    """执行卸载"""
    plugin_json_path = os.path.join(script_dir, "plugin.json")
    config_toml_path = os.path.expanduser("~/.config/xiaoo/config.toml")

    print(f"脚本目录: {script_dir}")

    # 1. 删除生成的 plugin.json
    if os.path.exists(plugin_json_path):
        os.remove(plugin_json_path)
        print("已删除 plugin.json")
    else:
        print("plugin.json 不存在，跳过")

    # 2. 从 config.toml 中移除插件路径
    if not os.path.exists(config_toml_path):
        print("config.toml 不存在，无需清理")
        print("卸载完成!")
        return

    with open(config_toml_path, "r", encoding="utf-8") as f:
        content = f.read()

    if plugin_json_path not in content:
        print("config.toml 中未找到本插件路径，无需清理")
        print("卸载完成!")
        return

    lines = content.splitlines(keepends=True)

    # 查找 [hooker] 段
    hooker_section_idx = find_hooker_section(lines)
    if hooker_section_idx is None:
        print("config.toml 中未找到 [hooker] 段，无需清理")
        print("卸载完成!")
        return

    next_section_idx = find_next_section(lines, hooker_section_idx)

    # 查找 plugins 行
    plugins_line_idx = None
    for i in range(hooker_section_idx + 1, next_section_idx):
        if lines[i].strip().startswith("plugins"):
            plugins_line_idx = i
            break

    if plugins_line_idx is None:
        print("config.toml 中未找到 plugins 行，无需清理")
        print("卸载完成!")
        return

    # 从 plugins 数组中移除路径
    existing_paths, end_line = parse_plugins_from_lines(lines, plugins_line_idx)
    remaining_paths = [p for p in existing_paths if p != plugin_json_path]

    # 替换原来的 plugins 定义
    del lines[plugins_line_idx : end_line + 1]
    lines.insert(plugins_line_idx, format_plugins_array(remaining_paths))

    print("已从 config.toml 的 plugins 列表中移除本插件")

    with open(config_toml_path, "w", encoding="utf-8") as f:
        f.writelines(lines)

    print("卸载完成!")


def main():
    parser = argparse.ArgumentParser(
        description="配置插件脚本",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
示例:
    python3 config.py install   - 安装插件
    python3 config.py uninstall - 卸载插件
""",
    )
    subparsers = parser.add_subparsers(dest="command", required=True, help="命令")

    subparsers.add_parser("install", help="安装插件")
    subparsers.add_parser("uninstall", help="卸载插件")

    args = parser.parse_args()

    script_dir = os.path.dirname(os.path.abspath(__file__))

    if args.command == "install":
        do_install(script_dir)
    elif args.command == "uninstall":
        do_uninstall(script_dir)


if __name__ == "__main__":
    main()
