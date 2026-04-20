SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" &>/dev/null && pwd)

cd $SCRIPT_DIR/audit_policy_checker
rm -rf venv
python3 -m venv venv
source venv/bin/activate
pip install .
