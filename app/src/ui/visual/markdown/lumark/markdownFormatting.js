// 出典: Lumark 0.15.0 の src/lib/markdownFormatting.js（純粋関数。全体をそのままコピー）。
import { getMarkdownLineContexts } from './markdownStructure.js';

// Textarea edits use source offsets; selections always refer to the resulting document.

function affectedLines(text, start, end) {
  return getMarkdownLineContexts(text).filter(line => line.start <= (end > start ? end - 1 : end) && line.end >= start);
}

function escaped(text, index) {
  let count = 0;
  while (index > 0 && text[--index] === '\\') count++;
  return count % 2 === 1;
}

function codeRanges(text) {
  const fenced = getMarkdownLineContexts(text).filter(line => line.code).map(line => [line.start, line.end, 'fence']);
  const ranges = [...fenced];
  for (let i = 0; i < text.length; i++) {
    const fence = fenced.find(range => i >= range[0] && i < range[1]);
    if (fence) { i = fence[1]; continue; }
    if (text[i] !== '`' || escaped(text, i)) continue;
    let end = i + 1;
    while (text[end] === '`') end++;
    const size = end - i;
    for (let next = end; next < text.length; next++) {
      if (fenced.some(range => next >= range[0] && next <= range[1])) break;
      if (text[next] !== '`') continue;
      let close = next + 1;
      while (text[close] === '`') close++;
      if (close - next === size) {
        ranges.push([i, close, 'inline']);
        i = close - 1;
        break;
      }
      next = close - 1;
    }
    if (i < end - 1) i = end - 1;
  }
  return ranges;
}

function inRanges(ranges, position) {
  return ranges.some(([start, end]) => position >= start && position < end);
}

function mapPosition(position, operations) {
  let delta = 0;
  for (const operation of operations) {
    if (position < operation.start) break;
    if (position <= operation.end) {
      return operation.start + delta + (operation.start === operation.end
        ? operation.text.length
        : position === operation.end ? operation.text.length : Math.min(position - operation.start, operation.text.length));
    }
    delta += operation.text.length - (operation.end - operation.start);
  }
  return position + delta;
}

function combineEdits(text, start, end, operations) {
  if (!operations.length) return null;
  operations.sort((a, b) => a.start - b.start);
  const from = operations[0].start;
  const to = operations.at(-1).end;
  let replacement = '';
  let cursor = from;
  for (const operation of operations) {
    replacement += text.slice(cursor, operation.start) + operation.text;
    cursor = operation.end;
  }
  replacement += text.slice(cursor, to);
  return {
    start: from, end: to, text: replacement,
    selectionStart: mapPosition(start, operations), selectionEnd: mapPosition(end, operations)
  };
}

export function createLinePrefixEdit(text, start, end, prefix) {
  const lines = affectedLines(text, start, end).filter(line => !line.code);
  if (!lines.length) return null;
  const heading = /^#{1,6} $/.test(prefix);
  const quote = prefix === '> ';
  const list = prefix === '- ' || prefix === '- [ ] ' || prefix === '1. ';
  if (!heading && !quote && !list) return null;
  const markers = lines.map(line => {
    const offset = (line.text.match(heading || quote ? /^ {0,3}/ : /^[ \t]*/) ?? [''])[0].length;
    const rest = line.text.slice(offset);
    const marker = heading ? rest.match(/^#{1,6}(?:[ \t]+|$)/)?.[0]
      : quote ? rest.match(/^>[ \t]?/)?.[0]
      : rest.match(/^(?:[-+*]|\d+[.)])[ \t]+(?:\[[ xX]\][ \t]+)?/)?.[0];
    const matches = quote ? !!marker : heading
      ? marker?.trim() === prefix.trim()
      : prefix === '- [ ] ' ? /^[-+*][ \t]+\[[ xX]\][ \t]+/.test(rest)
      : prefix === '1. ' ? /^\d+[.)][ \t]+/.test(rest)
      : /^[-+*][ \t]+(?!\[[ xX]\][ \t]+)/.test(rest);
    return { line, offset, marker: marker ?? '', matches };
  });
  const remove = markers.every(marker => marker.matches);
  const operations = markers.flatMap(({ line, offset, marker, matches }) => {
    if (!remove && matches) return [];
    const position = line.start + offset;
    return [{ start: position, end: position + marker.length, text: remove ? '' : prefix }];
  });
  return combineEdits(text, start, end, operations);
}

export function createHeadingLevelEdit(text, start, end, direction) {
  if (direction !== 1 && direction !== -1) return null;
  const operations = affectedLines(text, start, end).flatMap(line => {
    if (line.code) return [];
    const match = line.text.match(/^( {0,3}(?:>[ \t]? {0,3})*)(#{1,6})(?=[ \t\r]|$)/);
    if (!match) return [];
    const level = match[2].length + direction;
    if (level < 1 || level > 6) return [];
    const position = line.start + match[1].length;
    return [{ start: position, end: position + match[2].length, text: '#'.repeat(level) }];
  });
  return combineEdits(text, start, end, operations);
}

function removePair(text, start, end, pair) {
  return combineEdits(text, start, end, [
    { start: pair.start, end: pair.contentStart, text: '' },
    { start: pair.contentEnd, end: pair.end, text: '' }
  ]);
}

function codePairs(text, ranges) {
  return ranges.flatMap(([start, end, kind]) => {
    if (kind !== 'inline') return [];
    const size = text.slice(start).match(/^`+/)[0].length;
    if (text.slice(end - size, end) !== '`'.repeat(size) || end - start <= size * 2) return [];
    let contentStart = start + size;
    let contentEnd = end - size;
    const content = text.slice(contentStart, contentEnd);
    if (content.startsWith(' ') && content.endsWith(' ') && /[^ ]/.test(content)) {
      contentStart++; contentEnd--;
    }
    return [{ start, end, contentStart, contentEnd }];
  });
}

function decorationPairs(text, prefix, suffix, ranges) {
  const pairs = [];
  const stack = [];
  const star = prefix === '*' || prefix === '**';
  for (let i = 0; i < text.length; i++) {
    if (text[i] === '\n' && /^[ \t\r]*\n/.test(text.slice(i + 1))) stack.length = 0;
    const code = ranges.find(([start, end]) => i >= start && i < end);
    if (code) {
      if (code[2] === 'fence') stack.length = 0;
      i = code[1] - 1;
      continue;
    }
    if (escaped(text, i)) continue;
    if (star && text[i] === '*') {
      let runEnd = i;
      while (text[runEnd] === '*') runEnd++;
      const length = runEnd - i;
      const valid = prefix === '*' ? length % 2 === 1 : length >= 2;
      if (!valid) { i = runEnd - 1; continue; }
      const before = text[i - 1] ?? '';
      const after = text[runEnd] ?? '';
      const punctuation = character => !character || /[\s\p{P}\p{S}]/u.test(character);
      const canOpen = !!after && !/\s/.test(after) && (punctuation(before) || !punctuation(after));
      const canClose = !!before && !/\s/.test(before) && (punctuation(after) || !punctuation(before));
      if (canClose && stack.length) {
        const opening = stack.pop();
        const closingStart = prefix === '*' ? i : runEnd - 2;
        pairs.push({ start: opening, contentStart: opening + prefix.length, contentEnd: closingStart, end: closingStart + suffix.length });
      } else if (canOpen) {
        stack.push(prefix === '*' ? runEnd - 1 : i);
      }
      i = runEnd - 1;
      continue;
    }
    if (star) continue;
    if (stack.length && text.startsWith(suffix, i)) {
      const opening = stack.pop();
      pairs.push({ start: opening, contentStart: opening + prefix.length, contentEnd: i, end: i + suffix.length });
      i += suffix.length - 1;
    } else if (text.startsWith(prefix, i)) {
      stack.push(i);
      i += prefix.length - 1;
    }
  }
  return pairs;
}

export function createInlineFormatEdit(text, start, end, prefix, suffix = prefix) {
  if (!prefix || !suffix) return null;
  if (affectedLines(text, start, end).some(line => line.code)) return null;
  if (start === end && !escaped(text, start - prefix.length) && !escaped(text, end) &&
    text.slice(start - prefix.length, start) === prefix && text.slice(end, end + suffix.length) === suffix) {
    const precedingStars = text.slice(0, start).match(/\*+$/)?.[0].length ?? 0;
    const followingStars = text.slice(end).match(/^\*+/)?.[0].length ?? 0;
    if (prefix !== '*' || (precedingStars % 2 === 1 && followingStars % 2 === 1)) {
      return removePair(text, start, end, { start: start - prefix.length, contentStart: start, contentEnd: end, end: end + suffix.length });
    }
  }
  const ranges = codeRanges(text);
  const pairs = prefix === '`' && suffix === '`'
    ? codePairs(text, ranges) : decorationPairs(text, prefix, suffix, ranges);
  const pair = pairs.filter(pair =>
    (start >= pair.contentStart && end <= pair.contentEnd) || (start === pair.start && end === pair.end) ||
    ((prefix === '*' || prefix === '**') && start <= pair.start && end >= pair.end &&
      /^\**$/.test(text.slice(start, pair.start)) && /^\**$/.test(text.slice(pair.end, end)))
  ).sort((a, b) => a.end - a.start - (b.end - b.start))[0];
  if (pair) return removePair(text, start, end, pair);
  const selected = text.slice(start, end);
  let opening = prefix;
  let closing = suffix;
  if (prefix === '`' && suffix === '`') {
    const longest = Math.max(0, ...Array.from(selected.matchAll(/`+/g), match => match[0].length));
    const delimiter = '`'.repeat(longest + 1);
    const padding = selected.startsWith('`') || selected.endsWith('`') ? ' ' : '';
    opening = delimiter + padding;
    closing = padding + delimiter;
  }
  return {
    start, end, text: opening + selected + closing,
    selectionStart: start + opening.length, selectionEnd: end + opening.length
  };
}

const unescapeMarkdown = value => value.replace(/\\([!"#$%&'()*+,\-./:;<=>?@[\]\\^_`{|}~])/g, '$1');

function parseLink(text, start) {
  let depth = 1;
  let cursor = start + 1;
  for (; cursor < text.length; cursor++) {
    if (escaped(text, cursor)) continue;
    if (text[cursor] === '[') depth++;
    if (text[cursor] === ']' && --depth === 0) break;
  }
  if (depth || text[cursor + 1] !== '(') return null;
  const label = unescapeMarkdown(text.slice(start + 1, cursor));
  cursor += 2;
  while (/[ \t\r\n]/.test(text[cursor] ?? '') && cursor < text.length) cursor++;
  let destinationStart = cursor;
  let destinationEnd;
  if (text[cursor] === '<') {
    destinationStart = ++cursor;
    while (cursor < text.length && (text[cursor] !== '>' || escaped(text, cursor))) {
      if (text[cursor] === '\n' || text[cursor] === '<') return null;
      cursor++;
    }
    if (text[cursor] !== '>') return null;
    destinationEnd = cursor++;
  } else {
    let parentheses = 0;
    while (cursor < text.length) {
      const char = text[cursor];
      if (!escaped(text, cursor)) {
        if (/\s/.test(char) || (char === ')' && parentheses === 0)) break;
        if (char === '(') parentheses++;
        if (char === ')') parentheses--;
        if (char === '<') return null;
      }
      cursor++;
    }
    if (parentheses) return null;
    destinationEnd = cursor;
  }
  const url = unescapeMarkdown(text.slice(destinationStart, destinationEnd));
  const whitespaceStart = cursor;
  while (cursor < text.length && /\s/.test(text[cursor])) cursor++;
  let title;
  if (cursor > whitespaceStart && ['"', "'", '('].includes(text[cursor])) {
    const delimiter = text[cursor] === '(' ? ')' : text[cursor];
    const titleStart = ++cursor;
    while (cursor < text.length && (text[cursor] !== delimiter || escaped(text, cursor))) cursor++;
    if (cursor === text.length) return null;
    title = unescapeMarkdown(text.slice(titleStart, cursor++));
    while (cursor < text.length && /\s/.test(text[cursor])) cursor++;
  }
  if (text[cursor] !== ')') return null;
  return { start, end: cursor + 1, label, url, ...(title === undefined ? {} : { title }) };
}

export function findMarkdownLink(text, start, end = start) {
  const ranges = codeRanges(text);
  for (let i = 0; i < text.length; i++) {
    if (text[i] !== '[' || escaped(text, i) || inRanges(ranges, i)) continue;
    const link = parseLink(text, i);
    if (!link) continue;
    const image = text[i - 1] === '!' && !escaped(text, i - 1);
    if (!image && start >= link.start && end <= link.end) return link;
    i = link.end - 1;
  }
  return null;
}

export function createMarkdownLinkEdit(text, start, end, label, url, existingLink = null) {
  const link = existingLink ?? findMarkdownLink(text, start, end);
  const from = link?.start ?? start;
  const to = link?.end ?? end;
  const escapedLabel = label.replace(/\\/g, '\\\\').replace(/\[/g, '\\[').replace(/\]/g, '\\]').replace(/[\r\n]/g, ' ');
  let destination = url.replace(/[\r\n]/g, '%0A');
  if (/[\s()<>\\]/.test(destination)) {
    destination = '<' + destination.replace(/\\/g, '%5C').replace(/</g, '%3C').replace(/>/g, '%3E') + '>';
  }
  const title = link?.title === undefined ? '' : ' "' + link.title.replace(/\\/g, '\\\\').replace(/"/g, '\\"') + '"';
  const replacement = `[${escapedLabel}](${destination}${title})`;
  return {
    start: from, end: to, text: replacement,
    selectionStart: from + 1, selectionEnd: from + 1 + escapedLabel.length
  };
}
