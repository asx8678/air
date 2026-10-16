use std::collections::HashMap;

pub fn top_words(text: &str, n: usize) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for word in text.split(' ') {
        *counts.entry(word.to_string()).or_insert(0) += 1;
    }
    let mut pairs: Vec<(String, usize)> = counts.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1));
    pairs.truncate(n);
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words() {
        assert_eq!(top_words("a b a", 1), vec![("a".to_string(), 2)]);
    }

    #[test]
    fn ignores_case_and_punctuation() {
        assert_eq!(top_words("The cat. the CAT!", 1), vec![("cat".to_string(), 2)]);
    }

    #[test]
    fn ties_sorted_alphabetically() {
        assert_eq!(top_words("b a c", 2), vec![("a".to_string(), 1), ("b".to_string(), 1)]);
    }

    #[test]
    fn multiple_spaces_and_newlines() {
        assert_eq!(top_words("x  y\nx", 1), vec![("x".to_string(), 2)]);
    }
}
