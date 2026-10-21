use leptos::prelude::*;

/// Renders `text` with the characters at `positions` marked. Characters from `muted_from`
/// on (e.g. a unit address) are dimmed.
pub fn highlight(
    text: &str,
    positions: &[usize],
    muted_from: Option<usize>,
) -> impl IntoView + use<> {
    let mut runs: Vec<(bool, bool, String)> = Vec::new();
    for (i, c) in text.chars().enumerate() {
        let state = (positions.contains(&i), muted_from.is_some_and(|m| i >= m));
        match runs.last_mut() {
            Some((h, m, s)) if (*h, *m) == state => s.push(c),
            _ => runs.push((state.0, state.1, c.to_string())),
        }
    }
    runs.into_iter()
        .map(|(hit, muted, s)| {
            let class = match (hit, muted) {
                (true, true) => "hl muted",
                (true, false) => "hl",
                (false, true) => "muted",
                (false, false) => "",
            };
            view! { <span class=class>{s}</span> }
        })
        .collect_view()
}

pub fn file_stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}
