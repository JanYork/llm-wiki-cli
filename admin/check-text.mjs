import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
const source = fs.readFileSync(new URL('./src/text.ts', import.meta.url), 'utf8');
const javascript = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ES2022 } }).outputText;
const { english, labels, errors, translate, display } = await import(`data:text/javascript;base64,${Buffer.from(javascript).toString('base64')}`);
const ui = ts.createSourceFile('main.tsx', fs.readFileSync(new URL('./src/main.tsx', import.meta.url), 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const phrases = new Set([...Object.values(labels), ...Object.values(errors)]);
function visit(node) {
  if (ts.isStringLiteral(node) && /[\u3400-\u9fff]/.test(node.text)) phrases.add(node.text);
  ts.forEachChild(node, visit);
}
visit(ui);
visit(ts.createSourceFile('knowledge-browser.tsx', fs.readFileSync(new URL('./src/knowledge-browser.tsx', import.meta.url), 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX));
visit(ts.createSourceFile('resource-lifecycle.tsx', fs.readFileSync(new URL('./src/resource-lifecycle.tsx', import.meta.url), 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX));
const missing = [...phrases].filter(text => !english[text]);
assert.deepEqual(missing, [], `Missing English interface text: ${missing.join(' | ')}`);
for (const phrase of phrases) assert.ok(!/[\u3400-\u9fff]/.test(translate(phrase, 'en')), phrase);
assert.equal(display('manager', 'role', 'zh'), '管理者');
assert.equal(display('manager', 'role', 'en'), 'Manager');
assert.equal(display('计划', 'body', 'en'), '计划', 'Never translate stored memory content');
assert.equal(display(false, 'revoked', 'en'), 'No');
console.log(`Bilingual interface coverage passed: ${phrases.size} phrases; stored content preserved.`);
