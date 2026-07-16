"""FastAPI 应用主入口 — Audit Dashboard Web 管控台

启动方式：
  python -m audit_dashboard.app
  或
  python app.py

端口：localhost:9765（仅本地绑定）
安全：Bearer token 验证（继承 xiaoO auth）
"""

import os
import sys
from pathlib import Path

# 定位 audit_policy_checker 包：
#   RPM 安装：audit_dashboard 和 audit_policy_checker 都在 site-packages，直接 import 即可
#   开发环境：audit_dashboard 在 plugins/hookers/audit_agent/ 下，
#            audit_policy_checker 在同级 audit_policy_checker/ 目录，需要手动加入 sys.path
try:
    import audit_policy_checker  # noqa: F401  测试是否已在 sys.path 中
except ImportError:
    _dev_policy_checker_path = Path(__file__).parent.parent / "audit_policy_checker"
    if _dev_policy_checker_path.exists():
        sys.path.insert(0, str(_dev_policy_checker_path.parent))
        sys.path.insert(0, str(_dev_policy_checker_path / "audit_policy_checker"))

from fastapi import FastAPI, HTTPException, Depends, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.staticfiles import StaticFiles
from fastapi.responses import HTMLResponse, FileResponse
from pydantic import BaseModel

from audit_policy_checker.runtime_config import (
    load_runtime_config,
    save_runtime_config,
    update_layer_enabled,
    update_rule_enabled,
    update_rule_deny_mode,
    update_rule_skip_l3,
    update_category_enabled,
    add_custom_rule,
    delete_custom_rule,
    add_custom_skill,
    delete_custom_skill,
    update_skill_enabled,
    update_skill_category_enabled,
    get_env_overrides,
    generate_source_defaults,
)
from audit_policy_checker.token_stats import (
    get_token_stats,
    get_recent_records,
    get_token_trend,
    reset_token_stats,
)

# ==================== Auth ====================

AUTH_TOKEN = os.getenv("AUDIT_DASHBOARD_TOKEN", "")

def _verify_auth(request: Request) -> bool:
    """验证 Bearer token"""
    if not AUTH_TOKEN:
        return True  # 未配置 token → 不需要验证
    auth_header = request.headers.get("Authorization", "")
    if auth_header.startswith("Bearer "):
        token = auth_header[7:]
        return token == AUTH_TOKEN
    return False

async def auth_dependency(request: Request):
    if not _verify_auth(request):
        raise HTTPException(status_code=401, detail="Unauthorized: invalid or missing Bearer token")

# ==================== App ====================

app = FastAPI(
    title="xiaoO Audit Dashboard",
    description="安全策略可视化管理平台 — 动态管理增删拦截各层安全策略",
    version="1.0.0",
)

# CORS：允许本地前端访问
app.add_middleware(
    CORSMiddleware,
    allow_origins=["http://localhost:9765", "http://127.0.0.1:9765"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# ==================== Pydantic Models ====================

class LayerToggle(BaseModel):
    layer_key: str  # "L1_heuristic" | "L2_logic_rules" | "L3_llm_analysis"
    enabled: bool

class RuleToggle(BaseModel):
    layer: str  # "L1_rules" | "L2_rules"
    category: str
    rule_id: str
    enabled: bool

class CategoryToggle(BaseModel):
    layer: str
    category: str
    enabled: bool

class NewRule(BaseModel):
    layer: str
    category: str
    # L1 command pattern rule
    pattern: str | None = None
    risk_level: str | None = None
    risk_type: str | None = None
    reason: str | None = None
    # L1 injection keyword rule
    keyword: str | None = None
    lang: str | None = None
    # L2 path rule
    path: str | None = None
    desc: str | None = None
    credential: bool | None = None
    deny_mode: str | None = None  # 敏感路径拦截模式：deny_write / deny_read / deny_both


class DenyModeUpdate(BaseModel):
    layer: str
    category: str
    rule_id: str
    deny_mode: str

class SkipL3Update(BaseModel):
    layer: str
    category: str
    rule_id: str
    skip_l3_on_disabled: bool

class DeleteRule(BaseModel):
    layer: str
    category: str
    rule_id: str

class SkillToggle(BaseModel):
    skill_id: str
    enabled: bool

class SkillCategoryToggle(BaseModel):
    category: str
    enabled: bool

class NewSkill(BaseModel):
    skill_id: str
    category: str = "user_custom"
    keywords: list[str] = []
    content: str  # Markdown content

class DeleteSkill(BaseModel):
    skill_id: str
    category: str

# ==================== API Routes ====================

@app.get("/", response_class=HTMLResponse)
async def index():
    """返回前端页面"""
    static_dir = Path(__file__).parent / "static"
    index_file = static_dir / "index.html"
    if index_file.exists():
        return FileResponse(index_file)
    return HTMLResponse("<h1>xiaoO Audit Dashboard</h1><p>Frontend not built yet. Use API endpoints.</p>")

# 层级管理
@app.get("/api/layers", dependencies=[Depends(auth_dependency)])
async def get_layers():
    """获取 L1/L2/L3 开关状态"""
    runtime = load_runtime_config()
    env_overrides = get_env_overrides()
    return {
        "layers": runtime.get("layers", {}),
        "env_overrides": env_overrides,
    }

@app.put("/api/layers", dependencies=[Depends(auth_dependency)])
async def set_layers(body: LayerToggle):
    """设置层级开关"""
    valid_keys = {"L1_heuristic", "L2_logic_rules", "L3_llm_analysis"}
    if body.layer_key not in valid_keys:
        raise HTTPException(status_code=400, detail=f"Invalid layer_key: {body.layer_key}")
    # 检查环境变量是否覆盖
    env_overrides = get_env_overrides()
    if body.layer_key in env_overrides and env_overrides[body.layer_key]["overridden"]:
        raise HTTPException(
            status_code=409,
            detail=f"层级 {body.layer_key} 被环境变量 {env_overrides[body.layer_key]['env_var']}=1 强制覆盖，Web 设置无效",
        )
    runtime = update_layer_enabled(body.layer_key, body.enabled)
    return {"layers": runtime["layers"], "env_overrides": env_overrides}

# 规则管理
@app.get("/api/rules", dependencies=[Depends(auth_dependency)])
async def get_rules(layer: str | None = None, category: str | None = None):
    """获取规则列表"""
    runtime = load_runtime_config()
    result = {}
    for layer_key in ["L1_rules", "L2_rules"]:
        if layer and layer != layer_key:
            continue
        layer_data = runtime.get(layer_key, {})
        for cat_name, cat_data in layer_data.items():
            if category and category != cat_name:
                continue
            result.setdefault(layer_key, {})[cat_name] = cat_data
    return result

@app.put("/api/rules/enabled", dependencies=[Depends(auth_dependency)])
async def toggle_rule(body: RuleToggle):
    """开关单条规则"""
    runtime = update_rule_enabled(body.layer, body.category, body.rule_id, body.enabled)
    # 返回完整 runtimeConfig，避免前端全局状态被部分数据覆盖（曾导致搜索失效）
    return runtime


@app.put("/api/rules/deny_mode", dependencies=[Depends(auth_dependency)])
async def change_rule_deny_mode(body: DenyModeUpdate):
    """修改敏感路径规则的拦截模式"""
    if body.deny_mode not in ("deny_write", "deny_read", "deny_both"):
        raise HTTPException(status_code=400, detail=f"无效的 deny_mode: {body.deny_mode}")
    runtime = update_rule_deny_mode(body.layer, body.category, body.rule_id, body.deny_mode)
    return runtime


@app.put("/api/rules/skip_l3", dependencies=[Depends(auth_dependency)])
async def change_rule_skip_l3(body: SkipL3Update):
    """修改规则禁用时是否也跳过 L3 分析"""
    runtime = update_rule_skip_l3(body.layer, body.category, body.rule_id, body.skip_l3_on_disabled)
    return runtime


@app.put("/api/categories/enabled", dependencies=[Depends(auth_dependency)])
async def toggle_category(body: CategoryToggle):
    """开关整个分类"""
    runtime = update_category_enabled(body.layer, body.category, body.enabled)
    return runtime

@app.post("/api/rules", dependencies=[Depends(auth_dependency)])
async def create_rule(body: NewRule):
    """新增自定义规则"""
    rule_dict = {}
    if body.pattern:
        rule_dict["pattern"] = body.pattern
    if body.keyword:
        rule_dict["keyword"] = body.keyword
    if body.risk_level:
        rule_dict["risk_level"] = body.risk_level
    if body.risk_type:
        rule_dict["risk_type"] = body.risk_type
    if body.reason:
        rule_dict["reason"] = body.reason
    if body.lang:
        rule_dict["lang"] = body.lang
    if body.path:
        rule_dict["path"] = body.path
    if body.desc:
        rule_dict["desc"] = body.desc
    if body.credential:
        rule_dict["credential"] = body.credential
    if body.deny_mode:
        rule_dict["deny_mode"] = body.deny_mode

    runtime = add_custom_rule(body.layer, body.category, rule_dict)
    return runtime

@app.delete("/api/rules", dependencies=[Depends(auth_dependency)])
async def remove_rule(body: DeleteRule):
    """删除自定义规则（内置规则不可删除）"""
    runtime = delete_custom_rule(body.layer, body.category, body.rule_id)
    if runtime is None:
        raise HTTPException(status_code=403, detail="内置规则不可删除，只能禁用")
    return runtime

# Skill 管理
@app.get("/api/skills", dependencies=[Depends(auth_dependency)])
async def get_skills():
    """获取所有 skill 列表"""
    runtime = load_runtime_config()
    return runtime.get("L3_skills", {})

@app.put("/api/skills/enabled", dependencies=[Depends(auth_dependency)])
async def toggle_skill(body: SkillToggle):
    """开关单个 skill"""
    runtime = update_skill_enabled(body.skill_id, body.enabled)
    return runtime

@app.put("/api/skill-categories/enabled", dependencies=[Depends(auth_dependency)])
async def toggle_skill_category(body: SkillCategoryToggle):
    """开关 skill 分类"""
    runtime = update_skill_category_enabled(body.category, body.enabled)
    return runtime

@app.post("/api/skills", dependencies=[Depends(auth_dependency)])
async def create_skill(body: NewSkill):
    """新增自定义 skill"""
    runtime = add_custom_skill(body.skill_id, body.category, body.keywords, body.content)
    return runtime

@app.delete("/api/skills", dependencies=[Depends(auth_dependency)])
async def remove_skill(body: DeleteSkill):
    """删除自定义 skill"""
    runtime = delete_custom_skill(body.skill_id, body.category)
    if runtime is None:
        raise HTTPException(status_code=403, detail="内置 skill 不可删除，只能禁用")
    return runtime

@app.get("/api/skills/{skill_id}/content", dependencies=[Depends(auth_dependency)])
async def get_skill_content(skill_id: str):
    """获取 skill 的完整 Markdown 内容（详情弹窗用）。
    优先读用户自定义目录，再读内置 skills 目录。"""
    from audit_policy_checker.security.skill_engine import DEFAULT_SKILLS_DIR, USER_SKILLS_DIR
    # 防路径穿越：skill_id 只允许字母数字下划线横线
    if not skill_id.replace("-", "").replace("_", "").isalnum():
        raise HTTPException(status_code=400, detail="非法 skill id")
    for base in (USER_SKILLS_DIR, DEFAULT_SKILLS_DIR):
        p = base / f"{skill_id}.md"
        if p.exists():
            return {"skill_id": skill_id, "path": str(p), "content": p.read_text(encoding="utf-8")}
    raise HTTPException(status_code=404, detail=f"skill {skill_id} 不存在")

# 配置完整查看
@app.get("/api/config", dependencies=[Depends(auth_dependency)])
async def get_full_config():
    """获取完整 audit_runtime.json"""
    runtime = load_runtime_config()
    return runtime

# 环境变量覆盖
@app.get("/api/env-overrides", dependencies=[Depends(auth_dependency)])
async def get_env_status():
    """获取当前环境变量覆盖状态"""
    return get_env_overrides()

# 重置配置
@app.post("/api/reset", dependencies=[Depends(auth_dependency)])
async def reset_config():
    """重置配置到出厂默认值（删除用户本地副本，下次运行自动重新生成）"""
    from audit_policy_checker.runtime_config import RUNTIME_CONFIG_PATH, get_runtime_config_path
    config_path = get_runtime_config_path()
    if config_path.exists():
        config_path.unlink()
    defaults = generate_source_defaults()
    return defaults

# ==================== Token Stats API ====================

@app.get("/api/token-stats", dependencies=[Depends(auth_dependency)])
async def get_token_stats_api(
    days: int = 0,
    start_date: str | None = None,
    end_date: str | None = None,
):
    """
    获取 token 用量统计汇总。

    Args:
        days: 查询最近多少天的数据。0=全部, 1=今天, 7=近7天, 30=近30天
        start_date: 起始日期（YYYY-MM-DD，含当天）。优先于 days。
        end_date: 结束日期（YYYY-MM-DD，含当天全天）。优先于 days。
    """
    return get_token_stats(days, start_date=start_date, end_date=end_date)

@app.get("/api/token-stats/recent", dependencies=[Depends(auth_dependency)])
async def get_recent_token_records(limit: int = 20):
    """获取最近的 N 条 token 用量记录"""
    return get_recent_records(limit)

@app.get("/api/token-stats/trend", dependencies=[Depends(auth_dependency)])
async def get_token_trend_api(
    days: int = 0,
    start_date: str | None = None,
    end_date: str | None = None,
):
    """
    获取 Token 消耗趋势（按日期 + 模型聚合），供前端折线图渲染。
    days: 0=全部, 1=今天, 7=近7天。start_date/end_date 优先于 days。
    """
    return get_token_trend(days=days, start_date=start_date, end_date=end_date)

@app.post("/api/token-stats/reset", dependencies=[Depends(auth_dependency)])
async def reset_token_stats_api():
    """重置（删除）所有 token 统计记录"""
    reset_token_stats()
    return {"detail": "Token stats reset successfully"}

# ==================== Static Files ====================

static_dir = Path(__file__).parent / "static"
if static_dir.exists():
    app.mount("/static", StaticFiles(directory=str(static_dir)), name="static")

# ==================== Run ====================

def main():
    """启动 dashboard 服务"""
    import uvicorn
    port = int(os.getenv("AUDIT_DASHBOARD_PORT", "9765"))
    host = os.getenv("AUDIT_DASHBOARD_HOST", "127.0.0.1")
    print(f"🛡️ xiaoO Audit Dashboard starting at http://{host}:{port}")
    print(f"   API docs: http://{host}:{port}/docs")
    uvicorn.run(app, host=host, port=port)

if __name__ == "__main__":
    main()
