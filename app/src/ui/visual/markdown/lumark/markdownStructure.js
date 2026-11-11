// 出典: Lumark 0.15.0 の src/lib/markdownStructure.js（純粋関数。createIndentEdit・getMarkdownLineContexts・isInsideCodeFence とその補助だけ。行の移動・複製と表のTab移動は除く）。
/** Pure textarea edits. All offsets, including returned selections, are absolute. */
function linesOf(text) {
  const lines = [];
  const pattern = /([^\r\n]*)(\r\n|\n|\r|$)/g;
  let match;
  while ((match = pattern.exec(text))) {
    lines.push({ text: match[1], start: match.index, end: match.index + match[1].length, eol: match[2] });
    if (!match[2]) break;
  }
  return lines;
}

function lineAt(lines, position) {
  let index = 0;
  while (index + 1 < lines.length && lines[index + 1].start <= position) index++;
  return index;
}

function selectedLines(lines, start, end) {
  const first = lineAt(lines, start);
  let last = lineAt(lines, end);
  if (end > start && last > first && end === lines[last].start) last--;
  return [first, last];
}

function columns(text, tabSize) {
  let width = 0;
  for (const char of text) width += char === '\t' ? tabSize - width % tabSize : 1;
  return width;
}

function container(text) {
  const prefix = text.match(/^(?:[ \t]{0,3}>[ \t]?)+/)?.[0] ?? '';
  const rest = text.slice(prefix.length);
  return { prefix, rest, key: (prefix.match(/>/g) ?? []).length };
}

function analyze(text, tabSize = 2) {
  const lines = linesOf(text);
  let fence = null;
  for (const line of lines) {
    Object.assign(line, container(line.text));
    line.blank = !line.rest.trim();
    line.indentText = line.rest.match(/^[ \t]*/)[0];
    line.indent = columns(line.indentText, tabSize);
    line.fenced = false;
    line.item = null;
    if (fence && line.key !== fence.key) fence = null;
    if (fence) {
      line.fenced = true;
      const close = line.rest.match(/^[ \t]*(`+|~+)[ \t]*$/);
      if (close && close[1][0] === fence.char && close[1].length >= fence.length) fence = null;
      continue;
    }
    const marker = line.rest.match(/^([ \t]*)([-+*]|\d{1,9}[.)])([ \t]+)(.*)$/);
    if (marker) {
      line.item = {
        indent: line.indent,
        contentColumn: columns(marker[1] + marker[2] + marker[3], tabSize),
        content: marker[4]
      };
    }
    const opening = (line.item ? line.item.content : line.rest).match(/^[ \t]*(`{3,}|~{3,})(.*)$/);
    if (opening && !(opening[1][0] === '`' && opening[2].includes('`'))) {
      // List-marker fence openings remain list items for structural indentation,
      // while sharing the same code context as their contents and closing line.
      line.fenced = true;
      fence = { char: opening[1][0], length: opening[1].length, key: line.key };
    }
  }
  // A list item owns its indented continuations, descendants and internal blanks.
  const parents = [];
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index];
    if (!line.item) continue;
    let end = index;
    let afterBlank = false;
    for (let next = index + 1; next < lines.length; next++) {
      const candidate = lines[next];
      if (candidate.blank) { afterBlank = true; continue; }
      if (candidate.key !== line.key) break;
      if (candidate.item && candidate.indent <= line.indent) break;
      if (candidate.indent < line.item.contentColumn) {
        if (afterBlank || candidate.item || candidate.fenced || /^ {0,3}(?:#{1,6}\s|>|(?:[-*_]\s*){3,}$)/.test(candidate.rest)) break;
      }
      end = next;
      afterBlank = false;
    }
    line.item.end = end;
    while (parents.length) {
      const parent = lines[parents[parents.length - 1]];
      if (parent.key === line.key && parent.indent < line.indent && parent.item.end >= index) break;
      parents.pop();
    }
    line.item.parent = parents[parents.length - 1] ?? -1;
    parents.push(index);
  }
  return lines;
}

/** Shared fence context for formatting and structural commands. End excludes the newline. */
export function getMarkdownLineContexts(text) {
  return analyze(text).map(line => ({ start: line.start, end: line.end, text: line.text, code: line.fenced }));
}

export function isInsideCodeFence(text, position) {
  const lines = analyze(text);
  return lines[lineAt(lines, position)].fenced;
}

function makeEdit(text, replacements, start, end) {
  if (!replacements.length) return null;
  replacements.sort((a, b) => a.start - b.start);
  const from = replacements[0].start;
  const to = replacements[replacements.length - 1].end;
  let replacement = '';
  let cursor = from;
  for (const edit of replacements) {
    replacement += text.slice(cursor, edit.start) + edit.text;
    cursor = edit.end;
  }
  replacement += text.slice(cursor, to);
  const map = position => {
    let delta = 0;
    for (const edit of replacements) {
      if (position < edit.start) break;
      if (position < edit.end) return edit.start + delta + Math.min(position - edit.start, edit.text.length);
      delta += edit.text.length - (edit.end - edit.start);
    }
    return position + delta;
  };
  if (replacement === text.slice(from, to)) return null;
  return { start: from, end: to, text: replacement, selectionStart: map(start), selectionEnd: map(end) };
}

function removeIndent(text, amount, tabSize) {
  let count = 0;
  let width = 0;
  while (count < text.length && /[ \t]/.test(text[count]) && width < amount) {
    width += text[count] === '\t' ? tabSize - width % tabSize : 1;
    count++;
  }
  return { count, text: ' '.repeat(Math.max(0, width - amount)) };
}

/** Indent selected lines or whole list subtrees; ordinary collapsed Tab inserts at the caret. */
export function createIndentEdit(text, start, end, tabSize, outdent = false) {
  tabSize = Math.max(1, Math.floor(tabSize) || 2);
  const lines = analyze(text, tabSize);
  const [first, last] = selectedLines(lines, start, end);
  const roots = [];
  const affected = new Set();
  for (let index = first; index <= last; index++) {
    let owner = index;
    if (!lines[index].item && !lines[index].fenced) {
      for (let previous = index - 1; previous >= 0; previous--) {
        if (lines[previous].item?.end >= index) { owner = previous; break; }
      }
    }
    if (lines[owner].item && !roots.some(root => root <= owner && lines[root].item.end >= owner)) roots.push(owner);
    else if (!lines[owner].item) affected.add(index);
  }
  const replacements = [];
  const rootAmounts = new Map();
  for (const root of roots) {
    const line = lines[root];
    let amount;
    if (outdent) {
      const parent = line.item.parent;
      amount = parent >= 0 ? line.indent - lines[parent].indent : Math.min(line.indent, tabSize);
    } else {
      let sibling = -1;
      for (let previous = root - 1; previous >= 0; previous--) {
        const candidate = lines[previous];
        if (candidate.item && candidate.key === line.key && candidate.indent === line.indent && candidate.item.parent === line.item.parent) {
          if (lines.slice(candidate.item.end + 1, root).every(row => row.blank)) sibling = previous;
          break;
        }
        if (!candidate.blank && candidate.indent < line.indent) break;
      }
      if (sibling < 0) continue;
      amount = roots.includes(sibling) ? rootAmounts.get(sibling) : lines[sibling].item.contentColumn - line.indent;
    }
    if (!(amount > 0)) continue;
    rootAmounts.set(root, amount);
    for (let index = root; index <= line.item.end; index++) {
      const row = lines[index];
      if (row.blank) continue;
      affected.delete(index);
      const position = row.start + row.prefix.length;
      if (outdent) {
        const removal = removeIndent(row.rest, amount, tabSize);
        if (removal.count) replacements.push({ start: position, end: position + removal.count, text: removal.text });
      } else replacements.push({ start: position, end: position, text: ' '.repeat(amount) });
    }
  }
  if (!roots.length && first === last && start === end && !outdent) {
    const column = columns(text.slice(lines[first].start, start), tabSize);
    return makeEdit(text, [{ start, end: start, text: ' '.repeat(tabSize - column % tabSize) }], start, end);
  }
  for (const index of affected) {
    const line = lines[index];
    const position = line.start + line.prefix.length;
    if (outdent) {
      const removal = removeIndent(line.rest, tabSize, tabSize);
      if (removal.count) replacements.push({ start: position, end: position + removal.count, text: removal.text });
    } else replacements.push({ start: position, end: position, text: ' '.repeat(tabSize) });
  }
  return makeEdit(text, replacements, start, end);
}
