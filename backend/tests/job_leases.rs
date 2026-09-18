use food_ordering_runtime::jobs::can_acknowledge;
use uuid::Uuid;

#[test]
fn an_expired_lease_owner_cannot_acknowledge_a_new_claim() {
    assert!(!can_acknowledge(Uuid::new_v4(), Uuid::new_v4()));
}
