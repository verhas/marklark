//! Data files into marklark's plain data (JSON-shaped values): YAML, TOML, and a document's YAML
//! front matter.

use std::collections::BTreeMap;

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

/// Parses XML into plain data with mdship IMPORT's conventions:
///
/// - the result is wrapped in the root element's name: `{"project": {...}}`;
/// - attributes become `@name`; repeated child elements become a list;
/// - an element with only text is that text; otherwise its text goes under `_text`; an empty element is
///   an empty dict;
/// - namespaces are stripped, except those mapped in `xmlns` (name → namespace URL), whose names
///   become `name_local`; two different names that would collapse into one key are an error.
///
/// DTDs are refused (no external entities, no entity expansion).
pub fn parse_xml(text: &str, xmlns: &BTreeMap<String, String>) -> Result<serde_json::Value, String> {
    let mut by_url: BTreeMap<&str, &str> = BTreeMap::new();
    for (name, url) in xmlns {
        let valid = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(format!("xmlns name {name:?} must start with a letter or '_' and contain only letters, digits, '_'"));
        }
        if let Some(other) = by_url.insert(url.as_str(), name.as_str()) {
            return Err(format!("xmlns maps namespace {url} to both {other:?} and {name:?}"));
        }
    }
    let doc = roxmltree::Document::parse(text).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let mut out = serde_json::Map::new();
    out.insert(xml_key(root.tag_name().namespace(), root.tag_name().name(), &by_url), xml_element(root, &by_url)?);
    Ok(out.into())
}

fn xml_key(namespace: Option<&str>, local: &str, by_url: &BTreeMap<&str, &str>) -> String {
    match namespace.and_then(|ns| by_url.get(ns)) {
        Some(prefix) => format!("{prefix}_{local}"),
        None => local.to_owned(),
    }
}

fn xml_element(node: roxmltree::Node, by_url: &BTreeMap<&str, &str>) -> Result<serde_json::Value, String> {
    let mut map = serde_json::Map::new();
    // key -> the original (namespace, name), so different names never merge silently.
    let mut origins: BTreeMap<String, (Option<String>, String)> = BTreeMap::new();
    let element_name = xml_key(node.tag_name().namespace(), node.tag_name().name(), by_url);
    let mut claim = |key: &str, ns: Option<&str>, name: &str| -> Result<(), String> {
        let origin = (ns.map(str::to_owned), name.to_owned());
        match origins.get(key) {
            Some(prev) if *prev != origin => Err(format!(
                "XML names {} and {} inside <{element_name}> would both become '{key}'; map one of the namespaces with xmlns",
                describe(prev),
                describe(&origin)
            )),
            _ => {
                origins.insert(key.to_owned(), origin);
                Ok(())
            }
        }
    };
    for attr in node.attributes() {
        let key = format!("@{}", xml_key(attr.namespace(), attr.name(), by_url));
        claim(&key, attr.namespace(), &format!("@{}", attr.name()))?;
        map.insert(key, attr.value().into());
    }
    for child in node.children().filter(|c| c.is_element()) {
        let key = xml_key(child.tag_name().namespace(), child.tag_name().name(), by_url);
        claim(&key, child.tag_name().namespace(), child.tag_name().name())?;
        let value = xml_element(child, by_url)?;
        match map.get_mut(&key) {
            // An element's own value is a dict or a string, never a list, so a list here is one this
            // loop built for a repeated element.
            Some(serde_json::Value::Array(items)) => items.push(value),
            Some(existing) => {
                let first = existing.take();
                *existing = serde_json::Value::Array(vec![first, value]);
            }
            None => {
                map.insert(key, value);
            }
        }
    }
    let text = node.text().map(str::trim).unwrap_or("");
    if !text.is_empty() {
        if map.is_empty() {
            return Ok(text.into());
        }
        map.insert("_text".into(), text.into());
    }
    Ok(map.into())
}

fn describe(origin: &(Option<String>, String)) -> String {
    match &origin.0 {
        Some(ns) => format!("'{}' in namespace {ns}", origin.1),
        None => format!("'{}'", origin.1),
    }
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
    fn xml_like_mdship() {
        let pom = r#"<?xml version="1.0"?>
<project xmlns="http://maven.apache.org/POM/4.0.0" xmlns:x="urn:x">
  <version>1.2.3</version>
  <dependencies>
    <dependency scope="test"><artifactId>junit</artifactId></dependency>
    <dependency><artifactId>a</artifactId></dependency>
    <dependency><artifactId>b</artifactId></dependency>
  </dependencies>
  <name lang="en">Demo</name>
  <empty/>
  <x:version>9</x:version>
</project>"#;
        let none = BTreeMap::new();
        // Without a mapping, <version> and <x:version> would collide.
        assert!(parse_xml(pom, &none).unwrap_err().contains("would both become 'version'"));
        let mapped = BTreeMap::from([("x".to_owned(), "urn:x".to_owned())]);
        let v = parse_xml(pom, &mapped).unwrap();
        assert_eq!(v["project"]["version"], json!("1.2.3"));
        assert_eq!(v["project"]["x_version"], json!("9"));
        assert_eq!(v["project"]["dependencies"]["dependency"][0], json!({"@scope": "test", "artifactId": "junit"}));
        assert_eq!(v["project"]["dependencies"]["dependency"][2]["artifactId"], json!("b"));
        assert_eq!(v["project"]["name"], json!({"@lang": "en", "_text": "Demo"}));
        assert_eq!(v["project"]["empty"], json!({}));
        assert!(parse_xml("<!DOCTYPE a [<!ENTITY e \"x\">]><a>&e;</a>", &none).is_err());
        assert!(parse_xml("<a/>", &BTreeMap::from([("1x".to_owned(), "u".to_owned())])).is_err());
    }

    #[test]
    fn front_matter() {
        assert_eq!(front_matter_text("---\ntitle: T\n---\n# Doc\n"), Some("title: T\n"));
        assert_eq!(front_matter_text("---\r\ntitle: T\r\n...\r\nbody"), Some("title: T\r\n"));
        assert_eq!(front_matter_text("# Doc\n---\na: 1\n---\n"), None);
        assert_eq!(front_matter_text("---\nunterminated: 1\n"), None);
    }
}
