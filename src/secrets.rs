use anyhow::{bail, Result};
use regex::Regex;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub struct SecretStore {
    values: BTreeMap<String, String>,
    next: usize,
}

impl SecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, value: String) -> String {
        self.next += 1;
        let reference = format!("secretref-{}", self.next);
        self.values.insert(reference.clone(), value);
        reference
    }

    pub fn get(&self, reference: &str) -> Option<&str> {
        self.values.get(reference).map(String::as_str)
    }

    pub fn values(&self) -> Vec<String> {
        self.values.values().cloned().collect()
    }

    pub fn resolve(&self, text: &str) -> Result<String> {
        let re = Regex::new(r"\{\{\s*(secretref-\d+)\s*\}\}").expect("regex valida");
        let mut missing: Option<String> = None;
        let resolved = re
            .replace_all(text, |caps: &regex::Captures<'_>| {
                let reference = &caps[1];
                match self.values.get(reference) {
                    Some(value) => value.clone(),
                    None => {
                        missing = Some(reference.to_string());
                        String::new()
                    }
                }
            })
            .to_string();
        if let Some(reference) = missing {
            bail!("referencia secreta no encontrada: {reference}");
        }
        Ok(resolved)
    }

    pub fn contains_reference(text: &str) -> bool {
        Regex::new(r"\{\{\s*secretref-\d+\s*\}\}")
            .expect("regex valida")
            .is_match(text)
    }

    pub fn clear(&mut self) {
        self.values.clear();
        self.next = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_references() {
        let mut store = SecretStore::new();
        let reference = store.insert("abc123".to_string());
        assert_eq!(reference, "secretref-1");
        assert_eq!(
            store
                .resolve(&format!("nmcli password {{{{ {reference} }}}}"))
                .unwrap(),
            "nmcli password abc123"
        );
    }

    #[test]
    fn rejects_missing_reference() {
        let store = SecretStore::new();
        assert!(store.resolve("{{secretref-9}}").is_err());
    }
}
