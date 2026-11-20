// Emit every call / new / JSX site in a TS project with tsc's resolution of the callee.
// usage: node tsc_calls.js <repoRoot> <tsconfig> [srcDir...]  > sites.jsonl
const ts = require('typescript'); // resolved via NODE_PATH (see run.sh)
const fs = require('fs');
const path = require('path');

const [root, tsconfig, ...dirs] = process.argv.slice(2);
const ROOT = path.resolve(root);
const isTest = (f) => /(\.test\.|\.spec\.|__tests__|\/tests?\/|\/mocks?\/|\/fixtures?\/|benchmarks?\/|\.d\.ts$)/.test(f);

function walk(d, out) {
  for (const e of fs.readdirSync(d, { withFileTypes: true })) {
    if (e.name === 'node_modules' || e.name.startsWith('.')) continue;
    const p = path.join(d, e.name);
    if (e.isDirectory()) walk(p, out);
    else if (/\.(ts|tsx|mts|cts)$/.test(e.name) && !e.name.endsWith('.d.ts')) out.push(p);
  }
  return out;
}
const files = (dirs.length ? dirs : ['.']).flatMap((d) => walk(path.join(ROOT, d), []));
const parsed = ts.getParsedCommandLineOfConfigFile(path.resolve(ROOT, tsconfig), {}, {
  ...ts.sys, onUnRecoverableConfigFileDiagnostic: () => {},
});
const options = { ...(parsed ? parsed.options : {}), noEmit: true, skipLibCheck: true, allowJs: false };
const program = ts.createProgram(files, options);
const checker = program.getTypeChecker();
const rel = (f) => path.relative(ROOT, f).split(path.sep).join('/');
const line = (sf, pos) => sf.getLineAndCharacterOfPosition(pos).line + 1;

const FN_INIT = (n) => n && (ts.isArrowFunction(n) || ts.isFunctionExpression(n));
function declInfo(decl) {
  if (!decl) return null;
  let d = decl;
  if (ts.isConstructorDeclaration(d)) d = d.parent; // constructor -> class
  const sf = d.getSourceFile();
  const file = sf.fileName;
  if (file.includes('/node_modules/') || !file.startsWith(ROOT) || file.endsWith('.d.ts')) return { external: true };
  let kind = ts.SyntaxKind[d.kind];
  let eligible = ts.isFunctionDeclaration(d) || ts.isMethodDeclaration(d) || ts.isMethodSignature(d) ||
    ts.isClassDeclaration(d) || ts.isClassExpression(d) ||
    ((ts.isVariableDeclaration(d) || ts.isPropertyDeclaration(d) || ts.isPropertyAssignment(d)) && FN_INIT(d.initializer));
  if (ts.isFunctionExpression(d) || ts.isArrowFunction(d)) {
    // signature declared by the initializer itself: use its holder
    const p = d.parent;
    if (p && (ts.isVariableDeclaration(p) || ts.isPropertyDeclaration(p) || ts.isPropertyAssignment(p))) { d = p; kind = ts.SyntaxKind[d.kind]; eligible = true; }
  }
  const name = d.name ? d.name.getText() : (ts.isFunctionDeclaration(d) || ts.isClassDeclaration(d) ? 'default' : null);
  const container = d.parent && (ts.isClassLike(d.parent) || ts.isInterfaceDeclaration(d.parent)) && d.parent.name ? d.parent.name.getText() : null;
  const containerKind = d.parent ? ts.SyntaxKind[d.parent.kind] : null;
  const abstract = !!(ts.getCombinedModifierFlags(d) & ts.ModifierFlags.Abstract);
  return { file: rel(file), line: line(sf, (d.name || d).getStart()), name, kind, eligible: !!eligible, container, containerKind, abstract };
}

function follow(sym) {
  let hops = 0;
  while (sym && sym.flags & ts.SymbolFlags.Alias && hops < 10) {
    const next = checker.getImmediateAliasedSymbol(sym);
    if (!next) break;
    sym = next; hops++;
  }
  return { sym, hops };
}

function importOf(sym) {
  const d = sym && sym.declarations && sym.declarations[0];
  if (!d) return null;
  let n = d;
  while (n && !ts.isImportDeclaration(n) && !ts.isImportEqualsDeclaration(n) && !ts.isSourceFile(n)) n = n.parent;
  if (n && ts.isImportDeclaration(n)) {
    return { spec: n.moduleSpecifier.text, form: ts.SyntaxKind[d.kind], typeOnly: !!(n.importClause && n.importClause.isTypeOnly) };
  }
  return null;
}

function shape(expr) {
  // describe the receiver of a property call
  if (ts.isParenthesizedExpression(expr)) return shape(expr.expression);
  if (expr.kind === ts.SyntaxKind.ThisKeyword) return 'this';
  if (expr.kind === ts.SyntaxKind.SuperKeyword) return 'super';
  if (ts.isIdentifier(expr)) {
    const s = checker.getSymbolAtLocation(expr);
    const d = s && s.declarations && s.declarations[0];
    if (!d) return 'ident?';
    if (ts.isNamespaceImport(d)) return 'ns-import';
    if (ts.isImportSpecifier(d) || ts.isImportClause(d)) return 'imported';
    if (ts.isParameter(d)) return 'param';
    if (ts.isClassDeclaration(d)) return 'class-static';
    if (ts.isVariableDeclaration(d)) {
      if (d.type) return 'local-annotated';
      const init = d.initializer;
      if (init && ts.isNewExpression(init)) return 'local-new';
      if (init && (ts.isCallExpression(init) || ts.isAwaitExpression(init))) return 'local-from-call';
      return 'local-other';
    }
    return 'ident:' + ts.SyntaxKind[d.kind];
  }
  if (ts.isPropertyAccessExpression(expr)) {
    if (expr.expression.kind === ts.SyntaxKind.ThisKeyword) {
      const s = checker.getSymbolAtLocation(expr.name);
      const d = s && s.declarations && s.declarations[0];
      if (d && ts.isParameter(d)) return 'this.ctorParamProp';
      if (d && ts.isPropertyDeclaration(d)) return d.type ? 'this.field-annotated' : 'this.field-inferred';
      return 'this.member?';
    }
    return 'chain';
  }
  if (ts.isCallExpression(expr) || ts.isAwaitExpression(expr)) return 'call-result';
  if (ts.isElementAccessExpression(expr)) return 'element';
  if (ts.isNonNullExpression(expr) || ts.isAsExpression(expr)) return 'cast';
  return ts.SyntaxKind[expr.kind];
}

const out = [];
for (const sf of program.getSourceFiles()) {
  const f = sf.fileName;
  if (!f.startsWith(ROOT) || f.includes('/node_modules/') || isTest(rel(f))) continue;
  const visit = (node) => {
    let kind = null, callee = null, nameNode = null;
    if (ts.isCallExpression(node)) { kind = 'call'; callee = node.expression; }
    else if (ts.isNewExpression(node)) { kind = 'new'; callee = node.expression; }
    else if (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) {
      const t = node.tagName.getText();
      if (/^[A-Z]/.test(t) || t.includes('.')) { kind = 'jsx'; callee = node.tagName; }
    } else if (ts.isTaggedTemplateExpression(node)) { kind = 'tagged'; callee = node.tag; }
    if (kind && callee && callee.kind !== ts.SyntaxKind.ImportKeyword && callee.kind !== ts.SyntaxKind.SuperKeyword) {
      let c = callee;
      while (ts.isParenthesizedExpression(c) || ts.isNonNullExpression(c)) c = c.expression;
      nameNode = ts.isPropertyAccessExpression(c) ? c.name : c;
      let decl = null, via = 'sig';
      if (kind === 'call' || kind === 'tagged') {
        try { const sig = checker.getResolvedSignature(node); decl = sig && sig.declaration; } catch {}
      }
      if (decl && (ts.isJSDocSignature && ts.isJSDocSignature(decl))) decl = null;
      // signatures from function types / interfaces with call signatures: use the symbol instead
      if (decl && (ts.isFunctionTypeNode(decl) || ts.isCallSignatureDeclaration(decl) || ts.isConstructSignatureDeclaration(decl))) decl = null;
      let hops = 0, imp = null;
      const s0 = checker.getSymbolAtLocation(nameNode);
      if (s0) { imp = importOf(s0); const r = follow(s0); hops = r.hops; if (!decl) { via = 'sym'; decl = r.sym && (r.sym.valueDeclaration || (r.sym.declarations || [])[0]); } }
      if (ts.isPropertyAccessExpression(c)) {
        const left = c.expression;
        const ls = ts.isIdentifier(left) ? checker.getSymbolAtLocation(left) : null;
        if (ls && !imp) imp = importOf(ls);
      }
      const info = declInfo(decl);
      out.push({
        file: rel(f), line: line(sf, node.getStart()), nline: line(sf, nameNode.getStart()), end: line(sf, node.getEnd()), kind,
        text: c.getText().slice(0, 80),
        recv: ts.isPropertyAccessExpression(c) ? shape(c.expression) : (ts.isIdentifier(c) ? 'bare' : ts.SyntaxKind[c.kind]),
        optional: !!(node.questionDotToken || (ts.isPropertyAccessExpression(c) && c.questionDotToken)),
        hops, imp, via,
        status: !info ? 'unresolved' : info.external ? 'external' : 'project',
        decl: info && !info.external ? info : null,
      });
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
}
for (const o of out) process.stdout.write(JSON.stringify(o) + '\n');
process.stderr.write(`files=${files.length} sites=${out.length}\n`);
