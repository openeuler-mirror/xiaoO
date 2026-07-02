Name:           audit-agent
Version:        0.1.0
Release:        1
Summary:        xiaoO audit_agent plugin — tool action security audit
License:        MIT
URL:            https://gitee.com/openeuler/xiaoO
Source0:        %{name}-%{version}.tar.gz

BuildArch:      noarch
BuildRequires:  python3-devel
BuildRequires:  python3-setuptools
BuildRequires:  python3-wheel
BuildRequires:  python3-pip
Requires:       python3 >= 3.10
Requires:       python3-openai >= 1.0
Requires:       python3-httpx >= 0.25
Requires:       python3-pydantic >= 2.0
Requires:       python3-tenacity >= 8.0
# tomli only needed for Python < 3.11, but harmless to require
Requires:       python3-tomli >= 2.0
# Audit Dashboard 控制面板依赖（均在 openEuler 官方源）
Requires:       python3-fastapi
Requires:       python3-uvicorn
Requires:       python3-starlette

%description
audit_agent is a xiaoO plugin-hooker that intercepts tool actions
(bash commands, file operations, etc.) and performs security audit
using heuristic rules, logic rules, and optional LLM analysis.

%prep
%autosetup -n %{name}-%{version}

%build
cd audit_policy_checker
%py3_build

%install
# 1. Install the Python package to site-packages
cd audit_policy_checker
%py3_install
cd ..

# 2. Install plugin files to /usr/lib/xiaoo/plugins/audit_agent/
install -d %{buildroot}/usr/lib/xiaoo/plugins/audit_agent
install -m 0755 audit.py %{buildroot}/usr/lib/xiaoo/plugins/audit_agent/audit.py
install -m 0644 audit_settings.json.example %{buildroot}/usr/lib/xiaoo/plugins/audit_agent/audit_settings.json.example

# 3. Install plugin.json (RPM version with system Python path)
cat > %{buildroot}/usr/lib/xiaoo/plugins/audit_agent/plugin.json << 'EOF'
[
  {
    "id": "plugin_audit_tool_input",
    "hook_point": "*.Tool.*.pre",
    "command": "/usr/bin/python3 /usr/lib/xiaoo/plugins/audit_agent/audit.py"
  }
]
EOF

# 4. Install audit_dashboard 控制面板包到 site-packages（含 static 前端资源）
install -d %{buildroot}%{python3_sitelib}/audit_dashboard/static
install -m 0644 audit_dashboard/__init__.py %{buildroot}%{python3_sitelib}/audit_dashboard/__init__.py
install -m 0644 audit_dashboard/app.py      %{buildroot}%{python3_sitelib}/audit_dashboard/app.py
install -m 0644 audit_dashboard/static/index.html %{buildroot}%{python3_sitelib}/audit_dashboard/static/index.html

# 5. 注册 xiaoo-audit-dashboard 命令入口（按需启动控制面板）
install -d %{buildroot}/usr/bin
cat > %{buildroot}/usr/bin/xiaoo-audit-dashboard << 'EOF'
#!/bin/bash
exec /usr/bin/python3 -m audit_dashboard.app "$@"
EOF
chmod 0755 %{buildroot}/usr/bin/xiaoo-audit-dashboard

%post
# Generate audit_settings.json from example if not exists
if [ ! -f /usr/lib/xiaoo/plugins/audit_agent/audit_settings.json ]; then
    cp /usr/lib/xiaoo/plugins/audit_agent/audit_settings.json.example \
       /usr/lib/xiaoo/plugins/audit_agent/audit_settings.json
fi

%files
%doc README.md SECURITY_RULES.md
/usr/lib/xiaoo/plugins/audit_agent/
/usr/bin/xiaoo-audit-dashboard
%{python3_sitelib}/audit_policy_checker/
%{python3_sitelib}/audit_policy_checker-%{version}*
%{python3_sitelib}/audit_dashboard/

%changelog
* Wed Jun 04 2026 xiaoO Team <kenhkl11@hotmail.com> - 0.1.0-1
- Initial RPM package for openEuler
