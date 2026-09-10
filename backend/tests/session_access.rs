use food_ordering_runtime::auth::token_hash;

#[test]
fn opaque_session_storage_uses_a_non_reversible_hash() {
    assert_ne!(token_hash("forged-session"), "forged-session");
    assert_eq!(token_hash("same"), token_hash("same"));
}
