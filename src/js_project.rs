//! JS/TS module resolution beyond relative paths: tsconfig/jsconfig `paths`
//! and `baseUrl` (following relative `extends`), and workspace packages named
//! in a `package.json`. Built from the config files of the snapshot, so it is
//! as cacheable as the source files themselves.

use serde_json::Value;
use std::collections::HashMap;

const MAX_EXTENDS: u8 = 5;

/// Config files that change how JS/TS imports resolve.
pub fn is_config(path: &str) -> bool {
    if path.split('/').any(|seg| seg == "node_modules") {
        return false;
    }
    let base = path.rsplit('/').next().unwrap_or(path);
    base == "package.json"
        || base == "jsconfig.json"
        || (base.starts_with("tsconfig") && base.ends_with(".json"))
}

#[derive(Default, Debug)]
pub struct JsProject {
    /// Directory -> effective compiler options of the tsconfig/jsconfig in it.
    configs: HashMap<String, Options>,
    /// Workspace package name -> (package dir, entry file candidates).
    packages: HashMap<String, (String, Vec<String>)>,
}

#[derive(Default, Debug, Clone)]
struct Options {
    /// Directory bare specifiers resolve against.
    base_url: Option<String>,
    /// `paths` patterns with their targets, already made repo-relative.
    paths: Vec<(String, Vec<String>)>,
}

impl JsProject {
    pub fn parse(files: &[(String, Vec<u8>)]) -> JsProject {
        let raw: HashMap<&str, Value> =
            files.iter().filter_map(|(p, b)| Some((p.as_str(), parse_jsonc(b)?))).collect();
        let mut project = JsProject::default();
        for (path, json) in &raw {
            let base = path.rsplit('/').next().unwrap_or(path);
            let dir = dir_of(path).to_string();
            if base == "package.json" {
                if let Some(name) = json.get("name").and_then(Value::as_str) {
                    project.packages.insert(name.to_string(), (dir.clone(), package_entries(&dir, json)));
                }
            } else if base == "tsconfig.json" || base == "jsconfig.json" {
                // A directory's own tsconfig wins over jsconfig.
                if base == "jsconfig.json" && project.configs.contains_key(&dir) {
                    continue;
                }
                project.configs.insert(dir, options(&raw, path, MAX_EXTENDS));
            }
        }
        project
    }

    /// Repo-relative paths (with or without extension) that `spec`, imported
    /// from `importer`, may refer to. Empty when it's an external package.
    pub fn candidates(&self, importer: &str, spec: &str) -> Vec<String> {
        let mut out = vec![];
        if let Some(opts) = self.config_for(importer) {
            let mut best: Option<(usize, &Vec<String>, &str)> = None;
            for (pattern, targets) in &opts.paths {
                let captured = match pattern.split_once('*') {
                    Some((pre, suf)) if spec.len() >= pre.len() + suf.len() && spec.starts_with(pre) && spec.ends_with(suf) => {
                        Some((pre.len(), &spec[pre.len()..spec.len() - suf.len()]))
                    }
                    None if pattern == spec => Some((usize::MAX, "")),
                    _ => None,
                };
                // The most specific pattern (longest prefix, exact first) wins, as in tsc.
                if let Some((len, star)) = captured {
                    if best.is_none_or(|(b, _, _)| len > b) {
                        best = Some((len, targets, star));
                    }
                }
            }
            if let Some((_, targets, star)) = best {
                out.extend(targets.iter().map(|t| normalize(&t.replace('*', star))));
            }
            if let Some(base) = &opts.base_url {
                out.push(normalize(&join(base, spec)));
            }
        }
        if let Some((name, sub)) = self.package_of(spec) {
            let (dir, entries) = &self.packages[name];
            match sub {
                None => out.extend(entries.iter().cloned()),
                Some(sub) => {
                    out.push(normalize(&join(dir, sub)));
                    out.push(normalize(&join(dir, &format!("src/{sub}"))));
                }
            }
        }
        out
    }

    /// The nearest tsconfig/jsconfig in `path`'s directory or above it.
    fn config_for(&self, path: &str) -> Option<&Options> {
        let mut dir = dir_of(path);
        loop {
            if let Some(o) = self.configs.get(dir) {
                return Some(o);
            }
            if dir.is_empty() {
                return None;
            }
            dir = dir_of(dir);
        }
    }

    fn package_of<'s>(&self, spec: &'s str) -> Option<(&'s str, Option<&'s str>)> {
        if self.packages.contains_key(spec) {
            return Some((spec, None));
        }
        // `@scope/pkg/sub/path` or `pkg/sub/path`.
        let split = if spec.starts_with('@') { spec.match_indices('/').nth(1) } else { spec.match_indices('/').next() };
        let (name, sub) = spec.split_at(split?.0);
        self.packages.contains_key(name).then_some((name, Some(&sub[1..])))
    }
}

/// Effective options of the config at `path`, merged over what it `extends`.
fn options(raw: &HashMap<&str, Value>, path: &str, depth: u8) -> Options {
    let Some(json) = raw.get(path) else { return Options::default() };
    let dir = dir_of(path);
    let mut opts = match json.get("extends") {
        Some(Value::String(e)) if depth > 0 && e.starts_with('.') => {
            let mut target = normalize(&join(dir, e));
            if !target.ends_with(".json") {
                target.push_str(".json");
            }
            options(raw, &target, depth - 1)
        }
        _ => Options::default(),
    };
    let compiler = json.get("compilerOptions");
    let base_url = compiler.and_then(|c| c.get("baseUrl")).and_then(Value::as_str).map(|b| normalize(&join(dir, b)));
    if base_url.is_some() {
        opts.base_url = base_url.clone();
    }
    if let Some(paths) = compiler.and_then(|c| c.get("paths")).and_then(Value::as_object) {
        // Targets are relative to baseUrl when set, else to the config that declares `paths`.
        let root = opts.base_url.clone().unwrap_or_else(|| dir.to_string());
        opts.paths = paths
            .iter()
            .map(|(pattern, targets)| {
                let targets = targets.as_array().map(|a| a.iter().filter_map(Value::as_str).map(|t| join(&root, t)).collect()).unwrap_or_default();
                (pattern.clone(), targets)
            })
            .collect();
    }
    opts
}

/// Files a bare import of a workspace package may land on. Published entry
/// points usually name build output (`dist/index.js`), so source equivalents
/// and the conventional `src/index` / `index` are tried as well.
fn package_entries(dir: &str, json: &Value) -> Vec<String> {
    let mut fields: Vec<&str> = ["source", "types", "typings", "module", "main"]
        .iter()
        .filter_map(|k| json.get(*k).and_then(Value::as_str))
        .collect();
    if let Some(dot) = json.get("exports").map(|e| e.get(".").unwrap_or(e)) {
        collect_strings(dot, &mut fields);
    }
    let mut out = vec![];
    for f in fields {
        let f = f.strip_suffix(".d.ts").unwrap_or(f);
        out.push(normalize(&join(dir, f)));
        for build in ["dist/", "lib/", "build/", "esm/", "cjs/", "out/"] {
            if let Some(rest) = f.trim_start_matches("./").strip_prefix(build) {
                out.push(normalize(&join(dir, &format!("src/{rest}"))));
            }
        }
    }
    out.push(normalize(&join(dir, "src/index")));
    out.push(normalize(&join(dir, "index")));
    out.dedup();
    out
}

fn collect_strings<'v>(v: &'v Value, out: &mut Vec<&'v str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Object(m) => m.values().for_each(|x| collect_strings(x, out)),
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

fn join(dir: &str, rel: &str) -> String {
    if dir.is_empty() { rel.to_string() } else { format!("{dir}/{rel}") }
}

fn normalize(path: &str) -> String {
    let mut out: Vec<&str> = vec![];
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

/// tsconfig files are JSON with comments and trailing commas.
fn parse_jsonc(bytes: &[u8]) -> Option<Value> {
    let src = std::str::from_utf8(bytes).ok()?;
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                while chars.next_if(|x| *x != '\n').is_some() {}
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for x in chars.by_ref() {
                    if prev == '*' && x == '/' {
                        break;
                    }
                    prev = x;
                }
            }
            _ => out.push(c),
        }
    }
    // Drop trailing commas: `,` followed (after whitespace) by `}` or `]`.
    let bytes: Vec<char> = out.chars().collect();
    let mut clean = String::with_capacity(out.len());
    let mut in_str = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            clean.push(c);
            if c == '\\' && i + 1 < bytes.len() {
                clean.push(bytes[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            clean.push(c);
        } else if c == ',' {
            let next = bytes[i + 1..].iter().find(|x| !x.is_whitespace());
            if !matches!(next, Some('}') | Some(']')) {
                clean.push(c);
            }
        } else {
            clean.push(c);
        }
        i += 1;
    }
    serde_json::from_str(&clean).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> JsProject {
        JsProject::parse(&files.iter().map(|(p, b)| (p.to_string(), b.as_bytes().to_vec())).collect::<Vec<_>>())
    }

    #[test]
    fn paths_extends_and_comments() {
        let p = project(&[
            ("tsconfig.base.json", r#"{ // shared
                "compilerOptions": { "baseUrl": ".", "paths": { "@lib/*": ["src/lib/*"], "@svc": ["src/svc/index.ts"], }, },
            }"#),
            ("app/tsconfig.json", r#"{ "extends": "../tsconfig.base", /* no paths here */ }"#),
        ]);
        assert_eq!(p.candidates("app/x.ts", "@lib/math"), vec!["src/lib/math", "@lib/math"]);
        assert_eq!(p.candidates("app/x.ts", "@svc")[0], "src/svc/index.ts");
        assert!(p.candidates("other/x.ts", "@lib/math").is_empty());
    }

    #[test]
    fn workspace_packages() {
        let p = project(&[
            ("packages/math/package.json", r#"{ "name": "@acme/math", "main": "./dist/prod/index.js" }"#),
            ("packages/ui/package.json", r#"{ "name": "ui" }"#),
        ]);
        let c = p.candidates("apps/web/a.ts", "@acme/math");
        assert!(c.contains(&"packages/math/src/prod/index.js".to_string()) && c.contains(&"packages/math/index".to_string()));
        assert_eq!(p.candidates("a.ts", "ui/button"), vec!["packages/ui/button", "packages/ui/src/button"]);
        assert!(p.candidates("a.ts", "react").is_empty());
    }
}
