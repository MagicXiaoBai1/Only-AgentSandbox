mod common;

use oas_store::RedbStore;

#[test]
fn redb_store_conformance() {
    // 每次调用建一个独立内存 redb，保证用例间隔离。
    common::run_store_tests(|| RedbStore::open_in_memory().unwrap());
}
