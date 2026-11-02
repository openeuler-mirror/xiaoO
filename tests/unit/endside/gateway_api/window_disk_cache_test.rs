use self::test_support::{corrupt_disk_at, disk_at, disk_with_entry};
use super::*;

/// 单测专用句柄构造器:字段 `path` 私有,兄弟子树的测试无法直接构造,
/// 只能经这两个 pub(crate) 构造器拿隔离句柄(runtime_request 的测试也调用)。
impl WindowDiskCache {
    /// 关闭磁盘层(单测隔离:不读不写真实缓存目录)。
    pub(crate) fn disabled() -> Self {
        Self { path: None }
    }

    /// 指定缓存文件路径(单测注入临时目录)。
    pub(crate) fn at(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }
}

/// 本测试模块与 `runtime_request` 的测试共用的夹具。夹具要构造带私有字段
/// 的句柄,只能留在 window_disk_cache 子树内(即本文件);跨子树引用走
/// `crate::gateway_api::window_disk_cache::tests::test_support`。
pub(crate) mod test_support {
    use super::super::{
        PersistedWindowCache, PersistedWindowEntry, WindowDiskCache, CACHE_FILE_NAME,
    };

    /// 构造指向给定临时目录下缓存文件的句柄。
    pub(crate) fn disk_at(home: &tempfile::TempDir) -> WindowDiskCache {
        WindowDiskCache::at(home.path().join(CACHE_FILE_NAME))
    }

    /// 构造带单条手工条目的句柄:`age` 为条目距今的秒数,负值表示超前
    /// （时钟篡改/抖动用例）。绕过 `store` 的时间戳,用于 TTL 过期与篡改用例。
    pub(crate) fn disk_with_entry(
        home: &tempfile::TempDir,
        key: &str,
        value: Option<u64>,
        age_secs: i64,
    ) -> WindowDiskCache {
        let now = super::super::unix_now();
        let resolved_at = if age_secs < 0 {
            now.saturating_add(age_secs.unsigned_abs())
        } else {
            now.saturating_sub(age_secs.unsigned_abs())
        };
        write_persisted(
            &home.path().join(CACHE_FILE_NAME),
            key,
            PersistedWindowEntry { value, resolved_at },
        );
        disk_at(home)
    }

    /// 构造指向损坏缓存文件的句柄（损坏落穿/修复用例共用）。
    pub(crate) fn corrupt_disk_at(home: &tempfile::TempDir) -> WindowDiskCache {
        let path = home.path().join(CACHE_FILE_NAME);
        std::fs::write(&path, "{ not json").unwrap();
        WindowDiskCache::at(path)
    }

    /// 把手工构造的条目直接写成缓存文件——绕过 `store` 的时间戳。
    fn write_persisted(path: &std::path::Path, key: &str, entry: PersistedWindowEntry) {
        let mut cache = PersistedWindowCache::default();
        cache.entries.insert(key.to_string(), entry);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_string(&cache).unwrap()).unwrap();
    }
}

#[test]
fn store_then_load_roundtrips_positive_value() {
    let home = tempfile::tempdir().unwrap();
    let disk = disk_at(&home);
    let key = "zhipu|glm-5.2|https://api.example.com/v1";

    disk.store(key, Some(200_000));

    assert_eq!(disk.load(key), Some(Some(200_000)));
}

#[test]
fn store_then_load_roundtrips_negative_value() {
    let home = tempfile::tempdir().unwrap();
    let disk = disk_at(&home);
    let key = "ollama|glm-5.2|";

    disk.store(key, None);

    assert_eq!(disk.load(key), Some(None));
}

#[test]
fn load_ignores_corrupt_file() {
    let home = tempfile::tempdir().unwrap();
    let disk = corrupt_disk_at(&home);

    assert_eq!(disk.load("any|key"), None);
}

#[test]
fn load_rejects_zero_window_entry() {
    let home = tempfile::tempdir().unwrap();
    let key = "zhipu|glm-5.2";
    // 探测链只会产出 > 0 的窗口,0 只能来自被篡改的文件。
    let disk = disk_with_entry(&home, key, Some(0), 0);

    assert_eq!(disk.load(key), None);
}

#[test]
fn disabled_cache_neither_loads_nor_stores() {
    let home = tempfile::tempdir().unwrap();
    let disk = WindowDiskCache::disabled();
    let key = "zhipu|glm-5.2";

    disk.store(key, Some(200_000));
    assert_eq!(disk.load(key), None);
    // 关闭的磁盘层不得在缓存目录里留下任何文件。
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

#[test]
fn positive_entry_expires_after_positive_ttl() {
    let home = tempfile::tempdir().unwrap();
    let key = "zhipu|glm-5.2";
    let age = POSITIVE_TTL.as_secs() as i64 + 3600;
    let disk = disk_with_entry(&home, key, Some(200_000), age);

    assert_eq!(disk.load(key), None);
}

#[test]
fn negative_entry_expires_after_negative_ttl() {
    let home = tempfile::tempdir().unwrap();
    let key = "zhipu|glm-5.2";
    let age = NEGATIVE_TTL.as_secs() as i64 + 3600;
    let disk = disk_with_entry(&home, key, None, age);

    assert_eq!(disk.load(key), None);
}

#[test]
fn load_rejects_far_future_timestamp() {
    let home = tempfile::tempdir().unwrap();
    let key = "zhipu|glm-5.2";
    let age = POSITIVE_TTL.as_secs() as i64 + 3600;
    // 大幅超前的时间戳若不拒绝,条目在真实时间里永不过期。
    let disk = disk_with_entry(&home, key, Some(200_000), -age);

    assert_eq!(disk.load(key), None);
}

#[test]
fn slightly_future_timestamp_stays_fresh() {
    let home = tempfile::tempdir().unwrap();
    let key = "zhipu|glm-5.2";
    // 进程间的小幅时钟抖动不应当作过期。
    let disk = disk_with_entry(&home, key, Some(200_000), -3600);

    assert_eq!(disk.load(key), Some(Some(200_000)));
}

#[test]
fn store_preserves_entries_for_other_keys() {
    let home = tempfile::tempdir().unwrap();
    let disk = disk_at(&home);
    let first = "zhipu|glm-5.2";
    let second = "zhipu|glm-5.3";

    disk.store(first, Some(200_000));
    disk.store(second, None);

    assert_eq!(disk.load(first), Some(Some(200_000)));
    assert_eq!(disk.load(second), Some(None));
}

#[test]
fn store_leaves_no_temporary_file_behind() {
    let home = tempfile::tempdir().unwrap();
    let disk = disk_at(&home);

    disk.store("zhipu|glm-5.2", Some(200_000));

    // temp+rename 完成后目录里只应有缓存文件本身,不留 *.tmp 残骸。
    let entries: Vec<_> = std::fs::read_dir(home.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from(CACHE_FILE_NAME)]);
}
