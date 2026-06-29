mod common;

use oas_store::MemoryStore;

#[test]
fn memory_store_conformance() {
    common::run_store_tests(MemoryStore::new);
}
