//! Application state shared through context.

use std::sync::Arc;

use leptos::prelude::*;
use timber_core::{NodeId, Tree};

use std::collections::HashMap;

use crate::backend::{Capabilities, FolderHandle};
use crate::doc::Document;
use crate::graph::sim::Sim;
use crate::sources::{SharedSources, SourceSet};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub x: f64,
    pub y: f64,
    /// Zoom: screen pixels per world unit.
    pub k: f64,
    /// Where the camera is animating to.
    pub target: Option<(f64, f64, f64)>,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            x: 0.0,
            y: 0.0,
            k: 1.0,
            target: None,
        }
    }
}

pub const MIN_ZOOM: f64 = 0.08;
pub const MAX_ZOOM: f64 = 4.0;

impl Camera {
    pub fn to_screen(self, x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
        (
            (x - self.x) * self.k + w / 2.0,
            (y - self.y) * self.k + h / 2.0,
        )
    }

    pub fn to_world(self, sx: f64, sy: f64, w: f64, h: f64) -> (f64, f64) {
        (
            (sx - w / 2.0) / self.k + self.x,
            (sy - h / 2.0) / self.k + self.y,
        )
    }

    /// Moves toward the target; returns true while animating.
    pub fn step(&mut self) -> bool {
        let Some((tx, ty, tk)) = self.target else {
            return false;
        };
        let t = 0.18;
        self.x += (tx - self.x) * t;
        self.y += (ty - self.y) * t;
        self.k *= (tk / self.k).powf(t);
        if (tx - self.x).abs() < 0.5
            && (ty - self.y).abs() < 0.5
            && (tk / self.k - 1.0).abs() < 0.002
        {
            (self.x, self.y, self.k) = (tx, ty, tk);
            self.target = None;
        }
        true
    }
}

/// The force layout and camera, plus a version signal for UI that depends on which nodes
/// are in the web. Layout state is non-reactive; the canvas redraws on its own loop.
#[derive(Clone, Copy)]
pub struct GraphCtl {
    pub sim: StoredValue<Sim, LocalStorage>,
    pub cam: StoredValue<Camera, LocalStorage>,
    pub dirty: StoredValue<bool, LocalStorage>,
    /// CSS-pixel size of the canvas, updated by the render loop.
    pub size: StoredValue<(f64, f64), LocalStorage>,
    /// Bumped whenever nodes are added or removed.
    pub version: RwSignal<u64>,
}

impl GraphCtl {
    fn new() -> Self {
        GraphCtl {
            sim: StoredValue::new_local(Sim::new()),
            cam: StoredValue::new_local(Camera::default()),
            dirty: StoredValue::new_local(true),
            size: StoredValue::new_local((800.0, 600.0)),
            version: RwSignal::new(0),
        }
    }

    pub fn redraw(&self) {
        self.dirty.set_value(true);
    }

    fn changed(&self) {
        self.version.update(|v| *v += 1);
        self.redraw();
    }

    pub fn reset(&self) {
        self.sim.update_value(Sim::reset);
        self.cam.set_value(Camera::default());
        self.changed();
    }

    pub fn contains_untracked(&self, id: NodeId) -> bool {
        self.sim.with_value(|s| s.contains(id))
    }

    pub fn add_children(&self, tree: &Tree, parent: NodeId, ids: &[NodeId]) -> usize {
        let n = self
            .sim
            .try_update_value(|s| s.add_children(tree, parent, ids))
            .unwrap_or(0);
        if n > 0 {
            self.changed();
        }
        n
    }

    pub fn reveal(&self, tree: &Tree, id: NodeId) {
        if !self.contains_untracked(id) {
            self.sim.update_value(|s| s.reveal(tree, id));
            self.changed();
        }
    }

    pub fn collapse(&self, tree: &Tree, id: NodeId) -> usize {
        let n = self
            .sim
            .try_update_value(|s| s.collapse(tree, id))
            .unwrap_or(0);
        if n > 0 {
            self.changed();
        }
        n
    }

    pub fn remove_subtree(&self, tree: &Tree, id: NodeId) -> usize {
        let n = self
            .sim
            .try_update_value(|s| s.remove_subtree(tree, id))
            .unwrap_or(0);
        if n > 0 {
            self.changed();
        }
        n
    }

    /// Animates the camera to centre `id`, zooming in if currently far out.
    pub fn focus(&self, id: NodeId) {
        if let Some((x, y)) = self.sim.with_value(|s| s.pos(id)) {
            self.cam.update_value(|c| {
                let k = c.k.max(0.9);
                c.target = Some((x, y, k));
            });
            self.redraw();
        }
    }

    /// Animates the camera to show the whole web.
    pub fn fit(&self, tree: &Tree) {
        let Some((x0, y0, x1, y1)) = self.sim.with_value(|s| s.bounds(tree)) else {
            return;
        };
        let (w, h) = self.size.get_value();
        let k = ((w / (x1 - x0)).min(h / (y1 - y0)) * 0.9).clamp(MIN_ZOOM, 1.6);
        self.cam
            .update_value(|c| c.target = Some(((x0 + x1) / 2.0, (y0 + y1) / 2.0, k)));
        self.redraw();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemePref {
    Auto,
    Light,
    Dark,
}

impl ThemePref {
    pub fn next(self) -> Self {
        match self {
            ThemePref::Auto => ThemePref::Light,
            ThemePref::Light => ThemePref::Dark,
            ThemePref::Dark => ThemePref::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemePref::Auto => "Theme: auto",
            ThemePref::Light => "Theme: light",
            ThemePref::Dark => "Theme: dark",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            ThemePref::Auto => "auto",
            ThemePref::Light => "light",
            ThemePref::Dark => "dark",
        }
    }

    pub fn from_key(s: &str) -> Self {
        match s {
            "light" => ThemePref::Light,
            "dark" => ThemePref::Dark,
            _ => ThemePref::Auto,
        }
    }
}

/// The popup node's position relative to the graph area: (x, y, radius, area width, area height).
pub type Anchor = (f64, f64, f64, f64, f64);

#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub message: String,
    pub error: bool,
}

#[derive(Clone, Copy)]
pub struct AppState {
    pub doc: RwSignal<Option<Arc<Document>>>,
    pub selected: RwSignal<Option<NodeId>>,
    /// The node whose child popup is open.
    pub popup: RwSignal<Option<NodeId>>,
    pub anchor: RwSignal<Option<Anchor>>,
    pub graph: GraphCtl,
    pub theme: RwSignal<ThemePref>,
    /// The resolved theme.
    pub dark: RwSignal<bool>,
    pub toast: RwSignal<Option<Toast>>,
    pub diagnostics_open: RwSignal<bool>,
    /// Board paths waiting for the user to pick which one to show.
    pub choose: RwSignal<Option<Vec<String>>>,
    pub sources: RwSignal<SharedSources>,
    pub sources_open: RwSignal<bool>,
    /// Folders that can be re-read, by mount name.
    pub folder_handles: StoredValue<HashMap<String, FolderHandle>, LocalStorage>,
    pub help_open: RwSignal<bool>,
    /// A long-running step in progress (reading a folder, matching a board).
    pub busy: RwSignal<Option<String>>,
    pub capabilities: Capabilities,
}

impl AppState {
    pub fn new(capabilities: Capabilities) -> Self {
        AppState {
            doc: RwSignal::new(None),
            selected: RwSignal::new(None),
            popup: RwSignal::new(None),
            anchor: RwSignal::new(None),
            graph: GraphCtl::new(),
            theme: RwSignal::new(ThemePref::Auto),
            dark: RwSignal::new(false),
            toast: RwSignal::new(None),
            diagnostics_open: RwSignal::new(false),
            choose: RwSignal::new(None),
            sources: RwSignal::new(Arc::new(SourceSet::default())),
            sources_open: RwSignal::new(false),
            folder_handles: StoredValue::new_local(HashMap::new()),
            help_open: RwSignal::new(false),
            busy: RwSignal::new(None),
            capabilities,
        }
    }

    pub fn document(&self) -> Option<Arc<Document>> {
        self.doc.get_untracked()
    }

    pub fn notify(&self, message: impl Into<String>, error: bool) {
        self.toast.set(Some(Toast {
            message: message.into(),
            error,
        }));
    }

    pub fn set_document(&self, doc: Document) {
        let errors = doc.error_count();
        self.doc.set(Some(Arc::new(doc)));
        self.graph.reset();
        self.selected.set(Some(Tree::ROOT));
        self.popup.set(Some(Tree::ROOT));
        self.diagnostics_open.set(errors > 0);
    }

    /// Swaps in an updated version of the same tree (e.g. new source annotations), keeping
    /// the web, selection, and popup as they are.
    pub fn replace_document(&self, doc: Document) {
        self.doc.set(Some(Arc::new(doc)));
        self.graph.redraw();
    }

    /// Reveals `id` in the web, selects it, centres it, and opens its child popup.
    pub fn go_to(&self, id: NodeId) {
        let Some(doc) = self.document() else { return };
        self.graph.reveal(&doc.tree, id);
        self.selected.set(Some(id));
        self.popup.set(Some(id));
        self.graph.focus(id);
    }

    /// Adds `child` and moves the popup onto it (drill down).
    pub fn drill(&self, child: NodeId) {
        let Some(doc) = self.document() else { return };
        self.graph.reveal(&doc.tree, child);
        self.selected.set(Some(child));
        self.popup.set(Some(child));
    }
}
