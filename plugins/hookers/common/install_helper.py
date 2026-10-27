#!/usr/bin/env python3
"""
install_helper.py - plugins/hookers 各插件 config.py 的公共安装逻辑

维护 ~/.config/xiaoo/config.toml 的 [hooker] 段：解析既有字段、补全缺失
字段、把插件 plugin.json 路径注册进/移出 plugins 列表。各插件的
config.py 只保留自己的 plugin.json 生成逻辑，注册/注销统一走本模块：

    sys.path.insert(0, <plugins/hookers>/common)
    import install_helper
    install_helper.register_plugin_in_config(plugin_json_path)
    install_helper.unregister_plugin_from_config(plugin_json_path)
"""

import os
import re


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

    # 需要的字段及其默认值生成函数
    required_fields = {
        "default": None,  # 需要询问用户
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


def _read_config_lines(config_toml_path: str) -> list:
    """读取 config.toml 为行列表（保证末行换行）；文件不存在时返回空列表。"""
    config_dir = os.path.dirname(config_toml_path)
    os.makedirs(config_dir, exist_ok=True)

    if os.path.exists(config_toml_path):
        with open(config_toml_path, "r", encoding="utf-8") as f:
            content = f.read()
    else:
        content = ""

    lines = content.splitlines(keepends=True)
    if lines and not lines[-1].endswith("\n"):
        lines[-1] = lines[-1].rstrip("\n") + "\n"
    return lines


def _write_config_lines(config_toml_path: str, lines: list) -> None:
    with open(config_toml_path, "w", encoding="utf-8") as f:
        f.writelines(lines)


def register_plugin_in_config(plugin_json_path: str) -> None:
    """把 plugin.json 路径注册进 ~/.config/xiaoo/config.toml 的 [hooker] 段。

    没有段落时询问 default 模式后创建完整段；已有段落时补全缺失字段，
    再把路径加入 plugins 列表（已存在则跳过）。
    """
    config_toml_path = os.path.expanduser("~/.config/xiaoo/config.toml")

    lines = _read_config_lines(config_toml_path)

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
            print("已添加到 config.toml 的 plugins 列表")
        else:
            # plugins 行不存在，在 [hooker] 后插入
            insert_idx = hooker_section_idx + 1
            lines.insert(insert_idx, format_plugins_array([plugin_json_path]))
            print("已在 [hooker] 段下添加 plugins 行")

    _write_config_lines(config_toml_path, lines)


def unregister_plugin_from_config(plugin_json_path: str) -> None:
    """从 ~/.config/xiaoo/config.toml 的 plugins 列表中移除插件路径。"""
    config_toml_path = os.path.expanduser("~/.config/xiaoo/config.toml")

    if not os.path.exists(config_toml_path):
        print("config.toml 不存在，无需清理")
        return

    with open(config_toml_path, "r", encoding="utf-8") as f:
        content = f.read()

    if plugin_json_path not in content:
        print("config.toml 中未找到本插件路径，无需清理")
        return

    lines = content.splitlines(keepends=True)

    hooker_section_idx = find_hooker_section(lines)
    if hooker_section_idx is None:
        print("config.toml 中未找到 [hooker] 段，无需清理")
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
        return

    existing_paths, end_line = parse_plugins_from_lines(lines, plugins_line_idx)
    remaining_paths = [p for p in existing_paths if p != plugin_json_path]

    # 替换原来的 plugins 定义
    del lines[plugins_line_idx : end_line + 1]
    lines.insert(plugins_line_idx, format_plugins_array(remaining_paths))

    print("已从 config.toml 的 plugins 列表中移除本插件")

    _write_config_lines(config_toml_path, lines)
