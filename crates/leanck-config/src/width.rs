use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{fmt, str::FromStr};

/// A normalization line limit, or no limit on line length.
///
/// TOML and JSON represent finite widths as positive integers and unlimited
/// width as the string `"unlimited"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalizeWidth {
    Finite(usize),
    Unlimited,
}

impl NormalizeWidth {
    /// Whether the width is valid for normalization.
    pub fn is_valid(self) -> bool {
        self != Self::Finite(0)
    }

    /// Whether a line of this many characters fits within the limit.
    pub fn fits(self, characters: usize) -> bool {
        match self {
            Self::Finite(limit) => characters <= limit,
            Self::Unlimited => true,
        }
    }
}

impl From<usize> for NormalizeWidth {
    fn from(width: usize) -> Self {
        Self::Finite(width)
    }
}

impl fmt::Display for NormalizeWidth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Finite(width) => width.fmt(f),
            Self::Unlimited => f.write_str("unlimited"),
        }
    }
}

impl FromStr for NormalizeWidth {
    type Err = &'static str;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text == "unlimited" {
            return Ok(Self::Unlimited);
        }
        match text.parse::<usize>() {
            Ok(width) if width > 0 => Ok(Self::Finite(width)),
            _ => Err("expected a positive integer or \"unlimited\""),
        }
    }
}

impl Serialize for NormalizeWidth {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Finite(width) => width.serialize(serializer),
            Self::Unlimited => serializer.serialize_str("unlimited"),
        }
    }
}

impl<'de> Deserialize<'de> for NormalizeWidth {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = NormalizeWidth;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a positive integer or \"unlimited\"")
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                usize::try_from(value)
                    .ok()
                    .filter(|width| *width > 0)
                    .map(NormalizeWidth::Finite)
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Unsigned(value), &self))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                match u64::try_from(value) {
                    Ok(value) => self.visit_u64(value),
                    Err(_) => Err(E::invalid_value(de::Unexpected::Signed(value), &self)),
                }
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value == "unlimited" {
                    Ok(NormalizeWidth::Unlimited)
                } else {
                    Err(E::invalid_value(de::Unexpected::Str(value), &self))
                }
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
