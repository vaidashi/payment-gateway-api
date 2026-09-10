use serde::Serialize;

pub const MAX_TOTAL_CENTS: i64 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub subtotal_cents: i64,
    pub tax_cents: i64,
    pub service_fee_cents: i64,
    pub total_cents: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyError {
    NegativeRate,
    FeeAboveCap,
    Overflow,
    TotalTooLarge,
}

/// Half-up percentage rounding for non-negative cent amounts and basis points.
pub fn percentage_cents(amount: i64, basis_points: i32) -> Result<i64, MoneyError> {
    if amount < 0 || basis_points < 0 {
        return Err(MoneyError::NegativeRate);
    }
    let product = (amount as i128)
        .checked_mul(basis_points as i128)
        .ok_or(MoneyError::Overflow)?;
    let rounded = product.checked_add(5_000).ok_or(MoneyError::Overflow)? / 10_000;
    i64::try_from(rounded).map_err(|_| MoneyError::Overflow)
}

pub fn totals(
    subtotal_cents: i64,
    tax_basis_points: i32,
    fee_basis_points: Option<i32>,
) -> Result<Totals, MoneyError> {
    let fee_rate = fee_basis_points.unwrap_or(0);
    if fee_rate > 300 {
        return Err(MoneyError::FeeAboveCap);
    }
    let tax_cents = percentage_cents(subtotal_cents, tax_basis_points)?;
    let service_fee_cents = percentage_cents(subtotal_cents, fee_rate)?;
    let total_cents = subtotal_cents
        .checked_add(tax_cents)
        .and_then(|v| v.checked_add(service_fee_cents))
        .ok_or(MoneyError::Overflow)?;
    if total_cents > MAX_TOTAL_CENTS {
        return Err(MoneyError::TotalTooLarge);
    }
    Ok(Totals {
        subtotal_cents,
        tax_cents,
        service_fee_cents,
        total_cents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_percentages_are_not_compounded() {
        assert_eq!(
            totals(2000, 800, Some(300)).unwrap(),
            Totals {
                subtotal_cents: 2000,
                tax_cents: 160,
                service_fee_cents: 60,
                total_cents: 2220
            }
        );
    }
    #[test]
    fn half_cent_ties_round_up() {
        assert_eq!(percentage_cents(1, 5000), Ok(1));
    }
    #[test]
    fn null_fee_is_zero_and_cap_is_inclusive() {
        assert_eq!(totals(2000, 0, None).unwrap().total_cents, 2000);
        assert!(totals(2000, 0, Some(300)).is_ok());
    }
    #[test]
    fn rejects_invalid_rates_and_large_totals() {
        assert_eq!(totals(1, -1, None), Err(MoneyError::NegativeRate));
        assert_eq!(totals(1, 0, Some(301)), Err(MoneyError::FeeAboveCap));
        assert_eq!(
            totals(MAX_TOTAL_CENTS, 1, None),
            Err(MoneyError::TotalTooLarge)
        );
    }
}
