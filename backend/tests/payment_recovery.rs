use food_ordering_runtime::{
    orders::OrderStatus,
    payments::{CaptureEffect, capture_effect},
};

#[test]
fn late_capture_for_a_cancelled_order_requires_a_refund_without_reopening_it() {
    assert_eq!(
        capture_effect(OrderStatus::Cancelled),
        CaptureEffect::RefundOnly,
    );
}

#[test]
fn capture_only_marks_a_placed_order_paid() {
    assert_eq!(capture_effect(OrderStatus::Placed), CaptureEffect::MarkPaid);
    assert_eq!(capture_effect(OrderStatus::Paid), CaptureEffect::Noop);
    assert_eq!(capture_effect(OrderStatus::Completed), CaptureEffect::Noop);
}
