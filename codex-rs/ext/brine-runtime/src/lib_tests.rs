use std::sync::Arc;

use codex_brine_runtime::InMemoryRuntimeAuthority;
use codex_extension_api::ExtensionRegistryBuilder;

use super::install;

#[test]
fn install_registers_only_lifecycle_attachment() {
    let mut builder = ExtensionRegistryBuilder::<()>::new();
    install(
        &mut builder,
        Arc::new(InMemoryRuntimeAuthority::default()),
        |_| None,
    );
    let registry = builder.build();
    assert_eq!(registry.thread_lifecycle_contributors().len(), 1);
    assert!(registry.context_contributors().is_empty());
}
