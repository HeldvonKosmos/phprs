//! English singular for the foreign-key convention (`comments` → `comment_id`).

pub fn singular(word: &str) -> String {
    let exceptions: &[(&str, &str)] = &[("people", "person"), ("children", "child"), ("men", "man"), ("women", "woman"), ("data", "data"), ("news", "news"), ("series", "series"), ("status", "status")];
    if let Some((_, s)) = exceptions.iter().find(|(p, _)| *p == word) {
        return s.to_string();
    }
    if let Some(stem) = word.strip_suffix("ies")
        && !stem.is_empty()
    {
        return format!("{stem}y");
    }
    for suffix in ["sses", "shes", "ches", "xes", "zes"] {
        if let Some(stem) = word.strip_suffix(suffix) {
            return format!("{stem}{}", &suffix[..suffix.len() - 2]);
        }
    }
    if let Some(stem) = word.strip_suffix('s')
        && !word.ends_with("ss")
    {
        return stem.to_string();
    }
    word.to_string()
}

pub fn foreign_key(table: &str) -> String {
    format!("{}_id", singular(table))
}

#[cfg(test)]
mod tests {
    #[test]
    fn singulars() {
        for (p, s) in [("comments", "comment"), ("entries", "entry"), ("boxes", "box"), ("addresses", "address"), ("people", "person"), ("status", "status"), ("hours", "hour")] {
            assert_eq!(super::singular(p), s);
        }
        assert_eq!(super::foreign_key("tasks"), "task_id");
    }
}
