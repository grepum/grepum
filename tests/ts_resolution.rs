//! TypeScript constructs (plus the shared local-shadowing rule) measured against the TypeScript compiler on real
//! repos (hono, excalidraw). Each expected edge was a miss, and each
//! forbidden one a wrong link, before 0.3.0.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn setup() -> PathBuf {
    let root = std::env::temp_dir().join(format!("grepum-ts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    // tsconfig `paths` declared in a base config the app extends (with comments
    // and trailing commas, as tsconfig allows).
    write(&root, "tsconfig.base.json", r#"{
      // shared options
      "compilerOptions": { "baseUrl": ".", "paths": { "@lib/*": ["src/lib/*"], }, },
    }"#);
    write(&root, "src/tsconfig.json", r#"{ "extends": "../tsconfig.base.json" }"#);
    // A workspace package whose published entry is build output.
    write(&root, "packages/geo/package.json", r#"{ "name": "@acme/geo", "main": "./dist/index.js" }"#);
    write(&root, "packages/geo/src/index.ts", "export function distance(a: number, b: number) { return b - a }\n");
    write(&root, "src/lib/math.ts", "\
export function addB(a: number) { return a }
function hidden() { return 1 }
export { hidden as exposed }
export const util = { fmtObj(x: number) { return x }, fmtArrow: (x: number) => x }
export function log() { return 0 }
export function onPick() { return 0 }
export interface Response { ok: boolean }
");
    write(&root, "src/lib/scene.ts", "class Scene { load() { return 1 } }\nexport default Scene\n");
    write(&root, "src/lib/reexport.ts", "import { addB } from './math'\nexport { addB }\n");
    write(&root, "src/lib/repo.ts", "\
export class Repo { find(id: string) { return id } }
export class Ctx { #r = new Repo(); get repo(): Repo { return this.#r } }
");
    write(&root, "src/ui/widget.tsx", "\
import { memo } from 'react'
export const Widget = (p: { title: string }) => <div>{p.title}</div>
export const Toolbar = memo((p: {}) => <Widget title=\"t\" />)
");
    write(&root, "src/app.tsx", "\
import { addB, exposed, util } from '@lib/math'
import { addB as viaReexport } from './lib/reexport'
import Scene from './lib/scene'
import { Repo, Ctx } from './lib/repo'
import { Widget, Toolbar } from './ui/widget'
import { distance } from '@acme/geo'
export class Service {
  constructor(private scene: Scene) {}
  viaDefaultExport() { return this.scene.load() }
}
export function viaPathAlias() { return addB(1) }
export function viaReexportOfImport() { return viaReexport(1) }
export function viaExportAs() { return exposed() }
export function viaObjectNamespace() { return util.fmtObj(1) + util.fmtArrow(2) }
export function viaGetter(c: Ctx) { return c.repo.find('x') }
export function viaGeneric<T extends Repo>(r: T) { return r.find('g') }
export function viaJsx() { return <Widget title=\"t\" /> }
export function viaMemoJsx() { return <Toolbar /> }
export function viaWorkspace() { return distance(1, 2) }
export function viaConsole() { console.log('x') }
export function viaParam(onPick: () => void) { onPick() }
export function viaDestructured(props: { onPick: () => void }) { const { onPick } = props; onPick() }
export function viaNested() { function inner() { return 1 } return inner() }
export function viaNew() { return new Response() }
");
    write(&root, "src/other.ts", "export function inner() { return 2 }\n");
    // Not TS, but the same shadowing rule: a local only hides a function from the
    // line after it's bound, so `let globs = globs()` still calls the function.
    write(&root, "src/lib.rs", "fn globs() -> u32 { 1 }\nfn run(f: fn() -> u32) -> u32 {\n    let globs = globs();\n    f() + globs\n}\nfn f() -> u32 { 2 }\n");
    for args in [&["init", "-q"][..], &["add", "-A"], &["-c", "user.name=t", "-c", "user.email=t@e", "commit", "-qm", "i"]] {
        assert!(Command::new("git").args(args).current_dir(&root).status().unwrap().success());
    }
    root
}

/// Call edges as `caller -> callee` qualified names.
fn edges(root: &Path) -> HashSet<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_grepum")).arg("export").current_dir(root).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let g: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let defs = g["defs"].as_array().unwrap();
    let qual = |i: &serde_json::Value| defs[i.as_u64().unwrap() as usize]["qual"].as_str().unwrap().to_string();
    g["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "call" && !e["from"].is_null())
        .map(|e| format!("{} -> {}", qual(&e["from"]), qual(&e["to"])))
        .collect()
}

#[test]
fn typescript_constructs_resolve_like_tsc() {
    let root = setup();
    let e = edges(&root);
    let expected = [
        ("viaPathAlias -> addB", "tsconfig paths via extends"),
        ("viaReexportOfImport -> addB", "export { importedName }"),
        ("viaExportAs -> hidden", "export { local as name }"),
        ("Service.viaDefaultExport -> Scene.load", "export default Name;"),
        ("viaObjectNamespace -> util.fmtObj", "object-literal method"),
        ("viaObjectNamespace -> util.fmtArrow", "object-literal arrow property"),
        ("viaGetter -> Repo.find", "getter return type"),
        ("viaGeneric -> Repo.find", "generic constraint"),
        ("viaJsx -> Widget", "JSX component"),
        ("viaMemoJsx -> Toolbar", "memo-wrapped component"),
        ("viaWorkspace -> distance", "workspace package"),
        ("viaNested -> viaNested.inner", "function nested in the caller"),
        ("run -> globs", "`let globs = globs()` calls the function"),
    ];
    let missing: Vec<String> = expected.iter().filter(|(x, _)| !e.contains(*x)).map(|(x, why)| format!("{x}  ({why})")).collect();
    let forbidden = [
        ("viaConsole -> log", "console.* is a global"),
        ("viaParam -> onPick", "a parameter shadows a same-named function"),
        ("viaDestructured -> onPick", "a destructured local shadows it too"),
        ("viaNested -> inner", "a nested function wins over a global one"),
        ("viaNew -> Response", "interfaces can't be constructed"),
        ("run -> f", "a parameter shadows a same-named function (Rust)"),
    ];
    let wrong: Vec<String> = forbidden.iter().filter(|(x, _)| e.contains(*x)).map(|(x, why)| format!("{x}  ({why})")).collect();
    let _ = std::fs::remove_dir_all(&root);
    assert!(missing.is_empty() && wrong.is_empty(), "missing:\n  {}\nwrong:\n  {}\nall edges:\n  {}",
        missing.join("\n  "), wrong.join("\n  "), { let mut v: Vec<_> = e.into_iter().collect(); v.sort(); v.join("\n  ") });
}
