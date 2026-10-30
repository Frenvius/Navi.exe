use super::sort::SortKey;
#[derive(Clone, PartialEq, Debug)]
pub struct Filter {
    pub key: SortKey,
    pub labels: Vec<String>,
}

impl Filter {
    pub fn accepts(
        &self,
        entry: &crate::shell::listing::ShellEntry,
        today: &super::group::Today,
        value: Option<&crate::shell::columns::SortValue>,
    ) -> bool {
        self.labels
            .contains(&super::group::bucket(self.key, entry, today, value).label)
    }

    pub fn toggle(current: Option<&Self>, key: SortKey, label: &str) -> Option<Self> {
        let mut labels = current
            .filter(|filter| filter.key == key)
            .map(|filter| filter.labels.clone())
            .unwrap_or_default();
        if let Some(index) = labels.iter().position(|candidate| candidate == label) {
            labels.remove(index);
        } else {
            labels.push(label.to_owned());
        }
        (!labels.is_empty()).then_some(Self { key, labels })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkbox_changes_form_a_union_and_the_last_uncheck_clears_it() {
        let a = Filter::toggle(None, SortKey::Type, "A").unwrap();
        let both = Filter::toggle(Some(&a), SortKey::Type, "B").unwrap();
        assert_eq!(both.labels, ["A", "B"]);
        let b = Filter::toggle(Some(&both), SortKey::Type, "A").unwrap();
        assert_eq!(b.labels, ["B"]);
        assert_eq!(Filter::toggle(Some(&b), SortKey::Type, "B"), None);
        assert_eq!(
            Filter::toggle(Some(&both), SortKey::Name, "Z")
                .unwrap()
                .labels,
            ["Z"]
        );
    }
}
