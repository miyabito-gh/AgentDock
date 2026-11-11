// 出典: Lumark 0.15.0 の src/lib/markdownStructure.test.mjs（純粋関数。インデントとコードフェンスのテストだけ）。
import test from 'node:test';
import assert from 'node:assert/strict';
import { createIndentEdit, isInsideCodeFence, getMarkdownLineContexts } from './markdownStructure.js';

function apply(text, edit) {
  return edit ? text.slice(0, edit.start) + edit.text + text.slice(edit.end) : text;
}

test('Tab anywhere in an item moves its descendants and continuation paragraphs', () => {
  const source = '- previous\n- parent text\n  - child\n\n  paragraph\n  ```js\n  - literal\n  ```\n- next';
  const caret = source.indexOf('text') + 2;
  const edit = createIndentEdit(source, caret, caret, 2);
  assert.equal(apply(source, edit), '- previous\n  - parent text\n    - child\n\n    paragraph\n    ```js\n    - literal\n    ```\n- next');
  assert.equal(edit.selectionStart, caret + 2);
  assert.equal(edit.selectionEnd, caret + 2);
});

test('numbered and task list indentation follows marker content width', () => {
  const numbered = '100. previous\n101. next';
  assert.equal(apply(numbered, createIndentEdit(numbered, numbered.length, numbered.length, 2)), '100. previous\n     101. next');
  const task = '- [x] previous\n- [ ] next';
  assert.equal(apply(task, createIndentEdit(task, task.length, task.length, 2)), '- [x] previous\n  - [ ] next');
});

test('selected siblings indent together and exclude the next line at column zero', () => {
  const source = '- parent\n- one\n- two\n- untouched';
  const start = source.indexOf('- one');
  const end = source.indexOf('- untouched');
  const edit = createIndentEdit(source, start, end, 2);
  assert.equal(apply(source, edit), '- parent\n  - one\n  - two\n- untouched');
  assert.equal(edit.selectionStart, start + 2);
  assert.equal(edit.selectionEnd, end + 4);
});

test('the first list item cannot indent into an accidental code block', () => {
  assert.equal(createIndentEdit('- first', 5, 5, 4), null);
  const source = '- parent\n  - first child';
  assert.equal(createIndentEdit(source, source.length, source.length, 2), null);
});

test('ShiftTab follows the parent level, including partial spaces and tabs', () => {
  const source = '- parent\n   - child\n     continuation\n- next';
  assert.equal(apply(source, createIndentEdit(source, source.indexOf('child'), source.indexOf('child'), 2, true)), '- parent\n- child\n  continuation\n- next');
  const tabbed = '- parent\n\t- child\n\t  - grandchild';
  assert.equal(apply(tabbed, createIndentEdit(tabbed, tabbed.indexOf('child'), tabbed.indexOf('child'), 4, true)), '- parent\n- child\n  - grandchild');
  assert.equal(apply(' a', createIndentEdit(' a', 2, 2, 4, true)), 'a');
  assert.equal(apply('\tword', createIndentEdit('\tword', 5, 5, 2, true)), 'word');
});

test('quoted list subtrees indent inside the quote marker', () => {
  const source = '> - previous\n> - next\n>   continuation';
  assert.equal(apply(source, createIndentEdit(source, 20, 20, 2)), '> - previous\n>   - next\n>     continuation');
});

test('ordinary multiline indentation preserves selected content', () => {
  const source = 'first\nsecond\nthird';
  const edit = createIndentEdit(source, 2, source.indexOf('third'), 2);
  assert.equal(apply(source, edit), '  first\n  second\nthird');
  assert.equal(edit.selectionStart, 4);
  assert.equal(edit.selectionEnd, source.indexOf('third') + 4);
  assert.equal(apply('abc', createIndentEdit('abc', 1, 1, 4)), 'a   bc');
});

test('fence contents use ordinary Tab, and marker-like code is not a list', () => {
  const source = '```md\n- first\n- second\n```\n- real';
  const caret = source.indexOf('second') + 3;
  assert.equal(isInsideCodeFence(source, caret), true);
  assert.equal(isInsideCodeFence(source, source.indexOf('real')), false);
  assert.equal(apply(source, createIndentEdit(source, caret, caret, 2)), source.slice(0, caret) + ' ' + source.slice(caret));
});

test('shared fence contexts include nested list fence boundaries and preserve list indentation', () => {
  const source = '- previous\n- ```js\n  const value = 1;\n  ```\n- following';
  const contexts = getMarkdownLineContexts(source);
  assert.deepEqual(contexts.map(line => line.code), [false, true, true, true, false]);
  assert.deepEqual(contexts[1], { start: 11, end: 18, text: '- ```js', code: true });
  assert.equal(isInsideCodeFence(source, source.indexOf('```js')), true);
  assert.equal(isInsideCodeFence(source, source.lastIndexOf('```')), true);
  const caret = source.indexOf('```js') + 3;
  assert.equal(apply(source, createIndentEdit(source, caret, caret, 2)), '- previous\n  - ```js\n    const value = 1;\n    ```\n- following');
});

test('quoted fence context ends when its quote container ends', () => {
  const source = '> - ~~~js\n>   const value = 1;\nnormal\n> outside';
  assert.deepEqual(getMarkdownLineContexts(source).map(line => line.code), [true, true, false, false]);
  assert.equal(isInsideCodeFence(source, source.indexOf('normal')), false);
  assert.equal(isInsideCodeFence(source, source.indexOf('outside')), false);
});
