#!/usr/bin/env node
import { createRequire } from 'node:module';
import { readFileSync, realpathSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const require = createRequire(new URL('../web/package.json', import.meta.url));
const directive = /@|eslint|tslint|<reference|svelte-ignore|shellcheck|webpack|rollup|__PURE__|__NO_SIDE_EFFECTS__|sourceMappingURL|sourceURL|prettier|biome|rustfmt|clippy|istanbul|\bc8\b|noqa|nosec|pylint|mypy|pyright|yapf|type\s*:|coding\s*[:=]|^\s*[#!]/i;
const locations = new Set(['start', 'end', 'loc', 'range', 'name_loc']);
const commentFields = new Set(['comments', 'leadingComments', 'trailingComments', 'innerComments']);
const commentTypes = new Set(['Comment', 'CSSComment', 'Line', 'Block']);

function typescriptSyntax(path, source, checkDirectives = true) {
  const ts = require('typescript');
  const kind = /\.tsx$/i.test(path) ? ts.ScriptKind.TSX
    : /\.jsx$/i.test(path) ? ts.ScriptKind.JSX
      : /\.(?:ts|mts|cts)$/i.test(path) ? ts.ScriptKind.TS : ts.ScriptKind.JS;
  const ast = ts.createSourceFile(path, source, ts.ScriptTarget.Latest, true, kind);
  if (ast.parseDiagnostics.length) throw new Error('Source has parse diagnostics.');
  const inspectComments = (position) => {
    for (const range of [...(ts.getLeadingCommentRanges(source, position) ?? []),
      ...(ts.getTrailingCommentRanges(source, position) ?? [])]) {
      let text = source.slice(range.pos + 2, range.end);
      if (kind === ts.ScriptKind.TS || kind === ts.ScriptKind.TSX) {
        text = text.replace(/@(param|returns)\b/g, '');
      }
      if (directive.test(text)) throw new Error('Tool directive comments require verification.');
    }
  };
  const visit = (node) => {
    inspectComments(node.pos);
    inspectComments(node.end);
    ts.forEachChild(node, visit);
  };
  if (checkDirectives) visit(ast);
  return ts.createPrinter({ removeComments: true }).printFile(ast);
}

function svelteSyntax(source) {
  const { parse } = require('svelte/compiler');
  const ast = parse(source, { modern: true });
  const clean = (value) => {
    if (value === null || typeof value !== 'object') return value;
    if (commentTypes.has(value.type) && (typeof value.value === 'string' || typeof value.data === 'string')) {
      if (directive.test(value.value ?? value.data ?? '')) {
        throw new Error('Tool directive comments require verification.');
      }
      return undefined;
    }
    if (Array.isArray(value)) {
      const result = [];
      for (const item of value) {
        const entry = clean(item);
        if (entry === undefined) continue;
        const previous = result.at(-1);
        if (entry?.type === 'Text' && previous?.type === 'Text') {
          previous.raw += entry.raw;
          previous.data += entry.data;
        } else result.push(entry);
      }
      return result;
    }
    const result = {};
    for (const [key, entry] of Object.entries(value)) {
      if (locations.has(key)) continue;
      // CSS children retain selectors and declaration values; styles repeats the source.
      if (value.type === 'StyleSheet' && key === 'content') {
        clean(entry.comment);
        continue;
      }
      if (commentFields.has(key)) {
        clean(entry);
        continue;
      }
      const cleaned = clean(entry);
      if (cleaned !== undefined) result[key] = cleaned;
    }
    return result;
  };
  return clean(ast);
}

function withoutWhitespace(value) {
  if (Array.isArray(value)) {
    return value.filter((node) => !(node?.type === 'Text' && !node.data.trim()))
      .map(withoutWhitespace);
  }
  if (value === null || typeof value !== 'object') return value;
  return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, withoutWhitespace(entry)]));
}

function compiledSvelte(path, source) {
  const { compile } = require('svelte/compiler');
  const result = compile(source, { filename: path, dev: false, preserveComments: false });
  return JSON.stringify({
    js: typescriptSyntax('compiled.js', result.js.code, false),
    css: result.css?.code ?? null,
  });
}

export function compareSyntax(path, before, after) {
  try {
    if (!/\.svelte$/i.test(path)) return typescriptSyntax(path, before) === typescriptSyntax(path, after);
    const left = svelteSyntax(before);
    const right = svelteSyntax(after);
    if (JSON.stringify(left) === JSON.stringify(right)) return true;
    if (JSON.stringify(withoutWhitespace(left)) !== JSON.stringify(withoutWhitespace(right))) return false;
    // The compiler decides whether whitespace survives in production output.
    return compiledSvelte(path, before) === compiledSvelte(path, after);
  } catch (error) {
    if (error.code === 'MODULE_NOT_FOUND') throw error;
    return false;
  }
}

if (process.argv[1] && realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const [path, before, after] = process.argv.slice(2);
    if (!path || !before || !after) throw new Error('Usage: ci-comment-syntax.mjs PATH BEFORE AFTER');
    process.exitCode = compareSyntax(path, readFileSync(before, 'utf8'), readFileSync(after, 'utf8')) ? 0 : 1;
  } catch (error) {
    if (error.code === 'MODULE_NOT_FOUND') {
      console.error('Install pinned web dependencies with npm --prefix web ci, then rerun classification.');
      process.exitCode = 2;
    } else {
      console.error(error.message);
      process.exitCode = 1;
    }
  }
}
