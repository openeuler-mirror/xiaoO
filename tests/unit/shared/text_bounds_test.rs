use super::{floor_char_boundary, truncate_to_char_boundary};

#[test]
fn floor_is_identity_on_ascii() {
    for index in 0..="hello world".len() {
        assert_eq!(floor_char_boundary("hello world", index), index);
    }
}

#[test]
fn floor_clamps_to_len_past_the_end() {
    assert_eq!(floor_char_boundary("修复", 99), 6);
    assert_eq!(floor_char_boundary("", 3), 0);
}

#[test]
fn floor_walks_back_out_of_a_multibyte_char() {
    // "修复" = 6 bytes, each char is 3 bytes.
    assert_eq!(floor_char_boundary("修复", 0), 0);
    assert_eq!(floor_char_boundary("修复", 1), 0);
    assert_eq!(floor_char_boundary("修复", 2), 0);
    assert_eq!(floor_char_boundary("修复", 3), 3);
    assert_eq!(floor_char_boundary("修复", 4), 3);
    assert_eq!(floor_char_boundary("修复", 5), 3);
    assert_eq!(floor_char_boundary("修复", 6), 6);
}

#[test]
fn floor_handles_four_byte_chars() {
    // "😀" = 4 bytes.
    assert_eq!(floor_char_boundary("😀ab", 1), 0);
    assert_eq!(floor_char_boundary("😀ab", 2), 0);
    assert_eq!(floor_char_boundary("😀ab", 3), 0);
    assert_eq!(floor_char_boundary("😀ab", 4), 4);
    assert_eq!(floor_char_boundary("😀ab", 5), 5);
}

#[test]
fn truncate_keeps_ascii_prefix_semantics() {
    assert_eq!(truncate_to_char_boundary("hello world", 3), "hel");
    assert_eq!(truncate_to_char_boundary("hello", 5), "hello");
    assert_eq!(truncate_to_char_boundary("hello", 99), "hello");
    assert_eq!(truncate_to_char_boundary("hello", 0), "");
}

#[test]
fn truncate_stops_before_a_split_character() {
    // Cutting "修复参数错误" at 4 bytes would split '复' (bytes 3..6).
    assert_eq!(truncate_to_char_boundary("修复参数错误", 4), "修");
    assert_eq!(truncate_to_char_boundary("修复参数错误", 5), "修");
    assert_eq!(truncate_to_char_boundary("修复参数错误", 6), "修复");
    assert_eq!(truncate_to_char_boundary("😀ab", 2), "");
    assert_eq!(truncate_to_char_boundary("😀ab", 5), "😀a");
}

/// The regression guard: before this helper existed, every byte index in
/// `0..=len` was a potential panic point once the text contained multi-byte
/// characters.
#[test]
fn every_byte_index_is_a_safe_cut_point() {
    let samples = [
        "修复参数错误并验证边界处理",
        "mixed 中英文 ascii and 汉字",
        "😀 emoji 与中文 🚀",
        "",
        "plain ascii only",
    ];
    for sample in samples {
        for index in 0..=sample.len() + 3 {
            let cut = floor_char_boundary(sample, index);
            assert!(cut <= sample.len());
            assert!(sample.is_char_boundary(cut), "cut {cut} not on boundary");
            let prefix = truncate_to_char_boundary(sample, index);
            assert!(prefix.len() <= index.min(sample.len()));
            assert!(sample.starts_with(prefix));
        }
    }
}
