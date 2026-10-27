#!/usr/bin/env python3
"""
config.py - 配置插件脚本

用法:
    python3 config.py install   - 安装插件
    python3 config.py uninstall - 卸载插件

功能:
    install:
        1. 从 plugin.json.template 生成 plugin.json（填充绝对路径）
        2. 将当前目录的 plugin.json 路径注册进 ~/.config/xiaoo/config.toml
           的 [hooker] 段（公共逻辑见 common/install_helper.py）

    uninstall:
        1. 删除生成的 plugin.json
        2. 从 config.toml 的 plugins 列表中移除本插件路径
"""

import argparse
import json
import os
import sys

# 公共安装逻辑位于同级 common/ 目录
sys.path.insert(
    0,
    os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "common"
    ),
)

import install_helper  # noqa: E402


def do_install(script_dir: str):
    """执行安装"""
    plugin_json_template_path = os.path.join(script_dir, "plugin.json.template")
    plugin_json_path = os.path.join(script_dir, "plugin.json")
    python_script_path = os.path.join(script_dir, "guard_secret_like_request.py")

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

    install_helper.register_plugin_in_config(plugin_json_path)

    print("安装完成!")


def do_uninstall(script_dir: str):
    """执行卸载"""
    plugin_json_path = os.path.join(script_dir, "plugin.json")

    print(f"脚本目录: {script_dir}")

    if os.path.exists(plugin_json_path):
        os.remove(plugin_json_path)
        print("已删除 plugin.json")
    else:
        print("plugin.json 不存在，跳过")

    install_helper.unregister_plugin_from_config(plugin_json_path)

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
