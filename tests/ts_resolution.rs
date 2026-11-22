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

#[test]
fn values_built_from_external_imports_do_not_link_to_local_methods() {
    let root = std::env::temp_dir().join(format!("grepum-ext-origin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);

    // Local classes whose method names are unique in the repository.
    write(&root, "src/vault.ts", "\
export class Vault {
  safeParse<T>(input: string): T {
    return JSON.parse(input);
  }
  load(raw: string) {
    return this.safeParse(raw);
  }
}
export class Ctx {
  constructor(public vault: Vault) {}
}
");
    // Re-exported external package and external-derived binding.
    write(&root, "src/barrel.ts", "\
export { z } from 'zod';
export { configSchema } from './config';
");
    write(&root, "src/config.ts", "\
import { z, createSchema } from 'zod';
import * as ext from 'external-pkg';
import { Vault, Ctx } from './vault';

export const configSchema = z.object({ port: z.number() });
const chainedSchema = z.object({ port: z.number() }).strict().optional();
const extendedSchema = configSchema.extend({ host: z.string() });
const aliasSchema = configSchema;
const portSchema = configSchema.shape.port;
const directSchema = createSchema();
const extVault = new ext.Vault();

export function parseConfig(input: unknown) { return configSchema.safeParse(input); }
export function parseInline(input: unknown) { return z.string().safeParse(input); }
export function parseChained(input: unknown) { return chainedSchema.safeParse(input); }
export function parseExtended(input: unknown) { return extendedSchema.safeParse(input); }
export function parseAlias(input: unknown) { return aliasSchema.safeParse(input); }
export function parseProp(input: unknown) { return portSchema.safeParse(input); }
export function parseDirect(input: unknown) { return directSchema.safeParse(input); }
export function parseDirectInline(input: unknown) { return createSchema().safeParse(input); }
export function parseExtNew(input: unknown) { return extVault.safeParse(input); }
export function parseLocalConst(input: unknown) {
  const local = z.object({ port: z.number() });
  return local.safeParse(input);
}

export class ConfigLoader {
  schema = z.object({ port: z.number() });
  run(input: unknown) { return this.schema.safeParse(input); }
}

// Positive controls: real local receivers still resolve.
export function parseRealVault(raw: string) {
  const v = new Vault();
  return v.safeParse(raw);
}
export function parseViaFieldAlias(ctx: Ctx, raw: string) {
  const v = ctx.vault;
  return v.safeParse(raw);
}
");
    write(&root, "src/consumer.ts", "\
import { z, configSchema } from './barrel';

const fromBarrelZ = z.object({ ok: z.boolean() });

export function viaImportedConst(input: unknown) { return configSchema.safeParse(input); }
export function viaBarrelExternal(input: unknown) { return fromBarrelZ.safeParse(input); }
");

    // Python, Go and Rust: same external-origin rule.
    write(&root, "app/local.py", "\
class LocalPy:
    def unique_py_parse(self, x):
        return x
");
    write(&root, "app/use_ext.py", "\
import ext_pkg
from ext_pkg import build_schema

schema = ext_pkg.object()
direct = build_schema()

def py_ext_member(x):
    return schema.unique_py_parse(x)

def py_ext_direct(x):
    return direct.unique_py_parse(x)
");
    write(&root, "pkg/local.go", "\
package pkg
type LocalGo struct{}
func (l *LocalGo) UniqueGoParse() {}
");
    write(&root, "pkg/use_ext.go", "\
package pkg
import \"example.com/thirdparty/schema\"
func GoExt() {
    s := schema.New()
    s.UniqueGoParse()
}
");
    write(&root, "src/lib.rs", "\
use external_crate::Builder;
pub struct LocalRs;
impl LocalRs {
    pub fn unique_rs_parse(&self) {}
}
pub fn rs_ext() {
    let b = Builder::new();
    b.unique_rs_parse();
}
");

    for args in [&["init", "-q"][..], &["add", "-A"], &["-c", "user.name=t", "-c", "user.email=t@e", "commit", "-qm", "init"]] {
        assert!(Command::new("git").args(args).current_dir(&root).status().unwrap().success());
    }

    let e = edges(&root);
    let expected = [
        ("Vault.load -> Vault.safeParse", "this.safeParse"),
        ("parseRealVault -> Vault.safeParse", "const v = new Vault()"),
        ("parseViaFieldAlias -> Vault.safeParse", "const v = ctx.vault"),
    ];
    let missing: Vec<String> = expected.iter().filter(|(x, _)| !e.contains(*x)).map(|(x, why)| format!("{x}  ({why})")).collect();

    let forbidden = [
        ("parseConfig -> Vault.safeParse", "const initialized from external import"),
        ("parseInline -> Vault.safeParse", "chained call on external import"),
        ("parseChained -> Vault.safeParse", "chained calls in const initializer"),
        ("parseExtended -> Vault.safeParse", "derived from external-origin const"),
        ("parseAlias -> Vault.safeParse", "alias of external-origin const"),
        ("parseProp -> Vault.safeParse", "property of external-origin const"),
        ("parseDirect -> Vault.safeParse", "direct call to external imported function"),
        ("parseDirectInline -> Vault.safeParse", "inline call to external imported function"),
        ("parseExtNew -> Vault.safeParse", "new ext.Vault() from external namespace"),
        ("parseLocalConst -> Vault.safeParse", "function-local const from external import"),
        ("ConfigLoader.run -> Vault.safeParse", "class field initialized from external import"),
        ("viaImportedConst -> Vault.safeParse", "imported const from another file"),
        ("viaBarrelExternal -> Vault.safeParse", "re-exported external import through barrel"),
        ("py_ext_member -> LocalPy.unique_py_parse", "Python external module call"),
        ("py_ext_direct -> LocalPy.unique_py_parse", "Python external imported function call"),
        ("GoExt -> LocalGo.UniqueGoParse", "Go external package call"),
        ("rs_ext -> LocalRs.unique_rs_parse", "Rust external crate call"),
    ];
    let wrong: Vec<String> = forbidden.iter().filter(|(x, _)| e.contains(*x)).map(|(x, why)| format!("{x}  ({why})")).collect();

    let _ = std::fs::remove_dir_all(&root);
    assert!(missing.is_empty() && wrong.is_empty(), "missing:\n  {}\nwrong:\n  {}\nall edges:\n  {}",
        missing.join("\n  "), wrong.join("\n  "), { let mut v: Vec<_> = e.into_iter().collect(); v.sort(); v.join("\n  ") });
}
