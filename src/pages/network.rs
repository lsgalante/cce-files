use std::path::{Path, PathBuf};
use crate::pages::PageContent;
use crate::pages::browse::{BrowseState, DirEntry};
use cce_ui::widget::{Adapted, Graph, GraphNode, Breadcrumb, GraphController, PathController};

pub struct NetworkState {
    pub graph: Adapted<Graph>,
    pub breadcrumb: Adapted<Breadcrumb>,
    pub last_dir: PathBuf,
    /// The graph pane `(x, y, w, h)` the nodes were last laid out for: a
    /// pane of another size is laid out again (`view`), so the columns fit
    /// it. Panning moves the lattice's origin and is kept until then.
    pub laid_out_for: (f32, f32, f32, f32),
    /// The configured pitch at 100% — the floor a column's width starts from.
    base_pitch: (f32, f32),
    /// The cce-icons glyph each graph node wears, by node index (`folder` /
    /// `file`) — drawn over the node body by `view`, since a `GraphNode`
    /// carries a name and no glyph.
    pub node_glyphs: Vec<&'static str>,
}

impl Default for NetworkState {
    fn default() -> Self {
        // The configured lattice and node body (`style.surface.graph`), as
        // the designer's network uses them: a node is CENTRED on a lattice
        // crossing and its name hangs off its right side. The pitch across is
        // widened per directory to hold the names (`layout`); the origin is
        // set from the pane. Until 2026-10-06 this set the retired cell model
        // (140 x 70 cells, 35 px gaps, origin 60, 60), which under the
        // lattice put column 0 left of the pane, ran seven columns past its
        // right edge and left names 35 px to sit in.
        let mut graph = Graph::new();
        graph.set_show_network_grid(true);
        graph.set_grid_snap_enabled(true);
        graph.set_network_opacity(0.95);
        // A directory has no geometry to show: no toggle disc on the nodes.
        graph.inner_mut().set_show_toggles(false);
        let base_pitch = graph.inner().grid_pitch();

        let mut breadcrumb = Breadcrumb::new();
        breadcrumb.set_network_opacity(0.95);

        Self {
            graph,
            breadcrumb,
            last_dir: PathBuf::new(),
            node_glyphs: Vec::new(),
            laid_out_for: (0.0, 0.0, 0.0, 0.0),
            base_pitch,
        }
    }
}

/// Margin between the pane's edge and the nearest node or name.
const MARGIN: f32 = 16.0;
/// The most a column gives a name, at 100%: a longer one is cut with an
/// ellipsis (`fit_name`), so one long file name cannot widen every column.
const NAME_MAX: f32 = 180.0;

/// A node name's font size and its gap from the body, for a body `node_w`
/// wide — `Graph::node_labels`' rule, which this has to agree with.
fn label_metrics(node_w: f32) -> (f32, f32) {
    let k = node_w / 80.0;
    ((14.0 * k).clamp(6.0, 48.0), 8.0 * k)
}

/// `name`, cut with an ellipsis to `max` px at `font_size`.
fn fit_name(name: &str, font_size: f32, max: f32) -> String {
    use cce_ui::widget::display::TextLabel;
    if TextLabel::estimate_width(name, font_size) <= max {
        return name.to_string();
    }
    let mut out = String::new();
    for c in name.chars() {
        let next = format!("{out}{c}\u{2026}");
        if TextLabel::estimate_width(&next, font_size) > max {
            break;
        }
        out.push(c);
    }
    out.push('\u{2026}');
    out
}

/// Where a directory's nodes go in a pane `(x, y, w, h)`: the pitch across
/// (a body, its name's gap and the widest name the column holds, or the
/// configured pitch if that is wider), how many columns fit, and the lattice
/// origin that puts column 1 and row 1 — where the nodes begin — a margin
/// inside the pane: a node is centred on its crossing.
fn layout(pane: (f32, f32, f32, f32), node: (f32, f32), base_pitch: (f32, f32), names: &[String]) -> ((f32, f32), usize, (f32, f32)) {
    use cce_ui::widget::display::TextLabel;
    let (px, py, pw, _) = pane;
    let (nw, nh) = node;
    let (font, gap) = label_metrics(nw);
    let widest = names.iter().map(|n| TextLabel::estimate_width(n, font)).fold(0.0f32, f32::max);
    let pitch_x = (nw + gap + widest.min(NAME_MAX) + MARGIN).max(base_pitch.0);
    let cols = (((pw - 2.0 * MARGIN) - (nw + gap + widest.min(NAME_MAX))) / pitch_x).floor().max(0.0) as usize + 1;
    // The lattice's (0, 0) crossing a pitch above and left of the first
    // node, so its two heavy axes fall outside the pane: on the first
    // column they lay over its wires, which run down the lattice lines.
    let origin = (px + MARGIN + nw * 0.5 - pitch_x, py + MARGIN + nh * 0.5 - base_pitch.1);
    ((pitch_x, base_pitch.1), cols, origin)
}

impl NetworkState {
    /// Lay the directory out for the pane `pane` and fill the graph — see
    /// [`layout`]. The parent and the directory stand over the middle column,
    /// its entries in rows below them.
    pub fn lay_out(&mut self, pane: (f32, f32, f32, f32), current_dir: &Path, entries: &[DirEntry]) {
        let node = self.graph.inner().node_size();
        let (font, _) = label_metrics(node.0);
        let names: Vec<String> = entries.iter().map(|e| fit_name(&e.name, font, NAME_MAX)).collect();
        let ((pitch_x, pitch_y), cols, origin) = layout(pane, node, self.base_pitch, &names);
        self.graph.set_grid_pitch(pitch_x, pitch_y);
        self.graph.set_grid_origin(origin.0, origin.1);
        self.populate_graph(current_dir, entries, cols, &names);
        self.laid_out_for = pane;
    }

    /// Fill the graph: the parent at row 1 and the directory at row 2 over
    /// the middle column, then `entries` (shown as `names`) `cols` to a row,
    /// columns counted from 1.
    pub fn populate_graph(&mut self, current_dir: &Path, entries: &[DirEntry], cols: usize, names: &[String]) {
        let cols = cols.max(1);
        // Columns and rows from 1: the lattice's 0 lines are off the pane.
        let mid = ((cols - 1) / 2) as f32 + 1.0;
        let mut nodes = Vec::new();
        let mut glyphs = Vec::new();

        // 1. Parent directory (if any)
        let parent_node_name = if let Some(parent) = current_dir.parent() {
            let parent_name = parent
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());
            let name = format!(".. ({})", parent_name);
            glyphs.push("folder");
            nodes.push(GraphNode {
                id: String::new(),
                name: name.clone(),
                position: (mid, 1.0),
                parameters: Vec::new(),
                geom_visible: true,
                node_type: String::new(),
                inputs: 0,
                outputs: 1,
            });
            Some(name)
        } else {
            None
        };

        // 2. Current directory node
        let current_node_name = current_dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());
        glyphs.push("folder");

        let current_params = if let Some(ref p_name) = parent_node_name {
            vec![("input".to_string(), p_name.clone(), "string".to_string())]
        } else {
            Vec::new()
        };

        nodes.push(GraphNode {
            id: String::new(),
            name: current_node_name.clone(),
            position: (mid, 2.0),
            parameters: current_params,
            geom_visible: true,
            node_type: String::new(),
            inputs: 1,
            outputs: 1,
        });

        // 3. Children nodes
        for (idx, entry) in entries.iter().enumerate() {
            let node_name = names.get(idx).cloned().unwrap_or_else(|| entry.name.clone());
            glyphs.push(if entry.is_dir { "folder" } else { "file" });

            // `cols` to a row, from row 3.
            let col = 1.0 + (idx % cols) as f32;
            let row = 3.0 + (idx / cols) as f32;

            nodes.push(GraphNode {
                id: String::new(),
                name: node_name,
                position: (col, row),
                parameters: vec![("input".to_string(), current_node_name.clone(), "string".to_string())],
                geom_visible: true,
                node_type: String::new(),
                inputs: 1,
                outputs: 1,
            });
        }

        self.graph.set_nodes(&nodes);
        self.node_glyphs = glyphs;
        self.last_dir = current_dir.to_path_buf();
    }
}

pub fn view(state: &mut NetworkState, browse: &BrowseState, cx: f32, cy: f32, cw: f32, ch: f32, ctx: &mut cce_ui::context::UiContext) -> PageContent {
    let mut pc = PageContent::new();

    // Render the Breadcrumb, full width (the view switch is in its context menu).
    // TODO(style): the 4/6/16 offsets are a leftover inset from the pane rect
    // that browse.rs has already dropped.
    let breadcrumb_w = cw - 16.0;
    cce_ui::layout::render_widget(&mut pc, &mut state.breadcrumb, cx + 4.0, cy + 6.0, breadcrumb_w, 24.0, ctx);
    {
        let rect = cce_ui::scene::layout::Rect { x: cx + 4.0, y: cy + 6.0, width: breadcrumb_w, height: 24.0 };
        crate::pages::breadcrumb_relief(&mut pc, &state.breadcrumb, rect);
    }

    // The pane the graph is laid out in, under the breadcrumb.
    let pane = (cx, cy + 28.0, cw, (ch - 28.0).max(0.0));
    let moved = (pane.0 - state.laid_out_for.0).abs() > 0.5
        || (pane.1 - state.laid_out_for.1).abs() > 0.5
        || (pane.2 - state.laid_out_for.2).abs() > 0.5
        || (pane.3 - state.laid_out_for.3).abs() > 0.5;

    // Check if directory changed, or if last_dir is empty, and repopulate
    if state.last_dir != browse.current_dir || state.graph.get_nodes().is_empty() {
        state.lay_out(pane, &browse.current_dir, &browse.entries);

        // Update breadcrumb path
        let mut segments = Vec::new();
        for component in browse.current_dir.components() {
            let s = component.as_os_str().to_string_lossy().to_string();
            if s != "/" && !s.is_empty() {
                segments.push(s);
            }
        }
        state.breadcrumb.set_path(&segments);

        // Map browse selection to graph node selection if any
        let has_parent = browse.current_dir.parent().is_some();
        let offset = if has_parent { 2 } else { 1 };
        if let Some(sel) = browse.selected {
            state.graph.set_selected_node(Some(sel + offset));
        } else {
            state.graph.set_selected_node(None);
        }
    } else {
        // A pane of another size: the same nodes, laid out to fit it.
        if moved {
            let selected = state.graph.selected_node();
            state.lay_out(pane, &browse.current_dir, &browse.entries);
            state.graph.set_selected_node(selected);
        }
        // Sync graph selection with browse selection when they are in sync
        let has_parent = browse.current_dir.parent().is_some();
        let offset = if has_parent { 2 } else { 1 };

        if let Some(sel) = browse.selected {
            let expected_node_idx = sel + offset;
            if state.graph.selected_node() != Some(expected_node_idx) {
                if !state.graph.is_dragging() {
                    state.graph.set_selected_node(Some(expected_node_idx));
                }
            }
        } else {
            if let Some(graph_sel) = state.graph.selected_node() {
                if graph_sel >= offset {
                    state.graph.set_selected_node(None);
                }
            }
        }
    }

    // Render the Graph widget into PageContent, shifted down by 28.0 to leave
    // room for the breadcrumb — cut to its pane, since a graph panned or
    // zoomed past its edge would otherwise draw over the preview beside it.
    let mut graph_pc = PageContent::new();
    cce_ui::layout::render_widget(&mut graph_pc, &mut state.graph, cx, cy + 28.0, cw, ch - 28.0, ctx);
    pc.absorb(graph_pc.clipped_to([cx, cy + 28.0, cx + cw, cy + ch]));

    // Each node's glyph on the left of its body, at the size and inset the
    // graph's geometry toggle has on the right (hidden here): scaled with
    // the zoom. Cut to the graph's rect, as its labels are.
    let canvas = [cx, cy + 28.0, cx + cw, cy + ch];
    let glyph_color = [0xcc as f32 / 255.0, 0xcc as f32 / 255.0, 0xd4 as f32 / 255.0, 1.0];
    for (idx, glyph) in state.node_glyphs.iter().enumerate() {
        let Some((nx, ny, nw, nh)) = state.graph.node_rect(idx) else { continue };
        let scale_f = nw / 80.0;
        let side = (18.0 * scale_f).clamp(6.0, 50.0);
        pc.icon_bounded(glyph, nx + 6.0 * scale_f, ny + (nh - side) / 2.0, side, side, glyph_color, Some(canvas));
    }

    pc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_populate_graph_has_parent() {
        let mut state = NetworkState::default();
        let current_dir = Path::new("/home/user/project");
        let entries = vec![
            DirEntry {
                name: "file1.txt".to_string(),
                path: PathBuf::from("/home/user/project/file1.txt"),
                is_dir: false,
                size: 100,
                permissions: 0o644,
                modified: String::new(),
                origin: None,
            },
            DirEntry {
                name: "subdir".to_string(),
                path: PathBuf::from("/home/user/project/subdir"),
                is_dir: true,
                size: 4096,
                permissions: 0o755,
                modified: String::new(),
                origin: None,
            },
        ];

        state.lay_out((0.0, 0.0, 800.0, 600.0), current_dir, &entries);
        let nodes = state.graph.get_nodes();

        // 1 parent + 1 current + 2 children = 4 nodes
        assert_eq!(nodes.len(), 4);

        // Check node names
        assert_eq!(nodes[0].name, ".. (user)");
        assert_eq!(nodes[1].name, "project");
        assert_eq!(nodes[2].name, "file1.txt");
        assert_eq!(nodes[3].name, "subdir");

        // The kind rides as a glyph, not as a character in the name.
        assert_eq!(state.node_glyphs, vec!["folder", "folder", "file", "folder"]);

        // Check connection parameters
        // Current directory points to parent
        assert_eq!(nodes[1].parameters.len(), 1);
        assert_eq!(nodes[1].parameters[0].0, "input");
        assert_eq!(nodes[1].parameters[0].1, ".. (user)");

        // Children point to current directory
        assert_eq!(nodes[2].parameters.len(), 1);
        assert_eq!(nodes[2].parameters[0].0, "input");
        assert_eq!(nodes[2].parameters[0].1, "project");

        assert_eq!(nodes[3].parameters.len(), 1);
        assert_eq!(nodes[3].parameters[0].0, "input");
        assert_eq!(nodes[3].parameters[0].1, "project");
    }

    /// The page fits its pane: column 0 starts a margin inside the left
    /// edge, the rightmost column's body AND its name end inside the right
    /// edge, a name never reaches the next column's body, and a name too long
    /// for a column is cut with an ellipsis. Under the retired cell model
    /// (cce-ui's lattice centring nodes on crossings) column 0 started left
    /// of the pane, seven columns ran past its right edge and every name lay
    /// across its neighbour.
    #[test]
    fn the_graph_fits_its_pane_and_its_names_fit_their_columns() {
        use cce_ui::widget::display::TextLabel;
        let mut state = NetworkState::default();
        let names = ["Adwaita", "AdwaitaMono-BoldItalic.ttf", "a-really-quite-extraordinarily-long-font-family-name.otf", "x", "gnu-free", "noto-cjk", "liberation", "spleen", "xscreensaver"];
        let entries: Vec<DirEntry> = names
            .iter()
            .map(|n| DirEntry { name: n.to_string(), path: PathBuf::from(format!("/f/{n}")), is_dir: false, size: 1, permissions: 0o644, modified: String::new(), origin: None })
            .collect();
        let pane = (20.0, 60.0, 600.0, 640.0);
        state.lay_out(pane, Path::new("/usr/share/fonts"), &entries);
        let g = state.graph.inner();
        let (nw, _) = g.node_size();
        let (font, gap) = label_metrics(nw);
        let (pitch_x, _) = g.grid_pitch();
        let nodes = state.graph.get_nodes();
        for (i, node) in nodes.iter().enumerate() {
            let (x, _, w, _) = g.node_rect(i).unwrap();
            let label_w = TextLabel::estimate_width(&node.name, font);
            assert!(x >= pane.0 + MARGIN - 0.01, "{:?} starts left of the pane: {x}", node.name);
            assert!(x + w + gap + label_w <= pane.0 + pane.2 + 0.01, "{:?} runs past the pane", node.name);
            assert!(w + gap + label_w < pitch_x, "{:?}'s name reaches the next column", node.name);
            assert!(label_w <= NAME_MAX + 0.01, "{:?} is not cut to a column", node.name);
        }
        assert!(nodes.iter().any(|n| n.name.ends_with('\u{2026}')), "the long name is cut with an ellipsis");
        assert!(nodes.iter().skip(2).any(|n| n.position.0 > 1.0), "more than one column fits 600 px");
        let (ox, oy) = g.grid_origin();
        assert!(ox < pane.0 && oy < pane.1, "the lattice's heavy axes lie outside the pane");
        assert!(g.toggle_rect(0).is_none(), "a directory's nodes wear no geometry toggle");
    }

    #[test]
    fn test_populate_graph_root_no_parent() {
        let mut state = NetworkState::default();
        let current_dir = Path::new("/");
        let entries = vec![
            DirEntry {
                name: "bin".to_string(),
                path: PathBuf::from("/bin"),
                is_dir: true,
                size: 4096,
                permissions: 0o755,
                modified: String::new(),
                origin: None,
            },
        ];

        state.lay_out((0.0, 0.0, 800.0, 600.0), current_dir, &entries);
        let nodes = state.graph.get_nodes();

        // No parent, so 1 current + 1 child = 2 nodes
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "/");
        assert_eq!(nodes[1].name, "bin");
        assert_eq!(state.node_glyphs, vec!["folder", "folder"]);

        // Current has no parent parameter
        assert!(nodes[0].parameters.is_empty());

        // Child points to current
        assert_eq!(nodes[1].parameters.len(), 1);
        assert_eq!(nodes[1].parameters[0].0, "input");
        assert_eq!(nodes[1].parameters[0].1, "/");
    }
}
