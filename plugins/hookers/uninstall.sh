#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_FILE="$HOME/.config/xiaoo/config.toml"

# --- Check config file exists ---
if [ ! -f "$CONFIG_FILE" ]; then
    echo "Config file not found: $CONFIG_FILE"
    echo "Nothing to uninstall."
    exit 0
fi

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
CURRENT_PLUGINS=$(echo "$CURRENT_CONFIG" | python3 -c "import json,sys; d=json.load(sys.stdin).get('plugins', []); print('\\n'.join(d) if isinstance(d, list) else '')")
CURRENT_ENABLED=$(echo "$CURRENT_CONFIG" | python3 -c "import json,sys; print(json.load(sys.stdin).get('enabled', []))")
CURRENT_DISABLED=$(echo "$CURRENT_CONFIG" | python3 -c "import json,sys; d=json.load(sys.stdin).get('disabled', []); print(d if isinstance(d, list) else [])")
CURRENT_POLICIES=$(echo "$CURRENT_CONFIG" | python3 -c "import json,sys; d=json.load(sys.stdin).get('policies', {}); print('{}' if isinstance(d, dict) else d)")

# Build array of current plugins
declare -a INSTALLED_HOOKERS=()
while IFS= read -r line; do
    [ -n "$line" ] && INSTALLED_HOOKERS+=("$line")
done <<< "$CURRENT_PLUGINS"

if [ ${#INSTALLED_HOOKERS[@]} -eq 0 ]; then
    echo "No hookers installed in config."
    exit 0
fi

echo "Currently installed hookers:"
for i in "${!INSTALLED_HOOKERS[@]}"; do
    hooker_path="${INSTALLED_HOOKERS[$i]}"
    hooker_name=$(basename "$(dirname "$hooker_path")")
    echo "  $((i+1)). $hooker_name ($hooker_path)"
done

echo ""
echo "Uninstall options:"
echo "  1) Remove ALL hookers"
echo "  2) Remove specific hookers (select by number)"
echo "  3) Cancel"
read -rp "Enter choice [1/2/3]: " uninstall_choice

declare -a TO_REMOVE=()
declare -a TO_KEEP=()

case "$uninstall_choice" in
    1)
        TO_REMOVE=("${INSTALLED_HOOKERS[@]}")
        echo ""
        echo "Will remove ALL ${#INSTALLED_HOOKERS[@]} hooker(s)."
        ;;
    2)
        echo ""
        echo "Select hookers to remove by number (space-separated, e.g. 1 3):"
        for i in "${!INSTALLED_HOOKERS[@]}"; do
            hooker_path="${INSTALLED_HOOKERS[$i]}"
            hooker_name=$(basename "$(dirname "$hooker_path")")
            echo "  $((i+1)). $hooker_name"
        done
        read -rp "Enter numbers: " choices

        declare -A remove_indices=()
        for num in $choices; do
            if [[ "$num" =~ ^[0-9]+$ ]] && [ "$num" -ge 1 ] && [ "$num" -le "${#INSTALLED_HOOKERS[@]}" ]; then
                remove_indices["$((num-1))"]=1
            else
                echo "Invalid selection: $num (skipped)"
            fi
        done

        for i in "${!INSTALLED_HOOKERS[@]}"; do
            if [ -n "${remove_indices[$i]:-}" ]; then
                TO_REMOVE+=("${INSTALLED_HOOKERS[$i]}")
            else
                TO_KEEP+=("${INSTALLED_HOOKERS[$i]}")
            fi
        done
        ;;
    3|*)
        echo "Cancelled."
        exit 0
        ;;
esac

if [ ${#TO_REMOVE[@]} -eq 0 ]; then
    echo "No hookers selected for removal."
    exit 0
fi

echo ""
echo "Hookers to remove:"
for hooker_path in "${TO_REMOVE[@]}"; do
    hooker_name=$(basename "$(dirname "$hooker_path")")
    echo "  - $hooker_name"
done

read -rp "Proceed? [y/N]: " confirm
if [[ ! "$confirm" =~ ^[Yy]$ ]]; then
    echo "Cancelled."
    exit 0
fi

# --- Run uninstall.sh for hookers that have one ---
for hooker_path in "${TO_REMOVE[@]}"; do
    hooker_dir="$(dirname "$hooker_path")"
    hooker_name=$(basename "$hooker_dir")
    uninstall_script="$hooker_dir/uninstall.sh"
    if [ -f "$uninstall_script" ]; then
        echo ""
        echo "开始执行 $hooker_name 的 uninstall.sh..."
        (cd "$hooker_dir" && bash "$uninstall_script")
    fi
done

# --- Update config file ---
python3 -c "
import json
import re

config_file = '$CONFIG_FILE'
default_val = '$CURRENT_DEFAULT'
plugins_to_keep = $(printf '%s\n' "${TO_KEEP[@]}" | python3 -c 'import json,sys; print(json.dumps([line.strip() for line in sys.stdin if line.strip()]))')
plugins_to_remove = $(printf '%s\n' "${TO_REMOVE[@]}" | python3 -c 'import json,sys; print(json.dumps([line.strip() for line in sys.stdin if line.strip()]))')
enabled = $CURRENT_ENABLED
disabled = $CURRENT_DISABLED
policies = $CURRENT_POLICIES

# Filter out removed plugins from enabled and disabled
def filter_list(lst, removed):
    removed_set = set(removed)
    return [x for x in lst if x not in removed_set]

enabled = filter_list(enabled, plugins_to_remove)
disabled = filter_list(disabled, plugins_to_remove)

# Read existing config
try:
    with open(config_file, 'r') as f:
        content = f.read()
except FileNotFoundError:
    content = ''

# Remove old [hooker] section
content = re.sub(r'^\[hooker\].*?(?=^\[|\Z)', '', content, flags=re.MULTILINE | re.DOTALL)
content = content.lstrip('\n')

# Build new [hooker] section
hooker_lines = ['[hooker]', f'default = \"{default_val}\"']
hooker_lines.append('plugins = [')
for i, p in enumerate(plugins_to_keep):
    comma = ',' if i < len(plugins_to_keep) - 1 else ''
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

print(f'Removed {len(plugins_to_remove)} hooker(s) from config.')
"

echo ""
echo "Uninstall complete."
