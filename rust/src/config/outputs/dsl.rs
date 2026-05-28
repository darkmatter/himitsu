use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A single string-or-alias entry used while migrating from the flat env DSL.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputEntry {
    Selector(SelectorEntry),
    Alias { key: String, reference: String },
}

impl Serialize for OutputEntry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        match self {
            OutputEntry::Selector(selector) => selector.serialize(serializer),
            OutputEntry::Alias { key, reference } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(key, reference)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for OutputEntry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Selector(SelectorEntry),
            Alias(BTreeMap<String, String>),
        }

        match Raw::deserialize(deserializer)? {
            Raw::Selector(selector) => Ok(OutputEntry::Selector(selector)),
            Raw::Alias(alias) => {
                if alias.len() != 1 {
                    return Err(serde::de::Error::custom(
                        "alias entry must have exactly one key-value pair",
                    ));
                }
                let (key, reference) = alias.into_iter().next().unwrap();
                Ok(OutputEntry::Alias { key, reference })
            }
        }
    }
}

/// A single entry in the `selectors:` list of an output block.
/// Can be a plain selector string (path glob, tag:NAME, or +/,-combined).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SelectorEntry(pub String);

/// A single alias mapping: env-var-name → ref-string.
/// The ref-string can be a path, tag:NAME, or cross-store ref.
pub type AliasMap = BTreeMap<String, String>;

/// One output block definition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputDef {
    #[serde(default)]
    pub selectors: Vec<SelectorEntry>,
    #[serde(default)]
    pub aliases: AliasMap,
}

/// The full `outputs:` block: output-name → OutputDef.
pub type OutputsMap = BTreeMap<String, OutputDef>;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{AliasMap, OutputDef, OutputsMap, SelectorEntry};

    #[test]
    fn parse_minimal_outputs_block_with_one_selector() {
        let yaml = r#"
pci-prod:
  selectors:
    - tag:pci+tag:prod
"#;

        let outputs: OutputsMap = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(
            outputs["pci-prod"].selectors,
            vec![SelectorEntry("tag:pci+tag:prod".to_string())]
        );
    }

    #[test]
    fn parse_block_with_multiple_selectors_and_aliases() {
        let yaml = r#"
web-service-prod:
  selectors:
    - common/*
    - prod/database-url
  aliases:
    SHARED_SECRET: github:org/secrets#prod/api-key
    SOME_VALUE: path/to/some-secret
"#;

        let outputs: OutputsMap = serde_yaml::from_str(yaml).unwrap();
        let output = &outputs["web-service-prod"];

        assert_eq!(
            output.selectors,
            vec![
                SelectorEntry("common/*".to_string()),
                SelectorEntry("prod/database-url".to_string()),
            ]
        );
        assert_eq!(
            output.aliases["SHARED_SECRET"],
            "github:org/secrets#prod/api-key"
        );
        assert_eq!(output.aliases["SOME_VALUE"], "path/to/some-secret");
    }

    #[test]
    fn parse_brace_expansion_name_as_literal_key() {
        let yaml = r#"
web-service-{dev,staging,prod}:
  selectors:
    - common/*
"#;

        let outputs: OutputsMap = serde_yaml::from_str(yaml).unwrap();

        assert!(outputs.contains_key("web-service-{dev,staging,prod}"));
    }

    #[test]
    fn round_trip_parse_serialize_parse_preserves_outputs() {
        let yaml = r#"
web-service-{dev,staging,prod}:
  selectors:
    - common/*
    - $1/database-url
  aliases:
    SHARED_SECRET: github:org/secrets#prod/api-key
"#;
        let outputs: OutputsMap = serde_yaml::from_str(yaml).unwrap();

        let serialized = serde_yaml::to_string(&outputs).unwrap();
        let reparsed: OutputsMap = serde_yaml::from_str(&serialized).unwrap();

        assert_eq!(reparsed, outputs);
    }

    #[test]
    fn empty_selectors_list_is_valid() {
        let yaml = r#"
empty-selectors:
  selectors: []
  aliases:
    SOME_VALUE: path/to/some-secret
"#;

        let outputs: OutputsMap = serde_yaml::from_str(yaml).unwrap();

        assert!(outputs["empty-selectors"].selectors.is_empty());
    }

    #[test]
    fn empty_aliases_map_is_valid() {
        let yaml = r#"
empty-aliases:
  selectors:
    - common/*
  aliases: {}
"#;

        let outputs: OutputsMap = serde_yaml::from_str(yaml).unwrap();

        assert!(outputs["empty-aliases"].aliases.is_empty());
    }

    #[test]
    fn unknown_output_def_fields_are_rejected() {
        let yaml = r#"
strict:
  selectors:
    - common/*
  aliases: {}
  unexpected: true
"#;

        let err = serde_yaml::from_str::<OutputsMap>(yaml).unwrap_err();

        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn missing_selectors_key_defaults_to_empty_vec() {
        let yaml = r#"
aliases-only:
  aliases:
    STRIPE: tag:stripe
"#;

        let outputs: OutputsMap = serde_yaml::from_str(yaml).unwrap();

        assert!(outputs["aliases-only"].selectors.is_empty());
    }

    #[test]
    fn missing_aliases_key_defaults_to_empty_map() {
        let yaml = r#"
selectors-only:
  selectors:
    - tag:pci
"#;

        let outputs: OutputsMap = serde_yaml::from_str(yaml).unwrap();

        assert!(outputs["selectors-only"].aliases.is_empty());
    }

    #[test]
    fn serialize_produces_expected_yaml_shape() {
        let mut aliases: AliasMap = BTreeMap::new();
        aliases.insert(
            "SHARED_SECRET".to_string(),
            "github:org/secrets#prod/api-key".to_string(),
        );
        aliases.insert("SOME_VALUE".to_string(), "path/to/some-secret".to_string());

        let mut outputs = OutputsMap::new();
        outputs.insert(
            "web-service-prod".to_string(),
            OutputDef {
                selectors: vec![
                    SelectorEntry("common/*".to_string()),
                    SelectorEntry("prod/database-url".to_string()),
                ],
                aliases,
            },
        );

        let yaml = serde_yaml::to_string(&outputs).unwrap();

        assert_eq!(
            yaml,
            "web-service-prod:\n  selectors:\n  - common/*\n  - prod/database-url\n  aliases:\n    SHARED_SECRET: github:org/secrets#prod/api-key\n    SOME_VALUE: path/to/some-secret\n"
        );
    }
}
