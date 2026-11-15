import assert from 'node:assert/strict';
import { globSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import test from 'node:test';

const requireWeb = createRequire(resolve('web/package.json'));
const ts = requireWeb('typescript');

test('browser screenshots do not use absolute path literals', () => {
  const failures = [];
  for (const pattern of ['web/tests/**/*.ts', 'tests/browser/**/*.ts']) {
    for (const file of globSync(pattern)) {
      const source = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true);
      const visit = node => {
        if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression) && node.expression.name.text === 'screenshot') {
          const options = node.arguments[0];
          if (options && ts.isObjectLiteralExpression(options)) {
            for (const property of options.properties) {
              if (!ts.isPropertyAssignment(property) || property.name?.text !== 'path') continue;
              const value = property.initializer;
              const prefix = ts.isStringLiteralLike(value) ? value.text : ts.isTemplateExpression(value) ? value.head.text : '';
              if (/^(?:[\\/]|[A-Za-z]:[\\/])/.test(prefix)) {
                const line = source.getLineAndCharacterOfPosition(value.getStart(source)).line + 1;
                failures.push(`${file}:${line}: use test.info().outputPath() for screenshot output`);
              }
            }
          }
        }
        ts.forEachChild(node, visit);
      };
      visit(source);
    }
  }
  assert.deepEqual(failures, []);
});
