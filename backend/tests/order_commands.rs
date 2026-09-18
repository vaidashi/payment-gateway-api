use food_ordering_runtime::orders::{OrderStatus, TransitionError, next_status};

#[test]
fn lifecycle_allows_only_the_next_fulfillment_state() {
    assert_eq!(
        next_status(OrderStatus::Paid, OrderStatus::InProgress),
        Ok(OrderStatus::InProgress)
    );
    assert_eq!(
        next_status(OrderStatus::InProgress, OrderStatus::Ready),
        Ok(OrderStatus::Ready)
    );
    assert_eq!(
        next_status(OrderStatus::Ready, OrderStatus::Completed),
        Ok(OrderStatus::Completed)
    );
    assert_eq!(
        next_status(OrderStatus::Placed, OrderStatus::Paid),
        Err(TransitionError::PaymentIntegrationOnly)
    );
    assert_eq!(
        next_status(OrderStatus::Paid, OrderStatus::Ready),
        Err(TransitionError::InvalidTransition)
    );
}

#[test]
fn cancellation_closes_only_unstarted_orders() {
    assert_eq!(
        next_status(OrderStatus::Placed, OrderStatus::Cancelled),
        Ok(OrderStatus::Cancelled)
    );
    assert_eq!(
        next_status(OrderStatus::Paid, OrderStatus::Cancelled),
        Ok(OrderStatus::Cancelled)
    );
    assert_eq!(
        next_status(OrderStatus::InProgress, OrderStatus::Cancelled),
        Err(TransitionError::InvalidTransition)
    );
}
