use super::group::{DetailsLayout, Header};
use crate::shell::{library::Origin, listing::ShellEntry};
use std::collections::HashSet;

pub fn grouped(origins: &[Origin], group: Option<super::group::Group>) -> bool {
    !origins.is_empty()
        && group.is_some_and(|group| {
            group
                .key
                .property()
                .is_some_and(|key| crate::shell::library::is_location(&key))
        })
}

pub fn label(origin: &Origin) -> String {
    format!("{}{}{}", origin.name, super::GROUP_SUBTITLE, origin.parent)
}

pub fn ranges(
    entries: &[ShellEntry],
    order: &[usize],
    origins: &[Origin],
    group: Option<super::group::Group>,
) -> Vec<(String, usize, usize)> {
    let mut sources = origins.iter().collect::<Vec<_>>();
    if group.is_some_and(|group| group.descending) {
        sources.reverse();
    }
    let mut next = 0;
    sources
        .into_iter()
        .map(|origin| {
            let count = order
                .iter()
                .filter(|&&index| {
                    entries[index]
                        .library_origin
                        .as_ref()
                        .is_some_and(|source| super::ids::same(&source.path, &origin.path))
                })
                .count();
            let start = next;
            next += count;
            (label(origin), start, next)
        })
        .collect()
}

pub fn details(
    count: usize,
    ranges: &[(String, usize, usize)],
    collapsed: &HashSet<String>,
    pitch: f32,
) -> DetailsLayout {
    let mut layout = DetailsLayout {
        tops: vec![0.0; count],
        folded: vec![false; count],
        ..Default::default()
    };
    let mut y = super::GROUPED_TOP;
    for (index, (label, start, end)) in ranges.iter().enumerate() {
        if index > 0 {
            y += 13.0;
        }
        let folded = collapsed.contains(label);
        let height = if folded { 48.0 } else { 50.0 };
        layout.headers.push(Header {
            label: label.clone(),
            rows: *start..*end,
            top: y,
            height,
            folded,
        });
        y += if start == end {
            if folded {
                48.0
            } else {
                82.0
            }
        } else {
            height
        };
        for row in *start..*end {
            layout.tops[row] = y;
            layout.folded[row] = folded;
            if !folded {
                y += pitch;
            }
        }
    }
    layout.extent = y + 1.0;
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_sources_keep_their_header_and_spacing_across_collapse() {
        let ranges = vec![
            ("First\u{1f}Parent".into(), 0, 1),
            ("Empty\u{1f}Parent".into(), 1, 1),
            ("Last\u{1f}Parent".into(), 1, 2),
        ];
        let layout = details(2, &ranges, &HashSet::new(), 21.0);
        assert_eq!(layout.tops, [52.0, 231.0]);
        assert_eq!(layout.headers[1].top, 86.0);
        assert!(layout.headers[1].rows.is_empty());
        let collapsed = HashSet::from([ranges[0].0.clone(), ranges[1].0.clone()]);
        let layout = details(2, &ranges, &collapsed, 21.0);
        assert!(layout.folded[0]);
        assert_eq!(layout.tops[1], 174.0);
        assert_eq!(layout.extent, 196.0);
    }
}
