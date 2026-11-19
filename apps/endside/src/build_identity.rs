pub(crate) const VERSION: &str = env!("XIAOO_VERSION");
pub(crate) const COMMIT: &str = env!("XIAOO_COMMIT");

pub(crate) fn print_version(verbose: bool) {
    if verbose {
        println!("xiaoo {VERSION} (commit {COMMIT})");
    } else {
        println!("{VERSION}");
    }
}
