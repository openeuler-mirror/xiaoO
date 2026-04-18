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


def ensure_hooker_fields(lines: list, hooker_idx: int) -> list:
    """确保 [hooker] 段包含所有必要字段"""
    next_section_idx = len(lines)
    for i in range(hooker_idx + 1, len(lines)):
        stripped = lines[i].strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            next_section_idx = i
            break

    existing_fields = {}
    for i in range(hooker_idx + 1, next_section_idx):
        line = lines[i].strip()
        if "=" in line and not line.startswith("#"):
            key = line.split("=")[0].strip()
            existing_fields[key] = i

    required_fields = {
        "default": None,
        "plugins": lambda: "plugins = []\n",
        "enabled": lambda: "enabled = []\n",
        "disabled": lambda: "disabled = []\n",
        "policies": lambda: "policies = {}\n",
    }

    default_val_str = None
    if "default" not in existing_fields:
        default_value = prompt_default_mode()
        default_val_str = f'default = "{default_value}"\n'

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
    first_line = lines[plugins_line_idx]
    if "[" not in first_line:
        return [], plugins_line_idx

    raw = first_line[first_line.find("[") + 1 :]
    end_line = plugins_line_idx
    while "]" not in raw:
        end_line += 1
        if end_line >= len(lines):
            break
        raw += lines[end_line]

    if "]" in raw:
        raw = raw[: raw.find("]")]
    else:
        raw = ""

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


def do_install(script_dir: str):
    """执行安装"""
    plugin_json_template_path = os.path.join(script_dir, "plugin.json.template")
    plugin_json_path = os.path.join(script_dir, "plugin.json")
    python_script_path = os.path.join(script_dir, "guard_secret_like_request.py")
    config_toml_path = os.path.expanduser("~/.config/xiaoo/config.toml")

    print(f"脚本目录: {script_dir}")

    if not os.path.exists(plugin_json_template_path):
        print("错误: plugin.json.template 不存在")
        sys.exit(1)

    with open(plugin_json_template_path, "r", encoding="utf-8") as f:
        plugins = json.load(f)

    for plugin in plugins:
        if "command" in plugin:
            plugin["command"] = f"python3 {python_script_path}"

    with open(plugin_json_path, "w", encoding="utf-8") as f:
        json.dump(plugins, f, indent=2, ensure_ascii=False)
        f.write("\n")

    print(f"已从模板生成 plugin.json: {python_script_path}")

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

    hooker_section_idx = find_hooker_section(lines)

    if hooker_section_idx is None:
        default_value = prompt_default_mode()
        lines.append("\n[hooker]\n")
        lines.append(f'default = "{default_value}"\n')
        lines.append(format_plugins_array([plugin_json_path]))
        lines.append("enabled = []\n")
        lines.append("disabled = []\n")
        lines.append("policies = {}\n")
        print("已添加完整的 [hooker] 段")
    else:
        lines = ensure_hooker_fields(lines, hooker_section_idx)

        hooker_section_idx = find_hooker_section(lines)
        next_section_idx = find_next_section(lines, hooker_section_idx)

        plugins_line_idx = None
        for i in range(hooker_section_idx + 1, next_section_idx):
            if lines[i].strip().startswith("plugins"):
                plugins_line_idx = i
                break

        content = "".join(lines)
        if plugin_json_path in content:
            print("plugin.json 路径已存在于 config.toml 中")
        elif plugins_line_idx is not None:
            existing_paths, end_line = parse_plugins_from_lines(lines, plugins_line_idx)
            new_paths = [plugin_json_path] + existing_paths
            new_plugins_block = format_plugins_array(new_paths)

            del lines[plugins_line_idx : end_line + 1]
            lines.insert(plugins_line_idx, new_plugins_block)
            print(f"已添加到 config.toml 的 plugins 列表")
        else:
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

    if os.path.exists(plugin_json_path):
        os.remove(plugin_json_path)
        print("已删除 plugin.json")
    else:
        print("plugin.json 不存在，跳过")

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

    hooker_section_idx = find_hooker_section(lines)
    if hooker_section_idx is None:
        print("config.toml 中未找到 [hooker] 段，无需清理")
        print("卸载完成!")
        return

    next_section_idx = find_next_section(lines, hooker_section_idx)

    plugins_line_idx = None
    for i in range(hooker_section_idx + 1, next_section_idx):
        if lines[i].strip().startswith("plugins"):
            plugins_line_idx = i
            break

    if plugins_line_idx is None:
        print("config.toml 中未找到 plugins 行，无需清理")
        print("卸载完成!")
        return

    existing_paths, end_line = parse_plugins_from_lines(lines, plugins_line_idx)
    remaining_paths = [p for p in existing_paths if p != plugin_json_path]

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
