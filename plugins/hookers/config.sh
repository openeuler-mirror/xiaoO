#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_FILE="$HOME/.config/xiaoo/config.toml"

# --- Scan valid hookers ---
declare -a VALID_HOOKERS=()

for subdir in "$SCRIPT_DIR"/*/; do
    [ -d "$subdir" ] || continue
    if [ -f "${subdir%/}/plugin.json" ]; then
        VALID_HOOKERS+=("${subdir%/}/plugin.json")
    fi
done

if [ ${#VALID_HOOKERS[@]} -eq 0 ]; then
    echo "No valid hookers found (missing plugin.json)."
    exit 0
fi

echo "Found ${#VALID_HOOKERS[@]} valid hooker(s):"
for i in "${!VALID_HOOKERS[@]}"; do
    echo "  $((i+1)). ${VALID_HOOKERS[$i]}"
done

# --- Normalize command paths in plugin.json ---
for plugin_path in "${VALID_HOOKERS[@]}"; do
    hooker_dir="$(dirname "$plugin_path")"

    python3 -c "
import json, os

with open('$plugin_path', 'r') as f:
    data = json.load(f)

changed = False
hooker_dir = '$hooker_dir'

SHELL_TOKENS = {'&&', '||', ';', '|', '&', '>', '>>', '<', '<<', '(', ')'}
SHELL_BUILTINS = {'source', '.', 'cd', 'export', 'set', 'unset', 'eval', 'exec', 'exit', 'alias', 'unalias', 'echo', 'printf', 'return', 'shift', 'wait', 'trap', 'read', 'true', 'false', 'test'}

def needs_resolve(token):
    if not token:
        return False
    if token in SHELL_TOKENS or token in SHELL_BUILTINS:
        return False
    if token.startswith('~'):
        return True
    if token.startswith('/') or token.startswith('-') or token.startswith('--'):
        return False
    if '/' in token:
        return True
    if '.' in os.path.basename(token):
        return True
    return False

def resolve_token(token, hooker_dir):
    expanded = os.path.expanduser(token)
    if not expanded.startswith('/'):
        expanded = os.path.normpath(os.path.join(hooker_dir, expanded))
    return expanded

for item in data:
    command = item.get('command', '')
    raw_command = item.get('raw_command')

    if raw_command is not None:
        item['command'] = raw_command
        command = raw_command

    tokens = command.split(' ')
    any_changed = False
    new_tokens = []
    for token in tokens:
        if needs_resolve(token):
            new_tokens.append(resolve_token(token, hooker_dir))
            any_changed = True
        else:
            new_tokens.append(token)

    if any_changed:
        item['raw_command'] = command
        item['command'] = ' '.join(new_tokens)
        changed = True

if changed:
    with open('$plugin_path', 'w') as f:
        json.dump(data, f, indent=2, ensure_ascii=False)
    print(f'  Updated command paths in $plugin_path')
" || echo "  Warning: failed to process $plugin_path"
done

# --- Ensure config file exists ---
mkdir -p "$(dirname "$CONFIG_FILE")"

# --- Read existing [hooker] fields using Python ---
read_hooker_config() {
    python3 -c "
import sys
try:
    with open('$CONFIG_FILE', 'r') as f:
        content = f.read()
except FileNotFoundError:
    print('{}')
    sys.exit(0)

# Simple TOML parser for [hooker] section only - handles multi-line arrays
import re
hooker_section = re.search(r'^\[hooker\](.*?)(?=^\[|\Z)', content, re.MULTILINE | re.DOTALL)
if not hooker_section:
    print('{}')
    sys.exit(0)

section_text = hooker_section.group(1)
result = {}

# Parse lines, handling multi-line arrays
lines = section_text.strip().split('\n')
i = 0
while i < len(lines):
    line = lines[i].strip()
    if not line or line.startswith('#'):
        i += 1
        continue
    match = re.match(r'^(\w+)\s*=\s*(.*)$', line)
    if match:
        key, value = match.groups()
        # Handle string values
        if value.startswith('\"') and value.endswith('\"'):
            result[key] = value[1:-1]
        # Handle arrays (may be multi-line)
        elif value.startswith('['):
            if value == '[]':
                result[key] = []
            else:
                # Collect all lines until array closes
                array_lines = [line]
                while i + 1 < len(lines) and ']' not in ''.join(array_lines):
                    i += 1
                    array_lines.append(lines[i])
                # Extract items from all lines
                combined = ' '.join(array_lines)
                items = re.findall(r'\"([^\"]+)\"', combined)
                result[key] = items
        # Handle objects
        elif value.startswith('{'):
            result[key] = {}
        i += 1
    else:
        i += 1

import json
print(json.dumps(result))
"
}

# Parse current config
CURRENT_CONFIG=$(read_hooker_config)
CURRENT_DEFAULT=$(echo "$CURRENT_CONFIG" | python3 -c "import json,sys; print(json.load(sys.stdin).get('default', 'All'))")
CURRENT_ENABLED=$(echo "$CURRENT_CONFIG" | python3 -c "import json,sys; print(json.load(sys.stdin).get('enabled', []))")
CURRENT_DISABLED=$(echo "$CURRENT_CONFIG" | python3 -c "import json,sys; d=json.load(sys.stdin).get('disabled', []); print(d if isinstance(d, list) else [])")
CURRENT_POLICIES=$(echo "$CURRENT_CONFIG" | python3 -c "import json,sys; d=json.load(sys.stdin).get('policies', {}); print('{}' if isinstance(d, dict) else d)")

echo ""
echo "Current [hooker] default = \"$CURRENT_DEFAULT\""

# --- Ask user: modify default? ---
echo "Modify default setting?"
echo "  1) All  - all hookers enabled by default"
echo "  2) None - no hookers enabled by default"
echo "  3) Keep current (\"$CURRENT_DEFAULT\")"
read -rp "Enter choice [1/2/3]: " default_choice

DEFAULT_VAL="$CURRENT_DEFAULT"
case "$default_choice" in
    1) DEFAULT_VAL="All" ;;
    2) DEFAULT_VAL="None" ;;
    3) DEFAULT_VAL="$CURRENT_DEFAULT" ;;
    *)  echo "Invalid choice, keeping current: $CURRENT_DEFAULT" ;;
esac

# --- Ask user: modify plugins? ---
echo ""
echo "Modify plugins list?"
echo "  1) Yes - add all valid hookers"
echo "  2) Yes - let me choose specific hookers"
echo "  3) Keep current"
read -rp "Enter choice [1/2/3]: " plugins_choice

SELECTED=()
case "$plugins_choice" in
    1)
        SELECTED=("${VALID_HOOKERS[@]}")
        ;;
    2)
        echo "Select hookers by number (space-separated, e.g. 1 3):"
        for i in "${!VALID_HOOKERS[@]}"; do
            echo "  $((i+1)). ${VALID_HOOKERS[$i]}"
        done
        read -rp "Enter numbers: " choices
        for num in $choices; do
            if [[ "$num" =~ ^[0-9]+$ ]] && [ "$num" -ge 1 ] && [ "$num" -le "${#VALID_HOOKERS[@]}" ]; then
                SELECTED+=("${VALID_HOOKERS[$((num-1))]}")
            else
                echo "Invalid selection: $num (skipped)"
            fi
        done
        ;;
    3)
        echo "Keeping current plugins list."
        # Keep current plugins
        CURRENT_PLUGINS=$(echo "$CURRENT_CONFIG" | python3 -c "import json,sys; d=json.load(sys.stdin).get('plugins', []); print('\\n'.join(d) if isinstance(d, list) else '')")
        while IFS= read -r line; do
            [ -n "$line" ] && SELECTED+=("$line")
        done <<< "$CURRENT_PLUGINS"
        ;;
    *)
        echo "Invalid choice, keeping current plugins list."
        CURRENT_PLUGINS=$(echo "$CURRENT_CONFIG" | python3 -c "import json,sys; d=json.load(sys.stdin).get('plugins', []); print('\\n'.join(d) if isinstance(d, list) else '')")
        while IFS= read -r line; do
            [ -n "$line" ] && SELECTED+=("$line")
        done <<< "$CURRENT_PLUGINS"
        ;;
esac

# --- Write updated [hooker] section using Python ---
python3 -c "
import json

config_file = '$CONFIG_FILE'
default_val = '$DEFAULT_VAL'
plugins = $(printf '%s\n' "${SELECTED[@]}" | python3 -c 'import json,sys; print(json.dumps([line.strip() for line in sys.stdin if line.strip()]))')
enabled = $CURRENT_ENABLED
disabled = $CURRENT_DISABLED
policies = $CURRENT_POLICIES

# Read existing config
try:
    with open(config_file, 'r') as f:
        content = f.read()
except FileNotFoundError:
    content = ''

# Remove old [hooker] section
import re
content = re.sub(r'^\[hooker\].*?(?=^\[|\Z)', '', content, flags=re.MULTILINE | re.DOTALL)
content = content.lstrip('\n')

# Build new [hooker] section
hooker_lines = ['[hooker]', f'default = \"{default_val}\"']
hooker_lines.append('plugins = [')
for i, p in enumerate(plugins):
    comma = ',' if i < len(plugins) - 1 else ''
    hooker_lines.append(f'  \"{p}\"{comma}')
hooker_lines.append(']')
hooker_lines.append(f'enabled = {json.dumps(enabled)}')
hooker_lines.append(f'disabled = {json.dumps(disabled)}')
hooker_lines.append('policies = ' + json.dumps(policies))

new_hooker = '\n\n'.join(['', '\n'.join(hooker_lines), ''])

if content:
    # Append new [hooker] section at the end
    content = content.rstrip('\n') + new_hooker
else:
    content = new_hooker.strip()

with open(config_file, 'w') as f:
    f.write(content)
"

echo ""
echo "Config updated: $CONFIG_FILE"

# --- Run install.sh for hookers that have one ---
for plugin_path in "${SELECTED[@]}"; do
    hooker_dir="$(dirname "$plugin_path")"
    install_script="$hooker_dir/install.sh"
    if [ -f "$install_script" ]; then
        echo ""
        echo "Running install.sh for: $hooker_dir"
        (cd "$hooker_dir" && bash "$install_script")
    fi
done

echo ""
echo "Done."
