//! `grepum html`: a single self-contained HTML page for exploring the graph.
//! No network, no external scripts: the data and a small vanilla-JS viewer
//! are inlined, so the file works offline and can be attached or archived.

use crate::graph::{EdgeKind, Graph};
use anyhow::Result;
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;

const TEMPLATE: &str = include_str!("viz/graph.html");
const SLOT: &str = "/*__GREPUM_DATA__*/";

/// Compact JSON the viewer reads: defs as [qual, kind, file, line] and edges
/// as flat [from, to, ...] arrays keep large repos to a few MB.
fn data(g: &Graph, label: &str) -> serde_json::Value {
    const KINDS: [&str; 11] = ["fn", "method", "class", "interface", "struct", "enum", "trait", "type", "mod", "object", "field"];
    let kind_ix = |k: &str| KINDS.iter().position(|x| *x == k).unwrap_or(0);
    let defs: Vec<serde_json::Value> =
        g.defs.iter().map(|d| json!([d.qual, kind_ix(d.kind.label()), d.file, d.line])).collect();
    let mut seen = HashSet::new();
    let (mut calls, mut inherits) = (vec![], vec![]);
    for e in &g.edges {
        let Some(from) = e.from_def else { continue };
        if e.kind == EdgeKind::Ref {
            continue;
        }
        if !seen.insert((from, e.to, e.kind)) {
            continue;
        }
        let list = if e.kind == EdgeKind::Call { &mut calls } else { &mut inherits };
        list.push(from);
        list.push(e.to);
    }
    json!({
        "label": label,
        "files": g.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        "kinds": KINDS,
        "defs": defs,
        "calls": calls,
        "inherits": inherits,
    })
}

pub fn render(g: &Graph, label: &str) -> Result<String> {
    // `<` escaped so nothing in the data can close the <script> element.
    let json = serde_json::to_string(&data(g, label))?.replace('<', "\\u003c");
    let title = format!("<title>grepum · {}</title>", label.replace('&', "&amp;").replace('<', "&lt;"));
    Ok(TEMPLATE.replacen(SLOT, &json, 1).replacen("<title>grepum graph</title>", &title, 1))
}

/// Open a file in the default browser; best effort.
pub fn open(path: &Path) {
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    let _ = cmd.arg(path).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_cannot_break_out_of_the_script_tag() {
        let mut g = Graph::default();
        g.files.push(crate::graph::GFile {
            path: "x</script><script>alert(1)</script>.py".into(),
            oid: String::new(),
            lang: crate::facts::Lang::Python,
            lines: 1,
            first_def: 0,
            def_count: 0,
        });
        let html = render(&g, "t").unwrap();
        assert_eq!(html.matches("</script>").count(), TEMPLATE.matches("</script>").count());
        assert!(!html.contains(SLOT));
    }
}
