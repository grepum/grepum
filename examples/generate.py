#!/usr/bin/env python3
"""Generate the two fake benchmark projects plus their ground-truth call graphs.

    python3 examples/generate.py [--entities-ts 40] [--entities-py 60]

Writes:
    examples/projects/shopfront/     TypeScript service (domain/repository/service/routes layers)
    examples/projects/datapipe/      Python data pipeline (stages/connectors/models/pipelines)
    examples/projects/arena/         Unreal Engine 5 style C++ game module (Public/Private, UCLASS, RPCs)
    examples/truth/<project>.json    every definition and every call edge, with line numbers

The code uses everyday idioms: named/namespace/barrel imports, relative imports,
constructor-injected dependencies, inheritance, factories with return types,
same-named methods on many classes, and calls on external objects (`app.get`,
`dict.get`) that must NOT be linked to local methods. A few calls are
intentionally "hard" (untyped parameters, `super()`, chained calls); they are
in the truth set and tagged so you can see who gets them.
"""
from __future__ import annotations

import argparse
import json
import shutil
from pathlib import Path

ROOT = Path(__file__).resolve().parent

ENTITIES = [
    "order", "invoice", "customer", "product", "shipment", "coupon", "cart", "payment", "refund", "review",
    "warehouse", "supplier", "category", "address", "subscription", "plan", "ticket", "message", "notification",
    "report", "session", "token", "audit", "webhook", "discount", "tax", "currency", "region", "store",
    "inventory", "price", "bundle", "gift", "loyalty", "carrier", "label", "batch", "vendor", "contract",
    "employee", "team", "project", "task", "comment", "attachment", "device", "sensor", "reading", "alert",
    "metric", "dashboard", "widget", "theme", "locale", "translation", "feature", "experiment", "variant",
    "cohort", "segment",
]
CONNECTORS = ["s3", "gcs", "postgres", "kafka", "sftp", "salesforce", "stripe", "shopify"]


def pascal(s: str) -> str:
    return "".join(p.capitalize() for p in s.split("_"))


class Truth:
    def __init__(self) -> None:
        self.defs: dict[tuple[str, str], dict] = {}
        self.calls: list[dict] = []
        # C++: header prototype -> out-of-line definition (a link to either is correct).
        self.aliases: list[tuple[tuple[str, str], tuple[str, str]]] = []

    def define(self, path: str, qual: str, line: int, kind: str) -> tuple[str, str]:
        key = (path, qual)
        self.defs[key] = {"file": path, "qual": qual, "line": line, "kind": kind}
        return key

    def call(self, caller: tuple[str, str], callee: tuple[str, str], hard: str | None = None) -> None:
        self.calls.append({"caller": caller, "callee": callee, "hard": hard})

    def dump(self, path: Path) -> None:
        missing = [c for c in self.calls if c["callee"] not in self.defs or c["caller"] not in self.defs]
        assert not missing, f"truth references unknown defs: {missing[:3]}"
        seen = set()
        edges = []
        for c in self.calls:
            a, b = self.defs[c["caller"]], self.defs[c["callee"]]
            k = (a["file"], a["line"], b["file"], b["line"])
            if k in seen:
                continue
            seen.add(k)
            edges.append({
                "caller": f'{a["file"]}:{a["line"]}', "caller_qual": a["qual"],
                "callee": f'{b["file"]}:{b["line"]}', "callee_qual": b["qual"],
                "hard": c["hard"],
            })
        aliases = {}
        for decl, impl in self.aliases:
            a, b = self.defs[decl], self.defs[impl]
            aliases[f'{a["file"]}:{a["line"]}'] = f'{b["file"]}:{b["line"]}'
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({"defs": list(self.defs.values()), "calls": edges, "aliases": aliases}, indent=1))


class Src:
    """A file being written; `add` returns the 1-based line number of the line."""

    def __init__(self, root: Path, rel: str) -> None:
        self.root, self.rel, self.lines = root, rel, []

    def add(self, *lines: str) -> int:
        first = len(self.lines) + 1
        self.lines.extend(lines)
        return first

    def write(self) -> None:
        p = self.root / self.rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text("\n".join(self.lines) + "\n")


# --------------------------------------------------------------------------- TypeScript


def gen_shopfront(n: int) -> None:
    root = ROOT / "projects" / "shopfront"
    shutil.rmtree(root, ignore_errors=True)
    T = Truth()
    files: list[Src] = []

    def src(rel: str) -> Src:
        s = Src(root, rel)
        files.append(s)
        return s

    # lib/log.ts
    f = src("src/lib/log.ts")
    f.add("// Tiny structured logger.", "")
    LOG = {}
    for lvl in ["info", "warn", "error", "debug"]:
        l = f.add(f"export function {lvl}(message: string, meta?: Record<string, unknown>): void {{")
        LOG[lvl] = T.define(f.rel, lvl, l, "fn")
        f.add(f"  const line = `[{lvl}] ${{new Date().toISOString()}} ${{message}}`;",
              "  if (meta) {", "    console.log(line, JSON.stringify(meta));", "  } else {",
              "    console.log(line);", "  }", "}", "")

    # lib/money.ts
    f = src("src/lib/money.ts")
    l = f.add("export function roundCents(value: number): number {")
    ROUND = T.define(f.rel, "roundCents", l, "fn")
    f.add("  return Math.round(value * 100) / 100;", "}", "")
    l = f.add("export function addTax(amount: number, rate: number): number {")
    ADDTAX = T.define(f.rel, "addTax", l, "fn")
    f.add("  const gross = amount * (1 + rate);", "  return roundCents(gross);", "}", "")
    T.call(ADDTAX, ROUND)
    l = f.add('export function formatMoney(amount: number, currency = "EUR"): string {')
    FMT = T.define(f.rel, "formatMoney", l, "fn")
    f.add("  const rounded = roundCents(amount);", "  return `${rounded.toFixed(2)} ${currency}`;", "}")
    T.call(FMT, ROUND)

    # lib/strings.ts
    f = src("src/lib/strings.ts")
    l = f.add("export const slugify = (input: string): string => {")
    SLUG = T.define(f.rel, "slugify", l, "fn")
    f.add('  return input.toLowerCase().trim().replace(/[^a-z0-9]+/g, "-");', "};", "")
    l = f.add("export function capitalize(word: string): string {")
    CAP = T.define(f.rel, "capitalize", l, "fn")
    f.add("  return word.charAt(0).toUpperCase() + word.slice(1);", "}", "")
    l = f.add("export const titleCase = (input: string): string =>")
    TITLE = T.define(f.rel, "titleCase", l, "fn")
    f.add('  input.split(" ").map((w) => capitalize(w)).join(" ");')
    T.call(TITLE, CAP)

    # lib/validate.ts
    f = src("src/lib/validate.ts")
    l = f.add("export class ValidationError extends Error {}", "")
    VERR = T.define(f.rel, "ValidationError", l, "class")
    l = f.add("export class Validator {")
    VAL = T.define(f.rel, "Validator", l, "class")
    f.add("  private errors: string[] = [];", "")
    l = f.add("  requireString(value: unknown, field: string): void {")
    VREQS = T.define(f.rel, "Validator.requireString", l, "method")
    f.add('    if (typeof value !== "string" || value.length === 0) {', "      this.fail(field);", "    }", "  }", "")
    l = f.add("  requirePositive(value: number, field: string): void {")
    VREQP = T.define(f.rel, "Validator.requirePositive", l, "method")
    f.add("    if (!(value > 0)) {", "      this.fail(field);", "    }", "  }", "")
    l = f.add("  fail(field: string): void {")
    VFAIL = T.define(f.rel, "Validator.fail", l, "method")
    f.add("    this.errors.push(`${field} is invalid`);", "  }", "")
    l = f.add("  check(): void {")
    VCHECK = T.define(f.rel, "Validator.check", l, "method")
    f.add("    if (this.errors.length > 0) {", '      throw new ValidationError(this.errors.join(", "));', "    }", "  }", "}")
    T.call(VREQS, VFAIL)
    T.call(VREQP, VFAIL)
    T.call(VCHECK, VERR)

    # lib/db.ts
    f = src("src/lib/db.ts")
    f.add('import * as log from "./log";', "", "export interface Row {", "  [key: string]: unknown;", "}", "")
    l = f.add("export class Database {")
    DB = T.define(f.rel, "Database", l, "class")
    f.add("  constructor(private readonly url: string) {}", "")
    l = f.add("  query(sql: string, params: unknown[] = []): Row[] {")
    DBQ = T.define(f.rel, "Database.query", l, "method")
    f.add("    log.debug(sql, { url: this.url, params: params.length });", "    return [];", "  }", "")
    T.call(DBQ, LOG["debug"])
    l = f.add("  transaction<T>(fn: () => T): T {")
    DBT = T.define(f.rel, "Database.transaction", l, "method")
    f.add('    this.query("BEGIN");', "    const out = fn();", '    this.query("COMMIT");', "    return out;", "  }", "}", "")
    T.call(DBT, DBQ)
    l = f.add('export function connect(url = "postgres://localhost/shop"): Database {')
    CONNECT = T.define(f.rel, "connect", l, "fn")
    f.add("  return new Database(url);", "}")
    T.call(CONNECT, DB)

    # lib/cache.ts
    f = src("src/lib/cache.ts")
    l = f.add("export class Cache<T> {")
    CACHE = T.define(f.rel, "Cache", l, "class")
    f.add("  private store = new Map<string, T>();", "")
    CM = {}
    for meth, sig, body in [
        ("get", "get(key: string): T | undefined {", "    return this.store.get(key);"),
        ("set", "set(key: string, value: T): void {", "    this.store.set(key, value);"),
        ("has", "has(key: string): boolean {", "    return this.store.has(key);"),
        ("clear", "clear(): void {", "    this.store.clear();"),
    ]:
        l = f.add(f"  {sig}")
        CM[meth] = T.define(f.rel, f"Cache.{meth}", l, "method")
        f.add(body, "  }", "")
    f.add("}")

    f = src("src/lib/index.ts")
    f.add('export * from "./money";', 'export * from "./strings";', 'export { Validator } from "./validate";')

    # domain/base/repository.ts
    f = src("src/domain/base/repository.ts")
    f.add('import { Database, Row } from "../../lib/db";', "")
    l = f.add("export abstract class BaseRepository<T> {")
    BASE = T.define(f.rel, "BaseRepository", l, "class")
    f.add("  constructor(protected readonly db: Database) {}", "")
    l = f.add("  protected query(sql: string, params: unknown[] = []): Row[] {")
    BQ = T.define(f.rel, "BaseRepository.query", l, "method")
    f.add("    return this.db.query(sql, params);", "  }", "")
    T.call(BQ, DBQ)
    l = f.add("  count(table: string): number {")
    BCOUNT = T.define(f.rel, "BaseRepository.count", l, "method")
    f.add("    const rows = this.query(`SELECT count(*) FROM ${table}`);", "    return rows.length;", "  }", "")
    T.call(BCOUNT, BQ)
    l = f.add("  abstract findById(id: string): T | undefined;")
    T.define(f.rel, "BaseRepository.findById", l, "method")
    f.add("}")

    ents = ENTITIES[:n]
    SVC, REPO, FACT = {}, {}, {}
    # First pass: define per-entity symbols so peers can reference each other.
    plan = []
    for i, e in enumerate(ents):
        plan.append((i, e, pascal(e), ents[(i + 1) % n], pascal(ents[(i + 1) % n])))

    model_keys, repo_keys, svc_keys = {}, {}, {}
    for i, e, E, peer, P in plan:
        d = f"src/domain/{e}"
        # model.ts
        f = src(f"{d}/model.ts")
        f.add('import { Validator } from "../../lib/validate";', 'import { formatMoney, addTax } from "../../lib/money";', "")
        f.add(f"export interface {E} {{", "  id: string;", "  name: string;", "  amount: number;", "  createdAt: Date;", "}", "")
        l = f.add(f"export function create{E}(input: Partial<{E}>): {E} {{")
        CREATE = T.define(f.rel, f"create{E}", l, "fn")
        f.add("  const v = new Validator();", '  v.requireString(input.name, "name");',
              '  v.requirePositive(input.amount ?? 0, "amount");', "  v.check();",
              "  return {", "    id: crypto.randomUUID(),", "    name: input.name as string,",
              "    amount: input.amount as number,", "    createdAt: new Date(),", "  };", "}", "")
        for t in (VAL, VREQS, VREQP, VCHECK):
            T.call(CREATE, t)
        l = f.add(f"export function {e}Total(item: {E}, taxRate = 0.19): string {{")
        TOTAL = T.define(f.rel, f"{e}Total", l, "fn")
        f.add("  const gross = addTax(item.amount, taxRate);", "  return formatMoney(gross);", "}")
        T.call(TOTAL, ADDTAX)
        T.call(TOTAL, FMT)
        model_keys[e] = (CREATE, TOTAL)

        # repository.ts
        f = src(f"{d}/repository.ts")
        f.add('import { BaseRepository } from "../base/repository";', f'import {{ {E} }} from "./model";',
              'import * as log from "../../lib/log";', "")
        l = f.add(f"export class {E}Repository extends BaseRepository<{E}> {{")
        R = T.define(f.rel, f"{E}Repository", l, "class")
        RM = {}
        l = f.add(f"  findById(id: string): {E} | undefined {{")
        RM["findById"] = T.define(f.rel, f"{E}Repository.findById", l, "method")
        f.add(f'    const rows = this.query("SELECT * FROM {e}s WHERE id = $1", [id]);', f"    return rows[0] as {E} | undefined;", "  }", "")
        T.call(RM["findById"], BQ)
        l = f.add(f"  save(item: {E}): void {{")
        RM["save"] = T.define(f.rel, f"{E}Repository.save", l, "method")
        f.add(f'    log.info("saving {e}", {{ id: item.id }});', f'    this.query("INSERT INTO {e}s VALUES ($1)", [item.id]);', "  }", "")
        T.call(RM["save"], LOG["info"])
        T.call(RM["save"], BQ)
        l = f.add(f"  list(limit = 50): {E}[] {{")
        RM["list"] = T.define(f.rel, f"{E}Repository.list", l, "method")
        f.add(f'    return this.query("SELECT * FROM {e}s LIMIT $1", [limit]) as {E}[];', "  }", "")
        T.call(RM["list"], BQ)
        l = f.add("  remove(id: string): boolean {")
        RM["remove"] = T.define(f.rel, f"{E}Repository.remove", l, "method")
        f.add("    const existing = this.findById(id);", "    if (!existing) {", "      return false;", "    }",
              f'    this.query("DELETE FROM {e}s WHERE id = $1", [id]);', "    return true;", "  }", "}")
        T.call(RM["remove"], RM["findById"])
        T.call(RM["remove"], BQ)
        repo_keys[e] = (R, RM)

        f = src(f"{d}/index.ts")
        f.add('export * from "./model";', 'export * from "./repository";', 'export * from "./service";')

    for i, e, E, peer, P in plan:
        d = f"src/domain/{e}"
        CREATE, TOTAL = model_keys[e]
        R, RM = repo_keys[e]
        f = src(f"{d}/service.ts")
        f.add('import { Cache } from "../../lib/cache";', 'import * as log from "../../lib/log";',
              'import { slugify } from "../../lib";', f'import {{ {E}, create{E}, {e}Total }} from "./model";',
              f'import {{ {E}Repository }} from "./repository";', f'import {{ {P}Service }} from "../{peer}/service";', "")
        l = f.add(f"export class {E}Service {{")
        S = T.define(f.rel, f"{E}Service", l, "class")
        f.add(f"  private cache: Cache<{E}> = new Cache<{E}>();", "")
        l = f.add("  constructor(", f"    private readonly repo: {E}Repository,", f"    private readonly peers: {P}Service | null = null,", "  ) {}", "")
        T.define(f.rel, f"{E}Service.constructor", l, "method")
        SM = {}
        l = f.add(f"  get(id: string): {E} | undefined {{")
        SM["get"] = T.define(f.rel, f"{E}Service.get", l, "method")
        f.add("    const hit = this.cache.get(id);", "    if (hit) {", "      return hit;", "    }",
              "    const found = this.repo.findById(id);", "    if (found) {", "      this.cache.set(id, found);", "    }",
              "    return found;", "  }", "")
        for t in (CM["get"], RM["findById"], CM["set"]):
            T.call(SM["get"], t)
        l = f.add(f"  create(input: Partial<{E}>): string {{")
        SM["create"] = T.define(f.rel, f"{E}Service.create", l, "method")
        f.add(f"    const item = create{E}(input);", "    this.repo.save(item);",
              "    log.info(`created ${slugify(item.name)}`);", f"    return {e}Total(item);", "  }", "")
        for t in (CREATE, RM["save"], LOG["info"], SLUG, TOTAL):
            T.call(SM["create"], t)
        l = f.add("  archive(id: string): boolean {")
        SM["archive"] = T.define(f.rel, f"{E}Service.archive", l, "method")
        f.add(f'    log.warn("archiving {e}", {{ id }});', "    this.cache.clear();", "    return this.repo.remove(id);", "  }", "")
        for t in (LOG["warn"], CM["clear"], RM["remove"]):
            T.call(SM["archive"], t)
        l = f.add("  summary(id: string): string {")
        SM["summary"] = T.define(f.rel, f"{E}Service.summary", l, "method")
        f.add("    const item = this.get(id);", "    const linked = this.peers?.get(id);",
              '    return item ? `${item.name}${linked ? " (linked)" : ""}` : "missing";', "  }", "}")
        T.call(SM["summary"], SM["get"])
        SVC[e] = (S, SM)
    # peer edges need the peer's SM, so add after all services exist
    for i, e, E, peer, P in plan:
        T.call(SVC[e][1]["summary"], SVC[peer][1]["get"])

    # api/factory.ts
    f = src("src/api/factory.ts")
    for i, e, E, peer, P in plan:
        f.add(f'import {{ {E}Service, {E}Repository }} from "../domain/{e}";')
    f.add('import { connect } from "../lib/db";', "")
    for i, e, E, peer, P in plan:
        l = f.add(f"export function make{E}Service(): {E}Service {{")
        FK = T.define(f.rel, f"make{E}Service", l, "fn")
        f.add(f"  return new {E}Service(new {E}Repository(connect()));", "}", "")
        for t in (SVC[e][0], repo_keys[e][0], CONNECT):
            T.call(FK, t)
        FACT[e] = FK

    # api/routes/<e>.ts
    ROUTES = {}
    for i, e, E, peer, P in plan:
        S, SM = SVC[e]
        R, RM = repo_keys[e]
        f = src(f"src/api/routes/{e}.ts")
        f.add('import type { Express } from "express";', f'import {{ {E}Service, {E}Repository }} from "../../domain/{e}";',
              'import { connect } from "../../lib/db";', f'import {{ make{E}Service }} from "../factory";',
              'import { titleCase } from "../../lib";', "")
        hard = i % 5 == 0
        if hard:
            l = f.add(f"export function audit{E}(svc: any): string {{")
            AUD = T.define(f.rel, f"audit{E}", l, "fn")
            f.add('  return svc.summary("probe");', "}", "")
            T.call(AUD, SM["summary"], hard="untyped-param")
        l = f.add(f"export function register{E}Routes(app: Express): void {{")
        REG = T.define(f.rel, f"register{E}Routes", l, "fn")
        ROUTES[e] = REG
        if i % 2 == 0:
            f.add(f"  const service = make{E}Service();")
            T.call(REG, FACT[e])
        else:
            f.add(f"  const repo = new {E}Repository(connect());", f"  const service = new {E}Service(repo);")
            for t in (R, CONNECT, S):
                T.call(REG, t)
        f.add(f'  app.get("/{e}s/:id", (req, res) => {{', "    res.json(service.get(req.params.id));", "  });",
              f'  app.post("/{e}s", (req, res) => {{', f'    res.json({{ total: service.create(req.body), title: titleCase("{e}") }});', "  });",
              f'  app.delete("/{e}s/:id", (req, res) => {{', "    res.json({ ok: service.archive(req.params.id) });", "  });")
        for t in (SM["get"], SM["create"], TITLE, SM["archive"]):
            T.call(REG, t)
        if hard:
            f.add(f"  audit{E}(service);")
            T.call(REG, AUD)
        f.add("}")

    f = src("src/index.ts")
    f.add('import express from "express";')
    for i, e, E, peer, P in plan:
        f.add(f'import {{ register{E}Routes }} from "./api/routes/{e}";')
    f.add("")
    l = f.add("export function main(): void {")
    MAIN = T.define(f.rel, "main", l, "fn")
    f.add("  const app = express();")
    for i, e, E, peer, P in plan:
        f.add(f"  register{E}Routes(app);")
        T.call(MAIN, ROUTES[e])
    f.add("  app.listen(3000);", "}", "", "main();")

    (root / "package.json").parent.mkdir(parents=True, exist_ok=True)
    (root / "package.json").write_text(json.dumps({"name": "shopfront", "private": True, "dependencies": {"express": "^4"}}, indent=2) + "\n")
    for s in files:
        s.write()
    T.dump(ROOT / "truth" / "shopfront.json")
    print(f"shopfront: {len(files)} files, {sum(len(s.lines) for s in files)} lines, {len(T.defs)} defs, {len(T.calls)} calls")


# --------------------------------------------------------------------------- Python


def gen_datapipe(n: int) -> None:
    root = ROOT / "projects" / "datapipe"
    shutil.rmtree(root, ignore_errors=True)
    T = Truth()
    files: list[Src] = []

    def src(rel: str) -> Src:
        s = Src(root, rel)
        files.append(s)
        return s

    src("datapipe/__init__.py").add('"""datapipe: a small batch ETL framework."""')

    f = src("datapipe/utils/text.py")
    f.add("import re", "", '_WS = re.compile(r"\\s+")', "", "")
    l = f.add("def normalize(value: str) -> str:")
    NORM = T.define(f.rel, "normalize", l, "fn")
    f.add('    """Lower-case and collapse whitespace."""', '    return _WS.sub(" ", value.strip().lower())', "", "")
    l = f.add("def tokenize(value: str) -> list:")
    TOK = T.define(f.rel, "tokenize", l, "fn")
    f.add('    return normalize(value).split(" ")', "", "")
    T.call(TOK, NORM)
    l = f.add("def truncate(value: str, limit: int = 80) -> str:")
    TRUNC = T.define(f.rel, "truncate", l, "fn")
    f.add("    clean = normalize(value)", '    return clean if len(clean) <= limit else clean[: limit - 1] + "~"')
    T.call(TRUNC, NORM)

    f = src("datapipe/utils/dates.py")
    f.add("from datetime import datetime", "", "")
    l = f.add("def parse_date(value: str) -> datetime:")
    PARSE = T.define(f.rel, "parse_date", l, "fn")
    f.add("    return datetime.fromisoformat(value)", "", "")
    l = f.add("def to_iso(value: datetime) -> str:")
    ISO = T.define(f.rel, "to_iso", l, "fn")
    f.add("    return value.isoformat()", "", "")
    l = f.add("def days_between(start: str, end: str) -> int:")
    DAYS = T.define(f.rel, "days_between", l, "fn")
    f.add("    return (parse_date(end) - parse_date(start)).days")
    T.call(DAYS, PARSE)

    f = src("datapipe/utils/__init__.py")
    f.add("from .text import normalize, tokenize, truncate", "from .dates import parse_date, to_iso", "",
          '__all__ = ["normalize", "tokenize", "truncate", "parse_date", "to_iso"]')

    src("datapipe/core/__init__.py").add("")
    f = src("datapipe/core/metrics.py")
    l = f.add("class Metrics:")
    MET = T.define(f.rel, "Metrics", l, "class")
    l = f.add("    def __init__(self):")
    T.define(f.rel, "Metrics.__init__", l, "method")
    f.add("        self.counters = {}", "        self.timings = []", "")
    l = f.add("    def incr(self, key: str, by: int = 1) -> None:")
    INCR = T.define(f.rel, "Metrics.incr", l, "method")
    f.add("        self.counters[key] = self.counters.get(key, 0) + by", "")
    l = f.add("    def timing(self, key: str, ms: float) -> None:")
    TIMING = T.define(f.rel, "Metrics.timing", l, "method")
    f.add("        self.timings.append((key, ms))", "        self.incr(key)", "")
    T.call(TIMING, INCR)
    l = f.add("    def snapshot(self) -> dict:")
    T.define(f.rel, "Metrics.snapshot", l, "method")
    f.add('        return {"counters": dict(self.counters), "timings": len(self.timings)}')

    f = src("datapipe/core/stage.py")
    f.add("from .metrics import Metrics", "from ..utils.text import normalize", "", "")
    l = f.add("class BaseStage:")
    BASE = T.define(f.rel, "BaseStage", l, "class")
    l = f.add("    def __init__(self, name: str):")
    BINIT = T.define(f.rel, "BaseStage.__init__", l, "method")
    f.add("        self.name = normalize(name)", "        self.metrics = Metrics()", "        self.buffer = []", "")
    T.call(BINIT, NORM)
    T.call(BINIT, MET)
    l = f.add("    def emit(self, row: dict) -> None:")
    EMIT = T.define(f.rel, "BaseStage.emit", l, "method")
    f.add("        self.buffer.append(row)", '        self.metrics.incr("rows")', "")
    T.call(EMIT, INCR)
    l = f.add("    def flush(self) -> list:")
    FLUSH = T.define(f.rel, "BaseStage.flush", l, "method")
    f.add("        out, self.buffer = self.buffer, []", '        self.metrics.timing("flush", len(out))', "        return out", "")
    T.call(FLUSH, TIMING)
    l = f.add("    def run(self, rows: list) -> list:")
    T.define(f.rel, "BaseStage.run", l, "method")
    f.add("        raise NotImplementedError")

    f = src("datapipe/core/registry.py")
    l = f.add("class Registry:")
    REG = T.define(f.rel, "Registry", l, "class")
    l = f.add("    def __init__(self):")
    T.define(f.rel, "Registry.__init__", l, "method")
    f.add("        self.stages = {}", "")
    l = f.add("    def register(self, name: str, stage) -> None:")
    REGISTER = T.define(f.rel, "Registry.register", l, "method")
    f.add("        self.stages[name] = stage", "")
    l = f.add("    def lookup(self, name: str):")
    T.define(f.rel, "Registry.lookup", l, "method")
    f.add("        return self.stages.get(name)", "", "", "registry = Registry()")

    f = src("datapipe/core/pipeline.py")
    f.add("from .registry import registry", "", "")
    l = f.add("class Pipeline:")
    PIPE = T.define(f.rel, "Pipeline", l, "class")
    l = f.add("    def __init__(self, name: str):")
    T.define(f.rel, "Pipeline.__init__", l, "method")
    f.add("        self.name = name", "        self.stages = []", "")
    l = f.add('    def add(self, stage) -> "Pipeline":')
    PADD = T.define(f.rel, "Pipeline.add", l, "method")
    f.add("        self.stages.append(stage)", "        registry.register(stage.name, stage)", "        return self", "")
    T.call(PADD, REGISTER)
    l = f.add("    def run(self, rows: list) -> list:")
    PRUN = T.define(f.rel, "Pipeline.run", l, "method")
    f.add("        for stage in self.stages:", "            rows = stage.run(rows)", "        return rows")

    src("datapipe/connectors/__init__.py").add("")
    f = src("datapipe/connectors/http.py")
    l = f.add("class HttpClient:")
    HTTP = T.define(f.rel, "HttpClient", l, "class")
    l = f.add("    def __init__(self, base_url: str, timeout: float = 10.0):")
    T.define(f.rel, "HttpClient.__init__", l, "method")
    f.add("        self.base_url = base_url", "        self.timeout = timeout", "")
    HM = {}
    for meth, sig, body in [
        ("get", "def get(self, path: str) -> dict:", '        return self.request("GET", path)'),
        ("post", "def post(self, path: str, body: dict) -> dict:", '        return self.request("POST", path)'),
    ]:
        l = f.add(f"    {sig}")
        HM[meth] = T.define(f.rel, f"HttpClient.{meth}", l, "method")
        f.add(body, "")
    l = f.add("    def request(self, method: str, path: str) -> dict:")
    HM["request"] = T.define(f.rel, "HttpClient.request", l, "method")
    f.add('        return {"method": method, "url": self.base_url + path, "timeout": self.timeout}')
    T.call(HM["get"], HM["request"])
    T.call(HM["post"], HM["request"])

    CONN = {}
    for c in CONNECTORS:
        C = pascal(c)
        f = src(f"datapipe/connectors/{c}.py")
        f.add("from .http import HttpClient", "from ..utils import tokenize", "", "")
        l = f.add(f"class {C}Connector:")
        K = T.define(f.rel, f"{C}Connector", l, "class")
        l = f.add("    def __init__(self, client: HttpClient):")
        T.define(f.rel, f"{C}Connector.__init__", l, "method")
        f.add("        self.client = client", "")
        KM = {}
        l = f.add("    def fetch(self, key: str) -> list:")
        KM["fetch"] = T.define(f.rel, f"{C}Connector.fetch", l, "method")
        f.add(f'        payload = self.client.get("/{c}/" + key)', "        return [payload]", "")
        T.call(KM["fetch"], HM["get"])
        l = f.add("    def push(self, key: str, rows: list) -> None:")
        KM["push"] = T.define(f.rel, f"{C}Connector.push", l, "method")
        f.add(f'        self.client.post("/{c}/" + key, {{"rows": rows}})', "")
        T.call(KM["push"], HM["post"])
        l = f.add("    def describe(self) -> list:")
        KM["describe"] = T.define(f.rel, f"{C}Connector.describe", l, "method")
        f.add(f'        return tokenize("{C} connector")')
        T.call(KM["describe"], TOK)
        CONN[c] = (K, KM)

    f = src("datapipe/connectors/factory.py")
    f.add("from .http import HttpClient")
    for c in CONNECTORS:
        f.add(f"from .{c} import {pascal(c)}Connector")
    f.add("", "")
    MAKE = {}
    for c in CONNECTORS:
        C = pascal(c)
        l = f.add(f"def make_{c}() -> {C}Connector:")
        MAKE[c] = T.define(f.rel, f"make_{c}", l, "fn")
        f.add(f'    return {C}Connector(HttpClient("https://{c}.example.com"))', "", "")
        T.call(MAKE[c], CONN[c][0])
        T.call(MAKE[c], HTTP)

    src("datapipe/models/__init__.py").add("")
    src("datapipe/stages/__init__.py").add("")
    src("datapipe/pipelines/__init__.py").add("")
    f = src("datapipe/stages/helpers.py")
    l = f.add("def chunk(rows: list, size: int = 100) -> list:")
    CHUNK = T.define(f.rel, "chunk", l, "fn")
    f.add("    return [rows[i : i + size] for i in range(0, len(rows), size)]", "", "")
    l = f.add('def dedupe(rows: list, key: str = "id") -> list:')
    DEDUPE = T.define(f.rel, "dedupe", l, "fn")
    f.add("    seen = set()", "    out = []", "    for row in rows:", "        marker = row.get(key)",
          "        if marker not in seen:", "            seen.add(marker)", "            out.append(row)", "    return out")

    ents = ENTITIES[:n]
    RUN = {}
    for i, e in enumerate(ents):
        E = pascal(e)
        c = CONNECTORS[i % len(CONNECTORS)]
        C = pascal(c)
        K, KM = CONN[c]
        # models/<e>.py
        f = src(f"datapipe/models/{e}.py")
        f.add("import datapipe.utils.dates as dates", "from datapipe.utils.text import truncate", "", "")
        l = f.add(f"class {E}:")
        M = T.define(f.rel, E, l, "class")
        l = f.add("    def __init__(self, row: dict):")
        MINIT = T.define(f.rel, f"{E}.__init__", l, "method")
        f.add('        self.id = row.get("id")', '        self.title = truncate(row.get("title", ""))',
              '        self.created = dates.parse_date(row.get("created", "1970-01-01"))', "")
        T.call(MINIT, TRUNC)
        T.call(MINIT, PARSE)
        l = f.add("    def validate(self) -> bool:")
        MVAL = T.define(f.rel, f"{E}.validate", l, "method")
        f.add("        return bool(self.id) and len(self.title) > 0", "")
        l = f.add("    def to_row(self) -> dict:")
        MROW = T.define(f.rel, f"{E}.to_row", l, "method")
        f.add('        return {"id": self.id, "title": self.title, "created": dates.to_iso(self.created)}', "")
        T.call(MROW, ISO)
        l = f.add("    def age_days(self, today: str) -> int:")
        MAGE = T.define(f.rel, f"{E}.age_days", l, "method")
        f.add("        return dates.days_between(dates.to_iso(self.created), today)")
        T.call(MAGE, DAYS)
        T.call(MAGE, ISO)

        # stages/<e>_stage.py
        f = src(f"datapipe/stages/{e}_stage.py")
        f.add("from ..core.stage import BaseStage", f"from ..models.{e} import {E}", f"from ..connectors.{c} import {C}Connector",
              "from datapipe.utils import normalize", "from . import helpers", "", "")
        l = f.add(f"class {E}Stage(BaseStage):")
        ST = T.define(f.rel, f"{E}Stage", l, "class")
        l = f.add(f"    def __init__(self, connector: {C}Connector):")
        SINIT = T.define(f.rel, f"{E}Stage.__init__", l, "method")
        f.add(f'        super().__init__("{e}")', "        self.connector = connector", "")
        T.call(SINIT, BINIT, hard="super()")
        l = f.add("    def load(self, key: str) -> list:")
        SLOAD = T.define(f.rel, f"{E}Stage.load", l, "method")
        f.add("        rows = self.connector.fetch(normalize(key))", "        return helpers.dedupe(rows)", "")
        for t in (KM["fetch"], NORM, DEDUPE):
            T.call(SLOAD, t)
        l = f.add("    def run(self, rows: list) -> list:")
        SRUN = T.define(f.rel, f"{E}Stage.run", l, "method")
        f.add("        for batch in helpers.chunk(rows):", "            for raw in batch:", f"                item = {E}(raw)",
              "                if item.validate():", "                    self.emit(item.to_row())", "        return self.flush()", "")
        for t in (CHUNK, M, MVAL, EMIT, MROW, FLUSH):
            T.call(SRUN, t)
        l = f.add("    def export(self, key: str) -> None:")
        SEXP = T.define(f.rel, f"{E}Stage.export", l, "method")
        f.add("        rows = self.run(self.load(key))", "        self.connector.push(key, rows)")
        for t in (SRUN, SLOAD, KM["push"]):
            T.call(SEXP, t)

        # pipelines/<e>_pipeline.py
        f = src(f"datapipe/pipelines/{e}_pipeline.py")
        f.add("from datapipe.core.pipeline import Pipeline", f"from datapipe.connectors.factory import make_{c}",
              f"from datapipe.stages.{e}_stage import {E}Stage", "", "")
        hard = i % 5 == 0
        if hard:
            l = f.add("def inspect_stage(stage) -> list:")
            INSPECT = T.define(f.rel, "inspect_stage", l, "fn")
            f.add('    return stage.load("probe")', "", "")
            T.call(INSPECT, SLOAD, hard="untyped-param")
        l = f.add(f"def build_{e}_pipeline() -> Pipeline:")
        BUILD = T.define(f.rel, f"build_{e}_pipeline", l, "fn")
        f.add(f"    connector = make_{c}()", f"    stage = {E}Stage(connector)", f'    pipeline = Pipeline("{e}")', "    pipeline.add(stage)")
        for t in (MAKE[c], ST, PIPE, PADD):
            T.call(BUILD, t)
        if hard:
            f.add("    inspect_stage(stage)")
            T.call(BUILD, INSPECT)
        f.add("    return pipeline", "", "")
        l = f.add(f"def run_{e}(rows: list) -> list:")
        RUNF = T.define(f.rel, f"run_{e}", l, "fn")
        f.add(f"    pipeline = build_{e}_pipeline()", f"    info = make_{c}().describe()", "    rows.append({\"connector\": info})", "    return pipeline.run(rows)")
        for t in (BUILD, MAKE[c], PRUN):
            T.call(RUNF, t)
        T.call(RUNF, KM["describe"], hard="chained-call")
        RUN[e] = RUNF

    f = src("datapipe/cli.py")
    f.add("import sys", "")
    for e in ents:
        f.add(f"from datapipe.pipelines import {e}_pipeline")
    f.add("", "")
    l = f.add("def main(argv=None) -> int:")
    MAIN = T.define(f.rel, "main", l, "fn")
    f.add("    rows = []")
    for e in ents:
        f.add(f"    rows = {e}_pipeline.run_{e}(rows)")
        T.call(MAIN, RUN[e])
    f.add('    print(f"processed {len(rows)} rows")', "    return 0", "", "", 'if __name__ == "__main__":', "    sys.exit(main())")

    root.mkdir(parents=True, exist_ok=True)
    (root / "pyproject.toml").write_text('[project]\nname = "datapipe"\nversion = "0.1.0"\n')
    for s in files:
        s.write()
    T.dump(ROOT / "truth" / "datapipe.json")
    print(f"datapipe: {len(files)} files, {sum(len(s.lines) for s in files)} lines, {len(T.defs)} defs, {len(T.calls)} calls")




if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--entities-ts", type=int, default=40)
    ap.add_argument("--entities-py", type=int, default=60)
    ap.add_argument("--weapons", type=int, default=25)
    a = ap.parse_args()
    gen_shopfront(a.entities_ts)
    gen_datapipe(a.entities_py)
    from gen_arena import gen_arena

    gen_arena(ROOT, Truth, Src, a.weapons)
