//! The differences summary (D-035): every changed, new and gone node, each a link into the
//! web, plus a Markdown report for bug trackers and pull requests.

use std::sync::Arc;

use leptos::prelude::*;
use timber_core::NodeId;
use timber_core::bsp::compare_node;
use timber_core::diff::Mark;
use timber_core::format::format_prop_text;

use crate::doc::Document;
use crate::state::AppState;

/// Entries listed per group before "and N more".
const LIST_MAX: usize = 300;
/// Values longer than this are cut in the report.
const VALUE_MAX: usize = 120;

fn md_value(text: Option<String>) -> String {
    let Some(text) = text else {
        return "*not set*".into();
    };
    let text = if text.is_empty() {
        "true".to_string()
    } else {
        text
    };
    let mut short: String = text.chars().take(VALUE_MAX).collect();
    if short.len() < text.len() {
        short.push('…');
    }
    format!("`{}`", short.replace('`', "'").replace('|', "\\|"))
}

/// How many nodes are below `id` with the same mark (a new or gone subtree's size).
fn subtree_extra(doc: &Document, id: NodeId) -> usize {
    let d = doc.diff.as_ref().unwrap();
    let mark = d.marks[id];
    let mut n = 0;
    let mut stack = doc.tree.node(id).children.clone();
    while let Some(c) = stack.pop() {
        if d.marks[c] == mark {
            n += 1;
            stack.extend(doc.tree.node(c).children.iter().copied());
        }
    }
    n
}

/// The comparison as Markdown (D-035).
pub fn markdown(doc: &Document) -> String {
    let Some(d) = doc.diff.as_ref() else {
        return String::new();
    };
    let (a, b) = (doc.short_name(), d.against.as_str());
    let b_title = if d.from_source {
        format!("{b} (source)")
    } else {
        b.to_string()
    };
    let mut out = format!("## Differences: {a} vs {b_title}\n\n{}\n", d.summary());

    let changed = d.roots(&doc.tree, |m| matches!(m, Mark::Changed(_)));
    if !changed.is_empty() {
        out.push_str("\n### Changed\n");
        for id in changed {
            let Some(bid) = d.b_node(id) else { continue };
            let c = compare_node(&doc.tree, id, &d.b, bid);
            out.push_str(&format!(
                "\n#### `{}`\n\n| Property | {a} | {b} |\n| --- | --- | --- |\n",
                doc.tree.path(id)
            ));
            let value = |tree: &timber_core::Tree, node: NodeId, name: &str| {
                tree.node(node)
                    .props
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| format_prop_text(tree, node, p))
            };
            for name in c.differs.iter().chain(&c.only_a).chain(&c.only_b) {
                out.push_str(&format!(
                    "| `{name}` | {} | {} |\n",
                    md_value(value(&doc.tree, id, name)),
                    md_value(value(&d.b, bid, name)),
                ));
            }
        }
    }
    for (mark, title) in [
        (Mark::OnlyA, format!("Only in {a}")),
        (Mark::OnlyB, format!("Only in {b}")),
    ] {
        let roots = match mark {
            Mark::OnlyA => d.roots(&doc.tree, |m| *m == Mark::OnlyA),
            _ => d.roots(&doc.tree, |m| *m == Mark::OnlyB),
        };
        if roots.is_empty() {
            continue;
        }
        out.push_str(&format!("\n### {title}\n\n"));
        for id in roots {
            let extra = subtree_extra(doc, id);
            let below = if extra > 0 {
                format!(" (+{extra} below)")
            } else {
                String::new()
            };
            out.push_str(&format!("- `{}`{below}\n", doc.tree.path(id)));
        }
    }
    out
}

fn copy_markdown(state: AppState, doc: Arc<Document>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let promise = window.navigator().clipboard().write_text(&markdown(&doc));
    leptos::task::spawn_local(async move {
        match wasm_bindgen_futures::JsFuture::from(promise).await {
            Ok(_) => state.notify("Copied the differences as Markdown", false),
            Err(_) => state.notify("Couldn't copy to the clipboard", true),
        }
    });
}

#[component]
pub fn DiffSummary() -> impl IntoView {
    let state = expect_context::<AppState>();
    move || {
        if !state.summary_open.get() {
            return None;
        }
        let doc = state.doc.get()?;
        let d = doc.diff.as_ref()?;
        let group = |title: String, ids: Vec<NodeId>, pill: bool| {
            if ids.is_empty() {
                return None;
            }
            let more = ids.len().saturating_sub(LIST_MAX);
            let rows = ids
                .into_iter()
                .take(LIST_MAX)
                .map(|id| {
                    let path = doc.tree.path(id);
                    let extra = if pill { 0 } else { subtree_extra(&doc, id) };
                    let tag = if pill {
                        d.pill(id)
                    } else {
                        (extra > 0).then(|| format!("+{extra} below"))
                    };
                    view! {
                        <li>
                            <button class="link mono" on:click=move |_| state.go_to(id)>{path}</button>
                            {tag.map(|t| view! { <span class="diff-pill">{t}</span> })}
                        </li>
                    }
                })
                .collect_view();
            Some(view! {
                <h4>{title}</h4>
                <ul>{rows}</ul>
                {(more > 0).then(|| view! { <p class="muted small">{format!("and {more} more")}</p> })}
            })
        };
        let changed = group(
            "Changed".into(),
            d.roots(&doc.tree, |m| matches!(m, Mark::Changed(_))),
            true,
        );
        let new = group(
            format!("Only in {}", doc.short_name()),
            d.roots(&doc.tree, |m| *m == Mark::OnlyA),
            false,
        );
        let gone = group(
            format!("Only in {}", d.against),
            d.roots(&doc.tree, |m| *m == Mark::OnlyB),
            false,
        );
        let doc2 = doc.clone();
        let heading = format!("Differences from {}", d.b_name());
        let summary = d.summary();
        Some(view! {
            <section class="drawer summary" aria-label="Differences">
                <header>
                    <h3>{heading}</h3>
                    <span class="muted small">{summary}</span>
                    <button class="small" on:click=move |_| crate::actions::show_differences(state)>"Show all in the web"</button>
                    <button class="small" on:click=move |_| copy_markdown(state, doc2.clone())>"Copy as Markdown"</button>
                    <label class="check small">
                        <input
                            type="checkbox"
                            prop:checked=move || state.diff_only.get()
                            on:change=move |ev| state.diff_only.set(event_target_checked(&ev))
                        />
                        "Only differences in popup and search"
                    </label>
                    <button class="ghost" aria-label="Close" on:click=move |_| state.summary_open.set(false)>"✕"</button>
                </header>
                <div class="summary-body">
                    {changed}
                    {new}
                    {gone}
                    {(d.total() == 0).then(|| view! { <p class="muted">"No differences."</p> })}
                </div>
            </section>
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use timber_core::dts::compile_str;

    #[test]
    fn report() {
        let a = compile_str(
            "a.dts",
            "/dts-v1/;\n/ { chosen { bootargs = \"console=ttyS2 | quiet\"; }; n { x = <1>; }; extra { sub { }; }; };\n",
        )
        .tree;
        let b = compile_str(
            "b.dts",
            "/dts-v1/;\n/ { chosen { }; n { x = <2>; }; old { }; };\n",
        )
        .tree;
        let doc = Document::from_blob("dev.dtb", &timber_core::dtb::write(&a))
            .unwrap()
            .with_comparison("ref.dtb", false, b, None);
        let md = markdown(&doc);
        assert!(
            md.starts_with("## Differences: dev.dtb vs ref.dtb\n\n2 changed · 2 new · 1 gone\n"),
            "{md}"
        );
        assert!(md.contains("#### `/chosen`\n\n| Property | dev.dtb | ref.dtb |\n| --- | --- | --- |\n| `bootargs` | `\"console=ttyS2 \\| quiet\"` | *not set* |\n"), "{md}");
        assert!(md.contains("| `x` | `<0x01>` | `<0x02>` |"), "{md}");
        assert!(
            md.contains("### Only in dev.dtb\n\n- `/extra` (+1 below)\n"),
            "{md}"
        );
        assert!(md.contains("### Only in ref.dtb\n\n- `/old`\n"), "{md}");
    }
}
