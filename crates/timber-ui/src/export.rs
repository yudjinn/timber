//! Image export (SVG / PNG) of the web, with optional title and legend (D-019).

use leptos::prelude::*;
use timber_core::{Tree, dtb, format};
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::graph::painter::{CanvasPainter, Font, Painter, SvgPainter};
use crate::graph::render::{self, Palette};
use crate::kind::Kind;
use crate::state::{AppState, Camera};
use crate::util::file_stem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Svg,
    Png,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportTheme {
    Light,
    Dark,
    Current,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportOptions {
    /// The whole web, or just what's in view.
    pub whole: bool,
    pub theme: ExportTheme,
    pub legend: bool,
    pub title: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            whole: true,
            theme: ExportTheme::Light,
            legend: true,
            title: true,
        }
    }
}

/// Longest side of an exported graph, in CSS pixels.
const MAX_SIDE: f64 = 8000.0;
const PNG_SCALE: f64 = 2.0;
const PAD: f64 = 24.0;
const TITLE_H: f64 = 60.0;
const MIN_WIDTH: f64 = 440.0;
const TITLE_FONT: Font = Font::new(16.0, true);
const SUB_FONT: Font = Font::new(12.0, false);
const LEGEND_FONT: Font = Font::new(12.0, false);

fn scratch_context(
    w: u32,
    h: u32,
) -> Result<(HtmlCanvasElement, CanvasRenderingContext2d), String> {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let canvas: HtmlCanvasElement = document
        .create_element("canvas")
        .map_err(|_| "no canvas")?
        .unchecked_into();
    canvas.set_width(w);
    canvas.set_height(h);
    let ctx = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok())
        .ok_or("no 2d context")?;
    Ok((canvas, ctx))
}

struct Layout {
    width: f64,
    height: f64,
    graph: (f64, f64),
    graph_origin: (f64, f64),
    cam: Camera,
    /// Legend items with positions.
    legend: Vec<(Kind, f64, f64)>,
}

/// Exports the web as an image; returns the file name, MIME type, and bytes.
pub fn export_image(
    state: AppState,
    format: ImageFormat,
    opts: ExportOptions,
) -> Result<(String, &'static str, Vec<u8>), String> {
    let doc = state.document().ok_or("nothing to export")?;
    let tree = &doc.tree;
    let graph = state.graph;
    let dark = match opts.theme {
        ExportTheme::Light => false,
        ExportTheme::Dark => true,
        ExportTheme::Current => state.dark.get_untracked(),
    };
    let pal = Palette::new(dark);
    let (_, measure_ctx) = scratch_context(1, 1)?;
    let mut measurer = CanvasPainter::new(&measure_ctx);

    let layout = graph.sim.with_value(|sim| -> Result<Layout, String> {
        let (gw, gh, cam) = if opts.whole {
            let (x0, y0, x1, y1) = sim.bounds(tree).ok_or("the web is empty")?;
            let (bw, bh) = (x1 - x0, y1 - y0);
            let k = (MAX_SIDE / bw.max(bh)).min(1.0);
            (
                bw * k,
                bh * k,
                Camera {
                    x: (x0 + x1) / 2.0,
                    y: (y0 + y1) / 2.0,
                    k,
                    target: None,
                },
            )
        } else {
            let (w, h) = graph.size.get_value();
            (w, h, graph.cam.get_value())
        };
        let width = gw.max(if opts.title || opts.legend {
            MIN_WIDTH
        } else {
            0.0
        });
        let title_h = if opts.title { TITLE_H } else { 0.0 };

        // Legend: the kinds of the nodes in the web, wrapped into rows.
        let mut legend = Vec::new();
        let mut legend_h = 0.0;
        if opts.legend {
            let (mut x, mut row) = (PAD, 0.0);
            for kind in Kind::ALL
                .iter()
                .copied()
                .filter(|k| sim.nodes.iter().any(|n| doc.kinds[n.id] == *k))
            {
                let item_w = 16.0 + measurer.measure(kind.label(), LEGEND_FONT) + 18.0;
                if x + item_w > width - PAD && x > PAD {
                    x = PAD;
                    row += 1.0;
                }
                legend.push((kind, x, row));
                x += item_w;
            }
            if !legend.is_empty() {
                legend_h = (row + 1.0) * 22.0 + 16.0;
            }
        }
        let graph_origin = ((width - gw) / 2.0, title_h);
        let legend = legend
            .into_iter()
            .map(|(k, x, row)| (k, x, title_h + gh + 14.0 + row * 22.0))
            .collect();
        Ok(Layout {
            width,
            height: title_h + gh + legend_h,
            graph: (gw, gh),
            graph_origin,
            cam,
            legend,
        })
    })?;

    let model = tree.prop_str(Tree::ROOT, "model").map(str::to_string);
    let shown = graph.sim.with_value(|s| s.nodes.len());
    // Generated nodes (`__symbols__`, `__local_fixups__`) aren't part of the hardware tree.
    let total = (0..doc.node_count())
        .filter(|&id| !timber_core::bsp::is_generated(tree, id))
        .count();
    let mut subtitle = format!("{} · {shown} of {total} nodes", doc.short_name());
    if let Some(link) = &doc.link {
        subtitle.push_str(&format!(
            " · source {}",
            link.board.rsplit('/').next().unwrap_or(&link.board)
        ));
    }
    if let Some(d) = doc.diff.as_ref().filter(|d| !d.from_source) {
        subtitle.push_str(&format!(" · compared with {}: {}", d.against, d.summary()));
    }
    let title = model.unwrap_or_else(|| doc.short_name().to_string());

    let paint = |p: &mut dyn Painter| {
        p.rect(0.0, 0.0, layout.width, layout.height, pal.bg);
        if opts.title {
            p.text(PAD, 30.0, &title, TITLE_FONT, pal.text, None);
            p.text(PAD, 48.0, &subtitle, SUB_FONT, pal.muted, None);
        }
        graph.sim.with_value(|sim| {
            render::draw(
                p,
                &render::Frame {
                    tree,
                    kinds: &doc.kinds,
                    sim,
                    cam: &layout.cam,
                    w: layout.graph.0,
                    h: layout.graph.1,
                    selected: None,
                    hovered: None,
                    popup: None,
                    references: &[],
                    pal: &pal,
                    origin: layout.graph_origin,
                    all_labels: true,
                    diff: doc.diff.as_ref(),
                },
            )
        });
        for &(kind, x, y) in &layout.legend {
            p.circle(
                x + 5.0,
                y + 6.0,
                5.0,
                Some(kind.color(dark)),
                Some((pal.outline, 1.0)),
                &[],
                1.0,
            );
            p.text(
                x + 16.0,
                y + 10.0,
                kind.label(),
                LEGEND_FONT,
                pal.text,
                None,
            );
        }
    };

    let stem = file_stem(doc.short_name()).to_string();
    match format {
        ImageFormat::Svg => {
            let mut svg = SvgPainter::new(&measure_ctx);
            paint(&mut svg);
            Ok((
                format!("{stem}-web.svg"),
                "image/svg+xml",
                svg.finish(layout.width, layout.height).into_bytes(),
            ))
        }
        ImageFormat::Png => {
            let (canvas, ctx) = scratch_context(
                (layout.width * PNG_SCALE).ceil() as u32,
                (layout.height * PNG_SCALE).ceil() as u32,
            )?;
            let _ = ctx.scale(PNG_SCALE, PNG_SCALE);
            paint(&mut CanvasPainter::new(&ctx));
            let url = canvas
                .to_data_url_with_type("image/png")
                .map_err(|_| "couldn't encode PNG")?;
            let b64 = url.split_once(',').map(|(_, d)| d).ok_or("bad PNG data")?;
            let binary = web_sys::window()
                .ok_or("no window")?
                .atob(b64)
                .map_err(|_| "bad PNG data")?;
            let bytes = binary.chars().map(|c| c as u8).collect();
            Ok((format!("{stem}-web.png"), "image/png", bytes))
        }
    }
}

/// The tree itself, as DTS source or a DTB blob.
pub fn export_tree(state: AppState, as_dtb: bool) -> Option<(String, &'static str, Vec<u8>)> {
    let doc = state.document()?;
    let stem = file_stem(doc.short_name()).to_string();
    Some(if as_dtb {
        (
            format!("{stem}.dtb"),
            "application/octet-stream",
            dtb::write(&doc.own_tree()),
        )
    } else {
        (
            format!("{stem}.dts"),
            "text/plain",
            format::to_dts(&doc.own_tree()).into_bytes(),
        )
    })
}
