//! Declaration kinds independent of Lean's LSP transport.
use serde::{Deserialize, Serialize};

macro_rules! named_kinds {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),* $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        #[serde(rename_all = "snake_case")]
        $(#[$meta])*
                pub enum $name { $($variant),* }
        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),* }
            }
        }
        impl std::str::FromStr for $name {
            type Err = String;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value { $($text => Ok(Self::$variant),)* _ => Err(format!("unknown kind {value:?}")) }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.pad(self.as_str()) }
        }
    };
}

named_kinds!(LeanDeclarationKind {
    Theorem => "theorem", Lemma => "lemma", Def => "def", Definition => "definition",
    Abbrev => "abbrev", Instance => "instance", Structure => "structure", Class => "class",
    Inductive => "inductive", Opaque => "opaque", Axiom => "axiom",
});

named_kinds!(
    /// Unknown future LSP kinds remain typed without inventing a Lean keyword.
    LspSymbolKind {
    File => "file", Module => "module", Namespace => "namespace", Package => "package",
    Class => "class", Method => "method", Property => "property", Field => "field",
    Constructor => "constructor", Enum => "enum", Interface => "interface", Function => "function",
    Variable => "variable", Constant => "constant", String => "string", Number => "number",
    Boolean => "boolean", Array => "array", Object => "object", Key => "key", Null => "null",
    EnumMember => "enum_member", Struct => "struct", Event => "event", Operator => "operator",
    TypeParameter => "type_parameter", Unknown => "unknown",
});

named_kinds!(
    /// Closed request vocabulary. `class` selects either source; `section` selects
    /// LSP namespaces. Empty/omitted selector vectors mean all declarations.
    KindSelector {
    Theorem => "theorem", Lemma => "lemma", Def => "def", Definition => "definition",
    Abbrev => "abbrev", Instance => "instance", Structure => "structure", Class => "class",
    Inductive => "inductive", Opaque => "opaque", Axiom => "axiom", Section => "section",
    File => "file", Module => "module", Namespace => "namespace", Package => "package",
    Method => "method", Property => "property", Field => "field", Constructor => "constructor",
    Enum => "enum", Interface => "interface", Function => "function", Variable => "variable",
    Constant => "constant", String => "string", Number => "number", Boolean => "boolean",
    Array => "array", Object => "object", Key => "key", Null => "null", EnumMember => "enum_member",
    Struct => "struct", Event => "event", Operator => "operator", TypeParameter => "type_parameter",
    Unknown => "unknown",
});

/// Lexical Lean kinds take precedence over less precise LSP categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "source", content = "kind", rename_all = "snake_case")]
pub enum DeclarationKind {
    Lean(LeanDeclarationKind),
    Lsp(LspSymbolKind),
}

impl KindSelector {
    /// Lean selectors match lexical keywords exactly (`def`/`definition` are
    /// synonyms). Only unrefined LSP kinds use the legacy coarse fallback policy.
    /// LSP selectors do not match declarations refined to a lexical Lean kind.
    pub fn matches(self, kind: DeclarationKind) -> bool {
        use DeclarationKind::{Lean, Lsp};
        use LeanDeclarationKind as D;
        use LspSymbolKind as S;
        match (self, kind) {
            (Self::Theorem, Lean(D::Theorem))
            | (Self::Lemma, Lean(D::Lemma))
            | (Self::Def | Self::Definition, Lean(D::Def | D::Definition))
            | (Self::Abbrev, Lean(D::Abbrev))
            | (Self::Instance, Lean(D::Instance))
            | (Self::Structure, Lean(D::Structure))
            | (Self::Class, Lean(D::Class))
            | (Self::Inductive, Lean(D::Inductive))
            | (Self::Opaque, Lean(D::Opaque))
            | (Self::Axiom, Lean(D::Axiom)) => true,
            (Self::Theorem | Self::Lemma | Self::Instance, Lsp(S::Method))
            | (Self::Def | Self::Definition | Self::Abbrev, Lsp(S::Method | S::Function))
            | (Self::Structure, Lsp(S::Class | S::Struct))
            | (Self::Inductive, Lsp(S::Enum))
            | (Self::Axiom | Self::Opaque, Lsp(S::Constant))
            | (Self::Section, Lsp(S::Namespace)) => true,
            (selector, Lsp(symbol)) => match (selector, symbol) {
                (Self::File, S::File)
                | (Self::Module, S::Module)
                | (Self::Namespace, S::Namespace)
                | (Self::Package, S::Package)
                | (Self::Class, S::Class)
                | (Self::Method, S::Method)
                | (Self::Property, S::Property)
                | (Self::Field, S::Field)
                | (Self::Constructor, S::Constructor)
                | (Self::Enum, S::Enum)
                | (Self::Interface, S::Interface)
                | (Self::Function, S::Function)
                | (Self::Variable, S::Variable)
                | (Self::Constant, S::Constant)
                | (Self::String, S::String)
                | (Self::Number, S::Number)
                | (Self::Boolean, S::Boolean)
                | (Self::Array, S::Array)
                | (Self::Object, S::Object)
                | (Self::Key, S::Key)
                | (Self::Null, S::Null)
                | (Self::EnumMember, S::EnumMember)
                | (Self::Struct, S::Struct)
                | (Self::Event, S::Event)
                | (Self::Operator, S::Operator)
                | (Self::TypeParameter, S::TypeParameter)
                | (Self::Unknown, S::Unknown) => true,
                _ => false,
            },
            _ => false,
        }
    }
}

impl std::fmt::Display for DeclarationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lean(kind) => kind.fmt(f),
            Self::Lsp(LspSymbolKind::EnumMember) => f.pad("enumMember"),
            Self::Lsp(LspSymbolKind::TypeParameter) => f.pad("typeParameter"),
            Self::Lsp(kind) => kind.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selectors_are_closed_and_canonical() {
        for kind in KindSelector::ALL {
            assert_eq!(kind.as_str().parse::<KindSelector>().unwrap(), *kind);
            assert_eq!(
                serde_json::from_value::<KindSelector>(serde_json::json!(kind.as_str())).unwrap(),
                *kind
            );
        }
        for typo in ["enumMember", "typeParameter", "THEOREM", "thorem", " def "] {
            assert!(serde_json::from_value::<KindSelector>(serde_json::json!(typo)).is_err());
        }
    }
    #[test]
    fn fallback_does_not_override_lexical_kinds() {
        use DeclarationKind::{Lean, Lsp};
        use LeanDeclarationKind as D;
        use LspSymbolKind as S;
        assert!(KindSelector::Theorem.matches(Lsp(S::Method)));
        assert!(!KindSelector::Theorem.matches(Lsp(S::Function)));
        assert!(!KindSelector::Theorem.matches(Lean(D::Lemma)));
        assert!(!KindSelector::Method.matches(Lean(D::Theorem)));
        assert!(KindSelector::Definition.matches(Lean(D::Def)));
        assert!(KindSelector::Def.matches(Lean(D::Definition)));
        assert!(KindSelector::Structure.matches(Lsp(S::Class)));
        assert!(!KindSelector::Structure.matches(Lean(D::Class)));
        assert!(KindSelector::EnumMember.matches(Lsp(S::EnumMember)));
        assert!(KindSelector::TypeParameter.matches(Lsp(S::TypeParameter)));
        assert!(KindSelector::Unknown.matches(Lsp(S::Unknown)));
    }
    #[test]
    fn list_selection_uses_or_and_empty_means_all() {
        use super::super::ListParams;
        let theorem = DeclarationKind::Lean(LeanDeclarationKind::Theorem);
        let member = DeclarationKind::Lsp(LspSymbolKind::EnumMember);
        for kind in [None, Some(vec![])] {
            let params = ListParams {
                file: "A.lean".into(),
                kind,
            };
            assert!(params.includes_kind(theorem));
            assert!(params.includes_kind(member));
        }
        let params = ListParams {
            file: "A.lean".into(),
            kind: Some(vec![
                KindSelector::Theorem,
                KindSelector::EnumMember,
                KindSelector::Theorem,
            ]),
        };
        assert!(params.includes_kind(theorem));
        assert!(params.includes_kind(member));
        assert!(!params.includes_kind(DeclarationKind::Lean(LeanDeclarationKind::Lemma)));
        assert!(
            serde_json::from_value::<ListParams>(
                serde_json::json!({"file":"A.lean","kind":["thorem"]})
            )
            .is_err()
        );
    }

    #[test]
    fn every_selector_has_explicit_fallback_semantics() {
        use DeclarationKind::{Lean, Lsp};
        use LeanDeclarationKind as D;
        use LspSymbolKind as S;
        let fallbacks = [
            (KindSelector::Theorem, vec![S::Method]),
            (KindSelector::Lemma, vec![S::Method]),
            (KindSelector::Instance, vec![S::Method]),
            (KindSelector::Def, vec![S::Method, S::Function]),
            (KindSelector::Definition, vec![S::Method, S::Function]),
            (KindSelector::Abbrev, vec![S::Method, S::Function]),
            (KindSelector::Structure, vec![S::Class, S::Struct]),
            (KindSelector::Inductive, vec![S::Enum]),
            (KindSelector::Opaque, vec![S::Constant]),
            (KindSelector::Axiom, vec![S::Constant]),
            (KindSelector::Section, vec![S::Namespace]),
        ];
        for selector in KindSelector::ALL {
            for kind in S::ALL {
                let expected = selector.as_str() == kind.as_str()
                    || fallbacks
                        .iter()
                        .any(|(s, kinds)| s == selector && kinds.contains(kind));
                assert_eq!(
                    selector.matches(Lsp(*kind)),
                    expected,
                    "{selector} / {kind}"
                );
            }
            for kind in D::ALL {
                let expected = selector.as_str() == kind.as_str()
                    || matches!(
                        (selector, kind),
                        (KindSelector::Def, D::Definition) | (KindSelector::Definition, D::Def)
                    );
                assert_eq!(
                    selector.matches(Lean(*kind)),
                    expected,
                    "{selector} / {kind}"
                );
            }
        }
    }

    #[test]
    fn result_retains_kind_source_and_text_labels() {
        for kind in [
            DeclarationKind::Lean(LeanDeclarationKind::Class),
            DeclarationKind::Lsp(LspSymbolKind::Class),
        ] {
            let json = serde_json::to_value(kind).unwrap();
            assert_eq!(
                serde_json::from_value::<DeclarationKind>(json).unwrap(),
                kind
            );
            assert_eq!(kind.to_string(), "class");
        }
        assert_eq!(
            serde_json::to_value(DeclarationKind::Lean(LeanDeclarationKind::Theorem)).unwrap(),
            serde_json::json!({"source":"lean","kind":"theorem"})
        );
        assert_eq!(
            DeclarationKind::Lsp(LspSymbolKind::EnumMember).to_string(),
            "enumMember"
        );
    }
}
