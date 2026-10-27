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


def prompt_alert_log_path() -> str:
    """询问用户告警日志路径"""
    print("\n请配置告警日志路径（用于记录 Cerberus 沙箱拦截事件）。")
    default_path = "/tmp/cerberus_alert.log"
    while True:
        choice = input(f"告警日志路径 [默认: {default_path}]: ").strip()
        if not choice:
            return default_path
        return choice


def check_cerberus_installed() -> bool:
    """检查 cerberus 是否已安装"""
    cerberus_path = os.path.expanduser("~/.cargo/bin/cerberus")
    if os.path.exists(cerberus_path):
        return True

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

    print(f"脚本目录: {script_dir}")

    # 检查 cerberus 是否已安装
    if not check_cerberus_installed():
        print("错误: cerberus 未安装")
        print("请先按外部 Cerberus 项目的说明安装兼容的 CLI")
        print("确保 cerberus 在 PATH 中或位于 ~/.cargo/bin/cerberus")
        print("详情参考 README.md 中的安装说明")
        sys.exit(1)

    print("cerberus 已安装")

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

    alert_log_path = prompt_alert_log_path()

    with open(plugin_json_template_path, "r", encoding="utf-8") as f:
        content = f.read()

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

    # 注册进 config.toml 的 [hooker] 段
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
