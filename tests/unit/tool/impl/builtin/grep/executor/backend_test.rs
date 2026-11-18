use super::*;

fn input(pattern: &str) -> GrepInput {
    GrepInput {
        pattern: pattern.to_string(),
        ..Default::default()
    }
}

/// `-e` must immediately precede the pattern so it is never reparsed as a flag.
fn pattern_is_e_guarded(args: &[String], pattern: &str) -> bool {
    args.windows(2).any(|w| w[0] == "-e" && w[1] == pattern)
}

fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
    args.windows(2).any(|w| w[0] == flag && w[1] == value)
}

#[test]
fn recursive_default_mode_excludes_vcs_dirs() {
    let args = GrepExecutor::build_grep_args(&input("TODO"), ".");

    assert!(args.contains(&"-P".to_string()), "PCRE engine selected");
    assert!(args.contains(&"-r".to_string()), "recursive on directory");
    assert!(
        args.contains(&"-l".to_string()),
        "files-with-matches default"
    );
    assert!(pattern_is_e_guarded(&args, "TODO"));
    assert_eq!(
        args.last().map(String::as_str),
        Some("."),
        "target is last arg"
    );
    for dir in VCS_DIRECTORIES_TO_EXCLUDE {
        assert!(
            args.contains(&format!("--exclude-dir={}", dir)),
            "VCS dir {dir} excluded"
        );
    }
    assert!(
        !args.contains(&"-n".to_string()),
        "no line numbers outside content mode"
    );
}

/// A single-file target must NOT recurse: the `-r`, `--exclude-dir`, and
/// `--include`/`--exclude` glob flags are recursive-only and would change
/// the meaning of a one-file search if they leaked through.
#[test]
fn single_file_target_is_not_recursive() {
    let mut grep_input = input("TODO");
    grep_input.glob = Some("*.py".to_string());
    let args = GrepExecutor::build_grep_args(&grep_input, "foo.py");

    assert!(
        !args.contains(&"-r".to_string()),
        "no recursion on a single file"
    );
    assert!(
        !args.iter().any(|a| a.starts_with("--exclude-dir=")),
        "no VCS exclusions on a single file"
    );
    assert!(
        !args.iter().any(|a| a.starts_with("--include=")),
        "globs are recursive-only"
    );
    assert_eq!(args.last().map(String::as_str), Some("foo.py"));
}

#[test]
fn content_mode_emits_line_numbers_and_context() {
    let mut grep_input = input("needle");
    grep_input.output_mode = Some(OutputMode::Content);
    grep_input.context = Some(3);
    let args = GrepExecutor::build_grep_args(&grep_input, ".");

    assert!(
        args.contains(&"-n".to_string()),
        "line numbers on by default"
    );
    assert!(
        has_pair(&args, "-C", "3"),
        "symmetric context passed through"
    );
    assert!(pattern_is_e_guarded(&args, "needle"));
}

#[test]
fn count_mode_uses_dash_c() {
    let mut grep_input = input("x");
    grep_input.output_mode = Some(OutputMode::Count);
    let args = GrepExecutor::build_grep_args(&grep_input, ".");

    assert!(args.contains(&"-c".to_string()));
    assert!(!args.contains(&"-l".to_string()));
    assert!(!args.contains(&"-n".to_string()));
}

#[test]
fn case_insensitive_adds_dash_i() {
    let mut grep_input = input("x");
    grep_input.case_insensitive = Some(true);
    let args = GrepExecutor::build_grep_args(&grep_input, ".");
    assert!(args.contains(&"-i".to_string()));
}

#[test]
fn glob_maps_to_include_and_exclude() {
    let mut grep_input = input("x");
    grep_input.glob = Some("*.py, !*_test.py".to_string());
    let args = GrepExecutor::build_grep_args(&grep_input, ".");

    assert!(args.contains(&"--include=*.py".to_string()));
    assert!(args.contains(&"--exclude=*_test.py".to_string()));
}

/// A pattern beginning with `-` must not be swallowed as a grep flag.
#[test]
fn leading_dash_pattern_is_guarded() {
    let args = GrepExecutor::build_grep_args(&input("-n"), ".");
    assert!(pattern_is_e_guarded(&args, "-n"));
}

/// Content mode caps results with `--max-count` (rg) / `-m` (grep fallback)
/// using one shared formula: `resolve_head_limit(head_limit) + offset + 1` —
/// the +1 sentinel lets callers tell "exactly head_limit results" and
/// "truncated" apart, and `offset` is included so paging works. `head_limit`
/// is silently remapped when omitted (→250) or out of range (0 or
/// > ABSOLUTE_HARD_CAP → 2000).
#[test]
fn content_mode_max_count_formula_is_shared_by_rg_and_grep() {
    // (head_limit, offset, expected cap)
    let cases: [(Option<u32>, Option<u32>, u32); 7] = [
        // Default head_limit=250 → +1 sentinel.
        (None, None, 251),
        // User-supplied `head_limit` is honored.
        (Some(50), None, 51),
        // `head_limit == 0` (unlimited) → silently capped at ABSOLUTE_HARD_CAP=2000.
        (Some(0), None, 2_001),
        // `head_limit > ABSOLUTE_HARD_CAP` → capped at ABSOLUTE_HARD_CAP.
        (Some(50_000), None, 2_001),
        // head_limit=50 + offset=30 + 1 sentinel = 81.
        (Some(50), Some(30), 81),
        // Default head_limit=250 + offset=30 → 281.
        (None, Some(30), 281),
        // head_limit=0 → 2000, plus offset=5000 → 7001.
        (Some(0), Some(5_000), 7_001),
    ];
    for (head_limit, offset, expected) in cases {
        let mut grep_input = input("needle");
        grep_input.output_mode = Some(OutputMode::Content);
        grep_input.head_limit = head_limit;
        grep_input.offset = offset;
        let expected = expected.to_string();

        let rg_args = GrepExecutor::build_rg_args(&grep_input, ".");
        assert!(
            has_pair(&rg_args, "--max-count", &expected),
            "rg: head_limit={head_limit:?} offset={offset:?} → --max-count {expected}"
        );

        // The grep fallback path gets `-m` (per-file cap) mirroring rg's
        // `--max-count`, with the same +1 sentinel semantics and the same
        // ABSOLUTE_HARD_CAP remap for head_limit=0 or above the cap.
        let grep_args = GrepExecutor::build_grep_args(&grep_input, ".");
        assert!(
            has_pair(&grep_args, "-m", &expected),
            "grep: head_limit={head_limit:?} offset={offset:?} → -m {expected}"
        );
    }
}

/// `--max-count` is only meaningful in content mode: count mode needs
/// accurate per-file counts (`--max-count` would corrupt them), and
/// FilesWithMatches lists the file after 1 match regardless.
#[test]
fn rg_non_content_modes_skip_max_count() {
    for mode in [OutputMode::Count, OutputMode::FilesWithMatches] {
        let mut grep_input = input("needle");
        grep_input.output_mode = Some(mode);
        let args = GrepExecutor::build_rg_args(&grep_input, ".");
        assert!(
            !args.iter().any(|a| a == "--max-count"),
            "{mode:?} must not emit --max-count"
        );
    }
}

/// `saturating_add` prevents overflow on huge `offset`.
#[test]
fn rg_content_mode_max_count_saturates_on_huge_offset() {
    let mut input = input("needle");
    input.output_mode = Some(OutputMode::Content);
    input.head_limit = Some(50);
    input.offset = Some(u32::MAX);
    let args = GrepExecutor::build_rg_args(&input, ".");
    // Should contain --max-count with some value (saturated, not
    // wrapped to a small number). Just verify it's present and the
    // value is >= head_limit + 1.
    let max_count = args
        .windows(2)
        .find(|w| w[0] == "--max-count")
        .map(|w| w[1].parse::<u64>().unwrap_or(0))
        .expect("--max-count must be present");
    assert!(
        max_count >= 51,
        "saturated --max-count must be >= head_limit+1=51, got {max_count}"
    );
}

/// Collector accepts `head_limit + 1` lines then returns `false`.
#[test]
fn bounded_collector_stops_at_cap() {
    // head_limit=3 → max_keep = 3 + 1 = 4
    let collector = BoundedLineCollector::for_head_limit(3, 0);
    assert!(collector.on_line("a")); // 1 kept
    assert!(collector.on_line("b")); // 2 kept
    assert!(collector.on_line("c")); // 3 kept (== head_limit)
    assert!(collector.on_line("d")); // 4 kept (the +1 sentinel)
    assert!(!collector.on_line("e")); // cap reached — ask to stop
    let lines = collector.into_lines();
    assert_eq!(lines, vec!["a", "b", "c", "d"]);
}

/// `resolve_head_limit`: omitted→250, 0→2000, >cap→2000, at-cap→pass.
#[test]
fn resolve_head_limit_handles_all_branches() {
    // Omitted (None) → DEFAULT_HEAD_LIMIT = 250
    let mut input = input("x");
    assert_eq!(resolve_head_limit(&input), 250);
    // Explicit 0 → ABSOLUTE_HARD_CAP = 2000
    input.head_limit = Some(0);
    assert_eq!(resolve_head_limit(&input), 2_000);
    // Above cap → ABSOLUTE_HARD_CAP
    input.head_limit = Some(99_999);
    assert_eq!(resolve_head_limit(&input), 2_000);
    // Exactly at cap → pass through (boundary)
    input.head_limit = Some(2_000);
    assert_eq!(resolve_head_limit(&input), 2_000);
    // Just below cap → pass through
    input.head_limit = Some(1_999);
    assert_eq!(resolve_head_limit(&input), 1_999);
    // Small finite value → pass through
    input.head_limit = Some(50);
    assert_eq!(resolve_head_limit(&input), 50);
}

/// `files_with_matches_truncated` (head_limit, total_matches, want_k,
/// collector_capped):
/// - simple check: `total_matches > want_k`;
/// - regression: `collector_capped` must OR into truncation when
///   `want_k > max_keep` (so `total_matches` alone misses it);
/// - `head_limit == 0` (unlimited) → never truncated.
#[test]
fn files_with_matches_truncated_covers_all_signals() {
    // (head_limit, total_matches, want_k, collector_capped, expected)
    let cases: [(u32, usize, usize, bool, bool); 8] = [
        // head_limit=250, offset=0 → want_k=250. 300 stat'd matches.
        // 300 > 250 → truncated (collector_capped irrelevant here).
        (250, 300, 250, false, true),
        // Exactly at want_k → not truncated (all fit in heap).
        (250, 250, 250, false, false),
        // Below want_k → not truncated.
        (250, 100, 250, false, false),
        // want_k=2250 > total_matches=2001, collector capped → truncated.
        (250, 2_001, 2_250, true, true),
        // Same scenario but collector did NOT cap (actual == 2001 exactly,
        // stream ended naturally) → NOT truncated (the user sees all 2001
        // files, gets 1 after offset, but that's all there is — no more
        // matches exist).
        (250, 2_001, 2_250, false, false),
        // Sanity: collector capped with small want_k → truncated via the
        // simple check (total_matches > want_k already true).
        (250, 2_001, 250, true, true),
        // `head_limit == 0` (unlimited) → never truncated.
        (0, 5_000, 5_000, true, false),
        (0, 2_001, 2_001, true, false),
    ];
    for (head_limit, total, want_k, capped, expected) in cases {
        assert_eq!(
            GrepExecutor::files_with_matches_truncated(head_limit, total, want_k, capped),
            expected,
            "head_limit={head_limit} total_matches={total} want_k={want_k} capped={capped}"
        );
    }
}

/// Collector does NOT pre-skip `offset` at the stream level —
/// `apply_head_limit` does `skip(offset).take(limit)` downstream.
#[test]
fn bounded_collector_includes_offset_in_max_keep_no_stream_skip() {
    // head_limit=2, offset=3 → max_keep = 2 + 3 + 1 = 6, skip=0.
    // The collector keeps ALL 6 lines (no stream-level skip);
    // `apply_head_limit` will skip 3 and take 2 downstream.
    let collector = BoundedLineCollector::for_head_limit(2, 3);
    // First 3 lines are KEPT (not dropped) — the collector does
    // not pre-skip; downstream `apply_head_limit` handles offset.
    assert!(collector.on_line("drop1")); // kept[0]
    assert!(collector.on_line("drop2")); // kept[1]
    assert!(collector.on_line("drop3")); // kept[2]
    assert!(collector.on_line("keep1")); // kept[3]
    assert!(collector.on_line("keep2")); // kept[4]
    assert!(collector.on_line("keep3")); // kept[5] = max_keep
                                         // 7th line triggers stop — cap reached.
    assert!(!collector.on_line("keep4-overflow"));
    let lines = collector.into_lines();
    assert_eq!(
        lines,
        vec!["drop1", "drop2", "drop3", "keep1", "keep2", "keep3"]
    );
}

/// Regression: collector + `apply_head_limit` must not double-skip
/// `offset`. head_limit=3, offset=4 → correct window is [L4,L5,L6].
#[test]
fn collector_plus_apply_head_limit_no_double_skip_for_offset() {
    let head_limit = 3u32;
    let offset = 4u32;
    let collector = BoundedLineCollector::for_head_limit(head_limit, offset);
    // rg emits 8 lines (L0..L7) — more than head_limit+offset+1=8,
    // so the collector fills to max_keep and stops early.
    for i in 0..8 {
        assert!(collector.on_line(&format!("L{i}")));
    }
    // 9th line would be rejected (max_keep reached).
    assert!(!collector.on_line("L8-overflow"));
    let lines = collector.into_lines();
    assert_eq!(
        lines.len(),
        8,
        "collector keeps head_limit+offset+1=8 lines"
    );

    // `apply_head_limit` skips offset=4, takes head_limit=3.
    let (limited, applied_limit) = GrepExecutor::apply_head_limit(lines, head_limit, offset);
    assert_eq!(
        limited,
        vec!["L4".to_string(), "L5".to_string(), "L6".to_string()],
        "double-skip bug would return empty here; correct window is [L4,L5,L6]"
    );
    // 8 items - 4 skipped = 4 remaining > head_limit=3 → truncated.
    assert_eq!(applied_limit, Some(head_limit), "truncation flag must fire");
}

/// `\r` is stripped from each line.
#[test]
fn bounded_collector_strips_carriage_returns() {
    let collector = BoundedLineCollector::for_head_limit(2, 0);
    assert!(collector.on_line("hello\r"));
    assert!(collector.on_line("world\r"));
    let lines = collector.into_lines();
    assert_eq!(lines, vec!["hello", "world"]);
}

/// FilesWithMatches widens cap to `ABSOLUTE_HARD_CAP + 1` so the
/// downstream mtime top-K sort sees the full match set.
#[test]
fn for_rg_mode_files_with_matches_uses_wide_cap() {
    // head_limit=250 (default), offset=0, FilesWithMatches →
    // collector should accept up to ABSOLUTE_HARD_CAP + 1 = 2001
    // lines, not head_limit + 1 = 251.
    let collector = BoundedLineCollector::for_rg_mode(250, 0, OutputMode::FilesWithMatches);
    for i in 0..2_001 {
        assert!(
            collector.on_line(&format!("file-{i}")),
            "line {} should be accepted before hitting the wide cap",
            i
        );
    }
    assert!(
        !collector.on_line("file-2001-overflow"),
        "line 2002 must be rejected — ABSOLUTE_HARD_CAP+1 cap reached"
    );
    let lines = collector.into_lines();
    assert_eq!(lines.len(), 2_001);
}

/// FilesWithMatches doesn't pre-skip `offset` — downstream top-K
/// + `.skip(offset)` handles pagination over the full match set.
#[test]
fn for_rg_mode_files_with_matches_skips_no_offset() {
    // head_limit=50, offset=100, FilesWithMatches → collector keeps
    // from line 1 (not line 101), since downstream top-K + .skip(100)
    // handles pagination.
    let collector = BoundedLineCollector::for_rg_mode(50, 100, OutputMode::FilesWithMatches);
    // First 100 lines are kept (not dropped), proving skip=0.
    for i in 0..100 {
        assert!(
            collector.on_line(&format!("file-{i}")),
            "line {} must be kept (skip=0 for FilesWithMatches)",
            i
        );
    }
    let lines = collector.into_lines();
    assert_eq!(lines.len(), 100);
    assert_eq!(lines[0], "file-0");
}

/// Content/Count modes: `max_keep = head_limit + offset + 1`, no stream skip.
#[test]
fn for_rg_mode_content_uses_tight_cap_with_offset() {
    let collector = BoundedLineCollector::for_rg_mode(3, 2, OutputMode::Content);
    assert!(collector.on_line("L0"));
    assert!(collector.on_line("L1"));
    assert!(collector.on_line("L2"));
    assert!(collector.on_line("L3"));
    assert!(collector.on_line("L4"));
    assert!(collector.on_line("L5"));
    assert!(!collector.on_line("L6-overflow"));
    let lines = collector.into_lines();
    assert_eq!(lines, vec!["L0", "L1", "L2", "L3", "L4", "L5"]);
}
