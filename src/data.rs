//! Data files into marklark's plain data (JSON-shaped values): YAML, TOML, and a document's YAML
//! front matter.

use yaml_rust2::{Yaml, YamlLoader};

/// Parses YAML text. An empty file is `None`; with several documents, the first is used.
pub fn parse_yaml(text: &str) -> Result<serde_json::Value, String> {
    let docs = YamlLoader::load_from_str(text).map_err(|e| e.to_string())?;
    match docs.into_iter().next() {
        Some(doc) => yaml_to_json(doc),
        None => Ok(serde_json::Value::Null),
    }
}

fn yaml_to_json(y: Yaml) -> Result<serde_json::Value, String> {
    Ok(match y {
        Yaml::Null => serde_json::Value::Null,
        Yaml::Boolean(b) => b.into(),
        Yaml::Integer(i) => i.into(),
        Yaml::Real(s) => match s.parse::<f64>().ok().and_then(serde_json::Number::from_f64) {
            Some(n) => n.into(),
            None => s.into(), // .inf / .nan have no plain-data form; keep the text
        },
        Yaml::String(s) => s.into(),
        Yaml::Array(a) => a.into_iter().map(yaml_to_json).collect::<Result<Vec<_>, _>>()?.into(),
        Yaml::Hash(h) => {
            let mut map = serde_json::Map::new();
            for (k, v) in h {
                let key = match k {
                    Yaml::String(s) | Yaml::Real(s) => s,
                    Yaml::Integer(i) => i.to_string(),
                    Yaml::Boolean(b) => b.to_string(),
                    Yaml::Null => "null".to_owned(),
                    _ => return Err("mapping keys must be scalars".into()),
                };
                map.insert(key, yaml_to_json(v)?);
            }
            map.into()
        }
        Yaml::Alias(_) => return Err("unresolved alias".into()),
        Yaml::BadValue => return Err("invalid value".into()),
    })
}

/// Converts parsed TOML (dates become strings).
pub fn toml_to_json(v: toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => s.into(),
        toml::Value::Integer(i) => i.into(),
        toml::Value::Float(f) => serde_json::Number::from_f64(f).map_or(serde_json::Value::Null, Into::into),
        toml::Value::Boolean(b) => b.into(),
        toml::Value::Datetime(d) => d.to_string().into(),
        toml::Value::Array(a) => a.into_iter().map(toml_to_json).collect(),
        toml::Value::Table(t) => t.into_iter().map(|(k, v)| (k, toml_to_json(v))).collect::<serde_json::Map<_, _>>().into(),
    }
}

/// The YAML front matter of a document: a `---` line at the very start, up to the next `---` or `...`
/// line. `None` when the document has none.
pub fn front_matter_text(doc: &str) -> Option<&str> {
    let doc = doc.strip_prefix('\u{feff}').unwrap_or(doc);
    let first_end = doc.find('\n')?;
    if doc[..first_end].trim_end() != "---" {
        return None;
    }
    let body_start = first_end + 1;
    let mut pos = body_start;
    for line in doc[body_start..].split_inclusive('\n') {
        let t = line.trim_end();
        if t == "---" || t == "..." {
            return Some(&doc[body_start..pos]);
        }
        pos += line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn yaml_values() {
        let y = "name: demo\nversion: 1.2\nport: 8080\nbeta: true\ntags: [a, b]\nnested:\n  k: v\n  3: three\nempty:\n";
        assert_eq!(
            parse_yaml(y).unwrap(),
            json!({"name": "demo", "version": 1.2, "port": 8080, "beta": true, "tags": ["a", "b"],
                   "nested": {"k": "v", "3": "three"}, "empty": null})
        );
        assert_eq!(parse_yaml("").unwrap(), serde_json::Value::Null);
        assert_eq!(parse_yaml("a: &x 1\nb: *x\n").unwrap(), json!({"a": 1, "b": 1}));
        assert!(parse_yaml("a: [1, 2").is_err());
    }

    #[test]
    fn front_matter() {
        assert_eq!(front_matter_text("---\ntitle: T\n---\n# Doc\n"), Some("title: T\n"));
        assert_eq!(front_matter_text("---\r\ntitle: T\r\n...\r\nbody"), Some("title: T\r\n"));
        assert_eq!(front_matter_text("# Doc\n---\na: 1\n---\n"), None);
        assert_eq!(front_matter_text("---\nunterminated: 1\n"), None);
    }
}
