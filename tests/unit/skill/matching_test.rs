use super::*;

fn summary(
    skill_id: &str,
    tags: &[&str],
    paths: &[&str],
    disable_model_invocation: bool,
) -> SkillSummary {
    SkillSummary {
        skill_id: skill_id.to_string(),
        description: format!("description of {skill_id}"),
        when_to_use: None,
        paths: paths.iter().map(|path| path.to_string()).collect(),
        tags: tags.iter().map(|tag| tag.to_string()).collect(),
        disable_model_invocation,
    }
}

fn index(summaries: Vec<SkillSummary>) -> SkillMatchIndex {
    SkillMatchIndex::from_shared(summaries.into())
}

fn user_text_signals(text: &str) -> SkillMatchSignals<'_> {
    SkillMatchSignals {
        written_paths: &[],
        user_text: Some(text),
        workspace_root: None,
    }
}

#[test]
fn user_text_tag_match_requires_word_boundary() {
    let idx = index(vec![summary("rs-skill", &["rs"], &[], false)]);
    // ASCII 连续单词内出现（子串）不命中
    assert!(idx
        .match_skills_detailed(&user_text_signals("fix the parsers module"))
        .is_empty());
    // 非 ASCII 字符天然充当词边界
    let hits = idx.match_skills_detailed(&user_text_signals("帮我看下rs文件"));
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].summary.skill_id, "rs-skill");
    assert_eq!(
        hits[0].reason,
        SkillMatchReason::UserText {
            tag: "rs".to_string()
        }
    );
}

#[test]
fn user_text_tag_match_is_case_insensitive() {
    let idx = index(vec![summary("rs-skill", &["rs"], &[], false)]);
    let hits = idx.match_skills_detailed(&user_text_signals("please use RS here"));
    assert_eq!(hits.len(), 1);
}

#[test]
fn write_pivot_glob_matches_after_workspace_root_strip() {
    let idx = index(vec![summary("glob-skill", &[], &["src/*.rs"], false)]);
    let written = vec!["/ws/proj/src/main.rs".to_string()];
    // 绝对写路径剥去 workspace_root 后命中相对 pattern
    let signals = SkillMatchSignals {
        written_paths: &written,
        user_text: None,
        workspace_root: Some("/ws/proj"),
    };
    let hits = idx.match_skills_detailed(&signals);
    assert_eq!(hits.len(), 1);
    assert!(matches!(
        &hits[0].reason,
        SkillMatchReason::Path { pattern, .. } if pattern == "src/*.rs"
    ));
    // 无根可剥且原文不匹配相对 pattern → 不命中
    let signals = SkillMatchSignals {
        written_paths: &written,
        user_text: None,
        workspace_root: None,
    };
    assert!(idx.match_skills_detailed(&signals).is_empty());
}

#[test]
fn write_pivot_extension_matches_tag_with_language_alias() {
    let idx = index(vec![summary("rust-skill", &["rust"], &[], false)]);
    let written = vec!["/ws/proj/main.rs".to_string()];
    let signals = SkillMatchSignals {
        written_paths: &written,
        user_text: None,
        workspace_root: Some("/ws/proj"),
    };
    let hits = idx.match_skills_detailed(&signals);
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].reason,
        SkillMatchReason::Extension {
            written: "/ws/proj/main.rs".to_string(),
            tag: "rust".to_string()
        }
    );
}

#[test]
fn disabled_model_invocation_skills_never_match() {
    let idx = index(vec![summary("hidden-skill", &["rs"], &["src/*.rs"], true)]);
    assert!(idx
        .match_skills_detailed(&user_text_signals("look at rs files"))
        .is_empty());
    let written = vec!["src/main.rs".to_string()];
    let signals = SkillMatchSignals {
        written_paths: &written,
        user_text: None,
        workspace_root: None,
    };
    assert!(idx.match_skills_detailed(&signals).is_empty());
}

#[test]
fn empty_or_blank_signals_return_no_hits() {
    let idx = index(vec![summary("any-skill", &["rs"], &["src/*.rs"], false)]);
    let empty = SkillMatchSignals {
        written_paths: &[],
        user_text: None,
        workspace_root: None,
    };
    assert!(idx.match_skills_detailed(&empty).is_empty());
    // 纯空白用户文本视为无信号
    assert!(idx
        .match_skills_detailed(&user_text_signals("   "))
        .is_empty());
}
