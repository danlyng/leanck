use regex::Regex;
use std::collections::{BTreeMap, HashMap};
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
            let found = caps.get(0).unwrap();
            let matched = found.as_str();
            if identifier_like(text, found.range()) {
                return matched.to_owned();
            }
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
            let found = caps.get(0).unwrap();
            let matched = found.as_str();
            if identifier_like(&exact, found.range()) {
                return matched.to_owned();
            }
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
                    //
                    // Where the replacement is a prefix of the base and the
                    // inflected word doubles the replacement's final consonant,
                    // both spellings double it before `-ed` and `-ing`: British
                    // "fulfil" still gives "fulfilled" and "fulfilling", and
                    // American "program" gives "programmed", not "programed".
                    let doubled = replacement.chars().last().filter(|c| {
                        matches!(infl, Inflection::Past | Inflection::Gerund)
                            && c.is_ascii_alphabetic()
                            && !matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
                            && base.starts_with(replacement.as_str())
                            && word.starts_with(&format!("{replacement}{c}"))
                    });
                    let inflected = match (doubled, infl) {
                        (Some(c), Inflection::Past) => format!("{replacement}{c}ed"),
                        (Some(c), _) => format!("{replacement}{c}ing"),
                        (None, _) => inflect(&replacement, infl),
                    };
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

// ---------------------------------------------------------------------------
// Lean names in prose
// ---------------------------------------------------------------------------

/// True when the word at `range` is part of a Lean name rather than an English
/// word, so its spelling is part of the name and must not change.
///
/// Word-boundary matching already skips words joined to letters, digits, or
/// `_` (`colour_space`, `colour2`, and `ColourSpace`, which as a whole is not a
/// dictionary word). This also skips names joined by characters that are not
/// regex word characters: a dot (`Colour.red`, `foo.normalise`, `.colour`), a
/// prime (`colour'`, `colour'_def`), or a subscript (`colour₁`), and words with
/// internal capitals (`colourIn`, but not `COLOUR`). A sentence-final period (`the colour.`),
/// possessives (`colour's`, `colours'`), and quotes (`'colour'`, `'the colour'`) stay prose.
fn identifier_like(text: &str, range: std::ops::Range<usize>) -> bool {
    let word = &text[range.clone()];
    let before = text[..range.start].chars().next_back();
    let mut after = text[range.end..].chars();
    let (next, following) = (after.next(), after.next());
    let name_char = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    // Capitals after the first letter, unless the whole word is capitalized.
    let camel_case =
        word.chars().skip(1).any(char::is_uppercase) && word.chars().any(char::is_lowercase);
    let primed = next == Some('\'')
        && !following.is_some_and(|c| c.is_alphabetic())
        && !in_single_quotes(&text[..range.start])
        && !word.ends_with(['s', 'S']);
    camel_case
        || primed
        || before == Some('.')
        || (next == Some('.') && name_char(following))
        || name_char(before)
        || name_char(next)
}

/// Whether `before` ends inside a phrase opened by a single quote at the
/// start of a word in the same paragraph, so that a quote after the next
/// word closes the phrase rather than priming a name.
fn in_single_quotes(before: &str) -> bool {
    let Some(quote) = before.rfind('\'') else {
        return false;
    };
    let opens = before[..quote]
        .chars()
        .next_back()
        .is_none_or(|c| c.is_whitespace() || "([{\"“".contains(c));
    let phrase = &before[quote + 1..];
    opens && !phrase.starts_with(char::is_whitespace) && !phrase.contains("\n\n")
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

// The asset files are third-party (see the crate README) and stay verbatim.
// These entries are removed from them because the replacement is wrong, or the
// source word has another common meaning, in mathematical and software prose.
// Inflections of a removed word are no longer inferred either. Users can restore
// an entry through `normalize.rules.spelling.replacements`.

/// Removed from the British → American dictionary.
const BRITISH_TO_AMERICAN_EXCLUSIONS: &[&str] = &[
    // American usage has the same spelling ("gauge theory", "cancellation").
    "aesthete",
    "aesthetic",
    "archaeological",
    "cancellation",
    "dialogue",
    "epilogue",
    "exorcise",
    "fillet",
    "gauge",
    "glamour",
    "monologue",
    "prologue",
    "travelogue",
    // "Analogue" (an analogous object, a q-analogue) is standard in American
    // mathematics; "analog" is only the electronics sense.
    "analogue",
    // American plurals are also "buses" and "gases"; "busses" means kisses.
    "buses",
    "gases",
    "minibuses",
    // A different word or unit, not a spelling variant.
    "bevvy",
    "gryphon",
    "mum",
    "tonne",
];

/// Removed from the American → British dictionary.
const AMERICAN_TO_BRITISH_EXCLUSIONS: &[&str] = &[
    // British keeps these spellings in common senses ("computer program", "disk
    // drive", "first draft", "check" as a verb, "analog signal", "dialog box"),
    // where the British variant names something else ("storey", "tyre", "metre").
    "analog",
    "check",
    "dialog",
    "disk",
    "draft",
    "meter",
    "program",
    "story",
    "tire",
    "ton",
    // British distinguishes noun from verb: the noun stays "practice", and the
    // verb "license" (with "licensing") stays.
    "license",
    "licensing",
    "practice",
    // Mathematical names and variables ("Gram matrix", "ax + b"), a programming
    // language, and SI units that British English also spells "-gram".
    "ax",
    "centigram",
    "gram",
    "kilogram",
    "milligram",
    "prolog",
    // Standard British spellings too; the replacement is a rare or archaic variant.
    "almanac",
    "banister",
    "cipher",
    "connection",
    "gluing",
    "inflection",
    "inquire",
    "install",
    "jail",
    "medieval",
    "misspelled",
    "pizzazz",
    "presidium",
    "primeval",
    "siphon",
    "wagon",
    // Not British spellings: misspellings, plurals of another word, or another word.
    "bevy",
    "busses",
    "edema",
    "externalization",
    "filet",
    "gasses",
    "griffon",
    "minibusses",
];

/// A built-in dictionary without its excluded entries, as sorted JSON.
fn builtin_dictionary(json: &str, exclusions: &[&str]) -> String {
    let mut map: BTreeMap<String, String> =
        serde_json::from_str(json).expect("failed to parse spelling dictionary JSON");
    for word in exclusions {
        map.remove(*word);
    }
    serde_json::to_string_pretty(&map).expect("failed to serialize spelling dictionary") + "\n"
}

static B2A_JSON: LazyLock<String> = LazyLock::new(|| {
    builtin_dictionary(
        include_str!("../assets/british_to_american_sp.json"),
        BRITISH_TO_AMERICAN_EXCLUSIONS,
    )
});

static A2B_JSON: LazyLock<String> = LazyLock::new(|| {
    builtin_dictionary(
        include_str!("../assets/american_to_british_sp.json"),
        AMERICAN_TO_BRITISH_EXCLUSIONS,
    )
});

static B2A: LazyLock<Dictionary> = LazyLock::new(|| Dictionary::from_json(&B2A_JSON));

static A2B: LazyLock<Dictionary> = LazyLock::new(|| Dictionary::from_json(&A2B_JSON));

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

/// Editable built-in dictionaries as applied, in JSON: the bundled maps without
/// the entries this crate excludes.
pub fn builtin_json(direction: SpellingDirection) -> &'static str {
    match direction {
        SpellingDirection::BritishToAmerican => &B2A_JSON,
        SpellingDirection::AmericanToBritish => &A2B_JSON,
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
    fn lean_names_in_prose_keep_their_spelling() {
        for name in [
            "Colour.red",
            "foo.normalise",
            ".colour",
            "Colour.Space.red",
            "colour'",
            "colour''",
            "colour'_def",
            "colour₁",
            "colour_space",
            "colour2",
            "ColourSpace",
            "colourIn",
            "COLOURed",
        ] {
            let source = format!("see {name} here");
            assert_eq!(b2a(&source), source);
        }
        let source = "see Color.red, foo.normalize, and center'";
        assert_eq!(a2b(source), source);
    }

    #[test]
    fn prose_punctuation_next_to_words_is_still_corrected() {
        for (british, american) in [
            ("the colour.", "the color."),
            ("the colour. Next", "the color. Next"),
            ("the colour.\nNext", "the color.\nNext"),
            ("(the colour.)", "(the color.)"),
            ("we normalise...", "we normalize..."),
            ("the colour's hue", "the color's hue"),
            ("the colours' hues", "the colors' hues"),
            ("the word 'colour'", "the word 'color'"),
            ("COLOUR", "COLOR"),
        ] {
            assert_eq!(b2a(british), american);
        }
    }

    #[test]
    fn british_inflections_keep_a_doubled_final_l() {
        assert_eq!(
            a2b("fulfilled fulfilling fulfills distilled enrolling instills appalled"),
            "fulfilled fulfilling fulfils distilled enrolling instils appalled"
        );
        assert_eq!(a2b("fulfill distill"), "fulfil distil");
    }

    #[test]
    fn american_inflections_keep_a_doubled_final_consonant() {
        assert_eq!(
            b2a("programmed programming programmes"),
            "programmed programming programs"
        );
    }

    #[test]
    fn every_exclusion_names_a_bundled_entry() {
        for (json, exclusions) in [
            (
                include_str!("../assets/british_to_american_sp.json"),
                BRITISH_TO_AMERICAN_EXCLUSIONS,
            ),
            (
                include_str!("../assets/american_to_british_sp.json"),
                AMERICAN_TO_BRITISH_EXCLUSIONS,
            ),
        ] {
            let map: HashMap<String, String> = serde_json::from_str(json).unwrap();
            for word in exclusions {
                assert!(map.contains_key(*word), "{word} is not a bundled entry");
            }
        }
    }

    #[test]
    fn excluded_entries_and_their_inflections_are_left_alone() {
        let source = "gauge gauges gauged tonne tonnes cancellation buses gases analogue \
                      analogues dialogue prologue aesthetic";
        assert_eq!(b2a(source), source);
        let source = "program programs check checked checking story stories tire tired \
                      license licensed licensing practice practices meter meters ton tons \
                      draft drafts disk disks analog dialog Gram gram grams ax install \
                      gluing gluings";
        assert_eq!(a2b(source), source);
    }

    #[test]
    fn retained_entries_still_apply() {
        assert_eq!(
            b2a("cancelled storey storeys draught licence programme metre disc analyse"),
            "canceled story stories draft license program meter disk analyze"
        );
        assert_eq!(
            a2b("canceled cancelation practicing licenses? centimeter catalog"),
            "cancelled cancellation practising licenses? centimetre catalogue"
        );
    }

    #[test]
    fn exported_dictionaries_are_the_applied_dictionaries() {
        for (direction, exclusions) in [
            (
                SpellingDirection::BritishToAmerican,
                BRITISH_TO_AMERICAN_EXCLUSIONS,
            ),
            (
                SpellingDirection::AmericanToBritish,
                AMERICAN_TO_BRITISH_EXCLUSIONS,
            ),
        ] {
            let exported: BTreeMap<String, String> =
                serde_json::from_str(builtin_json(direction)).unwrap();
            assert!(exported.len() > 400);
            for word in exclusions {
                assert!(!exported.contains_key(*word));
                assert_eq!(correct_spelling(word, direction), *word);
            }
            for (word, replacement) in &exported {
                assert_eq!(correct_spelling(word, direction), *replacement);
            }
            let configured = configured_map(
                direction,
                &leanck_config::DictionaryRules::default(),
                &HashMap::new(),
            )
            .unwrap();
            let words = exported.keys().map(String::as_str);
            for word in words.chain(exclusions.iter().copied()) {
                assert_eq!(configured.correct(word), correct_spelling(word, direction));
            }
        }
    }

    #[test]
    fn multiple_entries_converted_correctly() {
        assert_eq!(b2a("programming"), "programming");
        assert_eq!(b2a("programing"), "programming");
    }
}
