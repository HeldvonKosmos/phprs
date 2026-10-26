//! English singular for the foreign-key convention (`comments` → `comment_id`).

pub fn singular(word: &str) -> String {
    // Laravel's Str::singular (Doctrine's inflector) for the words tables
    // are named after; the last word of a snake_case name counts
    // (`personal_access_tokens` → `personal_access_token`). Checked against
    // Laravel in tests/suite/php_oracles.rs.
    let (head, last) = match word.rfind('_') {
        Some(i) => word.split_at(i + 1),
        None => ("", word),
    };
    format!("{head}{}", singular_word(last))
}

fn singular_word(word: &str) -> String {
    const SAME: &[&str] = &["data", "news", "series", "species", "status", "sheep", "fish", "equipment", "information", "money", "rice"];
    const IRREGULAR: &[(&str, &str)] = &[
        ("people", "person"), ("children", "child"), ("men", "man"), ("women", "woman"), ("mice", "mouse"), ("geese", "goose"), ("teeth", "tooth"),
        ("feet", "foot"), ("oxen", "ox"), ("axes", "axe"), ("indices", "index"), ("vertices", "vertex"), ("matrices", "matrix"), ("quizzes", "quiz"),
        ("bases", "basis"), ("analyses", "analysis"), ("crises", "crisis"), ("theses", "thesis"), ("diagnoses", "diagnosis"), ("synopses", "synopsis"),
        ("aliases", "alias"), ("statuses", "status"), ("buses", "bus"), ("campuses", "campus"), ("viruses", "virus"), ("octopuses", "octopus"),
        ("halves", "half"), ("shelves", "shelf"), ("wolves", "wolf"), ("calves", "calf"), ("selves", "self"), ("leaves", "leaf"), ("thieves", "thief"), ("loaves", "loaf"),
        ("knives", "knife"), ("lives", "life"), ("wives", "wife"),
        ("heroes", "hero"), ("potatoes", "potato"), ("tomatoes", "tomato"), ("echoes", "echo"), ("vetoes", "veto"),
        ("movies", "movie"), ("cookies", "cookie"),
    ];
    if SAME.contains(&word) {
        return word.to_string();
    }
    if let Some((_, s)) = IRREGULAR.iter().find(|(p, _)| *p == word) {
        return s.to_string();
    }
    if let Some(stem) = word.strip_suffix("ies")
        && !stem.is_empty()
    {
        return format!("{stem}y");
    }
    for suffix in ["sses", "shes", "ches", "xes"] {
        if let Some(stem) = word.strip_suffix(suffix) {
            return format!("{stem}{}", &suffix[..suffix.len() - 2]);
        }
    }
    if let Some(stem) = word.strip_suffix('s')
        && !word.ends_with("ss")
        && !word.ends_with("us")
        && !word.ends_with("is")
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
