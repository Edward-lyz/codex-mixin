use std::cmp::Ordering;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Shared ordering for discovery, CLI output and client catalogs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelOrder {
    #[default]
    Family,
    Manual,
}

impl ModelOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Family => "family",
            Self::Manual => "manual",
        }
    }

    pub fn apply(self, models: &mut [String]) {
        if self == Self::Family {
            models.sort_by(|left, right| compare_model_ids(left, right));
        }
    }
}

impl FromStr for ModelOrder {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "family" => Ok(Self::Family),
            "manual" => Ok(Self::Manual),
            _ => anyhow::bail!("model order must be family or manual"),
        }
    }
}

/// Does not allocate keys or parse versions into bounded integers.
pub fn compare_model_ids(left: &str, right: &str) -> Ordering {
    left.eq_ignore_ascii_case("auto")
        .cmp(&right.eq_ignore_ascii_case("auto"))
        .then_with(|| compare_ascii(family(left), family(right)))
        .then_with(|| compare_versions(right, left))
        .then_with(|| tier(left).cmp(&tier(right)))
        .then_with(|| compare_ascii(left, right))
        .then_with(|| left.cmp(right))
}

fn words(id: &str) -> impl Iterator<Item = &str> {
    id.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
}

fn family(id: &str) -> &str {
    let first = words(id).next().unwrap_or(id);
    if ["claude", "opus", "sonnet", "haiku"]
        .iter()
        .any(|alias| first.eq_ignore_ascii_case(alias))
    {
        return "claude";
    }
    first.trim_end_matches(|ch: char| ch.is_ascii_digit())
}

fn tier(id: &str) -> u8 {
    words(id)
        .find_map(|word| {
            ["opus", "sonnet", "haiku"]
                .iter()
                .position(|tier| word.eq_ignore_ascii_case(tier))
        })
        .map_or(3, |index| index as u8)
}

fn compare_ascii(left: &str, right: &str) -> Ordering {
    left.bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .cmp(right.bytes().map(|byte| byte.to_ascii_lowercase()))
}

fn version_numbers(id: &str) -> impl Iterator<Item = &str> {
    id.split(|ch: char| !ch.is_ascii_digit())
        .filter(|number| !number.is_empty())
}

fn compare_versions(left: &str, right: &str) -> Ordering {
    let mut left = version_numbers(left);
    let mut right = version_numbers(right);
    loop {
        match (left.next(), right.next()) {
            (Some(left), Some(right)) => {
                let left = left.trim_start_matches('0');
                let right = right.trim_start_matches('0');
                let order = left.len().cmp(&right.len()).then_with(|| left.cmp(right));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (left, right) => return left.is_some().cmp(&right.is_some()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_numeric_descending_and_aliases_share_a_family() {
        let mut models = [
            "auto",
            "GLM-5.3",
            "Opus 4.8",
            "Claude Haiku 5.5",
            "Claude Sonnet 5.5",
            "Opus 5.5",
            "Opus 5",
            "gpt-6.9-sol",
            "gpt-6.10-sol",
        ]
        .map(str::to_owned);
        ModelOrder::Family.apply(&mut models);
        assert_eq!(
            models,
            [
                "Opus 5.5",
                "Claude Sonnet 5.5",
                "Claude Haiku 5.5",
                "Opus 5",
                "Opus 4.8",
                "GLM-5.3",
                "gpt-6.10-sol",
                "gpt-6.9-sol",
                "auto"
            ]
        );
    }

    #[test]
    fn manual_order_and_large_version_numbers_are_preserved() {
        let mut models = ["gpt-9", "gpt-100000000000000000000"].map(str::to_owned);
        ModelOrder::Manual.apply(&mut models);
        assert_eq!(models[0], "gpt-9");
        ModelOrder::Family.apply(&mut models);
        assert_eq!(models[0], "gpt-100000000000000000000");
    }
}
