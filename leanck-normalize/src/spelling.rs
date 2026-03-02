use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Controls which direction to auto-correct comment spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpellingDirection {
    /// British → American (the default).
    BritishToAmerican,
    /// American → British.
    AmericanToBritish,
}

// ---------------------------------------------------------------------------
// Dictionary – parsed once at first use
// ---------------------------------------------------------------------------

pub struct Dictionary {
    regex: Regex,
    replacements: HashMap<String, String>,
    inflections: bool,
    case_sensitive: bool,
}

/// Matches a single run of ASCII letters. Used for the inflection pass, which
/// walks word-by-word rather than matching whole dictionary keys.
static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b[a-z]+\b").expect("failed to compile word regex"));

impl Dictionary {
    fn from_json(json: &str) -> Self {
        let map: HashMap<String, String> =
            serde_json::from_str(json).expect("failed to parse spelling dictionary JSON");

        // Sort keys longest-first so the regex engine prefers the
        // longest match (e.g. "colourful" before "colour").
        Self::from_map(map, false, true).expect("invalid built-in dictionary")
    }
    pub fn from_map(
        mut map: HashMap<String, String>,
        case_sensitive: bool,
        inflections: bool,
    ) -> Result<Self, String> {
        let mut normalized = HashMap::new();
        for (key, value) in map {
            if key.is_empty()
                || value.is_empty()
                || value.contains(['\n', '\r'])
                || value.contains("-/")
                || value.contains("/-")
            {
                return Err("invalid dictionary word or replacement".into());
            }
            let key = if case_sensitive {
                key
            } else {
                key.to_lowercase()
            };
            if normalized.insert(key.clone(), value).is_some() {
                return Err(format!("ambiguous dictionary spelling {key:?}"));
            }
        }
        map = normalized;
        // Resolve replacement chains once and reject cycles, so a second run
        // cannot keep changing a custom term.
        let mut resolved = HashMap::new();
        for (key, value) in &map {
            let mut value = value.clone();
            let mut seen = std::collections::HashSet::from([key.clone()]);
            loop {
                let target = if case_sensitive {
                    value.clone()
                } else {
                    value.to_lowercase()
                };
                let Some(next) = map.get(&target) else {
                    break;
                };
                if next == &value {
                    break;
                }
                if !seen.insert(target) {
                    return Err(format!("dictionary replacement cycle starting at {key:?}"));
                }
                value = next.clone();
            }
            resolved.insert(key.clone(), value);
        }
        map = resolved;
        // Empty alternations must never match arbitrary word boundaries.
        if map.is_empty() {
            return Ok(Self {
                regex: Regex::new(r"\b\B").unwrap(),
                replacements: map,
                inflections,
                case_sensitive,
            });
        }
        let mut keys: Vec<&str> = map.keys().map(|s| s.as_str()).collect();
        keys.sort_by_key(|k| -(k.len() as isize));

        let pattern = keys
            .iter()
            .map(|k| regex::escape(k))
            .collect::<Vec<_>>()
            .join("|");

        // (?i) – case-insensitive so "Colour" / "COLOUR" also match.
        let regex = Regex::new(&format!(
            r"{}\b(?:{pattern})\b",
            if case_sensitive { "" } else { "(?i)" }
        ))
        .map_err(|e| format!("invalid or oversized dictionary: {e}"))?;

        Ok(Self {
            regex,
            replacements: map,
            inflections,
            case_sensitive,
        })
    }

    pub fn correct(&self, text: &str) -> String {
        // Pass 1 – exact dictionary matches (longest-first). This also covers
        // multi-token keys like "battle-axe" that the inflection pass cannot.
        let exact = self.regex.replace_all(text, |caps: &regex::Captures| {
            let matched = caps.get(0).unwrap().as_str();
            let key = if self.case_sensitive {
                matched.to_owned()
            } else {
                matched.to_lowercase()
            };
            let Some(replacement) = self.replacements.get(&key) else {
                return matched.to_owned();
            };
            if self.case_sensitive {
                replacement.clone()
            } else {
                preserve_case(matched, replacement)
            }
        });

        // Pass 2 – inflected forms (e.g. "marginalised", "marginalising") whose
        // base ("marginalise") is in the dictionary. Words whose stem is not a
        // known spelling are left untouched, so this can never invent a
        // correction for an unrelated word.
        if !self.inflections {
            return exact.into_owned();
        }
        WORD.replace_all(&exact, |caps: &regex::Captures| {
            let matched = caps.get(0).unwrap().as_str();
            let key = if self.case_sensitive {
                matched.to_owned()
            } else {
                matched.to_lowercase()
            };
            // Exact replacements (including protected words) take precedence
            // over inferred inflections.
            if self.replacements.contains_key(&key) {
                return matched.to_owned();
            }
            match self.correct_inflected(&key) {
                Some(replacement) => {
                    if self.case_sensitive {
                        replacement
                    } else {
                        preserve_case(matched, &replacement)
                    }
                }
                None => matched.to_string(),
            }
        })
        .into_owned()
    }

    /// Correct up to two regular inflections, such as `color` → `coloring` →
    /// `colorings`. Prefer a direct dictionary base over a compound analysis.
    fn correct_inflected(&self, word: &str) -> Option<String> {
        self.correct_inflected_with_depth(word, 1)
            .or_else(|| self.correct_inflected_with_depth(word, 2))
    }

    fn correct_inflected_with_depth(&self, word: &str, depth: usize) -> Option<String> {
        for base in candidate_bases(word) {
            // Confirm which inflection of the base actually produced `word`,
            // then apply that same inflection to the replacement spelling. The
            // ending may differ between the two forms (e.g. British "axe" →
            // American "ax"), so each side is inflected on its own terms.
            for infl in [Inflection::Plural, Inflection::Past, Inflection::Gerund] {
                if inflect(&base, infl) == word {
                    let replacement = self.replacements.get(&base).cloned().or_else(|| {
                        if depth > 1 {
                            self.correct_inflected_with_depth(&base, depth - 1)
                        } else {
                            None
                        }
                    });
                    let Some(replacement) = replacement else {
                        continue;
                    };
                    // Our naive `inflect` applies only regular rules, so the
                    // American form it produces can itself be a misspelling the
                    // dictionary knows how to fix (e.g. "program" + "ing" →
                    // "programing", which the dictionary corrects to the
                    // consonant-doubled "programming"). Defer to the dictionary
                    // when it has an opinion on the inflected result.
                    let inflected = inflect(&replacement, infl);
                    return Some(
                        self.replacements
                            .get(&inflected)
                            .cloned()
                            .unwrap_or(inflected),
                    );
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Regular English inflection (the only "morphology" we attempt)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Inflection {
    /// Plural / 3rd-person-singular: `+s`, `+es`, or `y → ies`.
    Plural,
    /// Past tense / participle: `+ed`, `+d`, or `y → ied`.
    Past,
    /// Present participle: `+ing`, dropping a trailing `e`.
    Gerund,
}

fn ends_with_sibilant(w: &str) -> bool {
    w.ends_with('s')
        || w.ends_with('x')
        || w.ends_with('z')
        || w.ends_with("ch")
        || w.ends_with("sh")
}

/// True for a consonant immediately followed by a trailing `y` (e.g. "carry").
fn ends_consonant_y(w: &str) -> bool {
    w.strip_suffix('y')
        .and_then(|rest| rest.chars().last())
        .is_some_and(|c| c.is_ascii_alphabetic() && !matches!(c, 'a' | 'e' | 'i' | 'o' | 'u'))
}

/// Apply a regular inflection to `word` using standard English rules.
fn inflect(word: &str, infl: Inflection) -> String {
    let drop_last = |w: &str| w[..w.len() - 1].to_string();
    match infl {
        Inflection::Plural => {
            if ends_consonant_y(word) {
                format!("{}ies", drop_last(word))
            } else if ends_with_sibilant(word) {
                format!("{word}es")
            } else {
                format!("{word}s")
            }
        }
        Inflection::Past => {
            if word.ends_with('e') {
                format!("{word}d")
            } else if ends_consonant_y(word) {
                format!("{}ied", drop_last(word))
            } else {
                format!("{word}ed")
            }
        }
        Inflection::Gerund => {
            if word.ends_with('e') {
                format!("{}ing", drop_last(word))
            } else {
                format!("{word}ing")
            }
        }
    }
}

/// Candidate base spellings for an inflected word, reversing the rules in
/// [`inflect`]. Over-generates; the caller verifies each against the dictionary
/// and re-derives the exact inflection, so spurious candidates are harmless.
fn candidate_bases(word: &str) -> Vec<String> {
    let mut bases = Vec::new();
    let mut push = |s: String| {
        if !s.is_empty() {
            bases.push(s);
        }
    };

    if let Some(stem) = word.strip_suffix("ies") {
        push(format!("{stem}y"));
    }
    if let Some(stem) = word.strip_suffix("ied") {
        push(format!("{stem}y"));
    }
    if let Some(stem) = word.strip_suffix("ing") {
        push(stem.to_string()); // colour + ing
        push(format!("{stem}e")); // marginalise + ing (dropped e)
    }
    if let Some(stem) = word.strip_suffix("es") {
        push(stem.to_string()); // box + es
    }
    if let Some(stem) = word.strip_suffix("ed") {
        push(stem.to_string()); // colour + ed
    }
    if let Some(stem) = word.strip_suffix('s') {
        push(stem.to_string()); // marginalise + s
    }
    if let Some(stem) = word.strip_suffix('d') {
        push(stem.to_string()); // marginalise + d
    }

    bases
}

/// Mirror the case of `original` onto `replacement`.
fn preserve_case(original: &str, replacement: &str) -> String {
    if original.is_empty() {
        return replacement.to_string();
    }
    if original.chars().all(|c| c.is_uppercase()) {
        replacement.to_uppercase()
    } else if original.starts_with(|c: char| c.is_uppercase()) {
        let first = replacement
            .chars()
            .next()
            .unwrap()
            .to_uppercase()
            .collect::<String>();
        let rest: String = replacement.chars().skip(1).collect();
        first + &rest
    } else {
        replacement.to_string()
    }
}

// ---------------------------------------------------------------------------
// Static dictionaries (embedded at compile time)
// ---------------------------------------------------------------------------

static B2A: LazyLock<Dictionary> =
    LazyLock::new(|| Dictionary::from_json(include_str!("../assets/british_to_american_sp.json")));

static A2B: LazyLock<Dictionary> =
    LazyLock::new(|| Dictionary::from_json(include_str!("../assets/american_to_british_sp.json")));

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Replace British/American spellings inside `text` according to `direction`.
pub fn correct_spelling(text: &str, direction: SpellingDirection) -> String {
    match direction {
        SpellingDirection::BritishToAmerican => B2A.correct(text),
        SpellingDirection::AmericanToBritish => A2B.correct(text),
    }
}

/// Editable built-in dictionaries, in their original JSON representation.
pub fn builtin_json(direction: SpellingDirection) -> &'static str {
    match direction {
        SpellingDirection::BritishToAmerican => {
            include_str!("../assets/british_to_american_sp.json")
        }
        SpellingDirection::AmericanToBritish => {
            include_str!("../assets/american_to_british_sp.json")
        }
    }
}
pub fn configured_map(
    direction: SpellingDirection,
    rules: &leanck_config::DictionaryRules,
    files: &HashMap<String, String>,
) -> Result<Dictionary, String> {
    let mut map = if rules.use_builtin {
        serde_json::from_str(builtin_json(direction)).unwrap()
    } else {
        HashMap::new()
    };
    for (key, value) in files.iter().chain(rules.replacements.iter()) {
        map.insert(
            if rules.case_sensitive {
                key.clone()
            } else {
                key.to_lowercase()
            },
            value.clone(),
        );
    }
    for word in &rules.protected_words {
        map.insert(
            if rules.case_sensitive {
                word.clone()
            } else {
                word.to_lowercase()
            },
            word.clone(),
        );
    }
    Dictionary::from_map(map, rules.case_sensitive, rules.inflections)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b2a(s: &str) -> String {
        correct_spelling(s, SpellingDirection::BritishToAmerican)
    }
    fn a2b(s: &str) -> String {
        correct_spelling(s, SpellingDirection::AmericanToBritish)
    }

    #[test]
    fn exact_base_still_works() {
        assert_eq!(b2a("marginalise"), "marginalize");
        assert_eq!(b2a("colour"), "color");
    }

    #[test]
    fn inflections_of_ise_verbs() {
        assert_eq!(b2a("marginalised"), "marginalized");
        assert_eq!(b2a("marginalises"), "marginalizes");
        assert_eq!(b2a("marginalising"), "marginalizing");
        assert_eq!(b2a("organised it"), "organized it");
    }

    #[test]
    fn inflections_of_our_words() {
        assert_eq!(b2a("colours"), "colors");
        assert_eq!(b2a("coloured"), "colored");
        assert_eq!(b2a("colouring"), "coloring");
    }

    #[test]
    fn differing_endings_inflect_independently() {
        // British "axe" → American "ax": past tense stays "axed" on both sides.
        assert_eq!(b2a("axed"), "axed");
    }

    #[test]
    fn case_is_preserved_on_inflections() {
        assert_eq!(b2a("Marginalising"), "Marginalizing");
        assert_eq!(b2a("MARGINALISED"), "MARGINALIZED");
    }

    #[test]
    fn unknown_stems_are_left_alone() {
        // "premised"/"wise" de-inflect to stems that are not in the dictionary.
        assert_eq!(b2a("premised"), "premised");
        assert_eq!(b2a("wise rises"), "wise rises");
    }

    #[test]
    fn american_to_british_inflections() {
        assert_eq!(a2b("organized"), "organised");
        assert_eq!(a2b("analyzing"), "analysing");
        assert_eq!(a2b("colors"), "colours");
    }

    #[test]
    fn compound_inflections_work_in_both_directions() {
        for (american, british) in [
            ("colorings", "colourings"),
            ("flavorings", "flavourings"),
            ("centerings", "centrings"),
        ] {
            assert_eq!(a2b(american), british);
            assert_eq!(b2a(british), american);
            assert_eq!(a2b(british), british);
            assert_eq!(b2a(american), american);
        }
    }

    #[test]
    fn compound_inflections_preserve_case() {
        assert_eq!(a2b("Colorings COLORINGS"), "Colourings COLOURINGS");
        assert_eq!(b2a("Colourings COLOURINGS"), "Colorings COLORINGS");
    }

    #[test]
    fn compound_inflections_require_a_known_base_and_two_suffixes_at_most() {
        let source = "paintings gatherings coloringss";
        assert_eq!(a2b(source), source);
        assert_eq!(b2a(source), source);
    }

    #[test]
    fn compound_inflections_respect_dictionary_options() {
        let map = HashMap::from([("color".to_owned(), "colour".to_owned())]);
        let disabled = Dictionary::from_map(map.clone(), false, false).unwrap();
        assert_eq!(disabled.correct("color colorings"), "colour colorings");
        let case_sensitive = Dictionary::from_map(map, true, true).unwrap();
        assert_eq!(
            case_sensitive.correct("colorings Colorings"),
            "colourings Colorings"
        );
    }

    #[test]
    fn compound_inflections_respect_exact_and_intermediate_entries() {
        let dictionary = Dictionary::from_map(
            HashMap::from([
                ("color".to_owned(), "colour".to_owned()),
                ("coloring".to_owned(), "sketch".to_owned()),
                ("colorings".to_owned(), "colorings".to_owned()),
                ("flavor".to_owned(), "flavour".to_owned()),
                ("flavoring".to_owned(), "seasoning".to_owned()),
            ]),
            false,
            true,
        )
        .unwrap();
        assert_eq!(
            dictionary.correct("coloring colorings flavorings"),
            "sketch colorings seasonings"
        );
    }

    #[test]
    fn already_converted_words_are_untouched() {
        assert_eq!(b2a("marginalized"), "marginalized");
        assert_eq!(b2a("colorful coloring"), "colorful coloring");
    }

    #[test]
    fn multiple_entries_converted_correctly() {
        assert_eq!(b2a("programming"), "programming");
        assert_eq!(b2a("programing"), "programming");
    }
}
