import json
import shlex
import sys


CONFIG_KEY_CERBERUS_PATH = "cerberus_path"
CONFIG_KEY_PROFILE = "profile"
CONFIG_KEY_POLICY_FILE = "policy_file"
DEFAULT_CERBERUS_PATH = "cerberus"
DEFAULT_PROFILE_NAME = "workspace-write-network-off"


def build_cerberus_command(config: dict, original_command: str) -> str:
    cerberus_path = config.get(CONFIG_KEY_CERBERUS_PATH, DEFAULT_CERBERUS_PATH)
    profile = config.get(CONFIG_KEY_PROFILE, DEFAULT_PROFILE_NAME)
    policy_file = config.get(CONFIG_KEY_POLICY_FILE)

    if not isinstance(cerberus_path, str) or not cerberus_path:
        raise ValueError(f"invalid definition.config.{CONFIG_KEY_CERBERUS_PATH}")

    if isinstance(policy_file, str) and policy_file:
        policy_args = f"--policy-file {shlex.quote(policy_file)}"
    elif isinstance(profile, str) and profile:
        policy_args = f"--profile {shlex.quote(profile)}"
    else:
        raise ValueError(
            f"missing definition.config.{CONFIG_KEY_PROFILE} or definition.config.{CONFIG_KEY_POLICY_FILE}"
        )

    return (
        f"{shlex.quote(cerberus_path)} {policy_args} exec -- "
        f"bash -lc {shlex.quote(original_command)}"
    )


def main() -> int:
    payload = json.load(sys.stdin)
    if payload.get("stage") != "pre":
        json.dump({"result": "allow"}, sys.stdout)
        return 0

    call = payload.get("call", {})
    if call.get("tool_name") != "bash":
        json.dump({"result": "allow"}, sys.stdout)
        return 0

    tool_input = call.get("input", {})
    original_command = tool_input.get("command")
    if not isinstance(original_command, str):
        json.dump({"result": "allow"}, sys.stdout)
        return 0

    definition = payload.get("definition", {})
    config = definition.get("config", {})
    try:
        wrapped_command = build_cerberus_command(config, original_command)
    except ValueError as error:
        json.dump({"result": "deny", "reason": str(error)}, sys.stdout)
        return 0

    modified_input = dict(tool_input)
    modified_input["command"] = wrapped_command
    json.dump({"result": "transform", "modified_input": modified_input}, sys.stdout)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
