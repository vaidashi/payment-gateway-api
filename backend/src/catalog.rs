use crate::money::{self, MoneyError, Totals};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuoteRequest {
    pub restaurant_id: String,
    pub lines: Vec<QuoteLine>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuoteLine {
    pub item_id: String,
    pub quantity: i32,
    #[serde(default)]
    pub selections: Vec<String>,
    #[serde(default)]
    pub extras: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct Item {
    pub id: String,
    pub restaurant_id: String,
    pub price_cents: i64,
    pub available: bool,
    pub groups: Vec<Group>,
}
#[derive(Debug, Clone)]
pub struct Group {
    pub required: bool,
    pub options: Vec<OptionDef>,
}
#[derive(Debug, Clone)]
pub struct OptionDef {
    pub id: String,
    pub adjustment_cents: i64,
}
#[derive(Debug, Serialize)]
pub struct Quote {
    pub totals: Totals,
    pub quote_digest: String,
    pub configuration_version: i64,
}
#[derive(Debug, PartialEq, Eq)]
pub enum QuoteError {
    EmptyCart,
    TooManyLines,
    InvalidQuantity,
    ForeignItem,
    Unavailable,
    MissingRequiredChoice,
    InvalidChoice,
    DuplicateExtra,
    TooManyExtras,
    Money(MoneyError),
}
impl From<MoneyError> for QuoteError {
    fn from(value: MoneyError) -> Self {
        Self::Money(value)
    }
}

pub fn quote(
    request: &QuoteRequest,
    items: &[Item],
    tax_bp: i32,
    fee_bp: Option<i32>,
    configuration_version: i64,
) -> Result<Quote, QuoteError> {
    if request.lines.is_empty() {
        return Err(QuoteError::EmptyCart);
    }
    if request.lines.len() > 50 {
        return Err(QuoteError::TooManyLines);
    }
    let index: HashMap<_, _> = items.iter().map(|item| (item.id.as_str(), item)).collect();
    let mut subtotal = 0_i64;
    for line in &request.lines {
        if !(1..=99).contains(&line.quantity) {
            return Err(QuoteError::InvalidQuantity);
        }
        if line.extras.len() > 20 {
            return Err(QuoteError::TooManyExtras);
        }
        let item = index
            .get(line.item_id.as_str())
            .ok_or(QuoteError::ForeignItem)?;
        if item.restaurant_id != request.restaurant_id {
            return Err(QuoteError::ForeignItem);
        }
        if !item.available {
            return Err(QuoteError::Unavailable);
        }
        let choices: HashSet<_> = line.selections.iter().collect();
        let extras: HashSet<_> = line.extras.iter().collect();
        if extras.len() != line.extras.len() {
            return Err(QuoteError::DuplicateExtra);
        }
        let mut unit = item.price_cents;
        let all_options: HashSet<_> = item
            .groups
            .iter()
            .flat_map(|group| group.options.iter().map(|option| option.id.as_str()))
            .collect();
        if choices
            .iter()
            .any(|choice| !all_options.contains(choice.as_str()))
        {
            return Err(QuoteError::InvalidChoice);
        }
        for group in &item.groups {
            let selected: Vec<_> = group
                .options
                .iter()
                .filter(|o| choices.contains(&o.id))
                .collect();
            if group.required && selected.len() != 1 {
                return Err(QuoteError::MissingRequiredChoice);
            }
            if selected.len() > 1 {
                return Err(QuoteError::InvalidChoice);
            }
            for option in selected {
                unit = unit
                    .checked_add(option.adjustment_cents)
                    .ok_or(MoneyError::Overflow)?;
            }
        }
        for id in &line.extras {
            let option = item
                .groups
                .iter()
                .filter(|group| !group.required)
                .flat_map(|g| &g.options)
                .find(|o| &o.id == id)
                .ok_or(QuoteError::InvalidChoice)?;
            unit = unit
                .checked_add(option.adjustment_cents)
                .ok_or(MoneyError::Overflow)?;
        }
        subtotal = subtotal
            .checked_add(
                unit.checked_mul(line.quantity as i64)
                    .ok_or(MoneyError::Overflow)?,
            )
            .ok_or(MoneyError::Overflow)?;
    }
    let totals = money::totals(subtotal, tax_bp, fee_bp)?;
    let canonical = serde_json::to_vec(&(
        request.restaurant_id.as_str(),
        &request.lines,
        &totals,
        configuration_version,
    ))
    .expect("serializable quote");
    Ok(Quote {
        totals,
        quote_digest: format!("{:x}", Sha256::digest(canonical)),
        configuration_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item() -> Item {
        Item {
            id: "item".into(),
            restaurant_id: "restaurant".into(),
            price_cents: 1000,
            available: true,
            groups: vec![
                Group {
                    required: true,
                    options: vec![OptionDef {
                        id: "size".into(),
                        adjustment_cents: 0,
                    }],
                },
                Group {
                    required: false,
                    options: vec![OptionDef {
                        id: "extra".into(),
                        adjustment_cents: 50,
                    }],
                },
            ],
        }
    }
    fn request() -> QuoteRequest {
        QuoteRequest {
            restaurant_id: "restaurant".into(),
            lines: vec![QuoteLine {
                item_id: "item".into(),
                quantity: 2,
                selections: vec!["size".into()],
                extras: vec!["extra".into()],
            }],
        }
    }
    #[test]
    fn prices_required_choice_and_distinct_extra() {
        let quote = quote(&request(), &[item()], 800, Some(300), 1).unwrap();
        assert_eq!(quote.totals.subtotal_cents, 2100);
        assert_eq!(quote.totals.tax_cents, 168);
        assert_eq!(quote.totals.service_fee_cents, 63);
    }
    #[test]
    fn rejects_required_duplicate_and_foreign_inputs() {
        let mut missing = request();
        missing.lines[0].selections.clear();
        assert!(matches!(
            quote(&missing, &[item()], 0, None, 1),
            Err(QuoteError::MissingRequiredChoice)
        ));
        let mut duplicate = request();
        duplicate.lines[0].extras.push("extra".into());
        assert!(matches!(
            quote(&duplicate, &[item()], 0, None, 1),
            Err(QuoteError::DuplicateExtra)
        ));
        let mut foreign = request();
        foreign.restaurant_id = "other".into();
        assert!(matches!(
            quote(&foreign, &[item()], 0, None, 1),
            Err(QuoteError::ForeignItem)
        ));
    }
    #[test]
    fn rejects_invalid_quantities_and_unavailable_items() {
        let mut invalid = request();
        invalid.lines[0].quantity = 0;
        assert!(matches!(
            quote(&invalid, &[item()], 0, None, 1),
            Err(QuoteError::InvalidQuantity)
        ));
        let mut unavailable = item();
        unavailable.available = false;
        assert!(matches!(
            quote(&request(), &[unavailable], 0, None, 1),
            Err(QuoteError::Unavailable)
        ));
    }
}
