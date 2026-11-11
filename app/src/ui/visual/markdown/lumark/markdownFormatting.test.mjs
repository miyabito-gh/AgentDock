// 出典: Lumark 0.15.0 の src/lib/markdownFormatting.test.mjs（純粋関数。全体をそのままコピー）。
import assert from 'node:assert/strict';

test('an escaped opening marker is not removed as an empty formatting pair', () => {
  const edit = createInlineFormatEdit('\\****', 3, 3, '**', '**');
  assert.equal(edit.start, 3);
  assert.equal(edit.end, 3);
  assert.equal(edit.text, '****');
});
import test from 'node:test';
import {
  createLinePrefixEdit, createHeadingLevelEdit, createInlineFormatEdit,
  findMarkdownLink, createMarkdownLinkEdit
} from './markdownFormatting.js';

function apply(text, edit) {
  assert.ok(edit);
  const result = text.slice(0, edit.start) + edit.text + text.slice(edit.end);
  return { text: result, selected: result.slice(edit.selectionStart, edit.selectionEnd), start: edit.selectionStart, end: edit.selectionEnd };
}

test('line quote toggles preserve the selected text and remove only one nested level', () => {
  const quoted = apply('alpha\nbeta', createLinePrefixEdit('alpha\nbeta', 1, 10, '> '));
  assert.equal(quoted.text, '> alpha\n> beta');
  assert.equal(quoted.selected, 'lpha\n> beta');
  const text = '> > alpha\n>> beta';
  assert.equal(apply(text, createLinePrefixEdit(text, 0, text.length, '> ')).text, '> alpha\n> beta');
  const mixed = '> alpha\nbeta\nend';
  assert.equal(apply(mixed, createLinePrefixEdit(mixed, 0, 13, '> ')).text, '> alpha\n> beta\nend');
});

test('line prefixes handle indentation, marker replacement, and end-at-next-line selection', () => {
  const text = '  ## alpha\nbeta\n';
  assert.equal(apply(text, createLinePrefixEdit(text, 5, 11, '# ')).text, '  # alpha\nbeta\n');
  const list = '  - [x] done\n  + item';
  assert.equal(apply(list, createLinePrefixEdit(list, 0, list.length, '- ')).text, '  - done\n  + item');
  assert.equal(apply('abc\ndef', createLinePrefixEdit('abc\ndef', 0, 4, '> ')).text, '> abc\ndef');
});

test('heading levels affect selected existing headings, respect limits and skip fences', () => {
  const text = '# one\nbody\n> ## two\n###### six\n```md\n## code\n```\n### last';
  const deeper = apply(text, createHeadingLevelEdit(text, 0, text.length, 1));
  assert.equal(deeper.text, '## one\nbody\n> ### two\n###### six\n```md\n## code\n```\n#### last');
  assert.equal(createHeadingLevelEdit('# one', 2, 2, -1), null);
  assert.equal(createHeadingLevelEdit('###### six', 7, 7, 1), null);
  assert.equal(apply('## one', createHeadingLevelEdit('## one', 2, 2, 1)).start, 3);
});

test('quote and list commands leave fenced code unchanged', () => {
  const text = 'before\n~~~\ncode\n~~~\nafter';
  assert.equal(apply(text, createLinePrefixEdit(text, 0, text.length, '> ')).text, '> before\n~~~\ncode\n~~~\n> after');
  assert.equal(createLinePrefixEdit(text, 12, 12, '- '), null);
});

test('list fence contexts protect code headings and release the following heading', () => {
  const text = '- ```\n  # code\n  ```\n# heading';
  assert.equal(apply(text, createHeadingLevelEdit(text, 0, text.length, 1)).text, '- ```\n  # code\n  ```\n## heading');
  assert.equal(createLinePrefixEdit(text, text.indexOf('code'), text.indexOf('code'), '> '), null);
  assert.equal(createInlineFormatEdit(text, text.indexOf('code'), text.indexOf('code') + 4, '**', '**'), null);
  const links = '- ```\n  [code](url)\n  ```\n[normal](url)';
  assert.equal(findMarkdownLink(links, links.indexOf('code')), null);
  assert.equal(findMarkdownLink(links, links.indexOf('normal'))?.label, 'normal');
});

test('quote fence contexts stop at the quote container boundary', () => {
  const text = '> ```\n> code\n\n# heading';
  assert.equal(apply(text, createHeadingLevelEdit(text, 0, text.length, 1)).text, '> ```\n> code\n\n## heading');
  assert.equal(createLinePrefixEdit(text, text.indexOf('code'), text.indexOf('code'), '> '), null);
  assert.equal(apply(text, createInlineFormatEdit(text, text.indexOf('heading'), text.length, '**', '**')).text, '> ```\n> code\n\n# **heading**');
  const links = '> ```\n> [code](url)\n\n[normal](url)';
  assert.equal(findMarkdownLink(links, links.indexOf('code')), null);
  assert.equal(findMarkdownLink(links, links.indexOf('normal'))?.label, 'normal');
});

test('inline toggles keep the inner selection when wrapping and unwrapping', () => {
  const first = apply('before hello after', createInlineFormatEdit('before hello after', 7, 12, '**', '**'));
  assert.equal(first.text, 'before **hello** after');
  assert.equal(first.selected, 'hello');
  const second = apply(first.text, createInlineFormatEdit(first.text, first.start, first.end, '**', '**'));
  assert.equal(second.text, 'before hello after');
  assert.equal(second.selected, 'hello');
  const full = apply('**hello**', createInlineFormatEdit('**hello**', 0, 9, '**', '**'));
  assert.deepEqual(full, { text: 'hello', selected: 'hello', start: 0, end: 5 });
});

test('a caret or partial selection inside formatting toggles the existing span', () => {
  assert.deepEqual(apply('**hello**', createInlineFormatEdit('**hello**', 4, 4, '**', '**')), { text: 'hello', selected: '', start: 2, end: 2 });
  assert.equal(apply('~~hello~~', createInlineFormatEdit('~~hello~~', 3, 5, '~~', '~~')).selected, 'el');
  assert.equal(apply('<u>hello</u>', createInlineFormatEdit('<u>hello</u>', 5, 5, '<u>', '</u>')).text, 'hello');
  const empty = apply('ab', createInlineFormatEdit('ab', 1, 1, '**', '**'));
  assert.equal(empty.text, 'a****b');
  assert.equal(empty.start, 3);
  assert.deepEqual(apply(empty.text, createInlineFormatEdit(empty.text, empty.start, empty.end, '**', '**')), { text: 'ab', selected: '', start: 1, end: 1 });
  assert.equal(apply('a``b', createInlineFormatEdit('a``b', 2, 2, '`', '`')).text, 'ab');
});

test('emphasis and strong toggles do not confuse shared asterisk runs', () => {
  assert.equal(apply('**hello**', createInlineFormatEdit('**hello**', 0, 9, '*', '*')).text, '***hello***');
  assert.equal(apply('***hello***', createInlineFormatEdit('***hello***', 0, 11, '*', '*')).text, '**hello**');
  assert.equal(apply('***hello***', createInlineFormatEdit('***hello***', 5, 5, '**', '**')).text, '*hello*');
  assert.equal(apply('**outer *inner* outer**', createInlineFormatEdit('**outer *inner* outer**', 10, 10, '*', '*')).text, '**outer inner outer**');
  assert.equal(apply('\\*literal*', createInlineFormatEdit('\\*literal*', 3, 3, '*', '*')).text, '\\*l**iteral*');
});

test('unmatched decoration markers cannot pair across paragraphs or fenced blocks', () => {
  for (const text of ['**one\n\ntwo**', '**one\n\na**two**b']) {
    const edit = createInlineFormatEdit(text, 3, 3, '**', '**');
    assert.equal(edit.start, 3);
    assert.equal(edit.end, 3);
  }
  for (const boundary of ['\n\n', '\n  \t\n', '\n```\ncode\n```\n']) {
    const text = '**one' + boundary + '**two**';
    assert.equal(apply(text, createInlineFormatEdit(text, 3, 3, '**', '**')).text, '**o****ne' + boundary + '**two**');
    const insideTwo = text.indexOf('two') + 1;
    assert.equal(apply(text, createInlineFormatEdit(text, insideTwo, insideTwo, '**', '**')).text, '**one' + boundary + 'two');
  }
  const valid = '**one `code` two**';
  assert.equal(apply(valid, createInlineFormatEdit(valid, 4, 4, '**', '**')).text, 'one `code` two');
});

test('inline code uses a longer run and preserves selected backticks', () => {
  const text = '`value`';
  const wrapped = apply(text, createInlineFormatEdit(text, 0, text.length, '`', '`'));
  assert.equal(wrapped.text, 'value');
  const source = 'a ` b';
  const added = apply(source, createInlineFormatEdit(source, 0, source.length, '`', '`'));
  assert.equal(added.text, '``a ` b``');
  assert.equal(added.selected, source);
  assert.equal(apply(added.text, createInlineFormatEdit(added.text, added.start, added.end, '`', '`')).text, source);
  const edge = apply('`x', createInlineFormatEdit('`x', 0, 2, '`', '`'));
  assert.equal(edge.text, '`` `x ``');
  assert.equal(edge.selected, '`x');
  assert.equal(apply('`` a ` b ``', createInlineFormatEdit('`` a ` b ``', 5, 5, '`', '`')).text, 'a ` b');
});

test('link finder recognizes nested parentheses and escaped brackets with an optional title', () => {
  const text = 'before [A \\[B\\]](https://example.test/a_(b) "a title") after';
  const link = findMarkdownLink(text, 13, 13);
  assert.ok(link);
  assert.equal(link.label, 'A [B]');
  assert.equal(link.url, 'https://example.test/a_(b)');
  assert.equal(link.title, 'a title');
  assert.equal(text.slice(link.start, link.end), '[A \\[B\\]](https://example.test/a_(b) "a title")');
  assert.equal(findMarkdownLink('[label](<a b(c)> \'title\')', 2)?.url, 'a b(c)');
});

test('link finder avoids images, reference links, escaped links, and code', () => {
  for (const text of ['![alt](image.png)', '[label][ref]', '\\[label](url)', '`[label](url)`', '```md\n[label](url)\n```', '~~~\n[label](url)\n~~~']) {
    assert.equal(findMarkdownLink(text, text.indexOf('label') + 1), null, text);
  }
  assert.equal(findMarkdownLink('![a [nested](url)](image)', 8), null);
  assert.equal(findMarkdownLink('plain text', 4), null);
  assert.equal(findMarkdownLink('[label](url)', 1, 20), null);
});

test('editing a link updates its label and URL and retains its title', () => {
  const text = 'before [old](url "keep me") after';
  const edit = createMarkdownLinkEdit(text, 10, 10, 'new [label]', 'https://example.test/a (b)');
  const result = apply(text, edit);
  assert.equal(result.text, 'before [new \\[label\\]](<https://example.test/a (b)> "keep me") after');
  assert.equal(findMarkdownLink(result.text, result.start)?.label, 'new [label]');
  assert.equal(findMarkdownLink(result.text, result.start)?.url, 'https://example.test/a (b)');
  assert.equal(findMarkdownLink(result.text, result.start)?.title, 'keep me');
});
