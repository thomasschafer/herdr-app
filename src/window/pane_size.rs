//! Startup geometry for panes that do not have a terminal view yet.
//! Layout metrics are shared with the renderer; normal resize still handles pixel rounding.

use super::*;
use crate::layout::Header;

pub(super) fn node_border(n: &Node, focused: bool) -> Option<&Line> {
    n.focus.as_ref().filter(|_| focused).and_then(|f| f.border.as_ref()).or(n.border.as_ref())
}

pub(super) fn padding(n: &Node) -> (f32, f32) {
    n.padding.map_or((0.0, 0.0), |p| p.axes())
}

pub(super) fn frame_border(frame: &PaneFrame, focused: bool) -> Option<&Line> {
    if focused { frame.focus.as_ref().or(frame.border.as_ref()) } else { frame.border.as_ref() }
}

pub(super) fn header_height(header: &Header) -> f32 {
    header.height.unwrap_or(HEADER_HEIGHT)
}

pub(super) fn gap(n: &Node, frame: Option<&PaneFrame>) -> f32 {
    n.gap.or(frame.and_then(|f| f.gap)).unwrap_or(0.0)
}

pub(super) fn separator_width(n: &Node, gap: f32) -> f32 {
    gap + n.divider.as_ref().map_or(0.0, |line| line.width)
}

pub(super) fn weight(n: &Node) -> f32 {
    match n.size {
        Some(Span::Px { .. }) => 0.0,
        Some(Span::Weight { weight }) => weight,
        None => 1.0,
    }
}

fn inset(size: Size<Pixels>, x: f32, y: f32) -> Size<Pixels> {
    gpui::size((size.width - px(x)).max(px(0.0)), (size.height - px(y)).max(px(0.0)))
}

fn content_size(n: &Node, size: Size<Pixels>, focused: bool, zoom: f32) -> Size<Pixels> {
    let border = node_border(n, focused).map_or(0.0, |line| line.width);
    let (y, x) = padding(n);
    inset(size, 2.0 * (x + border) * zoom, 2.0 * (y + border) * zoom)
}

/// Flex shares after fixed siblings, gaps and min/max constraints.
fn shares(children: &[&Node], available: f32, zoom: f32) -> Vec<f32> {
    let mut sizes = vec![0.0; children.len()];
    let mut pending = Vec::new();
    let mut left = available;
    for (i, n) in children.iter().enumerate() {
        match n.size {
            Some(Span::Px { px }) => {
                sizes[i] = (px * zoom).max(n.min.unwrap_or(0.0) * zoom).min(n.max.unwrap_or(f32::INFINITY) * zoom);
                left -= sizes[i];
            }
            _ => pending.push(i),
        }
    }
    let total_weight = pending.iter().map(|&i| weight(children[i])).sum::<f32>().max(f32::EPSILON);
    let initial_free = left.max(0.0);
    while !pending.is_empty() {
        let total = pending.iter().map(|&i| weight(children[i])).sum::<f32>().max(f32::EPSILON);
        // Grow factors stay normalized to the original siblings after an item freezes.
        let free = left.max(0.0).min(initial_free * (total / total_weight));
        let mut violations = Vec::new();
        for &i in &pending {
            let n = children[i];
            let share = free * weight(n) / total;
            sizes[i] = share.min(n.max.unwrap_or(f32::INFINITY) * zoom).max(n.min.unwrap_or(0.0) * zoom);
            violations.push((i, sizes[i] - share));
        }
        let total_violation = violations.iter().map(|(_, violation)| violation).sum::<f32>();
        if total_violation == 0.0 { break; }
        for (i, violation) in violations {
            if (total_violation > 0.0 && violation <= 0.0) || (total_violation < 0.0 && violation >= 0.0) { continue; }
            left -= sizes[i];
            pending.retain(|&j| j != i);
        }
    }
    sizes
}

/// Each shown block's box (inside its own border and padding) when `n` fills
/// `area`, laid out as the renderer does: flex shares, gaps, dividers, stack headers.
fn block_boxes<'a>(n: &'a Node, area: Bounds<Pixels>, frame: Option<&PaneFrame>, focused: &HashSet<String>, zoom: f32, out: &mut Vec<(&'a Node, Bounds<Pixels>)>) {
    if n.hidden { return; }
    let border = node_border(n, focused.contains(&n.id)).map_or(0.0, |line| line.width);
    let (y, x) = padding(n);
    let origin = area.origin + point(px((x + border) * zoom), px((y + border) * zoom));
    let area = Bounds::new(origin, content_size(n, area.size, focused.contains(&n.id), zoom));
    match &n.kind {
        Kind::Block { .. } => out.push((n, area)),
        Kind::Row { children } | Kind::Column { children } => {
            let row = n.axis() == Some(Axis::Row);
            let children: Vec<_> = children.iter().filter(|n| !n.hidden).collect();
            let separator = separator_width(n, gap(n, frame)) * zoom;
            let separators = separator * children.len().saturating_sub(1) as f32;
            let available = f32::from(if row { area.size.width } else { area.size.height }) - separators;
            let mut at = 0.0;
            for (child, span) in children.iter().zip(shares(&children, available, zoom)) {
                let child_area = if row {
                    Bounds::new(area.origin + point(px(at), px(0.0)), gpui::size(px(span), area.size.height))
                } else {
                    Bounds::new(area.origin + point(px(0.0), px(at)), gpui::size(area.size.width, px(span)))
                };
                block_boxes(child, child_area, frame, focused, zoom, out);
                at += span + separator;
            }
        }
        Kind::Stack { header, .. } => {
            let height = header.as_ref().map_or(0.0, header_height) * zoom;
            if let Some(child) = n.active_child() {
                let below = Bounds::new(area.origin + point(px(0.0), px(height)), inset(area.size, 0.0, height));
                block_boxes(child, below, frame, focused, zoom, out);
            }
        }
    }
}

/// Where a tab's layout goes: the tab's area on screen (its context block, inside
/// its padding) and the frame its panes get, if any.
struct TabArea<'a> {
    area: Bounds<Pixels>,
    frame: Option<&'a PaneFrame>,
    zoom: f32,
}

impl AppWindow {
    fn tab_area(&self, root: &Node) -> Option<TabArea<'_>> {
        let context = self.chrome.visible_blocks().into_iter().find(|n| matches!(n.content(), Some(Content::Context { .. })))?;
        let bounds = *self.bounds.borrow().get(&context.id)?;
        if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) { return None; }
        let frame = match context.content() {
            Some(Content::Context { pane: Some(frame), .. }) if frame.lone || root.panes().len() > 1 => Some(frame),
            _ => None,
        };
        let zoom = settings::zoom();
        // The absolute bounds canvas already excludes the context border (padding box).
        let (y, x) = padding(context);
        let origin = bounds.origin + point(px(x * zoom), px(y * zoom));
        Some(TabArea { area: Bounds::new(origin, inset(bounds.size, 2.0 * x * zoom, 2.0 * y * zoom)), frame, zoom })
    }

    /// Where each pane of `root` would be on screen with nothing zoomed, `focused`
    /// drawn as the focused one.
    pub(super) fn unzoomed_panes(&self, root: &Node, focused: &str) -> Option<Vec<(String, Bounds<Pixels>)>> {
        let TabArea { area, frame, zoom } = self.tab_area(root)?;
        let focused = root.path_to(focused)?.into_iter().collect();
        let mut boxes = Vec::new();
        block_boxes(root, area, frame, &focused, zoom, &mut boxes);
        Some(boxes.into_iter().filter(|(n, _)| n.is_pane()).map(|(n, b)| (n.id.clone(), b)).collect())
    }

    pub(super) fn initial_size(&self, root: &Node, pane: &str, window: &Window) -> Option<(u16, u16)> {
        let TabArea { area, frame, zoom } = self.tab_area(root)?;
        let focused = root.path_to(pane)?.into_iter().collect();
        let mut boxes = Vec::new();
        block_boxes(root, area, frame, &focused, zoom, &mut boxes);
        let (n, b) = boxes.into_iter().find(|(n, _)| n.id == pane)?;
        let mut size = b.size;
        if let Some(frame) = frame.filter(|_| n.is_pane()) {
            let border = frame_border(frame, focused.contains(&n.id)).map_or(0.0, |line| line.width) * zoom;
            let header = frame.header.as_ref().filter(|h| !h.hover()).map_or(0.0, header_height) * zoom;
            size = inset(size, 2.0 * border, 2.0 * border + header);
        }
        Some(crate::terminal::grid_size(size, crate::terminal::cell_metrics(window).size))
    }
}
