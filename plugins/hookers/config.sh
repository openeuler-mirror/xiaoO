#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_FILE="$HOME/.config/xiaoo/config.toml"

# --- Scan valid hookers ---
declare -a VALID_HOOKERS=()

for subdir in "$SCRIPT_DIR"/*/; do
    [ -d "$subdir" ] || continue
    if [ -f "$subdir/plugin.json" ]; then
        VALID_HOOKERS+=("$subdir/plugin.json")
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

# --- Ensure [hooker] section with all required fields in config.toml ---
mkdir -p "$(dirname "$CONFIG_FILE")"

# Extract a field line from [hooker] section only
get_hooker_field() {
    local key="$1"
    [ -f "$CONFIG_FILE" ] || return 1
    awk -v k="$key" '
        /^\[hooker\]/{found=1; next}
        /^\[/{found=0}
        found && index($0, k " = ") == 1 {print; exit}
    ' "$CONFIG_FILE"
}

# Ensure a field exists in [hooker] section; if missing, append after the last field in that section
ensure_hooker_field() {
    local key="$1"
    local default_line="$2"
    local existing
    existing=$(get_hooker_field "$key")
    if [ -z "$existing" ]; then
        # Find the last line number belonging to [hooker] section
        local line_num
        line_num=$(awk '
            /^\[hooker\]/{found=1; next}
            /^\[/{found=0}
            found && /=/{last=NR}
            END{print last}
        ' "$CONFIG_FILE")
        if [ -z "$line_num" ]; then
            # No fields at all yet, insert right after [hooker]
            line_num=$(grep -n '^\[hooker\]' "$CONFIG_FILE" | head -1 | cut -d: -f1)
        fi
        sed -i "${line_num}a\\${default_line}" "$CONFIG_FILE"
    fi
}

if [ ! -f "$CONFIG_FILE" ]; then
    cat >> "$CONFIG_FILE" <<'HOOKER_EOF'

[hooker]
default = "All"
plugins = []
enabled = []
disabled = []
policies = {}
HOOKER_EOF
elif ! grep -q '^\[hooker\]' "$CONFIG_FILE"; then
    cat >> "$CONFIG_FILE" <<'HOOKER_EOF'

[hooker]
default = "All"
plugins = []
enabled = []
disabled = []
policies = {}
HOOKER_EOF
else
    # [hooker] exists but may be incomplete — only add missing fields, keep existing values
    ensure_hooker_field "default" 'default = "All"'
    ensure_hooker_field "plugins" 'plugins = []'
    ensure_hooker_field "enabled" 'enabled = []'
    ensure_hooker_field "disabled" 'disabled = []'
    ensure_hooker_field "policies" 'policies = {}'
fi

# --- Replace a field value in [hooker] section only, safely ---
replace_hooker_field() {
    local key="$1"
    local new_value="$2"
    # Write new line to a temp file to avoid quoting issues in awk
    local tmpfile
    tmpfile=$(mktemp)
    printf '%s = %s\n' "$key" "$new_value" > "$tmpfile"
    awk -v key="$key" -v repfile="$tmpfile" '
        /^\[hooker\]/{in_hooker=1; print; next}
        /^\[/{in_hooker=0}
        in_hooker && index($0, key " = ") == 1 {
            while((getline line < repfile) > 0) print line
            close(repfile)
            next
        }
        {print}
    ' "$CONFIG_FILE" > "$CONFIG_FILE.tmp" && mv "$CONFIG_FILE.tmp" "$CONFIG_FILE"
    rm -f "$tmpfile"
}

# --- Read current default value ---
CURRENT_DEFAULT=$(get_hooker_field "default" | sed 's/^default = "\(.*\)"/\1/')
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

if [ "$DEFAULT_VAL" != "$CURRENT_DEFAULT" ]; then
    replace_hooker_field "default" "\"$DEFAULT_VAL\""
fi

# --- Read current plugins list ---
CURRENT_PLUGINS=$(get_hooker_field "plugins" | sed 's/^plugins = //')
echo ""
echo "Current [hooker] plugins = $CURRENT_PLUGINS"

# --- Ask user: modify plugins? ---
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
        ;;
    *)
        echo "Invalid choice, keeping current plugins list."
        ;;
esac

if [ ${#SELECTED[@]} -gt 0 ]; then
    # Build TOML array value: [ "path1", "path2" ]
    PLUGINS_VAL="["
    for i in "${!SELECTED[@]}"; do
        if [ "$i" -gt 0 ]; then
            PLUGINS_VAL+=", "
        fi
        PLUGINS_VAL+="\"${SELECTED[$i]}\""
    done
    PLUGINS_VAL+="]"

    replace_hooker_field "plugins" "$PLUGINS_VAL"
fi

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
