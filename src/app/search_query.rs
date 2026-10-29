pub fn refine(text: &str, keyword: &str, value: &str) -> String {
    let tag = format!("{keyword}:");
    match text.find(&tag) {
        Some(at) => format!(
            "{}{value}{}",
            &text[..at + tag.len()],
            &text[at + tag.len()..]
        ),
        None if text.trim().is_empty() => format!("{tag}{value}"),
        None => format!("{} {tag}{value}", text.trim_end()),
    }
}

pub fn name_range(original: &str, query: &str) -> Option<std::ops::Range<usize>> {
    let name = original.to_lowercase();
    let query = query.trim().to_lowercase();
    let at = (!query.is_empty() && !query.contains(':'))
        .then(|| name.find(&query))
        .flatten()?;
    let start = name[..at].chars().count();
    Some(start..start + query.chars().count())
}

pub fn literal_spans(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let query = query.trim().to_lowercase();
    if query.is_empty() || query.contains(':') {
        return Vec::new();
    }
    let mut folded = String::new();
    let mut boundaries = Vec::new();
    for (byte, ch) in text.char_indices() {
        for lower in ch.to_lowercase() {
            let start = folded.len();
            folded.push(lower);
            for _ in start..folded.len() {
                boundaries.push((byte, byte + ch.len_utf8()));
            }
        }
    }
    let terms: Vec<&str> = if folded.contains(&query) {
        vec![query.as_str()]
    } else {
        query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let mut spans: Vec<_> = terms
        .iter()
        .flat_map(|term| {
            folded
                .match_indices(term)
                .map(|(at, matched)| (boundaries[at].0, boundaries[at + matched.len() - 1].1))
        })
        .collect();
    spans.sort_unstable();
    spans.dedup();
    spans.into_iter().map(|(start, end)| start..end).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refinement_retains_existing_suffix_and_unicode_matches_use_characters() {
        assert_eq!(
            refine(" alpha  ", "tipo", "documento"),
            " alpha tipo:documento"
        );
        assert_eq!(
            refine("tipo:pasta alpha", "tipo", "documento"),
            "tipo:documentopasta alpha"
        );
        assert_eq!(name_range("Árvore Meio.txt", "meio"), Some(7..11));
        assert!(name_range("nome:meio.txt", "nome:meio").is_none());
    }
}
