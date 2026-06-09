use std::path::{Path, PathBuf};
use crate::pages::PageContent;
use crate::pages::browse::{BrowseState, DirEntry};
use clear_ui::widget::{Graph, GraphNode, Element, Breadcrumb};

pub struct NetworkState {
    pub graph: Graph,
    pub breadcrumb: Breadcrumb,
    pub last_dir: PathBuf,
}

impl Default for NetworkState {
    fn default() -> Self {
        let mut graph = Graph::new();
        // Configure grid settings
        graph.set_show_network_grid(true);
        graph.set_grid_sizes(140.0, 70.0);
        graph.set_skipped_sizes(35.0, 35.0);
        graph.set_grid_origin(60.0, 60.0);
        graph.set_grid_snap_enabled(true);
        graph.set_uniform_background(false);
        graph.set_network_opacity(0.95);

        let mut breadcrumb = Breadcrumb::new();
        breadcrumb.set_network_opacity(0.95);

        Self {
            graph,
            breadcrumb,
            last_dir: PathBuf::new(),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum NetworkMessage {}

impl NetworkState {
    pub fn populate_graph(&mut self, current_dir: &Path, entries: &[DirEntry]) {
        let mut nodes = Vec::new();

        // 1. Parent directory (if any)
        let parent_node_name = if let Some(parent) = current_dir.parent() {
            let parent_name = parent
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());
            let name = format!("📁 .. ({})", parent_name);
            nodes.push(GraphNode {
                name: name.clone(),
                position: (3.0, 0.0),
                parameters: Vec::new(),
                geom_visible: true,
            });
            Some(name)
        } else {
            None
        };

        // 2. Current directory node
        let current_name = current_dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());
        let current_node_name = format!("📁 {}", current_name);

        let current_params = if let Some(ref p_name) = parent_node_name {
            vec![("input".to_string(), p_name.clone(), "string".to_string())]
        } else {
            Vec::new()
        };

        nodes.push(GraphNode {
            name: current_node_name.clone(),
            position: (3.0, 1.0),
            parameters: current_params,
            geom_visible: true,
        });

        // 3. Children nodes
        for (idx, entry) in entries.iter().enumerate() {
            let icon = if entry.is_dir { "📁" } else { "📄" };
            let node_name = format!("{} {}", icon, entry.name);

            // Spreading items in 7 columns starting at row 2
            let col = (idx % 7) as f32;
            let row = 2.0 + (idx / 7) as f32;

            nodes.push(GraphNode {
                name: node_name,
                position: (col, row),
                parameters: vec![("input".to_string(), current_node_name.clone(), "string".to_string())],
                geom_visible: true,
            });
        }

        self.graph.set_nodes(&nodes);
        self.last_dir = current_dir.to_path_buf();
    }
}

pub fn view(state: &mut NetworkState, browse: &BrowseState, cx: f32, cy: f32, cw: f32, ch: f32, ctx: &mut clear_ui::context::UiContext) -> PageContent {
    let mut pc = PageContent::new();

    // Render the Breadcrumb widget into PageContent
    clear_ui::layout::render_widget(&mut pc, &mut state.breadcrumb, cx + 4.0, cy + 6.0, cw - 16.0, 24.0, ctx);

    // Check if directory changed, or if last_dir is empty, and repopulate
    if state.last_dir != browse.current_dir || state.graph.get_nodes().is_empty() {
        state.populate_graph(&browse.current_dir, &browse.entries);

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

    // Render the Graph widget into PageContent, shifted down by 28.0 to leave room for the breadcrumb
    clear_ui::layout::render_widget(&mut pc, &mut state.graph, cx, cy + 28.0, cw, ch - 28.0, ctx);

    pc
}

#[allow(dead_code)]
pub fn update(_state: &mut NetworkState, _msg: NetworkMessage) {}

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
            },
            DirEntry {
                name: "subdir".to_string(),
                path: PathBuf::from("/home/user/project/subdir"),
                is_dir: true,
                size: 4096,
                permissions: 0o755,
                modified: String::new(),
            },
        ];

        state.populate_graph(current_dir, &entries);
        let nodes = state.graph.get_nodes();

        // 1 parent + 1 current + 2 children = 4 nodes
        assert_eq!(nodes.len(), 4);

        // Check node names
        assert_eq!(nodes[0].name, "📁 .. (user)");
        assert_eq!(nodes[1].name, "📁 project");
        assert_eq!(nodes[2].name, "📄 file1.txt");
        assert_eq!(nodes[3].name, "📁 subdir");

        // Check connection parameters
        // Current directory points to parent
        assert_eq!(nodes[1].parameters.len(), 1);
        assert_eq!(nodes[1].parameters[0].0, "input");
        assert_eq!(nodes[1].parameters[0].1, "📁 .. (user)");

        // Children point to current directory
        assert_eq!(nodes[2].parameters.len(), 1);
        assert_eq!(nodes[2].parameters[0].0, "input");
        assert_eq!(nodes[2].parameters[0].1, "📁 project");

        assert_eq!(nodes[3].parameters.len(), 1);
        assert_eq!(nodes[3].parameters[0].0, "input");
        assert_eq!(nodes[3].parameters[0].1, "📁 project");
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
            },
        ];

        state.populate_graph(current_dir, &entries);
        let nodes = state.graph.get_nodes();

        // No parent, so 1 current + 1 child = 2 nodes
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "📁 /");
        assert_eq!(nodes[1].name, "📁 bin");

        // Current has no parent parameter
        assert!(nodes[0].parameters.is_empty());

        // Child points to current
        assert_eq!(nodes[1].parameters.len(), 1);
        assert_eq!(nodes[1].parameters[0].0, "input");
        assert_eq!(nodes[1].parameters[0].1, "📁 /");
    }
}
