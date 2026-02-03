use std::collections::BTreeMap;

pub fn format_labels_compact(labels: &BTreeMap<String, String>) -> String {
    labels
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

pub fn metric_label(name: &str, labels: &BTreeMap<String, String>) -> String {
    if labels.is_empty() {
        name.to_string()
    } else {
        format!("{name}:{}", format_labels_compact(labels))
    }
}

pub fn truncate_label(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // ASCII-only truncation; labels/metric names are expected to be ASCII.
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_label_empty_labels() {
        let labels = BTreeMap::new();
        assert_eq!(metric_label("x", &labels), "x");
    }

    #[test]
    fn metric_label_with_labels() {
        let mut labels = BTreeMap::new();
        labels.insert("a".to_string(), "1".to_string());
        labels.insert("b".to_string(), "2".to_string());
        assert_eq!(metric_label("x", &labels), "x:a=1,b=2");
    }

    #[test]
    fn truncate_label_adds_ellipsis() {
        assert_eq!(truncate_label("hello", 10), "hello");
        assert_eq!(truncate_label("abcdefghijklmnopqrstuvwxyz", 8), "abcde...");
    }
}
